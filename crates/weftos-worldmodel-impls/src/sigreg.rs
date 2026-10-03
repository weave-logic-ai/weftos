//! SIGReg health monitor with online Welford stats + auto-rollback (WEFT-528).
//!
//! Measures how well streaming latents match the isotropic Gaussian prior
//! `N(0, I_192)`. When `sigreg_health` stays below
//! [`SIGREG_HEALTH_ROLLBACK_THRESHOLD`](0.85) for
//! [`SIGREG_HEALTH_WINDOW_SECS`](30), the monitor triggers auto-rollback:
//! resets running stats and bumps `version_tag` (ExoChain-attestable).

use weftos_worldmodel_core::{
    zero_latent, Latent, SigRegHealth, SigRegMonitor, WorldModelResult, LATENT_DIM,
    SIGREG_HEALTH_ROLLBACK_THRESHOLD,
};

/// Stub monitor: always reports perfect health.
#[derive(Debug, Clone, Copy, Default)]
pub struct NullSigRegMonitor {
    /// Version tag reported in health snapshots (ExoChain attestation).
    pub version_tag: u64,
}

impl SigRegMonitor for NullSigRegMonitor {
    fn update(&mut self, _z: &Latent) -> WorldModelResult<SigRegHealth> {
        Ok(self.health())
    }

    fn health(&self) -> SigRegHealth {
        SigRegHealth::perfect(self.version_tag)
    }
}

/// One health / rollback event for metrics and ExoChain logging.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SigRegHealthEvent {
    /// Health snapshot at the event.
    pub health: SigRegHealth,
    /// True when the auto-rollback gate fired on this update.
    pub rolled_back: bool,
    /// Sample count after the update (0 after rollback reset).
    pub sample_count: u64,
}

/// Online Welford monitor over the 192-d SIGReg manifold.
///
/// # Health score
///
/// For prior `N(0, I)` we want mean ≈ 0 and per-dim variance ≈ 1. After a
/// warm-up of [`WelfordSigRegMonitor::min_samples`]:
///
/// ```text
/// mean_term = mean_i (μ_i²)
/// var_term  = mean_i (σ_i² − 1)²
/// score     = 1 / (1 + mean_term + var_term)   ∈ (0, 1]
/// ```
///
/// Scores are clamped to `[0, 1]`. With fewer than `min_samples` samples the
/// monitor reports perfect health so cold start does not false-trip rollback.
///
/// # Time window
///
/// Callers should prefer [`WelfordSigRegMonitor::update_at`] with a monotonic
/// millisecond clock. When using [`SigRegMonitor::update`] alone, each call
/// advances the unhealthy timer by `default_dt_ms` (default 1000 ms ⇒ one
/// sample per second).
#[derive(Debug, Clone)]
pub struct WelfordSigRegMonitor {
    /// Model / encoder version tag (bumped on auto-rollback).
    pub version_tag: u64,
    /// Minimum samples before score can leave 1.0 (warm-up).
    pub min_samples: u64,
    /// Default Δt in ms when [`SigRegMonitor::update`] is used without a clock.
    pub default_dt_ms: u64,
    /// Number of samples ingested since last rollback.
    n: u64,
    /// Running mean per dimension.
    mean: Latent,
    /// Welford M2 (sum of squared deviations) per dimension.
    m2: Latent,
    /// Current health score in `[0, 1]`.
    score: f32,
    /// Consecutive milliseconds spent below the rollback threshold.
    ms_below_threshold: u64,
    /// Last sample timestamp (ms), if known.
    last_ts_ms: Option<u64>,
    /// True if the last update triggered auto-rollback.
    last_rolled_back: bool,
    /// Total rollback events since construction.
    pub rollback_count: u64,
    /// Last event for hosts that log to ExoChain / metrics.
    pub last_event: Option<SigRegHealthEvent>,
}

impl Default for WelfordSigRegMonitor {
    fn default() -> Self {
        Self::new(1)
    }
}

impl WelfordSigRegMonitor {
    /// Construct with an initial version tag.
    pub fn new(version_tag: u64) -> Self {
        Self {
            version_tag,
            min_samples: 8,
            default_dt_ms: 1000,
            n: 0,
            mean: zero_latent(),
            m2: zero_latent(),
            score: 1.0,
            ms_below_threshold: 0,
            last_ts_ms: None,
            last_rolled_back: false,
            rollback_count: 0,
            last_event: None,
        }
    }

    /// Sample count since last rollback.
    pub fn sample_count(&self) -> u64 {
        self.n
    }

    /// Whether the last update fired auto-rollback.
    pub fn last_rolled_back(&self) -> bool {
        self.last_rolled_back
    }

    /// Running mean (copy).
    pub fn mean(&self) -> Latent {
        self.mean
    }

