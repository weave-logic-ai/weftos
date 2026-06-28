//! Baseline float-only attention for head-to-head comparison with EML.
//!
//! Same public surface as [`crate::ToyEmlAttention`] (construct, forward,
//! record, train_end_to_end, JSON roundtrip) but every sub-layer is a plain
//! `W·x + b` affine map with no EML tree. Uses the exact same random-param
//! gradient-free coordinate-descent optimizer so comparisons are apples-to-
//! apples on substrate, not on language or optimizer.
//!
//! Feature: `experimental-attention` (same as `ToyEmlAttention`).

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::attention::{AttentionError, EndToEndTrainConfig, MAX_TOY_D_MODEL, MAX_TOY_SEQ_LEN};

/// Plain float-only single-head attention.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BaselineAttention {
    name: String,
    d_model: usize,
    d_k: usize,
    seq_len: usize,

    // Four affine layers (W, b) laid out flat. Each layer stores
    // (heads × inputs) weights followed by (heads) biases.
    q: Affine,
    k: Affine,
    v: Affine,
    out: Affine,

    scale: f64,

    #[serde(skip, default)]
    buffer: VecDeque<(Vec<f64>, Vec<f64>)>,

    trained: bool,
    training_rounds: u64,
    #[serde(skip, default)]
    last_accepts: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Affine {
    inputs: usize,
    heads: usize,
    params: Vec<f64>, // heads * inputs weights, then heads biases
}

impl Affine {
    fn new(inputs: usize, heads: usize) -> Self {
        Self {
            inputs,
            heads,
            params: vec![0.0; heads * inputs + heads],
        }
    }
    fn param_count(&self) -> usize {
        self.params.len()
    }
    fn predict(&self, x: &[f64]) -> Vec<f64> {
        let mut out = vec![0.0; self.heads];
        let bias_off = self.heads * self.inputs;
        for h in 0..self.heads {
            let mut acc = self.params[bias_off + h];
            let row = h * self.inputs;
            for i in 0..self.inputs {
                acc += self.params[row + i] * x[i];
            }
            out[h] = acc;
        }
        out
    }
    fn params_slice_mut(&mut self) -> &mut [f64] {
        &mut self.params
    }
}

impl BaselineAttention {
    pub fn new(
        name: impl Into<String>,
        d_model: usize,
        d_k: usize,
        seq_len: usize,
    ) -> Result<Self, AttentionError> {
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
            q: Affine::new(proj_in, proj_out),
            k: Affine::new(proj_in, proj_out),
            v: Affine::new(proj_in, proj_out),
            out: Affine::new(proj_out, proj_in),
            scale: 1.0 / (d_k as f64).sqrt(),
            buffer: VecDeque::with_capacity(256),
            trained: false,
            training_rounds: 0,
            last_accepts: 0,
        };

