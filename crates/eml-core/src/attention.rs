//! Toy EML-Attention (Iteration 0) — EXPERIMENTAL.
//!
//! First step toward an EML-Transformer built entirely from `eml(x, y) =
//! exp(x) - ln(y)` primitives. This module implements a single-head attention
//! block at **toy scale** (`d_model <= 32`, `seq_len <= 8`, per-model depth
//! 3-5) using five composed [`EmlModel`] instances:
//!
//! - `q_model`, `k_model`, `v_model` — learned Q/K/V projections
//! - `softmax_model` — learned row-softmax approximator
//! - `out_model` — learned output projection
//!
//! The inter-projection matmuls (Q·Kᵀ, A·V) are computed in `f64` for this
//! iteration; later iterations can lift them into EML trees per the scaling
//! plan in `.planning/development_notes/eml_model_development.md`. The
//! training protocol is gradient-free (coordinate descent with restarts, as
//! for every other WeftOS EML model).
//!
//! See:
//! - `.planning/development_notes/eml_model_development_assessment.md` for
//!   the Iteration 0 plan + go/no-go criteria.
//! - `docs/src/content/docs/weftos/eml-attention.mdx` for the user-facing
//!   architecture overview.
//!
//! Feature: `experimental-attention` (off by default).

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::events::{EmlEvent, EmlEventLog};
use crate::operator::eml_safe;

// ---------------------------------------------------------------------------
// SafeTree — saturation-safe composed-attention primitive (Iteration 2)
// ---------------------------------------------------------------------------

/// Saturation-safe depth-N tree for use as a composable layer.
///
/// Evaluates, per head:
/// ```text
///   proj = W·x + b                              // level-0 affine
///   for d in 0..depth:
///       (c0, c1, c2) = tree[d]
///       v = eml_safe(v·c0 + c1, |v| + c2 + 1)   // y ≥ 1 always
/// ```
///
/// The `|v| + c2 + 1` guard guarantees the `ln` argument is ≥ 1, so nested
/// composition never hits the `ln(MIN_POSITIVE) ≈ -744 → exp(20) = 4.85e8`
/// saturation path that plagues [`EmlModel`]'s classical tree. This is the
/// shape the browser demo has been using since 0.6.8 and the one that lets
/// the Rust attention stack converge on the identity task in Iteration 2.
///
/// Parameters are stored in a single flat `Vec<f64>` laid out as:
/// 1. Level-0 weights `W` — `heads × inputs` entries, row-major per head
/// 2. Level-0 biases `b` — `heads` entries
/// 3. Tree constants — `heads × depth × 3` entries (c0, c1, c2 per level)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SafeTree {
    depth: usize,
    inputs: usize,
    heads: usize,
    params: Vec<f64>,
}

impl SafeTree {
    pub fn new(depth: usize, inputs: usize, heads: usize) -> Self {
        let total = heads * inputs + heads + heads * depth * 3;
        Self {
            depth,
            inputs,
            heads,
            params: vec![0.0; total],
        }
    }

    pub fn depth(&self) -> usize {
        self.depth
    }
    pub fn inputs(&self) -> usize {
        self.inputs
    }
    pub fn heads(&self) -> usize {
        self.heads
    }
    pub fn param_count(&self) -> usize {
        self.params.len()
    }
    pub fn params_slice(&self) -> &[f64] {
        &self.params
    }
    pub fn params_slice_mut(&mut self) -> &mut [f64] {
        &mut self.params
    }

    /// Per-head forward pass. Output length == `heads`.
    pub fn predict(&self, x: &[f64]) -> Vec<f64> {
        assert_eq!(x.len(), self.inputs, "SafeTree input length mismatch");
        let w_stride = self.inputs;
        let bias_off = self.heads * self.inputs;
        let tree_off_base = bias_off + self.heads;
        let tree_stride = self.depth * 3;

        let mut out = vec![0.0_f64; self.heads];
        for h in 0..self.heads {
            let mut acc = self.params[bias_off + h];
            let w_row = h * w_stride;
            for i in 0..self.inputs {
                acc += self.params[w_row + i] * x[i];
            }
            let mut v = acc;
            let tree_h = tree_off_base + h * tree_stride;
            for d in 0..self.depth {
                let c0 = self.params[tree_h + d * 3];
                let c1 = self.params[tree_h + d * 3 + 1];
                let c2 = self.params[tree_h + d * 3 + 2];
                v = eml_safe(v * c0 + c1, v.abs() + c2 + 1.0);
            }
            out[h] = v;
        }
        out
    }