    /// Per-dimension sample variance (`m2 / (n-1)` for n ≥ 2).
    pub fn variance(&self) -> Latent {
        let mut v = zero_latent();
        if self.n >= 2 {
            let denom = (self.n - 1) as f32;
            for (vi, m2) in v.iter_mut().zip(self.m2.iter()) {
                *vi = *m2 / denom;
            }
        }
        v
    }

    /// Ingest a latent with an explicit timestamp (milliseconds, monotonic).
    pub fn update_at(&mut self, z: &Latent, timestamp_ms: u64) -> WorldModelResult<SigRegHealth> {
        let dt_ms = match self.last_ts_ms {
            Some(prev) if timestamp_ms >= prev => timestamp_ms - prev,
            _ => self.default_dt_ms,
        };
        self.last_ts_ms = Some(timestamp_ms);
        self.ingest(z, dt_ms)
    }

    fn ingest(&mut self, z: &Latent, dt_ms: u64) -> WorldModelResult<SigRegHealth> {
        self.last_rolled_back = false;

        // Welford online update.
        self.n = self.n.saturating_add(1);
        let n_f = self.n as f32;
        for (i, &zi) in z.iter().enumerate() {
            let delta = zi - self.mean[i];
            self.mean[i] += delta / n_f;
            let delta2 = zi - self.mean[i];
            self.m2[i] += delta * delta2;
        }

        self.score = self.compute_score();
        let unhealthy = self.score < SIGREG_HEALTH_ROLLBACK_THRESHOLD;
        if unhealthy {
            self.ms_below_threshold = self.ms_below_threshold.saturating_add(dt_ms.max(1));
        } else {
            self.ms_below_threshold = 0;
        }

        let mut health = self.snapshot();
        if health.should_rollback() {
            self.perform_rollback();
            health = self.snapshot();
            self.last_rolled_back = true;
        }

        self.last_event = Some(SigRegHealthEvent {
            health,
            rolled_back: self.last_rolled_back,
            sample_count: self.n,
        });
        Ok(health)
    }

    fn compute_score(&self) -> f32 {
        // Inclusive warm-up: first `min_samples` observations report perfect.
        if self.n <= self.min_samples {
            return 1.0;
        }
        let mut mean_term = 0.0f32;
        let mut var_term = 0.0f32;
        let var = self.variance();
        for (m, v) in self.mean.iter().zip(var.iter()) {
            mean_term += m * m;
            let dv = v - 1.0;
            var_term += dv * dv;
        }
        mean_term /= LATENT_DIM as f32;
        var_term /= LATENT_DIM as f32;
        // Map non-negative energy into (0, 1].
        let score = 1.0 / (1.0 + mean_term + var_term);
        score.clamp(0.0, 1.0)
    }

    fn snapshot(&self) -> SigRegHealth {
        let secs = (self.ms_below_threshold / 1000) as u32;
        SigRegHealth {
            score: self.score,
            seconds_below_threshold: secs,
            version_tag: self.version_tag,
        }
    }

    fn perform_rollback(&mut self) {
        self.version_tag = self.version_tag.saturating_add(1);
        self.rollback_count = self.rollback_count.saturating_add(1);
        self.n = 0;
        self.mean = zero_latent();
        self.m2 = zero_latent();
        self.score = 1.0;
        self.ms_below_threshold = 0;
        // Keep last_ts_ms so next dt is still meaningful.
    }
}

impl SigRegMonitor for WelfordSigRegMonitor {
    fn update(&mut self, z: &Latent) -> WorldModelResult<SigRegHealth> {
        // Synthetic clock: advance by default_dt_ms each call.
        let ts = self
            .last_ts_ms
            .map(|t| t.saturating_add(self.default_dt_ms))
            .unwrap_or(0);
        self.update_at(z, ts)
    }

    fn health(&self) -> SigRegHealth {
        self.snapshot()
    }
}

/// JSON-friendly payload for ExoChain / metrics logging of SIGReg health.
#[derive(Debug, Clone, PartialEq)]
pub struct SigRegHealthLog {
    /// Event kind string for chain append.
    pub kind: &'static str,
    /// Health score.
    pub score: f32,
    /// Seconds below threshold.
    pub seconds_below_threshold: u32,
    /// Version tag.
    pub version_tag: u64,
    /// Whether rollback fired.
    pub rolled_back: bool,
    /// Sample count.
    pub sample_count: u64,
}

/// Canonical ExoChain event kind for SIGReg health (mirrors kernel constant).
pub const EVENT_KIND_SIGREG_HEALTH: &str = "lewm.sigreg.health";

