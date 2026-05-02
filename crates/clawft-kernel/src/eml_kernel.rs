//! EML learned functions for kernel subsystems.
//!
//! Each model replaces a hardcoded heuristic with a trainable
//! [`eml_core::EmlModel`], falling back to the original fixed logic
//! when the model has not yet been trained.
//!
//! # Models
//!
//! | Model | Replaces | Inputs | Outputs |
//! |-------|----------|--------|---------|
//! | [`GovernanceScorerModel`] | EffectVector L2 norm | 5 dimensions | 1 composite score |
//! | [`RestartStrategyModel`] | Fixed backoff delays | 4 features | 2 (delay, should_retry) |
//! | [`HealthThresholdModel`] | Fixed probe thresholds | 3 features | 2 (degraded, failed) |
//! | [`DeadLetterModel`] | Fixed retry policy | 3 features | 2 (delay, should_discard) |
//! | [`GossipTimingModel`] | Fixed gossip intervals | 3 features | 1 interval |
//! | [`ComplexityModel`] | 500-line threshold | 3 features | 1 threshold |

use eml_core::{EmlEvent, EmlModel};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// 1. Governance EffectVector Scoring
// ---------------------------------------------------------------------------

/// Learned governance scorer replacing the L2 norm on EffectVector.
///
/// Inputs (5): risk, fairness, privacy, novelty, security
/// Output (1): composite governance score
///
/// Fallback: standard L2 norm (sqrt of sum of squares).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernanceScorerModel {
    inner: EmlModel,
}

impl Default for GovernanceScorerModel {
    fn default() -> Self {
        Self::new()
    }
}

impl GovernanceScorerModel {
    /// Create a new untrained governance scorer.
    pub fn new() -> Self {
        let mut inner = EmlModel::new(3, 5, 1);
        inner.set_model_name("governance_scorer");
        Self { inner }
    }

    /// Drain accumulated EML lifecycle events for ExoChain forwarding.
    pub fn drain_events(&mut self) -> Vec<EmlEvent> {
        self.inner.drain_events()
    }

    /// Whether the model has been trained.
    pub fn is_trained(&self) -> bool {
        self.inner.is_trained()
    }

    /// Number of training samples collected.
    pub fn training_sample_count(&self) -> usize {
        self.inner.training_sample_count()
    }

    /// Predict a composite governance score from effect dimensions.
    ///
    /// When untrained, falls back to L2 norm (sqrt of sum of squares).
    pub fn predict(&self, risk: f64, fairness: f64, privacy: f64, novelty: f64, security: f64) -> f64 {
        if !self.inner.is_trained() {
            return (risk * risk
                + fairness * fairness
                + privacy * privacy
                + novelty * novelty
                + security * security)
                .sqrt();
        }
        let inputs = [risk, fairness, privacy, novelty, security];
        self.inner.predict_primary(&inputs).max(0.0)
    }

    /// Record a governance decision for training.
    ///
    /// `score` is the expert-reviewed composite importance of this decision.
    pub fn record(
        &mut self,
        risk: f64,
        fairness: f64,
        privacy: f64,
        novelty: f64,
        security: f64,
        score: f64,
    ) {
        let inputs = [risk, fairness, privacy, novelty, security];
        self.inner.record(&inputs, &[Some(score)]);
    }

    /// Train the model from recorded samples.
    ///
    /// Returns `true` if the model converged.
    pub fn train(&mut self) -> bool {
        self.inner.train()
    }
}

// ---------------------------------------------------------------------------
// 2. Agent Restart Strategy
// ---------------------------------------------------------------------------

/// Learned restart strategy replacing fixed backoff delays and retry decisions.
///
/// Inputs (4): failure_count, failure_type_ordinal, uptime_before_failure_secs,
///             system_load (0.0-1.0)
/// Outputs (2): optimal_delay_ms, should_retry (>0.5 = yes)
///
/// Fallback: exponential backoff 100ms * 2^(n-1), capped at 30s; always retry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestartStrategyModel {
    inner: EmlModel,
}

impl Default for RestartStrategyModel {
    fn default() -> Self {
        Self::new()
    }
}

impl RestartStrategyModel {
    /// Create a new untrained restart strategy model.
    pub fn new() -> Self {
        let mut inner = EmlModel::new(2, 4, 2);
        inner.set_model_name("restart_strategy");
        Self { inner }
    }