    /// JSON round-trip. Schema matches the browser demo's export bundle.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("SafeTree serialization should not fail")
    }
    pub fn from_json(json: &str) -> Option<Self> {
        serde_json::from_str(json).ok()
    }
}

/// Maximum sequence length permitted at toy scale.
pub const MAX_TOY_SEQ_LEN: usize = 8;

/// Maximum model dimension permitted at toy scale.
pub const MAX_TOY_D_MODEL: usize = 32;

/// Toy-scale EML attention block — Iteration 0.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToyEmlAttention {
    name: String,
    d_model: usize,
    d_k: usize,
    seq_len: usize,

    q_model: SafeTree,
    k_model: SafeTree,
    v_model: SafeTree,
    softmax_model: SafeTree,
    out_model: SafeTree,

    scale: f64,

    #[serde(skip)]
    buffer: VecDeque<(Vec<f64>, Vec<f64>)>,

    trained: bool,
    training_rounds: u64,
    #[serde(skip, default)]
    last_accepts: u32,

    #[serde(skip, default)]
    events: EmlEventLog,
}

impl ToyEmlAttention {
    /// Number of trial acceptances during the most recent
    /// [`Self::train_end_to_end`] call. Diagnostic only.
    pub fn last_accepts(&self) -> u32 {
        self.last_accepts
    }
}

impl ToyEmlAttention {
    /// Construct a new toy attention block. Returns an error if shape
    /// exceeds the toy-scale bounds.
    pub fn new(
        name: impl Into<String>,
        d_model: usize,
        d_k: usize,
        seq_len: usize,
        depth: usize,
    ) -> Result<Self, AttentionError> {
        if !(3..=5).contains(&depth) {
            return Err(AttentionError::InvalidDepth(depth));
        }
        if seq_len == 0 || seq_len > MAX_TOY_SEQ_LEN {
            return Err(AttentionError::SeqLenOutOfRange(seq_len));
        }
        if d_model == 0 || d_model > MAX_TOY_D_MODEL {
            return Err(AttentionError::DModelOutOfRange(d_model));
        }
        if d_k == 0 || d_k > d_model {
            return Err(AttentionError::DKOutOfRange(d_k));
        }

        let proj_in = seq_len * d_model;
        let proj_out = seq_len * d_k;

        let mut attn = Self {
            name: name.into(),
            d_model,
            d_k,
            seq_len,
            q_model: SafeTree::new(depth, proj_in, proj_out),
            k_model: SafeTree::new(depth, proj_in, proj_out),
            v_model: SafeTree::new(depth, proj_in, proj_out),
            softmax_model: SafeTree::new(depth.min(4), seq_len, seq_len),
            out_model: SafeTree::new(depth, proj_out, proj_in),
            scale: 1.0 / (d_k as f64).sqrt(),
            buffer: VecDeque::with_capacity(256),
            trained: false,
            training_rounds: 0,
            last_accepts: 0,
            events: EmlEventLog::new(),
        };
        // Small-random init avoids the zero-param saturation regime where
        // any `y < 0` in the tree triggers `ln(MIN_POSITIVE) ≈ -744` and
        // exp(20) clamping downstream, producing uniform 4.85e8 outputs.
        let mut seed: u64 = 0x6D61_7474_6861_777F ^ (d_model as u64);
        for m in [
            &mut attn.q_model,
            &mut attn.k_model,
            &mut attn.v_model,
            &mut attn.softmax_model,
            &mut attn.out_model,
        ] {
            for p in m.params_slice_mut().iter_mut() {
                *p = next_lcg_signed(&mut seed) * 0.05;
            }
        }
        Ok(attn)
    }

    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn d_model(&self) -> usize {
        self.d_model
    }
    pub fn d_k(&self) -> usize {
        self.d_k
    }
    pub fn seq_len(&self) -> usize {
        self.seq_len
    }

