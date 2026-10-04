//! Per-session adaptive silence-timeout estimator (WEFT-230).
//!
//! Product VAD historically used a fixed `silence_timeout_ms` (default
//! 1500). Speakers with long mid-phrase pauses get truncated; snappy
//! speakers wait too long before the turn ends. This estimator keeps a
//! **rolling window** of observed end-of-utterance silence needs and
//! speech durations, blends them toward the user's pattern with an EMA,
//! and clamps the result to a sensible range (default 500–3000 ms).
//!
//! Pure logic — no microphone, no async. Callers (plugin VAD stub,
//! `EnergyVad`, Talk Mode) feed observations after each utterance.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

/// Tunables for adaptive silence-timeout learning (nested under
/// [`super::VadConfig::adaptive`]).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AdaptiveSilenceConfig {
    /// When `false`, the estimator always returns the baseline.
    #[serde(default = "default_adaptive_enabled")]
    pub enabled: bool,

    /// Floor for the learned timeout (ms). Default 500.
    #[serde(default = "default_adaptive_min_ms", alias = "minMs")]
    pub min_ms: u32,

    /// Ceiling for the learned timeout (ms). Default 3000.
    #[serde(default = "default_adaptive_max_ms", alias = "maxMs")]
    pub max_ms: u32,

    /// Rolling observation window size. Default 8.
    #[serde(default = "default_adaptive_window_size", alias = "windowSize")]
    pub window_size: usize,

    /// EMA blend of a new observation into the estimate (`0.0`–`1.0`).
    /// Default 0.35 — responsive but not jumpy.
    #[serde(default = "default_adaptive_learning_rate", alias = "learningRate")]
    pub learning_rate: f32,

    /// Extra ms added on top of the longest observed *intra-utterance*
    /// pause when deriving a target timeout. Default 250.
    #[serde(default = "default_adaptive_pause_margin_ms", alias = "pauseMarginMs")]
    pub pause_margin_ms: u32,
}

fn default_adaptive_enabled() -> bool {
    true
}
fn default_adaptive_min_ms() -> u32 {
    500
}
fn default_adaptive_max_ms() -> u32 {
    3_000
}
fn default_adaptive_window_size() -> usize {
    8
}
fn default_adaptive_learning_rate() -> f32 {
    0.35
}
fn default_adaptive_pause_margin_ms() -> u32 {
    250
}

impl Default for AdaptiveSilenceConfig {
    fn default() -> Self {
        Self {
            enabled: default_adaptive_enabled(),
            min_ms: default_adaptive_min_ms(),
            max_ms: default_adaptive_max_ms(),
            window_size: default_adaptive_window_size(),
            learning_rate: default_adaptive_learning_rate(),
            pause_margin_ms: default_adaptive_pause_margin_ms(),
        }
    }
}

impl AdaptiveSilenceConfig {
    /// Clamp min/max/window/learning_rate to safe ranges (defensive).
    pub fn sanitized(&self) -> Self {
        let mut min_ms = self.min_ms.max(50);
        let mut max_ms = self.max_ms.max(min_ms);
        if max_ms < min_ms {
            std::mem::swap(&mut min_ms, &mut max_ms);
        }
        Self {
            enabled: self.enabled,
            min_ms,
            max_ms,
            window_size: self.window_size.clamp(1, 64),
            learning_rate: self.learning_rate.clamp(0.01, 1.0),
            pause_margin_ms: self.pause_margin_ms.min(2_000),
        }
    }
}

/// Per-session silence-timeout estimator.
///
/// Start from a config baseline ([`Self::from_baseline`]); feed
/// [`Self::record_utterance`] after each end-of-speech; read the live
/// timeout with [`Self::current_ms`].
#[derive(Debug, Clone)]
pub struct AdaptiveSilenceTimeout {
    baseline_ms: u32,
    cfg: AdaptiveSilenceConfig,
    /// Rolling window of derived target timeouts (ms).
    observations: VecDeque<u32>,
    /// EMA of the learned timeout (ms as f32 for smooth updates).
    estimate_ms: f32,
}

impl AdaptiveSilenceTimeout {
    /// Build an estimator from a baseline silence timeout and adaptive
    /// tunables. Baseline is clamped into `[min, max]`.
    pub fn new(baseline_ms: u32, cfg: AdaptiveSilenceConfig) -> Self {
        let cfg = cfg.sanitized();
        let baseline_ms = baseline_ms.clamp(cfg.min_ms, cfg.max_ms);
        Self {
            baseline_ms,
            estimate_ms: baseline_ms as f32,
            cfg,
            observations: VecDeque::new(),
        }
    }

    /// Convenience: baseline only, default adaptive config.
    pub fn from_baseline(baseline_ms: u32) -> Self {
        Self::new(baseline_ms, AdaptiveSilenceConfig::default())
    }