    /// Drain accumulated EML lifecycle events for ExoChain forwarding.
    pub fn drain_events(&mut self) -> Vec<EmlEvent> {
        self.inner.drain_events()
    }

    /// Whether the model has been trained.
    pub fn is_trained(&self) -> bool {
        self.inner.is_trained()
    }

    /// Number of training samples collected.
    pub fn training_sample_count(&self) -> usize {
        self.inner.training_sample_count()
    }

    /// Predict restart parameters.
    ///
    /// Returns `(delay_ms, should_retry)` where `should_retry` is true
    /// when the model's second head exceeds 0.5.
    ///
    /// When untrained, uses exponential backoff: 100 * 2^(failure_count-1),
    /// capped at 30_000ms, and always retries.
    pub fn predict(
        &self,
        failure_count: u32,
        failure_type: u32,
        uptime_before_failure_secs: f64,
        system_load: f64,
    ) -> (u64, bool) {
        if !self.inner.is_trained() {
            let base: u64 = 100;
            let exponent = failure_count.saturating_sub(1);
            let delay = base.saturating_mul(1u64 << exponent.min(20));
            return (delay.min(30_000), true);
        }

        let inputs = [
            failure_count as f64,
            failure_type as f64,
            uptime_before_failure_secs,
            system_load,
        ];
        let heads = self.inner.predict(&inputs);
        let delay_ms = (heads[0].max(0.0) * 1000.0) as u64;
        let should_retry = heads[1] > 0.5;
        (delay_ms.min(60_000), should_retry)
    }

    /// Record a restart outcome for training.
    ///
    /// `actual_delay_ms` is the delay that led to successful recovery.
    /// `recovery_succeeded` indicates whether the restart succeeded.
    pub fn record(
        &mut self,
        failure_count: u32,
        failure_type: u32,
        uptime_before_failure_secs: f64,
        system_load: f64,
        actual_delay_ms: u64,
        recovery_succeeded: bool,
    ) {
        let inputs = [
            failure_count as f64,
            failure_type as f64,
            uptime_before_failure_secs,
            system_load,
        ];
        let targets = [
            Some(actual_delay_ms as f64 / 1000.0),
            Some(if recovery_succeeded { 1.0 } else { 0.0 }),
        ];
        self.inner.record(&inputs, &targets);
    }

    /// Train the model from recorded samples.
    pub fn train(&mut self) -> bool {
        self.inner.train()
    }
}

// ---------------------------------------------------------------------------
// 3. Health Check Thresholds
// ---------------------------------------------------------------------------

/// Learned health check thresholds replacing fixed probe configuration.
///
/// Inputs (3): service_type_ordinal, history_depth (sample count), recent_latency_ms
/// Outputs (2): degraded_threshold (consecutive failures), failed_threshold
///
/// Fallback: degraded = 1 failure, failed = 3 failures (from ProbeConfig defaults).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthThresholdModel {
    inner: EmlModel,
}

impl Default for HealthThresholdModel {
    fn default() -> Self {
        Self::new()
    }
}

impl HealthThresholdModel {
    /// Create a new untrained health threshold model.
    pub fn new() -> Self {
        let mut inner = EmlModel::new(2, 3, 2);
        inner.set_model_name("health_threshold");
        Self { inner }
    }

    /// Drain accumulated EML lifecycle events for ExoChain forwarding.
    pub fn drain_events(&mut self) -> Vec<EmlEvent> {
        self.inner.drain_events()
    }

    /// Whether the model has been trained.
    pub fn is_trained(&self) -> bool {
        self.inner.is_trained()
    }

    /// Number of training samples collected.
    pub fn training_sample_count(&self) -> usize {
        self.inner.training_sample_count()
    }

    /// Predict health thresholds: (degraded_threshold, failed_threshold).
    ///
    /// Both values are clamped to [1, 20] and rounded to integers.
    ///
    /// When untrained, returns (1, 3) matching ProbeConfig defaults.
    pub fn predict(
        &self,
        service_type: u32,
        history_depth: u32,
        recent_latency_ms: f64,
    ) -> (u32, u32) {
        if !self.inner.is_trained() {
            return (1, 3);
        }

        let inputs = [
            service_type as f64,
            history_depth as f64,
            recent_latency_ms / 1000.0, // normalize to seconds
        ];
        let heads = self.inner.predict(&inputs);
        let degraded = heads[0].clamp(1.0, 20.0).round() as u32;
        let failed = heads[1].clamp(1.0, 20.0).round() as u32;
        // Ensure failed >= degraded
        (degraded, failed.max(degraded))
    }