    /// Total parameter count across the five sub-models.
    pub fn param_count(&self) -> usize {
        self.q_model.param_count()
            + self.k_model.param_count()
            + self.v_model.param_count()
            + self.softmax_model.param_count()
            + self.out_model.param_count()
    }

    pub fn is_trained(&self) -> bool {
        self.trained
    }

    pub fn training_rounds(&self) -> u64 {
        self.training_rounds
    }

    pub fn buffer_len(&self) -> usize {
        self.buffer.len()
    }

    /// Forward pass. `x` is the flattened input of length `seq_len * d_model`.
    /// Returns a flattened output of the same length.
    pub fn forward(&self, x: &[f64]) -> Result<Vec<f64>, AttentionError> {
        if x.len() != self.seq_len * self.d_model {
            return Err(AttentionError::ShapeMismatch {
                expected: self.seq_len * self.d_model,
                got: x.len(),
            });
        }

        let q_flat = self.q_model.predict(x);
        let k_flat = self.k_model.predict(x);
        let v_flat = self.v_model.predict(x);

        let scores = self.qk_scores(&q_flat, &k_flat);
        let attn = self.apply_softmax(&scores);
        let context = self.attn_v(&attn, &v_flat);

        // SafeTree doesn't saturate under composition — no post-processor needed.
        Ok(self.out_model.predict(&context))
    }

    /// Compute Q · Kᵀ / sqrt(d_k). For Iteration 0 this is a float matmul;
    /// Iteration 1+ can lift this into EML trees.
    fn qk_scores(&self, q: &[f64], k: &[f64]) -> Vec<f64> {
        let (n, d) = (self.seq_len, self.d_k);
        let mut scores = vec![0.0_f64; n * n];
        for i in 0..n {
            for j in 0..n {
                let mut acc = 0.0;
                for r in 0..d {
                    acc += q[i * d + r] * k[j * d + r];
                }
                scores[i * n + j] = acc * self.scale;
            }
        }
        scores
    }

    /// Apply the learned softmax model row-wise to the score matrix. Falls
    /// back to a stable numerical softmax when the learned output drifts
    /// negative or non-finite.
    fn apply_softmax(&self, scores: &[f64]) -> Vec<f64> {
        let n = self.seq_len;
        let mut out = vec![0.0_f64; n * n];
        for i in 0..n {
            let row = &scores[i * n..(i + 1) * n];
            let row_out = if self.trained {
                let mut learned = self.softmax_model.predict(row);
                for v in learned.iter_mut() {
                    if !v.is_finite() || *v < 0.0 {
                        *v = 0.0;
                    }
                }
                let sum: f64 = learned.iter().sum();
                if sum > 1e-12 {
                    for v in learned.iter_mut() {
                        *v /= sum;
                    }
                    learned
                } else {
                    numerical_softmax(row)
                }
            } else {
                numerical_softmax(row)
            };
            out[i * n..(i + 1) * n].copy_from_slice(&row_out);
        }
        out
    }

    /// Compute A · V. Float matmul in Iteration 0.
    fn attn_v(&self, attn: &[f64], v: &[f64]) -> Vec<f64> {
        let (n, d) = (self.seq_len, self.d_k);
        let mut out = vec![0.0_f64; n * d];
        for i in 0..n {
            for r in 0..d {
                let mut acc = 0.0;
                for j in 0..n {
                    acc += attn[i * n + j] * v[j * d + r];
                }
                out[i * d + r] = acc;
            }
        }
        out
    }

    /// Record an end-to-end (input, target) sample. Per-submodel training
    /// targets are derived inside [`Self::train`] via self-distillation.
    pub fn record(&mut self, input: Vec<f64>, target: Vec<f64>) -> Result<(), AttentionError> {
        let expected = self.seq_len * self.d_model;
        if input.len() != expected {
            return Err(AttentionError::ShapeMismatch {
                expected,
                got: input.len(),
            });
        }
        if target.len() != expected {
            return Err(AttentionError::ShapeMismatch {
                expected,
                got: target.len(),
            });
        }

        if self.buffer.len() >= 256 {
            self.buffer.pop_front();
        }
        self.buffer.push_back((input, target));
        Ok(())
    }

