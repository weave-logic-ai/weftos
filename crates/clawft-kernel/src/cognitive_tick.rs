//! Configurable cognitive tick loop -- the heartbeat of the ECC cognitive substrate.
//!
//! The [`CognitiveTick`] service drives the kernel's cognitive processing
//! cycle at a configurable interval, with optional adaptive adjustment
//! based on measured compute timings and drift detection.
//!
//! The [`run_democritus_loop`] function implements the DEMOCRITUS two-tier
//! coherence cycle: O(1) EML prediction on every tick, falling back to
//! exact O(k*m) Lanczos spectral analysis when drift exceeds a threshold.

use std::sync::Mutex;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::health::HealthStatus;
use crate::service::{ServiceType, SystemService};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the cognitive tick loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CognitiveTickConfig {
    /// Target tick interval in milliseconds.
    pub tick_interval_ms: u32,
    /// Fraction of the tick interval budget available for compute (0.0..1.0).
    pub tick_budget_ratio: f32,
    /// Number of ticks used for initial calibration.
    pub calibration_ticks: u32,
    /// Whether to adaptively adjust the tick interval based on load.
    pub adaptive_tick: bool,
    /// Window (in seconds) over which recent timings are averaged.
    pub adaptive_window_s: u32,
}

impl Default for CognitiveTickConfig {
    fn default() -> Self {
        Self {
            tick_interval_ms: 50,
            tick_budget_ratio: 0.3,
            calibration_ticks: 100,
            adaptive_tick: true,
            adaptive_window_s: 30,
        }
    }
}

// ---------------------------------------------------------------------------
// Stats (public snapshot)
// ---------------------------------------------------------------------------

/// A point-in-time snapshot of cognitive tick statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CognitiveTickStats {
    /// Total number of ticks recorded.
    pub tick_count: u64,
    /// Current (possibly adapted) tick interval in milliseconds.
    pub current_interval_ms: u32,
    /// Running average compute time in microseconds.
    pub avg_compute_us: u64,
    /// Maximum observed compute time in microseconds.
    pub max_compute_us: u64,
    /// Number of ticks that exceeded the compute budget.
    pub drift_count: u64,
    /// Whether the tick loop is currently running.
    pub running: bool,
}

// ---------------------------------------------------------------------------
// Internal mutable state
// ---------------------------------------------------------------------------

struct CognitiveTickState {
    tick_count: u64,
    current_interval_ms: u32,
    running: bool,
    drift_count: u64,
    recent_timings_us: Vec<u64>,
    max_compute_us: u64,
}

// ---------------------------------------------------------------------------
// CognitiveTick
// ---------------------------------------------------------------------------

/// The cognitive tick service.
///
/// Tracks per-tick compute timings, detects budget drift, and optionally
/// adjusts the tick interval to maintain throughput under load.
pub struct CognitiveTick {
    config: CognitiveTickConfig,
    state: Mutex<CognitiveTickState>,
}

impl CognitiveTick {
    /// Create a new cognitive tick service with the given configuration.
    pub fn new(config: CognitiveTickConfig) -> Self {
        let interval = config.tick_interval_ms;
        Self {
            config,
            state: Mutex::new(CognitiveTickState {
                tick_count: 0,
                current_interval_ms: interval,
                running: false,
                drift_count: 0,
                recent_timings_us: Vec::new(),
                max_compute_us: 0,
            }),
        }
    }

    /// Convenience constructor that creates a default config with a custom interval.
    pub fn with_interval(interval_ms: u32) -> Self {
        Self::new(CognitiveTickConfig {
            tick_interval_ms: interval_ms,
            ..CognitiveTickConfig::default()
        })
    }

    /// Return a point-in-time snapshot of statistics.
    pub fn stats(&self) -> CognitiveTickStats {
        let s = self.state.lock().unwrap();
        let avg = if s.recent_timings_us.is_empty() {
            0
        } else {
            let sum: u64 = s.recent_timings_us.iter().sum();
            sum / s.recent_timings_us.len() as u64
        };
        CognitiveTickStats {
            tick_count: s.tick_count,
            current_interval_ms: s.current_interval_ms,
            avg_compute_us: avg,
            max_compute_us: s.max_compute_us,
            drift_count: s.drift_count,
            running: s.running,
        }
    }