    /// Record a confirmed health alert for training.
    ///
    /// `was_true_positive`: true if the alert correctly identified a problem.
    /// `optimal_degraded` / `optimal_failed`: retrospective best thresholds.
    pub fn record(
        &mut self,
        service_type: u32,
        history_depth: u32,
        recent_latency_ms: f64,
        optimal_degraded: u32,
        optimal_failed: u32,
    ) {
        let inputs = [
            service_type as f64,
            history_depth as f64,
            recent_latency_ms / 1000.0,
        ];
        let targets = [
            Some(optimal_degraded as f64),
            Some(optimal_failed as f64),
        ];
        self.inner.record(&inputs, &targets);
    }

    /// Train the model from recorded samples.
    pub fn train(&mut self) -> bool {
        self.inner.train()
    }
}

// ---------------------------------------------------------------------------
// 4. Dead Letter Policy
// ---------------------------------------------------------------------------

/// Learned dead letter retry policy replacing fixed backoff and discard rules.
///
/// Inputs (3): retry_count, message_age_ms, queue_depth
/// Outputs (2): retry_delay_ms, should_discard (>0.5 = discard)
///
/// Fallback: retry_delay = 1000 * 2^retry_count (capped at 60s), discard after 5 retries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeadLetterModel {
    inner: EmlModel,
}

impl Default for DeadLetterModel {
    fn default() -> Self {
        Self::new()
    }
}

impl DeadLetterModel {
    /// Create a new untrained dead letter policy model.
    pub fn new() -> Self {
        let mut inner = EmlModel::new(2, 3, 2);
        inner.set_model_name("dead_letter");
        Self { inner }
    }

    /// Drain accumulated EML lifecycle events for ExoChain forwarding.
    pub fn drain_events(&mut self) -> Vec<EmlEvent> {
        self.inner.drain_events()
    }

    /// Whether the model has been trained.
    pub fn is_trained(&self) -> bool {
        self.inner.is_trained()
    }

    /// Number of training samples collected.
    pub fn training_sample_count(&self) -> usize {
        self.inner.training_sample_count()
    }

    /// Predict retry policy: (retry_delay_ms, should_discard).
    ///
    /// When untrained, uses exponential backoff: 1000 * 2^retry_count
    /// capped at 60s, discards after 5 retries.
    pub fn predict(
        &self,
        retry_count: u32,
        message_age_ms: u64,
        queue_depth: usize,
    ) -> (u64, bool) {
        if !self.inner.is_trained() {
            let delay = (1000u64).saturating_mul(1u64 << retry_count.min(20));
            return (delay.min(60_000), retry_count >= 5);
        }

        let inputs = [
            retry_count as f64,
            message_age_ms as f64 / 1000.0, // normalize to seconds
            queue_depth as f64,
        ];
        let heads = self.inner.predict(&inputs);
        let delay_ms = (heads[0].max(0.0) * 1000.0) as u64;
        let should_discard = heads[1] > 0.5;
        (delay_ms.min(120_000), should_discard)
    }

    /// Record a retry outcome for training.
    ///
    /// `delivery_succeeded`: whether the retry eventually delivered.
    /// `actual_delay_ms`: the delay used before the retry attempt.
    pub fn record(
        &mut self,
        retry_count: u32,
        message_age_ms: u64,
        queue_depth: usize,
        actual_delay_ms: u64,
        delivery_succeeded: bool,
    ) {
        let inputs = [
            retry_count as f64,
            message_age_ms as f64 / 1000.0,
            queue_depth as f64,
        ];
        let targets = [
            Some(actual_delay_ms as f64 / 1000.0),
            Some(if delivery_succeeded { 0.0 } else { 1.0 }), // discard if not successful
        ];
        self.inner.record(&inputs, &targets);
    }

    /// Train the model from recorded samples.
    pub fn train(&mut self) -> bool {
        self.inner.train()
    }
}

// ---------------------------------------------------------------------------
// 5. Mesh Assessment Gossip Timing
// ---------------------------------------------------------------------------