        // Same small-random init scheme ToyEmlAttention uses, so any
        // head-to-head difference comes from the architecture not init.
        let mut seed: u64 = 0x426C_696E_6553_7465_u64 ^ (d_model as u64);
        for aff in [&mut attn.q, &mut attn.k, &mut attn.v, &mut attn.out] {
            for p in aff.params_slice_mut().iter_mut() {
                *p = next_signed(&mut seed) * 0.05;
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

    pub fn param_count(&self) -> usize {
        self.q.param_count() + self.k.param_count() + self.v.param_count() + self.out.param_count()
    }

    pub fn is_trained(&self) -> bool {
        self.trained
    }
    pub fn training_rounds(&self) -> u64 {
        self.training_rounds
    }
    pub fn last_accepts(&self) -> u32 {
        self.last_accepts
    }
    pub fn buffer_len(&self) -> usize {
        self.buffer.len()
    }

    pub fn forward(&self, x: &[f64]) -> Result<Vec<f64>, AttentionError> {
        if x.len() != self.seq_len * self.d_model {
            return Err(AttentionError::ShapeMismatch {
                expected: self.seq_len * self.d_model,
                got: x.len(),
            });
        }
        let q = self.q.predict(x);
        let k = self.k.predict(x);
        let v = self.v.predict(x);
        let scores = self.qk_scores(&q, &k);
        let attn = self.apply_softmax(&scores);
        let context = self.attn_v(&attn, &v);
        Ok(self.out.predict(&context))
    }

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

    fn apply_softmax(&self, scores: &[f64]) -> Vec<f64> {
        let n = self.seq_len;
        let mut out = vec![0.0_f64; n * n];
        for i in 0..n {
            let row = &scores[i * n..(i + 1) * n];
            let m = row.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let e: Vec<f64> = row.iter().map(|v| (v - m).exp()).collect();
            let sum: f64 = e.iter().sum();
            let row_out: Vec<f64> = if sum > 0.0 {
                e.iter().map(|v| v / sum).collect()
            } else {
                vec![1.0 / n as f64; n]
            };
            out[i * n..(i + 1) * n].copy_from_slice(&row_out);
        }
        out
    }

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

    /// Same random-param coordinate-descent training as
    /// [`crate::ToyEmlAttention::train_end_to_end`], for fair head-to-head.
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
        let mut rng_state = cfg
            .seed
            .wrapping_add(self.training_rounds.wrapping_mul(0x9E37_79B9_7F4A_7C15));

        let mut accepts: u32 = 0;
        for trial in 0..cfg.trials {
            let frac = trial as f64 / cfg.trials.max(1) as f64;
            let step = cfg.step_init * (cfg.step_final / cfg.step_init).powf(frac);

            let u = next_unit(&mut rng_state);
            let pidx = ((u * total_params as f64) as usize).min(total_params - 1);
            let delta = next_signed(&mut rng_state) * step;

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
        }
        let _ = mse_before;
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

    fn apply_param_delta(&mut self, pidx: usize, delta: f64) -> (f64, bool) {
        let mut remaining = pidx;
        for aff in [&mut self.q, &mut self.k, &mut self.v, &mut self.out] {
            let slice = aff.params_slice_mut();
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
        for aff in [&mut self.q, &mut self.k, &mut self.v, &mut self.out] {
            let slice = aff.params_slice_mut();
            if remaining < slice.len() {
                slice[remaining] = saved;
                return;
            }
            remaining -= slice.len();
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("BaselineAttention serialization should not fail")
    }
    pub fn from_json(json: &str) -> Option<Self> {
        serde_json::from_str(json).ok()
    }
}

// -- LCG helpers (local mirror of the attention module's) -------------------

fn next_lcg(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state
}
fn next_unit(state: &mut u64) -> f64 {
    let raw = next_lcg(state);
    ((raw >> 33) as u32) as f64 / (u32::MAX as f64 + 1.0)
}
fn next_signed(state: &mut u64) -> f64 {
    next_unit(state) * 2.0 - 1.0
}

// -- Side-by-side comparison harness ---------------------------------------

/// Result of running the same workload against both ToyEmlAttention and
/// BaselineAttention with identical CD optimizer + trial budget.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttentionComparison {
    pub shape: (usize, usize, usize, usize), // (d_model, d_k, seq_len, depth)
    pub trials: usize,
    pub rounds: usize,

    pub eml_param_count: usize,
    pub eml_baseline_mse: f64,
    pub eml_final_mse: f64,
    pub eml_mse_reduction: f64,
    pub eml_inference_ns_p99: u128,

    pub baseline_param_count: usize,
    pub baseline_baseline_mse: f64,
    pub baseline_final_mse: f64,
    pub baseline_mse_reduction: f64,
    pub baseline_inference_ns_p99: u128,
}

/// Run an identical per-position-mean training workload on both stacks.
pub fn compare_eml_vs_baseline(
    d_model: usize,
    d_k: usize,
    seq_len: usize,
    depth: usize,
    cfg: EndToEndTrainConfig,
    rounds: usize,
) -> Result<AttentionComparison, AttentionError> {
    use crate::ToyEmlAttention;

    let mut eml = ToyEmlAttention::new("eml", d_model, d_k, seq_len, depth)?;
    let mut base = BaselineAttention::new("base", d_model, d_k, seq_len)?;

    // Shared synthetic dataset. 96 samples of per-position-mean broadcast.
    let mut rng_state = 0xA11CE_u64;
    let n = seq_len * d_model;
    let samples: Vec<(Vec<f64>, Vec<f64>)> = (0..96)
        .map(|_| {
            let x: Vec<f64> = (0..n).map(|_| next_signed(&mut rng_state)).collect();
            let mut t = vec![0.0; n];
            for i in 0..seq_len {
                let mean: f64 =
                    x[i * d_model..(i + 1) * d_model].iter().sum::<f64>() / d_model as f64;
                for j in 0..d_model {
                    t[i * d_model + j] = mean;
                }
            }
            (x, t)
        })
        .collect();

    for (x, t) in &samples {
        eml.record(x.clone(), t.clone())?;
        base.record(x.clone(), t.clone())?;
    }

    // Baseline MSE for each.
    let mse_before = |attn_mse: f64| attn_mse;
    let eml_baseline = {
        let mut sum = 0.0;
        let mut count = 0;
        for (x, t) in samples.iter().take(16) {
            let y = eml.forward(x)?;
            for (a, b) in y.iter().zip(t.iter()) {
                sum += (a - b).powi(2);
                count += 1;
            }
        }
        sum / count.max(1) as f64
    };
    let base_baseline = {
        let mut sum = 0.0;
        let mut count = 0;
        for (x, t) in samples.iter().take(16) {
            let y = base.forward(x)?;
            for (a, b) in y.iter().zip(t.iter()) {
                sum += (a - b).powi(2);
                count += 1;
            }
        }
        sum / count.max(1) as f64
    };
    let _ = mse_before;

    // Train both identically.
    let mut eml_final = f64::INFINITY;
    let mut base_final = f64::INFINITY;
    for _ in 0..rounds {
        eml_final = eml.train_end_to_end(cfg);
        base_final = base.train_end_to_end(cfg);
    }

    // Phase-3 inference p99 for both.
    let mut timing = |forward: &dyn Fn(&[f64]) -> Result<Vec<f64>, AttentionError>| -> u128 {
        let mut lats = Vec::with_capacity(256);
        for _ in 0..256 {
            let x: Vec<f64> = (0..n).map(|_| next_signed(&mut rng_state)).collect();
            let t = std::time::Instant::now();
            let _ = forward(&x);
            lats.push(t.elapsed().as_nanos());
        }
        lats.sort_unstable();
        lats[(lats.len() * 99) / 100]
    };
    let eml_p99 = timing(&|x| eml.forward(x));
    let base_p99 = timing(&|x| base.forward(x));

    Ok(AttentionComparison {
        shape: (d_model, d_k, seq_len, depth),
        trials: cfg.trials,
        rounds,
        eml_param_count: eml.param_count(),
        eml_baseline_mse: eml_baseline,
        eml_final_mse: eml_final,
        eml_mse_reduction: if eml_baseline > 1e-12 {
            1.0 - eml_final / eml_baseline
        } else {
            0.0
        },
        eml_inference_ns_p99: eml_p99,
        baseline_param_count: base.param_count(),
        baseline_baseline_mse: base_baseline,
        baseline_final_mse: base_final,
        baseline_mse_reduction: if base_baseline > 1e-12 {
            1.0 - base_final / base_baseline
        } else {
            0.0
        },
        baseline_inference_ns_p99: base_p99,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn construct_valid() {
        let b = BaselineAttention::new("t", 4, 2, 2).unwrap();
        assert!(b.param_count() > 0);
        assert!(!b.is_trained());
    }

    #[test]
    fn forward_shape_and_finite() {
        let b = BaselineAttention::new("t", 4, 2, 2).unwrap();
        let y = b
            .forward(&[0.5, -0.3, 0.1, 0.7, -0.2, 0.4, 0.0, 0.6])
            .unwrap();
        assert_eq!(y.len(), 8);
        for v in y {
            assert!(v.is_finite());
        }
    }

    #[test]
    fn serialization_roundtrip() {
        let b = BaselineAttention::new("t", 4, 2, 2).unwrap();
        let j = b.to_json();
        let b2 = BaselineAttention::from_json(&j).unwrap();
        assert_eq!(b.param_count(), b2.param_count());
        assert_eq!(b.d_model(), b2.d_model());
    }

    #[test]
    fn comparison_runs() {
        let cfg = EndToEndTrainConfig {
            trials: 200,
            ..Default::default()
        };
        let c = compare_eml_vs_baseline(4, 2, 2, 3, cfg, 1).unwrap();
        assert!(c.eml_param_count > 0);
        assert!(c.baseline_param_count > 0);
        assert!(c.eml_inference_ns_p99 > 0);
        assert!(c.baseline_inference_ns_p99 > 0);
    }
}