    /// Fixed (non-learning) estimator that always returns `baseline_ms`
    /// clamped to the default min/max. Useful when adaptive is disabled.
    pub fn fixed(baseline_ms: u32) -> Self {
        Self::new(
            baseline_ms,
            AdaptiveSilenceConfig {
                enabled: false,
                ..AdaptiveSilenceConfig::default()
            },
        )
    }

    /// Baseline the estimator was constructed with.
    pub fn baseline_ms(&self) -> u32 {
        self.baseline_ms
    }

    /// Adaptive tunables (sanitized).
    pub fn config(&self) -> &AdaptiveSilenceConfig {
        &self.cfg
    }

    /// Whether learning is active.
    pub fn enabled(&self) -> bool {
        self.cfg.enabled
    }

    /// Number of observations currently in the rolling window.
    pub fn observation_count(&self) -> usize {
        self.observations.len()
    }

    /// Current silence timeout to apply (clamped, rounded).
    ///
    /// With learning disabled, always returns the baseline. With no
    /// observations yet, also returns the baseline (warm start).
    pub fn current_ms(&self) -> u32 {
        if !self.cfg.enabled || self.observations.is_empty() {
            return self.baseline_ms;
        }
        self.estimate_ms
            .round()
            .clamp(self.cfg.min_ms as f32, self.cfg.max_ms as f32) as u32
    }

    /// Record a raw target timeout observation (already includes any
    /// margin the caller wants). Clamped to `[min, max]` then EMA-blended.
    pub fn record_observation(&mut self, target_ms: u32) {
        if !self.cfg.enabled {
            return;
        }
        let target = target_ms.clamp(self.cfg.min_ms, self.cfg.max_ms);
        self.push_observation(target);
        let alpha = self.cfg.learning_rate;
        self.estimate_ms = (1.0 - alpha) * self.estimate_ms + alpha * (target as f32);
        self.estimate_ms = self
            .estimate_ms
            .clamp(self.cfg.min_ms as f32, self.cfg.max_ms as f32);
    }

    /// Primary learning hook after an utterance ends.
    ///
    /// * `speech_ms` — active speech duration (excludes trailing silence).
    /// * `max_intra_pause_ms` — longest silence run *inside* the utterance
    ///   that did **not** end it (natural mid-phrase pause). `0` if none.
    ///
    /// Target timeout = `max_intra_pause_ms + pause_margin`, with a mild
    /// speech-length bias (longer monologues keep a slightly longer tail)
    /// and a floor at half the baseline so we do not collapse too fast
    /// on pause-free blips.
    pub fn record_utterance(&mut self, speech_ms: u32, max_intra_pause_ms: u32) {
        if !self.cfg.enabled {
            return;
        }
        let margin = self.cfg.pause_margin_ms;
        // Natural pause signal: longest intra pause + comfort margin.
        let from_pause = max_intra_pause_ms.saturating_add(margin);
        // Mild length bias: every full second of speech adds ~25 ms, capped.
        let length_bias = (speech_ms / 1_000).saturating_mul(25).min(200);
        // Floor near half baseline so sparse-pause speech does not slam
        // the timeout to min on the first short command.
        let floor = (self.baseline_ms / 2).max(self.cfg.min_ms);
        let target = from_pause
            .saturating_add(length_bias)
            .max(floor)
            .clamp(self.cfg.min_ms, self.cfg.max_ms);
        self.record_observation(target);
    }

    /// User resumed speaking shortly after an endpoint — timeout was too
    /// aggressive. Bump toward `current + bump_ms` (default path uses 200).
    pub fn record_false_endpoint(&mut self, bump_ms: u32) {
        if !self.cfg.enabled {
            return;
        }
        let bump = bump_ms.max(50);
        let target = self
            .current_ms()
            .saturating_add(bump)
            .clamp(self.cfg.min_ms, self.cfg.max_ms);
        self.record_observation(target);
    }

    /// Reset estimate and window back to the original baseline.
    pub fn reset(&mut self) {
        self.observations.clear();
        self.estimate_ms = self.baseline_ms as f32;
    }

    /// Replace baseline (e.g. session reconfigured) and reset learning.
    pub fn rebaseline(&mut self, baseline_ms: u32) {
        self.baseline_ms = baseline_ms.clamp(self.cfg.min_ms, self.cfg.max_ms);
        self.reset();
    }