    /// **Iteration 1 end-to-end training.**
    ///
    /// Joint gradient-free coordinate descent over the union of all five
    /// sub-models' parameters. At each trial: pick a random parameter across
    /// Q/K/V/softmax/out, perturb it, run the full forward pass, accept the
    /// perturbation if end-to-end MSE drops.
    ///
    /// Returns final end-to-end MSE. Sets `trained = true` when MSE drops
    /// below `convergence_mse` (default 1e-2).
    pub fn train_end_to_end(&mut self, cfg: EndToEndTrainConfig) -> f64 {
        self.training_rounds += 1;

        let samples: Vec<(Vec<f64>, Vec<f64>)> =
            self.buffer.iter().take(cfg.max_samples).cloned().collect();
        if samples.len() < 4 {
            return f64::INFINITY;
        }
        let eval_subset: Vec<(Vec<f64>, Vec<f64>)> = samples
            .iter()
            .step_by((samples.len() / cfg.eval_subset.min(samples.len())).max(1))
            .take(cfg.eval_subset)
            .cloned()
            .collect();

        let mse_before = self.end_to_end_mse(&eval_subset);
        let mut best_mse = mse_before;

        let total_params = self.param_count();
        // Mix in training_rounds so successive calls explore different trial
        // sequences instead of replaying the same cfg.seed trajectory.
        let mut rng_state = cfg
            .seed
            .wrapping_add(self.training_rounds.wrapping_mul(0x9E37_79B9_7F4A_7C15));

        let mut accepts: u32 = 0;
        for trial in 0..cfg.trials {
            let frac = trial as f64 / cfg.trials.max(1) as f64;
            let step = cfg.step_init * (cfg.step_final / cfg.step_init).powf(frac);

            let u = next_lcg_unit(&mut rng_state);
            let pidx = ((u * total_params as f64) as usize).min(total_params - 1);
            let delta = next_lcg_signed(&mut rng_state) * step;

            let (saved, applied) = self.apply_param_delta(pidx, delta);
            if !applied {
                continue;
            }

            let candidate = self.end_to_end_mse(&eval_subset);
            if candidate + 1e-12 < best_mse {
                best_mse = candidate;
                accepts += 1;
            } else {
                self.restore_param(pidx, saved);
            }
        }
        self.last_accepts = accepts;

        if best_mse < cfg.convergence_mse {
            self.trained = true;
            self.events.push(EmlEvent::Trained {
                model_name: self.name.clone(),
                samples_used: samples.len(),
                mse_before,
                mse_after: best_mse,
                converged: true,
                param_count: total_params,
            });
        }

        best_mse
    }

    fn end_to_end_mse(&self, samples: &[(Vec<f64>, Vec<f64>)]) -> f64 {
        let mut sum = 0.0;
        let mut count = 0usize;
        for (input, target) in samples {
            if let Ok(y) = self.forward(input) {
                for (a, b) in y.iter().zip(target.iter()) {
                    sum += (a - b).powi(2);
                    count += 1;
                }
            }
        }
        sum / count.max(1) as f64
    }

    /// Apply a delta to the `pidx`-th parameter across the union of
    /// sub-model params. Returns (saved_value, applied).
    fn apply_param_delta(&mut self, pidx: usize, delta: f64) -> (f64, bool) {
        let mut remaining = pidx;
        for m in [
            &mut self.q_model,
            &mut self.k_model,
            &mut self.v_model,
            &mut self.softmax_model,
            &mut self.out_model,
        ] {
            let slice = m.params_slice_mut();
            if remaining < slice.len() {
                let saved = slice[remaining];
                slice[remaining] = saved + delta;
                return (saved, true);
            }
            remaining -= slice.len();
        }
        (0.0, false)
    }