/// Learned gossip interval replacing fixed timing constants.
///
/// Inputs (3): peer_count, network_latency_ms, update_frequency (Hz)
/// Output (1): optimal gossip interval in seconds
///
/// Fallback: 5 seconds (matching cluster heartbeat default).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GossipTimingModel {
    inner: EmlModel,
}

impl Default for GossipTimingModel {
    fn default() -> Self {
        Self::new()
    }
}

impl GossipTimingModel {
    /// Create a new untrained gossip timing model.
    pub fn new() -> Self {
        let mut inner = EmlModel::new(2, 3, 1);
        inner.set_model_name("gossip_timing");
        Self { inner }
    }

    /// Drain accumulated EML lifecycle events for ExoChain forwarding.
    pub fn drain_events(&mut self) -> Vec<EmlEvent> {
        self.inner.drain_events()
    }

    /// Whether the model has been trained.
    pub fn is_trained(&self) -> bool {
        self.inner.is_trained()
    }

    /// Number of training samples collected.
    pub fn training_sample_count(&self) -> usize {
        self.inner.training_sample_count()
    }

    /// Predict optimal gossip interval in seconds.
    ///
    /// Result is clamped to [1, 60] seconds.
    ///
    /// When untrained, returns 5s (default heartbeat interval).
    pub fn predict(
        &self,
        peer_count: usize,
        network_latency_ms: f64,
        update_frequency_hz: f64,
    ) -> f64 {
        if !self.inner.is_trained() {
            return 5.0;
        }

        let inputs = [
            peer_count as f64,
            network_latency_ms / 1000.0, // normalize to seconds
            update_frequency_hz,
        ];
        self.inner.predict_primary(&inputs).clamp(1.0, 60.0)
    }

    /// Record an observed gossip interval and its effectiveness.
    ///
    /// `optimal_interval_secs`: the interval that provided the best
    /// freshness/bandwidth trade-off in this context.
    pub fn record(
        &mut self,
        peer_count: usize,
        network_latency_ms: f64,
        update_frequency_hz: f64,
        optimal_interval_secs: f64,
    ) {
        let inputs = [
            peer_count as f64,
            network_latency_ms / 1000.0,
            update_frequency_hz,
        ];
        self.inner.record(&inputs, &[Some(optimal_interval_secs)]);
    }

    /// Train the model from recorded samples.
    pub fn train(&mut self) -> bool {
        self.inner.train()
    }
}

// ---------------------------------------------------------------------------
// 6b. Tick Interval Recommender (Finding #7)
// ---------------------------------------------------------------------------

/// Learned tick-interval recommender replacing the four-tier
/// step-function in [`crate::weaver::WeaverEngine::recommend_tick_interval`].
///
/// Inputs (3): `cpm` (changes per minute), `idle_ticks`,
/// `variance` (a free-form change-rate variance signal in `[0, 1]`).
/// Output (1): recommended tick interval in milliseconds.
///
/// Fallback: returns 1000 ms (the steady-state default the old
/// step-function returned for moderate change rates). When the model
/// is not yet trained, callers must keep the original step-function
/// reachable so the historical thresholds (`200 / 1000 / 3000 / 5000`)
/// continue to apply — see [`Self::recommend_or`] for the helper that
/// implements that policy.
///
/// NOTE(eml-swap): wired — Finding #7 (TickIntervalModel).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TickIntervalModel {
    inner: EmlModel,
}

impl Default for TickIntervalModel {
    fn default() -> Self {
        Self::new()
    }
}

impl TickIntervalModel {
    /// Create a new untrained tick-interval recommender.
    pub fn new() -> Self {
        let mut inner = EmlModel::new(2, 3, 1);
        inner.set_model_name("tick_interval");
        Self { inner }
    }

    /// Drain accumulated EML lifecycle events for ExoChain forwarding.
    pub fn drain_events(&mut self) -> Vec<EmlEvent> {
        self.inner.drain_events()
    }

    /// Whether the model has been trained.
    pub fn is_trained(&self) -> bool {
        self.inner.is_trained()
    }

    /// Number of training samples collected.
    pub fn training_sample_count(&self) -> usize {
        self.inner.training_sample_count()
    }