    fn push_observation(&mut self, target: u32) {
        if self.observations.len() >= self.cfg.window_size {
            self.observations.pop_front();
        }
        self.observations.push_back(target);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_at_baseline_until_observations() {
        let est = AdaptiveSilenceTimeout::from_baseline(1_500);
        assert_eq!(est.current_ms(), 1_500);
        assert_eq!(est.observation_count(), 0);
        assert!(est.enabled());
    }

    #[test]
    fn fixed_never_learns() {
        let mut est = AdaptiveSilenceTimeout::fixed(1_500);
        est.record_utterance(2_000, 900);
        est.record_false_endpoint(300);
        assert_eq!(est.current_ms(), 1_500);
        assert_eq!(est.observation_count(), 0);
    }

    #[test]
    fn learns_toward_long_intra_pauses() {
        let mut est = AdaptiveSilenceTimeout::from_baseline(1_500);
        // Speakers with ~900 ms mid-phrase pauses → target ≈ 900+250 = 1150
        // (plus small length bias). Repeated observations should pull
        // estimate down from 1500 toward that band.
        for _ in 0..8 {
            est.record_utterance(1_200, 900);
        }
        let cur = est.current_ms();
        assert!(
            (500..1_500).contains(&cur),
            "expected pull down from 1500 toward ~1150, got {cur}"
        );
        assert!(
            (1_000..=1_400).contains(&cur),
            "expected roughly pause+margin band, got {cur}"
        );
    }

    #[test]
    fn learns_toward_short_pauses() {
        let mut est = AdaptiveSilenceTimeout::from_baseline(1_500);
        // Snappy speech, no mid pauses → target floors near baseline/2.
        for _ in 0..10 {
            est.record_utterance(400, 0);
        }
        let cur = est.current_ms();
        assert!(cur < 1_500, "expected reduction from baseline, got {cur}");
        assert!(cur >= 500, "must respect min clamp, got {cur}");
    }

    #[test]
    fn clamps_to_min_max() {
        let cfg = AdaptiveSilenceConfig {
            min_ms: 500,
            max_ms: 3_000,
            learning_rate: 1.0, // snap fully to observation
            ..AdaptiveSilenceConfig::default()
        };
        let mut est = AdaptiveSilenceTimeout::new(1_500, cfg);
        // Enormous pause → hard cap at max.
        est.record_observation(50_000);
        assert_eq!(est.current_ms(), 3_000);
        // Tiny pause → hard floor at min.
        est.record_observation(10);
        assert_eq!(est.current_ms(), 500);
    }

    #[test]
    fn false_endpoint_raises_timeout() {
        let cfg = AdaptiveSilenceConfig {
            learning_rate: 1.0,
            ..AdaptiveSilenceConfig::default()
        };
        let mut est = AdaptiveSilenceTimeout::new(1_000, cfg);
        // Seed one observation so current_ms leaves pure-baseline path.
        est.record_observation(1_000);
        assert_eq!(est.current_ms(), 1_000);
        est.record_false_endpoint(200);
        assert!(
            est.current_ms() > 1_000,
            "false endpoint should raise timeout"
        );
    }

    #[test]
    fn rolling_window_evicts_oldest() {
        let cfg = AdaptiveSilenceConfig {
            window_size: 3,
            learning_rate: 0.5,
            ..AdaptiveSilenceConfig::default()
        };
        let mut est = AdaptiveSilenceTimeout::new(1_500, cfg);
        est.record_observation(1_000);
        est.record_observation(1_100);
        est.record_observation(1_200);
        assert_eq!(est.observation_count(), 3);
        est.record_observation(1_300);
        assert_eq!(est.observation_count(), 3);
    }

    #[test]
    fn reset_returns_to_baseline() {
        let mut est = AdaptiveSilenceTimeout::from_baseline(1_500);
        for _ in 0..5 {
            est.record_utterance(3_000, 1_200);
        }
        assert_ne!(est.current_ms(), 1_500);
        est.reset();
        assert_eq!(est.current_ms(), 1_500);
        assert_eq!(est.observation_count(), 0);
    }

    #[test]
    fn baseline_clamped_into_range() {
        let est = AdaptiveSilenceTimeout::new(
            50,
            AdaptiveSilenceConfig {
                min_ms: 500,
                max_ms: 3_000,
                ..AdaptiveSilenceConfig::default()
            },
        );
        assert_eq!(est.baseline_ms(), 500);
        assert_eq!(est.current_ms(), 500);
    }

    #[test]
    fn serde_defaults_roundtrip() {
        let cfg: AdaptiveSilenceConfig = serde_json::from_str("{}").unwrap();
        assert!(cfg.enabled);
        assert_eq!(cfg.min_ms, 500);
        assert_eq!(cfg.max_ms, 3_000);
        assert_eq!(cfg.window_size, 8);
        assert!((cfg.learning_rate - 0.35).abs() < f32::EPSILON);
        assert_eq!(cfg.pause_margin_ms, 250);

        let json = serde_json::to_string(&cfg).unwrap();
        let back: AdaptiveSilenceConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }
}