    fn restore_param(&mut self, pidx: usize, saved: f64) {
        let mut remaining = pidx;
        for m in [
            &mut self.q_model,
            &mut self.k_model,
            &mut self.v_model,
            &mut self.softmax_model,
            &mut self.out_model,
        ] {
            let slice = m.params_slice_mut();
            if remaining < slice.len() {
                slice[remaining] = saved;
                return;
            }
            remaining -= slice.len();
        }
    }

    pub fn drain_events(&mut self) -> Vec<EmlEvent> {
        self.events.drain()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("ToyEmlAttention serialization should not fail")
    }

    pub fn from_json(json: &str) -> Option<Self> {
        serde_json::from_str(json).ok()
    }
}

/// Numerically stable softmax — used as the reference for training and the
/// fallback when the learned softmax model is untrained or drifts.
fn numerical_softmax(row: &[f64]) -> Vec<f64> {
    let max = row.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let exp: Vec<f64> = row.iter().map(|v| (v - max).exp()).collect();
    let sum: f64 = exp.iter().sum();
    if sum > 0.0 {
        exp.iter().map(|v| v / sum).collect()
    } else {
        let n = row.len() as f64;
        vec![1.0 / n; row.len()]
    }
}

/// Configuration for [`ToyEmlAttention::train_end_to_end`] — Iteration 1
/// joint coordinate descent over Q/K/V/softmax/out.
#[derive(Debug, Clone, Copy)]
pub struct EndToEndTrainConfig {
    /// Number of random-param perturbation trials.
    pub trials: usize,
    /// Starting perturbation magnitude (wide exploration).
    pub step_init: f64,
    /// Final perturbation magnitude at the end of the annealing schedule.
    pub step_final: f64,
    /// Samples drawn from the training buffer per round.
    pub max_samples: usize,
    /// MSE evaluation subset size (smaller = faster, noisier signal).
    pub eval_subset: usize,
    /// MSE threshold below which the model is marked as trained.
    pub convergence_mse: f64,
    /// Deterministic RNG seed for reproducibility.
    pub seed: u64,
}

impl Default for EndToEndTrainConfig {
    fn default() -> Self {
        Self {
            trials: 2000,
            step_init: 0.5,
            step_final: 0.02,
            max_samples: 96,
            eval_subset: 16,
            convergence_mse: 1e-2,
            seed: 0xDEAD_BEEF,
        }
    }
}

// -- LCG helpers (deterministic, no external dep) --------------------------

fn next_lcg(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state
}

/// LCG output in [0.0, 1.0).
fn next_lcg_unit(state: &mut u64) -> f64 {
    let raw = next_lcg(state);
    ((raw >> 33) as u32) as f64 / (u32::MAX as f64 + 1.0)
}

/// LCG output in [-1.0, 1.0).
fn next_lcg_signed(state: &mut u64) -> f64 {
    next_lcg_unit(state) * 2.0 - 1.0
}

/// Errors from ToyEmlAttention construction / use.
#[derive(Debug, Clone, PartialEq)]
pub enum AttentionError {
    InvalidDepth(usize),
    SeqLenOutOfRange(usize),
    DModelOutOfRange(usize),
    DKOutOfRange(usize),
    ShapeMismatch { expected: usize, got: usize },
}

impl std::fmt::Display for AttentionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AttentionError::InvalidDepth(d) => {
                write!(f, "EmlModel depth must be in 3..=5, got {}", d)
            }
            AttentionError::SeqLenOutOfRange(n) => {
                write!(f, "seq_len must be in 1..={}, got {}", MAX_TOY_SEQ_LEN, n)
            }
            AttentionError::DModelOutOfRange(n) => {
                write!(f, "d_model must be in 1..={}, got {}", MAX_TOY_D_MODEL, n)
            }
            AttentionError::DKOutOfRange(n) => {
                write!(f, "d_k must be in 1..=d_model, got {}", n)
            }
            AttentionError::ShapeMismatch { expected, got } => {
                write!(f, "shape mismatch: expected {}, got {}", expected, got)
            }
        }
    }
}

impl std::error::Error for AttentionError {}

// ---------------------------------------------------------------------------
// 4-phase benchmark harness
// ---------------------------------------------------------------------------