    /// Predict the recommended tick interval in milliseconds.
    ///
    /// Result is clamped to `[100, 60_000]` ms. When untrained,
    /// returns 1000 ms (the central tier of the legacy step-function).
    pub fn predict(&self, cpm: f64, idle_ticks: u64, variance: f64) -> u32 {
        if !self.inner.is_trained() {
            return 1000;
        }
        let inputs = [cpm, idle_ticks as f64, variance];
        let raw = self.inner.predict_primary(&inputs);
        raw.clamp(100.0, 60_000.0).round() as u32
    }

    /// Recommend a tick interval: when trained returns
    /// [`Self::predict`]; otherwise returns the caller-provided
    /// `fallback` (typically the existing step-function's choice).
    pub fn recommend_or(
        &self,
        cpm: f64,
        idle_ticks: u64,
        variance: f64,
        fallback: u32,
    ) -> u32 {
        if self.inner.is_trained() {
            self.predict(cpm, idle_ticks, variance)
        } else {
            fallback
        }
    }

    /// Record an observed (cpm, idle_ticks, variance, recommended_ms)
    /// tuple for training.
    pub fn record(
        &mut self,
        cpm: f64,
        idle_ticks: u64,
        variance: f64,
        recommended_ms: u32,
    ) {
        let inputs = [cpm, idle_ticks as f64, variance];
        self.inner.record(&inputs, &[Some(recommended_ms as f64)]);
    }

    /// Train the model from recorded samples.
    pub fn train(&mut self) -> bool {
        self.inner.train()
    }
}

// ---------------------------------------------------------------------------
// 6. Assessment Complexity Threshold
// ---------------------------------------------------------------------------

/// Learned complexity threshold replacing the fixed 500-line limit.
///
/// Inputs (3): language_type_ordinal, avg_file_size_in_project, team_size_proxy
/// Output (1): per-language complexity threshold (line count)
///
/// Fallback: 500 lines (matching ComplexityAnalyzer default).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplexityModel {
    inner: EmlModel,
}

impl Default for ComplexityModel {
    fn default() -> Self {
        Self::new()
    }
}

impl ComplexityModel {
    /// Create a new untrained complexity threshold model.
    pub fn new() -> Self {
        let mut inner = EmlModel::new(2, 3, 1);
        inner.set_model_name("complexity");
        Self { inner }
    }

    /// Drain accumulated EML lifecycle events for ExoChain forwarding.
    pub fn drain_events(&mut self) -> Vec<EmlEvent> {
        self.inner.drain_events()
    }

    /// Whether the model has been trained.
    pub fn is_trained(&self) -> bool {
        self.inner.is_trained()
    }

    /// Number of training samples collected.
    pub fn training_sample_count(&self) -> usize {
        self.inner.training_sample_count()
    }

    /// Predict the complexity threshold (line count) for a given context.
    ///
    /// Result is clamped to [100, 5000] lines.
    ///
    /// When untrained, returns 500 lines.
    pub fn predict(
        &self,
        language_type: u32,
        avg_file_size: f64,
        team_size_proxy: f64,
    ) -> usize {
        if !self.inner.is_trained() {
            return 500;
        }

        let inputs = [
            language_type as f64,
            avg_file_size / 1000.0, // normalize
            team_size_proxy,
        ];
        let raw = self.inner.predict_primary(&inputs);
        (raw.clamp(100.0, 5000.0)).round() as usize
    }

    /// Record a complexity threshold observation for training.
    ///
    /// `optimal_threshold`: the line count threshold that best separates
    /// "too complex" from "acceptable" for this language and project context.
    pub fn record(
        &mut self,
        language_type: u32,
        avg_file_size: f64,
        team_size_proxy: f64,
        optimal_threshold: usize,
    ) {
        let inputs = [
            language_type as f64,
            avg_file_size / 1000.0,
            team_size_proxy,
        ];
        self.inner
            .record(&inputs, &[Some(optimal_threshold as f64)]);
    }