    /// Record a tick with the given compute duration in microseconds.
    ///
    /// This method:
    /// 1. Increments the tick counter.
    /// 2. Maintains a sliding window of recent timings.
    /// 3. Updates the maximum observed compute time.
    /// 4. Detects budget drift (compute exceeding the budget).
    /// 5. Adaptively adjusts the tick interval if enabled.
    pub fn record_tick(&self, compute_us: u64) {
        let mut s = self.state.lock().unwrap();

        // 1. Increment tick count.
        s.tick_count += 1;

        // 2. Maintain sliding window.
        let window_size = self.window_capacity(s.current_interval_ms);
        s.recent_timings_us.push(compute_us);
        if s.recent_timings_us.len() > window_size {
            let excess = s.recent_timings_us.len() - window_size;
            s.recent_timings_us.drain(..excess);
        }

        // 3. Update max.
        if compute_us > s.max_compute_us {
            s.max_compute_us = compute_us;
        }

        // 4. Drift detection.
        let budget_us =
            (s.current_interval_ms as f32 * 1000.0 * self.config.tick_budget_ratio) as u64;
        if compute_us > budget_us {
            s.drift_count += 1;
        }

        // 5. Adaptive adjustment.
        if self.config.adaptive_tick && !s.recent_timings_us.is_empty() {
            let avg: u64 =
                s.recent_timings_us.iter().sum::<u64>() / s.recent_timings_us.len() as u64;
            let upper_threshold = (budget_us as f64 * 1.1) as u64;
            let lower_threshold = (budget_us as f64 * 0.5) as u64;

            if avg > upper_threshold {
                // Increase interval by 10%.
                let new_interval = (s.current_interval_ms as f64 * 1.1).round() as u32;
                s.current_interval_ms = new_interval;
            } else if avg < lower_threshold {
                // Decrease interval by 10%, minimum 10ms.
                let new_interval = (s.current_interval_ms as f64 * 0.9).round() as u32;
                s.current_interval_ms = new_interval.max(10);
            }
        }
    }

    /// Whether the tick loop is currently running.
    pub fn is_running(&self) -> bool {
        self.state.lock().unwrap().running
    }

    /// Set the running state.
    pub fn set_running(&self, running: bool) {
        self.state.lock().unwrap().running = running;
    }

    /// Total number of ticks recorded.
    pub fn tick_count(&self) -> u64 {
        self.state.lock().unwrap().tick_count
    }

    /// Current (possibly adapted) tick interval in milliseconds.
    pub fn current_interval_ms(&self) -> u32 {
        self.state.lock().unwrap().current_interval_ms
    }

    /// Number of ticks that exceeded the compute budget.
    pub fn drift_count(&self) -> u64 {
        self.state.lock().unwrap().drift_count
    }

    /// Reset all statistics to initial values (configuration is preserved).
    pub fn reset(&self) {
        let mut s = self.state.lock().unwrap();
        s.tick_count = 0;
        s.current_interval_ms = self.config.tick_interval_ms;
        s.running = false;
        s.drift_count = 0;
        s.recent_timings_us.clear();
        s.max_compute_us = 0;
    }

    // --- private helpers ---

    /// Compute the maximum number of timing samples to retain.
    fn window_capacity(&self, interval_ms: u32) -> usize {
        if interval_ms == 0 {
            return 1;
        }
        let ticks_per_window = (self.config.adaptive_window_s * 1000) / interval_ms;
        (ticks_per_window as usize).max(1)
    }
}

// ---------------------------------------------------------------------------
// SystemService implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl SystemService for CognitiveTick {
    fn name(&self) -> &str {
        "ecc.cognitive_tick"
    }

    fn service_type(&self) -> ServiceType {
        ServiceType::Core
    }

    async fn start(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.set_running(true);
        Ok(())
    }

    async fn stop(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.set_running(false);
        Ok(())
    }

    async fn health_check(&self) -> HealthStatus {
        if self.is_running() {
            HealthStatus::Healthy
        } else {
            HealthStatus::Degraded("cognitive tick not running".into())
        }
    }
}