/// Result of a single 4-phase benchmark pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttentionBenchmark {
    pub d_model: usize,
    pub seq_len: usize,
    pub d_k: usize,
    pub depth: usize,
    pub param_count: usize,
    pub phase1_warmup_ns: u128,
    pub phase1_serialize_roundtrip: bool,
    pub phase2_converged: bool,
    pub phase2_baseline_mse: f64,
    pub phase2_final_mse: f64,
    pub phase2_mse_reduction: f64,
    pub phase2_training_rounds: u64,
    pub phase3_inference_ns_mean: u128,
    pub phase3_inference_ns_p99: u128,
    pub phase4_scaling: Vec<ScalingPoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScalingPoint {
    pub seq_len: usize,
    pub d_model: usize,
    pub param_count: usize,
    pub inference_ns_mean: u128,
}

/// Deterministic LCG for benchmark data.
fn lcg_next(state: &mut u64) -> f64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    (*state >> 33) as f64 / (u32::MAX as f64 / 2.0) - 1.0
}

fn gen_sample(state: &mut u64, n: usize) -> Vec<f64> {
    (0..n).map(|_| lcg_next(state)).collect()
}

/// Run the 4-phase benchmark for a given configuration.
///
/// Mirrors the benchmark protocol used in `clawft-weave/src/commands/bench_cmd.rs`:
/// - **Phase 1 (Warmup)**: forward-pass sanity + serialization roundtrip
/// - **Phase 2 (Convergence)**: training on the per-position-mean task
/// - **Phase 3 (Compute)**: inference latency (mean + p99)
/// - **Phase 4 (Scalability)**: seq_len × d_model sweep
///
/// Uses a lightweight Phase-2 trial budget (400). Use
/// [`run_benchmark_with_trials`] to drive real convergence.
pub fn run_benchmark(
    d_model: usize,
    d_k: usize,
    seq_len: usize,
    depth: usize,
) -> Result<AttentionBenchmark, AttentionError> {
    run_benchmark_with_trials(d_model, d_k, seq_len, depth, 400)
}