impl SigRegHealthEvent {
    /// Build a log record for chain / metrics sinks.
    pub fn to_log(&self) -> SigRegHealthLog {
        SigRegHealthLog {
            kind: EVENT_KIND_SIGREG_HEALTH,
            score: self.health.score,
            seconds_below_threshold: self.health.seconds_below_threshold,
            version_tag: self.health.version_tag,
            rolled_back: self.rolled_back,
            sample_count: self.sample_count,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_spike(i: usize, v: f32) -> Latent {
        let mut z = zero_latent();
        z[i] = v;
        z
    }

    #[test]
    fn null_perfect() {
        let mon = NullSigRegMonitor { version_tag: 7 };
        assert_eq!(mon.health().version_tag, 7);
        assert!(mon.health().is_healthy());
        let mut mon = mon;
        assert!(mon.update(&zero_latent()).unwrap().is_healthy());
    }

    #[test]
    fn welford_zero_latent_is_unhealthy_after_warmup() {
        // Pure zeros have mean 0 but variance 0 (not 1) → score = 1/(1+0+1) = 0.5.
        let mut mon = WelfordSigRegMonitor::new(1);
        mon.min_samples = 4;
        for t in 0..4u64 {
            let h = mon.update_at(&zero_latent(), t * 1000).unwrap();
            // Warm-up: report perfect health.
            assert!(h.is_healthy(), "warm-up score={}", h.score);
        }
        let h = mon.update_at(&zero_latent(), 5_000).unwrap();
        assert!(
            h.score < SIGREG_HEALTH_ROLLBACK_THRESHOLD,
            "zeros should look off-manifold: score={}",
            h.score
        );
    }

    #[test]
    fn welford_isotropic_samples_healthy() {
        let mut mon = WelfordSigRegMonitor::new(1);
        mon.min_samples = 4;
        // Deterministic pseudo-samples with mean~0 and var~1.
        for t in 0..40u64 {
            let mut z = zero_latent();
            for i in 0..LATENT_DIM {
                // Alternating ±1 pattern across time and dim → var ≈ 1, mean ≈ 0.
                let sign = if (t as usize + i) % 2 == 0 { 1.0 } else { -1.0 };
                z[i] = sign;
            }
            let h = mon.update_at(&z, t * 1000).unwrap();
            if t >= mon.min_samples {
                assert!(
                    h.score >= SIGREG_HEALTH_ROLLBACK_THRESHOLD,
                    "t={t} score={}",
                    h.score
                );
            }
        }
        assert_eq!(mon.rollback_count, 0);
    }

    #[test]
    fn forced_low_health_triggers_rollback_after_30s() {
        let mut mon = WelfordSigRegMonitor::new(10);
        mon.min_samples = 2;
        mon.default_dt_ms = 1000;

        // Huge constant bias → mean far from 0, variance ~0 → score very low.
        let bad = unit_spike(0, 50.0);
        let mut rolled = false;
        let mut last = SigRegHealth::perfect(10);
        // 2 warm-up + 30 unhealthy seconds.
        for t in 0..40u64 {
            last = mon.update_at(&bad, t * 1000).unwrap();
            if mon.last_rolled_back() {
                rolled = true;
                break;
            }
        }
        assert!(rolled, "expected auto-rollback, last={last:?}");
        assert!(mon.rollback_count >= 1);
        // Version tag bumped on rollback.
        assert!(mon.version_tag > 10);
        let log = mon.last_event.unwrap().to_log();
        assert_eq!(log.kind, EVENT_KIND_SIGREG_HEALTH);
        assert!(log.rolled_back);
    }

    #[test]
    fn health_recovers_timer_when_score_rises() {
        let mut mon = WelfordSigRegMonitor::new(1);
        mon.min_samples = 2;
        // Seed unhealthy briefly.
        for t in 0..5u64 {
            let _ = mon.update_at(&unit_spike(0, 20.0), t * 1000).unwrap();
        }
        // If not yet rolled back, timer should be non-zero while unhealthy.
        if mon.rollback_count == 0 {
            assert!(mon.health().seconds_below_threshold > 0 || mon.n < mon.min_samples);
        }
        // Feed good ±1 samples for a while.
        for t in 10..50u64 {
            let mut z = zero_latent();
            for i in 0..LATENT_DIM {
                z[i] = if (t as usize + i) % 2 == 0 { 1.0 } else { -1.0 };
            }
            let h = mon.update_at(&z, t * 1000).unwrap();
            if h.is_healthy() {
                assert_eq!(h.seconds_below_threshold, 0);
            }
        }
    }

    #[test]
    fn should_rollback_gate_constants() {
        use weftos_worldmodel_core::SIGREG_HEALTH_WINDOW_SECS;
        assert!((SIGREG_HEALTH_ROLLBACK_THRESHOLD - 0.85).abs() < f32::EPSILON);
        assert_eq!(SIGREG_HEALTH_WINDOW_SECS, 30);
    }
}