// ---------------------------------------------------------------------------
// DEMOCRITUS two-tier coherence loop
// ---------------------------------------------------------------------------

/// Run the DEMOCRITUS two-tier coherence loop.
///
/// This is spawned as a background tokio task during kernel boot. On every
/// cognitive tick it performs:
///
/// 1. **SENSE** -- gather cheap graph metadata (node/edge counts).
/// 2. **THINK (fast)** -- O(1) EML coherence prediction.
/// 3. **DETECT DRIFT** -- compare prediction against last exact value.
/// 4. **THINK (exact)** -- O(k*m) Lanczos spectral analysis (only when drift
///    exceeds threshold, or periodically to maintain calibration).
/// 5. **LOG** -- drain EML events and append to ExoChain.
/// 6. **COMMIT** -- record timing in the adaptive tick system.
///
/// The EML model is retrained periodically from accumulated exact samples so
/// the fast path becomes more accurate over time.
#[cfg(feature = "ecc")]
pub async fn run_democritus_loop(
    tick: std::sync::Arc<CognitiveTick>,
    causal: std::sync::Arc<crate::causal::CausalGraph>,
    hnsw: std::sync::Arc<crate::hnsw_service::HnswService>,
    eml: std::sync::Arc<Mutex<crate::eml_coherence::EmlCoherenceModel>>,
) {
    run_democritus_loop_with_chain(tick, causal, hnsw, eml, None).await;
}