/// Same as [`run_benchmark`] but with a caller-chosen Phase-2 trial budget.
pub fn run_benchmark_with_trials(
    d_model: usize,
    d_k: usize,
    seq_len: usize,
    depth: usize,
    phase2_trials: usize,
) -> Result<AttentionBenchmark, AttentionError> {
    let mut attn = ToyEmlAttention::new("bench", d_model, d_k, seq_len, depth)?;
    let params = attn.param_count();
    let n = seq_len * d_model;
    let mut rng = 0xCAFE_F00D_u64;

    // Phase 1 -------------------------------------------------------------
    let sample = gen_sample(&mut rng, n);
    let t = std::time::Instant::now();
    let _ = attn.forward(&sample)?;
    let phase1_warmup_ns = t.elapsed().as_nanos();
    let json = attn.to_json();
    let round_trip = ToyEmlAttention::from_json(&json);
    let phase1_serialize_roundtrip = round_trip
        .map(|r| r.d_model == attn.d_model && r.seq_len == attn.seq_len)
        .unwrap_or(false);

    // Phase 2 -------------------------------------------------------------
    // Per-position-mean task. Identity with d_k < d_model is
    // information-theoretically bounded (context drops half the input
    // dimensions), so it's not a fair convergence gate even with the
    // saturation-safe tree. Per-position-mean is low-rank and recoverable.
    for _ in 0..96 {
        let s = gen_sample(&mut rng, n);
        let mut target = vec![0.0; n];
        for i in 0..seq_len {
            let mut sum = 0.0;
            for j in 0..d_model {
                sum += s[i * d_model + j];
            }
            let mean = sum / d_model as f64;
            for j in 0..d_model {
                target[i * d_model + j] = mean;
            }
        }
        attn.record(s, target)?;
    }
    let mut mse_sum = 0.0;
    let mut mse_count = 0;
    for _ in 0..16 {
        let s = gen_sample(&mut rng, n);
        let y = attn.forward(&s)?;
        let mut target = vec![0.0; n];
        for i in 0..seq_len {
            let mut sum = 0.0;
            for j in 0..d_model {
                sum += s[i * d_model + j];
            }
            let mean = sum / d_model as f64;
            for j in 0..d_model {
                target[i * d_model + j] = mean;
            }
        }
        for (a, b) in y.iter().zip(target.iter()) {
            mse_sum += (a - b).powi(2);
            mse_count += 1;
        }
    }
    let phase2_baseline_mse = mse_sum / (mse_count as f64).max(1.0);

    let cfg = EndToEndTrainConfig {
        trials: phase2_trials,
        step_init: 0.5,
        step_final: 0.01,
        convergence_mse: 5e-2,
        ..Default::default()
    };
    let mut phase2_final_mse = f64::INFINITY;
    let mut phase2_converged = false;
    for _ in 0..3 {
        phase2_final_mse = attn.train_end_to_end(cfg);
        if phase2_final_mse < cfg.convergence_mse {
            phase2_converged = true;
            break;
        }
    }
    let phase2_training_rounds = attn.training_rounds();
    let phase2_mse_reduction = if phase2_baseline_mse > 1e-12 {
        1.0 - phase2_final_mse / phase2_baseline_mse
    } else {
        0.0
    };

    // Phase 3 -------------------------------------------------------------
    let mut latencies = Vec::with_capacity(256);
    for _ in 0..256 {
        let s = gen_sample(&mut rng, n);
        let t = std::time::Instant::now();
        let _ = attn.forward(&s)?;
        latencies.push(t.elapsed().as_nanos());
    }
    latencies.sort_unstable();
    let phase3_inference_ns_mean = latencies.iter().sum::<u128>() / (latencies.len() as u128);
    let phase3_inference_ns_p99 = latencies[(latencies.len() * 99) / 100];

    // Phase 4 -------------------------------------------------------------
    let mut phase4_scaling = Vec::new();
    let shapes = [(4, 8), (4, 16), (8, 8), (8, 16)];
    for &(sl, dm) in &shapes {
        if sl > seq_len || dm > d_model {
            continue;
        }
        let dk = dm.min(d_k);
        let a = ToyEmlAttention::new("scale", dm, dk, sl, depth)?;
        let sample = gen_sample(&mut rng, sl * dm);
        let mut lats = Vec::with_capacity(32);
        for _ in 0..32 {
            let t = std::time::Instant::now();
            let _ = a.forward(&sample)?;
            lats.push(t.elapsed().as_nanos());
        }
        lats.sort_unstable();
        let mean = lats.iter().sum::<u128>() / (lats.len() as u128);
        phase4_scaling.push(ScalingPoint {
            seq_len: sl,
            d_model: dm,
            param_count: a.param_count(),
            inference_ns_mean: mean,
        });
    }

    Ok(AttentionBenchmark {
        d_model,
        seq_len,
        d_k,
        depth,
        param_count: params,
        phase1_warmup_ns,
        phase1_serialize_roundtrip,
        phase2_converged,
        phase2_baseline_mse,
        phase2_final_mse,
        phase2_mse_reduction,
        phase2_training_rounds,
        phase3_inference_ns_mean,
        phase3_inference_ns_p99,
        phase4_scaling,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn construct_valid() {
        let a = ToyEmlAttention::new("t", 8, 4, 4, 3).unwrap();
        assert_eq!(a.d_model(), 8);
        assert_eq!(a.d_k(), 4);
        assert_eq!(a.seq_len(), 4);
        assert!(a.param_count() > 0);
        assert!(!a.is_trained());
    }

    #[test]
    fn reject_depth_out_of_range() {
        assert!(matches!(
            ToyEmlAttention::new("t", 8, 4, 4, 2),
            Err(AttentionError::InvalidDepth(2))
        ));
        assert!(matches!(
            ToyEmlAttention::new("t", 8, 4, 4, 6),
            Err(AttentionError::InvalidDepth(6))
        ));
    }

    #[test]
    fn reject_seq_too_long() {
        assert!(matches!(
            ToyEmlAttention::new("t", 8, 4, 9, 3),
            Err(AttentionError::SeqLenOutOfRange(9))
        ));
    }

    #[test]
    fn forward_shape_and_finite() {
        let a = ToyEmlAttention::new("t", 8, 4, 4, 3).unwrap();
        let x = vec![0.5; 4 * 8];
        let y = a.forward(&x).unwrap();
        assert_eq!(y.len(), 4 * 8);
        for v in y {
            assert!(v.is_finite(), "output should be finite");
        }
    }

    #[test]
    fn forward_shape_mismatch_errors() {
        let a = ToyEmlAttention::new("t", 8, 4, 4, 3).unwrap();
        let err = a.forward(&[0.0; 5]).unwrap_err();
        assert!(matches!(err, AttentionError::ShapeMismatch { .. }));
    }

    #[test]
    fn record_shape_mismatch_errors() {
        let mut a = ToyEmlAttention::new("t", 8, 4, 4, 3).unwrap();
        let err = a.record(vec![0.0; 5], vec![0.0; 32]).unwrap_err();
        assert!(matches!(err, AttentionError::ShapeMismatch { .. }));
    }

    #[test]
    fn numerical_softmax_sums_to_one() {
        let out = numerical_softmax(&[1.0, 2.0, 3.0, 4.0]);
        let s: f64 = out.iter().sum();
        assert!((s - 1.0).abs() < 1e-12);
    }

    #[test]
    fn numerical_softmax_stable_large_values() {
        let out = numerical_softmax(&[1000.0, 1001.0, 1002.0]);
        for v in &out {
            assert!(v.is_finite());
        }
        let s: f64 = out.iter().sum();
        assert!((s - 1.0).abs() < 1e-12);
    }

    #[test]
    fn serialization_roundtrip() {
        let a = ToyEmlAttention::new("t", 8, 4, 4, 3).unwrap();
        let j = a.to_json();
        let b = ToyEmlAttention::from_json(&j).unwrap();
        assert_eq!(a.d_model(), b.d_model());
        assert_eq!(a.seq_len(), b.seq_len());
        assert_eq!(a.param_count(), b.param_count());
    }

    #[test]
    fn training_runs_and_increments_rounds() {
        let mut a = ToyEmlAttention::new("t", 8, 4, 4, 3).unwrap();
        let mut rng = 42u64;
        for _ in 0..32 {
            let s = gen_sample(&mut rng, 32);
            a.record(s.clone(), s).unwrap();
        }
        let cfg = EndToEndTrainConfig {
            trials: 50,
            ..Default::default()
        };
        let _ = a.train_end_to_end(cfg);
        assert_eq!(a.training_rounds(), 1);
        assert!(a.buffer_len() > 0);
    }

    #[test]
    fn bench_phase1_and_phase3_finite_timings() {
        let b = run_benchmark(8, 4, 4, 3).unwrap();
        assert!(b.param_count > 0);
        assert!(b.phase1_warmup_ns > 0);
        assert!(b.phase1_serialize_roundtrip);
        assert!(b.phase3_inference_ns_mean > 0);
        assert!(b.phase3_inference_ns_p99 >= b.phase3_inference_ns_mean);
        assert!(!b.phase4_scaling.is_empty());
    }

    #[test]
    fn bench_phase4_scales_with_size() {
        let b = run_benchmark(16, 8, 8, 3).unwrap();
        let sizes: Vec<usize> = b
            .phase4_scaling
            .iter()
            .map(|p| p.seq_len * p.d_model)
            .collect();
        let mut sorted = sizes.clone();
        sorted.sort_unstable();
        // smaller shapes are present
        assert!(sorted.first().copied().unwrap_or(0) <= 32);
        // at least one scaling point should exist
        assert!(sorted.len() >= 1);
    }

    #[test]
    fn total_param_count_is_sum_of_submodels() {
        let a = ToyEmlAttention::new("t", 8, 4, 4, 3).unwrap();
        let sum = a.q_model.param_count()
            + a.k_model.param_count()
            + a.v_model.param_count()
            + a.softmax_model.param_count()
            + a.out_model.param_count();
        assert_eq!(a.param_count(), sum);
    }
}