    /// Train the model from recorded samples.
    pub fn train(&mut self) -> bool {
        self.inner.train()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── GovernanceScorerModel ───────────────────────────────────────

    #[test]
    fn governance_scorer_fallback_matches_l2_norm() {
        let model = GovernanceScorerModel::new();
        assert!(!model.is_trained());

        let score = model.predict(0.5, 0.3, 0.2, 0.1, 0.4);
        let expected = (0.5_f64.powi(2)
            + 0.3_f64.powi(2)
            + 0.2_f64.powi(2)
            + 0.1_f64.powi(2)
            + 0.4_f64.powi(2))
        .sqrt();
        assert!((score - expected).abs() < 1e-10);
    }

    #[test]
    fn governance_scorer_zero_vector() {
        let model = GovernanceScorerModel::new();
        assert_eq!(model.predict(0.0, 0.0, 0.0, 0.0, 0.0), 0.0);
    }

    #[test]
    fn governance_scorer_record_increments_count() {
        let mut model = GovernanceScorerModel::new();
        assert_eq!(model.training_sample_count(), 0);
        model.record(0.5, 0.3, 0.2, 0.1, 0.4, 0.75);
        assert_eq!(model.training_sample_count(), 1);
    }

    #[test]
    fn governance_scorer_serde_roundtrip() {
        let model = GovernanceScorerModel::new();
        let json = serde_json::to_string(&model).unwrap();
        let restored: GovernanceScorerModel = serde_json::from_str(&json).unwrap();
        assert!(!restored.is_trained());
        assert_eq!(restored.predict(1.0, 0.0, 0.0, 0.0, 0.0), 1.0);
    }

    // ── RestartStrategyModel ────────────────────────────────────────

    #[test]
    fn restart_strategy_fallback_exponential_backoff() {
        let model = RestartStrategyModel::new();
        assert!(!model.is_trained());

        // failure_count=1: 100ms * 2^0 = 100ms
        let (delay, retry) = model.predict(1, 0, 60.0, 0.5);
        assert_eq!(delay, 100);
        assert!(retry);

        // failure_count=3: 100ms * 2^2 = 400ms
        let (delay, _) = model.predict(3, 0, 60.0, 0.5);
        assert_eq!(delay, 400);

        // failure_count=10: 100ms * 2^9 = 51200ms, capped at 30000
        let (delay, _) = model.predict(10, 0, 60.0, 0.5);
        assert_eq!(delay, 30_000);
    }

    #[test]
    fn restart_strategy_record_increments_count() {
        let mut model = RestartStrategyModel::new();
        model.record(1, 0, 60.0, 0.5, 100, true);
        assert_eq!(model.training_sample_count(), 1);
    }

    #[test]
    fn restart_strategy_serde_roundtrip() {
        let model = RestartStrategyModel::new();
        let json = serde_json::to_string(&model).unwrap();
        let restored: RestartStrategyModel = serde_json::from_str(&json).unwrap();
        assert!(!restored.is_trained());
    }

    // ── HealthThresholdModel ────────────────────────────────────────

    #[test]
    fn health_threshold_fallback_defaults() {
        let model = HealthThresholdModel::new();
        assert!(!model.is_trained());

        let (degraded, failed) = model.predict(0, 100, 50.0);
        assert_eq!(degraded, 1);
        assert_eq!(failed, 3);
    }

    #[test]
    fn health_threshold_record_increments_count() {
        let mut model = HealthThresholdModel::new();
        model.record(0, 100, 50.0, 2, 5);
        assert_eq!(model.training_sample_count(), 1);
    }

    #[test]
    fn health_threshold_serde_roundtrip() {
        let model = HealthThresholdModel::new();
        let json = serde_json::to_string(&model).unwrap();
        let restored: HealthThresholdModel = serde_json::from_str(&json).unwrap();
        assert!(!restored.is_trained());
    }

    // ── DeadLetterModel ─────────────────────────────────────────────

    #[test]
    fn dead_letter_fallback_exponential() {
        let model = DeadLetterModel::new();
        assert!(!model.is_trained());

        // retry_count=0: 1000 * 2^0 = 1000ms, no discard
        let (delay, discard) = model.predict(0, 5000, 100);
        assert_eq!(delay, 1000);
        assert!(!discard);

        // retry_count=3: 1000 * 2^3 = 8000ms, no discard
        let (delay, discard) = model.predict(3, 10000, 100);
        assert_eq!(delay, 8000);
        assert!(!discard);

        // retry_count=5: discard
        let (_, discard) = model.predict(5, 30000, 100);
        assert!(discard);

        // retry_count=7: 1000 * 2^7 = 128000, capped at 60000
        let (delay, _) = model.predict(7, 60000, 100);
        assert_eq!(delay, 60_000);
    }

    #[test]
    fn dead_letter_record_increments_count() {
        let mut model = DeadLetterModel::new();
        model.record(1, 5000, 50, 2000, true);
        assert_eq!(model.training_sample_count(), 1);
    }

    #[test]
    fn dead_letter_serde_roundtrip() {
        let model = DeadLetterModel::new();
        let json = serde_json::to_string(&model).unwrap();
        let restored: DeadLetterModel = serde_json::from_str(&json).unwrap();
        assert!(!restored.is_trained());
    }

    // ── GossipTimingModel ───────────────────────────────────────────

    #[test]
    fn gossip_timing_fallback_5s() {
        let model = GossipTimingModel::new();
        assert!(!model.is_trained());

        let interval = model.predict(10, 50.0, 1.0);
        assert!((interval - 5.0).abs() < 1e-10);
    }

    #[test]
    fn gossip_timing_record_increments_count() {
        let mut model = GossipTimingModel::new();
        model.record(10, 50.0, 1.0, 3.0);
        assert_eq!(model.training_sample_count(), 1);
    }

    #[test]
    fn gossip_timing_serde_roundtrip() {
        let model = GossipTimingModel::new();
        let json = serde_json::to_string(&model).unwrap();
        let restored: GossipTimingModel = serde_json::from_str(&json).unwrap();
        assert!(!restored.is_trained());
    }

    // ── TickIntervalModel (Finding #7) ──────────────────────────────

    #[test]
    fn tick_interval_fallback_returns_1000ms() {
        let model = TickIntervalModel::new();
        assert!(!model.is_trained());
        assert_eq!(model.predict(5.0, 10, 0.2), 1000);
    }

    #[test]
    fn tick_interval_recommend_or_uses_fallback_when_untrained() {
        let model = TickIntervalModel::new();
        // 1234 is the caller's step-function choice.
        assert_eq!(model.recommend_or(5.0, 10, 0.2, 1234), 1234);
    }

    #[test]
    fn tick_interval_record_increments_count() {
        let mut model = TickIntervalModel::new();
        model.record(5.0, 10, 0.2, 1000);
        assert_eq!(model.training_sample_count(), 1);
    }

    #[test]
    fn tick_interval_serde_roundtrip() {
        let model = TickIntervalModel::new();
        let json = serde_json::to_string(&model).unwrap();
        let restored: TickIntervalModel = serde_json::from_str(&json).unwrap();
        assert!(!restored.is_trained());
        assert_eq!(restored.predict(0.0, 0, 0.0), 1000);
    }

    // ── ComplexityModel ─────────────────────────────────────────────

    #[test]
    fn complexity_fallback_500() {
        let model = ComplexityModel::new();
        assert!(!model.is_trained());

        let threshold = model.predict(0, 200.0, 5.0);
        assert_eq!(threshold, 500);
    }

    #[test]
    fn complexity_record_increments_count() {
        let mut model = ComplexityModel::new();
        model.record(0, 200.0, 5.0, 600);
        assert_eq!(model.training_sample_count(), 1);
    }

    #[test]
    fn complexity_serde_roundtrip() {
        let model = ComplexityModel::new();
        let json = serde_json::to_string(&model).unwrap();
        let restored: ComplexityModel = serde_json::from_str(&json).unwrap();
        assert!(!restored.is_trained());
        assert_eq!(restored.predict(0, 200.0, 5.0), 500);
    }

    // ── Cross-model tests ───────────────────────────────────────────

    #[test]
    fn all_models_default_untrained() {
        assert!(!GovernanceScorerModel::default().is_trained());
        assert!(!RestartStrategyModel::default().is_trained());
        assert!(!HealthThresholdModel::default().is_trained());
        assert!(!DeadLetterModel::default().is_trained());
        assert!(!GossipTimingModel::default().is_trained());
        assert!(!ComplexityModel::default().is_trained());
    }

    #[test]
    fn all_models_zero_initial_samples() {
        assert_eq!(GovernanceScorerModel::new().training_sample_count(), 0);
        assert_eq!(RestartStrategyModel::new().training_sample_count(), 0);
        assert_eq!(HealthThresholdModel::new().training_sample_count(), 0);
        assert_eq!(DeadLetterModel::new().training_sample_count(), 0);
        assert_eq!(GossipTimingModel::new().training_sample_count(), 0);
        assert_eq!(ComplexityModel::new().training_sample_count(), 0);
    }
}