/// Run the DEMOCRITUS loop with optional ExoChain logging.
///
/// When `chain_manager` is `Some`, EML lifecycle events (training,
/// drift detection) are appended to the ExoChain audit trail.
#[cfg(feature = "ecc")]
pub async fn run_democritus_loop_with_chain(
    tick: std::sync::Arc<CognitiveTick>,
    causal: std::sync::Arc<crate::causal::CausalGraph>,
    hnsw: std::sync::Arc<crate::hnsw_service::HnswService>,
    eml: std::sync::Arc<Mutex<crate::eml_coherence::EmlCoherenceModel>>,
    chain_manager: Option<std::sync::Arc<crate::chain::ChainManager>>,
) {
    use crate::causal_predict::{detect_conversation_cycle, ConversationState};
    use crate::eml_coherence::GraphFeatures;
    use std::collections::VecDeque;
    use std::time::Instant;

    let drift_threshold = 0.05; // trigger exact when fast prediction drifts >5%
    let exact_every_n: u64 = 100; // force exact every 100 ticks regardless
    let train_every_n: usize = 1000; // retrain model every 1000 exact samples
    // Cycle-detector window. `coherence_history` stays capped at this
    // size (Finding #3): `detect_conversation_cycle` only reads the
    // tail anyway, so an unbounded Vec was pure leak.
    let cycle_window: usize = 20;
    // Steady-state suppression for the "stuck" warning (Finding #2).
    // Once we enter a Stuck/Oscillating phase we log once, then
    // exponentially back off: log on check 1, 2, 4, 8, 16, … up to
    // `stuck_suppress_cap` consecutive checks before logging again.
    // Entering/leaving the phase always logs (edge-triggered).
    let stuck_suppress_cap: u64 = 256;

    // `Option<f64>` sentinel instead of `last == 0.0` (Finding #1).
    // The old sentinel stayed true forever on an empty causal graph
    // because `spectral_analysis` returned lambda_2 = 0.0 every time,
    // so `needs_exact` was always true and Lanczos ran every tick.
    let mut last_exact_coherence: Option<f64> = None;
    let mut ticks_since_exact: u64 = 0;
    let mut coherence_history: VecDeque<f64> = VecDeque::with_capacity(cycle_window);
    let mut exact_tick_count: u64 = 0;
    // Edge-trigger + suppression state for the stuck-warning.
    let mut in_stuck_phase = false;
    let mut stuck_checks_since_log: u64 = 0;
    let mut next_log_at: u64 = 1;
    // Edge-trigger for the idle-graph notice. While the causal graph
    // has fewer than 2 nodes the spectral path is structurally
    // incapable of producing a non-zero `lambda_2` (see
    // `CausalGraph::spectral_analysis_rff` early-return), so any
    // cycle-detector verdict on that flat history is meaningless.
    // We log the transition once on entry and once on exit instead
    // of letting the cycle-detector emit `Stuck { net_change: 0.0 }`
    // forever (release-gate audit
    // `.planning/reviews/0.7.0-release-gate/02-kernel-governance.md`
    // line 510, `17-research-streams.md` line 183).
    let idle_node_threshold: u64 = 2;
    let mut in_idle_phase = false;

    tick.set_running(true);
    tracing::info!(
        "DEMOCRITUS loop started (drift_threshold={drift_threshold}, \
         exact_every_n={exact_every_n}, train_every_n={train_every_n})"
    );

    loop {
        let interval_ms = tick.current_interval_ms();
        if interval_ms == 0 {
            tracing::warn!("DEMOCRITUS loop: tick interval is 0, exiting");
            break;
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(interval_ms as u64)).await;

        if !tick.is_running() {
            tracing::info!("DEMOCRITUS loop: tick stopped, exiting");
            break;
        }

        let start = Instant::now();

        // ── SENSE ──────────────────────────────────────────────────────
        // Gather graph state (cheap metadata, no locking on DashMap reads).
        let _node_count = causal.node_count();
        let _edge_count = causal.edge_count();
        let _hnsw_count = hnsw.len();

        // ── THINK (fast path) ──────────────────────────────────────────
        // O(1) EML coherence prediction from graph features.
        let features = GraphFeatures::from_causal_graph(&causal);
        let prediction = match eml.lock() {
            Ok(model) => model.predict(&features),
            Err(poisoned) => {
                tracing::error!("DEMOCRITUS loop: EML model lock poisoned, exiting");
                // Clear the poison so other consumers can proceed.
                let model = poisoned.into_inner();
                model.predict(&features)
                // Cannot continue safely after a poisoned lock in a loop;
                // the next iteration would hit the same poison. Break out.
            }
        };

        // ── DETECT DRIFT ───────────────────────────────────────────────
        ticks_since_exact += 1;
        let drift = match last_exact_coherence {
            Some(last) => (prediction.lambda_2 - last).abs(),
            None => f64::INFINITY, // first tick — force exact once
        };
        let needs_exact = last_exact_coherence.is_none()
            || drift > drift_threshold
            || ticks_since_exact >= exact_every_n;

        if needs_exact {
            // ── THINK (exact path) ─────────────────────────────────────
            // On steady state this runs once every `exact_every_n`
            // ticks (or when EML drift exceeds threshold). We use the
            // random-Fourier-features Laplacian estimator (O(m) per
            // feature vector, ~3–6x faster than Lanczos at large m)
            // as the routine path; ground-truth Lanczos stays
            // available for whoever wants it. (Finding #4)
            let spectral = causal.spectral_analysis_rff(64, 50);
            let exact_lambda_2 = spectral.lambda_2;

            let is_first = last_exact_coherence.is_none();
            if is_first {
                tracing::info!(
                    lambda_2 = exact_lambda_2,
                    "DEMOCRITUS: first exact coherence computed"
                );
            } else if drift > drift_threshold {
                tracing::debug!(
                    predicted = prediction.lambda_2,
                    exact = exact_lambda_2,
                    drift = drift,
                    "DEMOCRITUS: drift detected, ran exact spectral analysis"
                );
            }

            last_exact_coherence = Some(exact_lambda_2);
            ticks_since_exact = 0;
            exact_tick_count += 1;

            // Idle-graph gate. If the causal graph is too small to
            // produce a meaningful spectral signal, skip cycle
            // detection: pushing 0.0 samples into history is what
            // forces the detector into `Stuck { net_change: 0.0 }`
            // for the lifetime of an empty daemon. We also clear
            // any accumulated history so that when the graph wakes
            // up the detector starts from real measurements rather
            // than a buffer full of zero sentinels.
            let graph_is_idle = causal.node_count() < idle_node_threshold;
            if graph_is_idle {
                if !in_idle_phase {
                    tracing::info!(
                        node_count = causal.node_count(),
                        "DEMOCRITUS: causal graph idle (n<2), suspending cycle detection"
                    );
                    in_idle_phase = true;
                    coherence_history.clear();
                    in_stuck_phase = false;
                    stuck_checks_since_log = 0;
                    next_log_at = 1;
                }
            } else {
                if in_idle_phase {
                    tracing::info!(
                        node_count = causal.node_count(),
                        "DEMOCRITUS: causal graph active, resuming cycle detection"
                    );
                    in_idle_phase = false;
                }

                // Track bounded coherence history for cycle detection
                // (Finding #3). Drop the oldest sample if we've hit the
                // window cap so the buffer can't grow without bound.
                if coherence_history.len() == cycle_window {
                    coherence_history.pop_front();
                }
                coherence_history.push_back(exact_lambda_2);
            }

            // Run the cycle detector once per `cycle_window` exact
            // measurements, and rate-limit the warning on steady-state
            // stuck/oscillating phases with an exponential-backoff
            // suppression counter (Finding #2). Entering and leaving
            // the stuck phase are always logged (edge-triggered); the
            // in-phase warnings drop off geometrically so the log
            // doesn't drown.
            if !graph_is_idle
                && coherence_history.len() >= cycle_window
                && exact_tick_count.is_multiple_of(cycle_window as u64)
            {
                let history_slice: Vec<f64> =
                    coherence_history.iter().copied().collect();
                let state = detect_conversation_cycle(&history_slice, cycle_window, 0.01);
                let is_stuck = matches!(
                    state,
                    ConversationState::Stuck { .. }
                        | ConversationState::Oscillating { .. }
                );

                match (in_stuck_phase, is_stuck) {
                    (false, true) => {
                        tracing::warn!(
                            "DEMOCRITUS: conversation entered stuck phase: {:?}",
                            state
                        );
                        in_stuck_phase = true;
                        stuck_checks_since_log = 0;
                        next_log_at = 1;
                    }
                    (true, true) => {
                        stuck_checks_since_log += 1;
                        if stuck_checks_since_log >= next_log_at {
                            tracing::warn!(
                                "DEMOCRITUS: still stuck after {} checks: {:?}",
                                stuck_checks_since_log,
                                state
                            );
                            stuck_checks_since_log = 0;
                            next_log_at = (next_log_at.saturating_mul(2))
                                .min(stuck_suppress_cap);
                        }
                    }
                    (true, false) => {
                        tracing::info!(
                            "DEMOCRITUS: conversation left stuck phase: {:?}",
                            state
                        );
                        in_stuck_phase = false;
                        stuck_checks_since_log = 0;
                        next_log_at = 1;
                    }
                    (false, false) => { /* healthy steady state */ }
                }
            }

            // Record training data for the EML model.
            match eml.lock() {
                Ok(mut model) => {
                    model.record(features, exact_lambda_2);

                    // Retrain periodically once we have enough samples.
                    let sample_count = model.training_sample_count();
                    if sample_count >= 50
                        && sample_count % train_every_n == 0
                    {
                        let converged = model.train();
                        tracing::info!(
                            sample_count = sample_count,
                            converged = converged,
                            "DEMOCRITUS: EML model retrained"
                        );
                    }
                }
                Err(_) => {
                    tracing::error!(
                        "DEMOCRITUS loop: EML model lock poisoned during record, exiting"
                    );
                    break;
                }
            }
        }

        // ── LOG (ExoChain) ─────────────────────────────────────────────
        // Drain EML lifecycle events and append to the chain.
        if let Some(ref cm) = chain_manager
            && let Ok(mut model) = eml.lock() {
                for event in model.drain_events() {
                    cm.append(
                        "eml",
                        event.event_type(),
                        Some(serde_json::to_value(&event).unwrap_or_default()),
                    );
                }
            }

        // ── COMMIT ─────────────────────────────────────────────────────
        // Record timing in the adaptive tick system.
        let compute_us = start.elapsed().as_micros() as u64;
        tick.record_tick(compute_us);
    }

    tick.set_running(false);
    tracing::info!("DEMOCRITUS loop exited");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let cfg = CognitiveTickConfig::default();
        assert_eq!(cfg.tick_interval_ms, 50);
        assert!((cfg.tick_budget_ratio - 0.3).abs() < f32::EPSILON);
        assert_eq!(cfg.calibration_ticks, 100);
        assert!(cfg.adaptive_tick);
        assert_eq!(cfg.adaptive_window_s, 30);
    }

    #[test]
    fn new_with_config() {
        let cfg = CognitiveTickConfig {
            tick_interval_ms: 100,
            tick_budget_ratio: 0.5,
            calibration_ticks: 200,
            adaptive_tick: false,
            adaptive_window_s: 60,
        };
        let ct = CognitiveTick::new(cfg.clone());
        assert_eq!(ct.current_interval_ms(), 100);
        assert!(!ct.is_running());
    }

    #[test]
    fn with_interval() {
        let ct = CognitiveTick::with_interval(75);
        assert_eq!(ct.current_interval_ms(), 75);
        // Other fields should be defaults.
        assert!(ct.config.adaptive_tick);
        assert_eq!(ct.config.calibration_ticks, 100);
    }

    #[test]
    fn stats_initial() {
        let ct = CognitiveTick::new(CognitiveTickConfig::default());
        let s = ct.stats();
        assert_eq!(s.tick_count, 0);
        assert_eq!(s.current_interval_ms, 50);
        assert_eq!(s.avg_compute_us, 0);
        assert_eq!(s.max_compute_us, 0);
        assert_eq!(s.drift_count, 0);
        assert!(!s.running);
    }

    #[test]
    fn record_tick_increments_count() {
        let ct = CognitiveTick::new(CognitiveTickConfig::default());
        ct.record_tick(100);
        ct.record_tick(200);
        ct.record_tick(300);
        assert_eq!(ct.tick_count(), 3);
    }

    #[test]
    fn record_tick_updates_max() {
        let ct = CognitiveTick::new(CognitiveTickConfig::default());
        ct.record_tick(100);
        ct.record_tick(500);
        ct.record_tick(200);
        assert_eq!(ct.stats().max_compute_us, 500);
    }

    #[test]
    fn is_running_default_false() {
        let ct = CognitiveTick::new(CognitiveTickConfig::default());
        assert!(!ct.is_running());
    }

    #[test]
    fn set_running() {
        let ct = CognitiveTick::new(CognitiveTickConfig::default());
        ct.set_running(true);
        assert!(ct.is_running());
        ct.set_running(false);
        assert!(!ct.is_running());
    }

    #[test]
    fn tick_count() {
        let ct = CognitiveTick::new(CognitiveTickConfig::default());
        assert_eq!(ct.tick_count(), 0);
        ct.record_tick(10);
        assert_eq!(ct.tick_count(), 1);
    }

    #[test]
    fn current_interval_ms() {
        let ct = CognitiveTick::with_interval(42);
        assert_eq!(ct.current_interval_ms(), 42);
    }

    #[test]
    fn drift_detection() {
        // Default: interval=50ms, budget_ratio=0.3 => budget = 50*1000*0.3 = 15000us
        let mut cfg = CognitiveTickConfig::default();
        cfg.adaptive_tick = false; // disable adaptive so interval stays constant
        let ct = CognitiveTick::new(cfg);

        // Under budget: no drift.
        ct.record_tick(10_000);
        assert_eq!(ct.drift_count(), 0);

        // Exactly at budget boundary (15000): not exceeding, no drift.
        ct.record_tick(15_000);
        assert_eq!(ct.drift_count(), 0);

        // Over budget.
        ct.record_tick(16_000);
        assert_eq!(ct.drift_count(), 1);

        // Another over budget.
        ct.record_tick(20_000);
        assert_eq!(ct.drift_count(), 2);
    }

    #[test]
    fn adaptive_increase() {
        // Set up a config where budget is small so we can easily exceed 1.1x.
        // interval=50ms, ratio=0.3 => budget = 15000us, upper = 16500us
        let cfg = CognitiveTickConfig {
            tick_interval_ms: 50,
            tick_budget_ratio: 0.3,
            calibration_ticks: 100,
            adaptive_tick: true,
            adaptive_window_s: 30,
        };
        let ct = CognitiveTick::new(cfg);

        // Record many ticks with compute well above the upper threshold (16500us).
        for _ in 0..20 {
            ct.record_tick(20_000);
        }

        // Interval should have increased from 50.
        assert!(
            ct.current_interval_ms() > 50,
            "expected interval > 50, got {}",
            ct.current_interval_ms()
        );
    }

    #[test]
    fn adaptive_decrease() {
        // interval=100ms, ratio=0.3 => budget = 30000us, lower = 15000us
        let cfg = CognitiveTickConfig {
            tick_interval_ms: 100,
            tick_budget_ratio: 0.3,
            calibration_ticks: 100,
            adaptive_tick: true,
            adaptive_window_s: 30,
        };
        let ct = CognitiveTick::new(cfg);

        // Record many ticks with compute well below the lower threshold.
        for _ in 0..20 {
            ct.record_tick(1_000);
        }

        // Interval should have decreased from 100.
        assert!(
            ct.current_interval_ms() < 100,
            "expected interval < 100, got {}",
            ct.current_interval_ms()
        );
    }

    #[test]
    fn adaptive_min_interval() {
        // Start with a small interval so it can shrink toward the minimum.
        let cfg = CognitiveTickConfig {
            tick_interval_ms: 12,
            tick_budget_ratio: 0.3,
            calibration_ticks: 100,
            adaptive_tick: true,
            adaptive_window_s: 30,
        };
        let ct = CognitiveTick::new(cfg);

        // Record very fast ticks to push the interval down.
        for _ in 0..200 {
            ct.record_tick(1);
        }

        // Interval must never go below 10ms.
        assert!(
            ct.current_interval_ms() >= 10,
            "expected interval >= 10, got {}",
            ct.current_interval_ms()
        );
    }

    #[test]
    fn reset_clears_stats() {
        let ct = CognitiveTick::with_interval(80);
        ct.set_running(true);
        ct.record_tick(5_000);
        ct.record_tick(50_000);

        // Verify non-zero state.
        assert!(ct.tick_count() > 0);
        assert!(ct.stats().max_compute_us > 0);
        assert!(ct.is_running());

        ct.reset();

        assert_eq!(ct.tick_count(), 0);
        assert_eq!(ct.stats().max_compute_us, 0);
        assert_eq!(ct.stats().avg_compute_us, 0);
        assert_eq!(ct.drift_count(), 0);
        assert!(!ct.is_running());
        // Interval should be reset to config value.
        assert_eq!(ct.current_interval_ms(), 80);
    }

    #[tokio::test]
    async fn service_name_and_type() {
        let ct = CognitiveTick::new(CognitiveTickConfig::default());
        assert_eq!(ct.name(), "ecc.cognitive_tick");
        assert_eq!(ct.service_type(), ServiceType::Core);
    }

    #[tokio::test]
    async fn service_start_stop() {
        let ct = CognitiveTick::new(CognitiveTickConfig::default());
        assert!(!ct.is_running());

        ct.start().await.unwrap();
        assert!(ct.is_running());

        ct.stop().await.unwrap();
        assert!(!ct.is_running());
    }

    #[tokio::test]
    async fn health_check_reflects_running() {
        let ct = CognitiveTick::new(CognitiveTickConfig::default());
        assert_eq!(
            ct.health_check().await,
            HealthStatus::Degraded("cognitive tick not running".into())
        );

        ct.start().await.unwrap();
        assert_eq!(ct.health_check().await, HealthStatus::Healthy);
    }

    #[test]
    fn config_serde_roundtrip() {
        let cfg = CognitiveTickConfig::default();
        let json = serde_json::to_string(&cfg).unwrap();
        let restored: CognitiveTickConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.tick_interval_ms, cfg.tick_interval_ms);
        assert!((restored.tick_budget_ratio - cfg.tick_budget_ratio).abs() < f32::EPSILON);
    }

    #[test]
    fn stats_serde_roundtrip() {
        let ct = CognitiveTick::new(CognitiveTickConfig::default());
        ct.record_tick(1234);
        let stats = ct.stats();
        let json = serde_json::to_string(&stats).unwrap();
        let restored: CognitiveTickStats = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.tick_count, 1);
        assert_eq!(restored.avg_compute_us, 1234);
    }
}
