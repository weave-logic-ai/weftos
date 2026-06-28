//! EML learned retry timing for LLM providers.
//!
//! Replaces the hardcoded exponential backoff parameters with an
//! [`eml_core::EmlModel`] that learns optimal retry delays from
//! historical success/failure outcomes.
//!
//! # Architecture
//!
//! - [`RetryModel`] wraps an `EmlModel::new(2, 3, 1)` with inputs:
//!   `(error_type_ordinal, attempt_number, hour_of_day)` and output:
//!   `optimal_delay_ms`.
//! - When untrained, falls back to the standard [`RetryConfig`] defaults.
//!
//! # Training
//!
//! After each retry outcome (success or final failure), call
//! [`RetryModel::record`] with the actual delay used and whether the
//! next attempt succeeded. The model learns which delays lead to
//! successful retries.

use eml_core::EmlModel;
use serde::{Deserialize, Serialize};

use crate::error::ProviderError;
use crate::retry::RetryConfig;

// ═══════════════════════════════════════════════════════════════════
// Error type encoding
// ═══════════════════════════════════════════════════════════════════

/// Map a [`ProviderError`] to a numeric ordinal for the model.
///
/// Exposed `pub(crate)` so `retry::RetryPolicy` can snapshot the
/// ordinal at retry time (when it still has an `&ProviderError`) and
/// replay it later via [`RetryModel::record_by_ordinal`] — needed
/// because `ProviderError` doesn't impl `Clone`.
pub(crate) fn error_ordinal(err: &ProviderError) -> f64 {
    match err {
        ProviderError::RateLimited { .. } => 0.0,
        ProviderError::Timeout => 1.0,
        ProviderError::Http(_) => 2.0,
        ProviderError::ServerError { status, .. } => {
            // Encode common status codes as distinct ordinals
            match *status {
                500 => 3.0,
                502 => 4.0,
                503 => 5.0,
                504 => 6.0,
                _ => 7.0,
            }
        }
        _ => 8.0,
    }
}

/// Get the current hour of day (0..23) as a normalized feature.
fn hour_of_day_normalized() -> f64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let hour = (secs % 86400) / 3600;
    hour as f64 / 24.0
}

// ═══════════════════════════════════════════════════════════════════
// RetryModel
// ═══════════════════════════════════════════════════════════════════

/// Learned retry timing model for LLM provider calls.
///
/// Learns the optimal delay between retries based on the error type,
/// attempt number, and time of day. Falls back to exponential backoff
/// from [`RetryConfig`] when untrained.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryModel {
    /// EML model: 3 inputs (error_ordinal, attempt, hour), 1 output (delay_ms)
    model: EmlModel,
    /// Fallback config used when the model is untrained.
    #[serde(skip)]
    fallback: RetryConfig,
}

impl RetryModel {
    /// Create a new untrained retry model.
    pub fn new() -> Self {
        Self {
            model: EmlModel::new(2, 3, 1),
            fallback: RetryConfig::default(),
        }
    }

    /// Create with a custom fallback configuration.
    pub fn with_fallback(fallback: RetryConfig) -> Self {
        Self {
            model: EmlModel::new(2, 3, 1),
            fallback,
        }
    }

    /// Compute the optimal delay for a retry attempt.
    ///
    /// When untrained, falls back to [`crate::retry::compute_delay`].
    pub fn delay_ms(&self, err: &ProviderError, attempt: u32) -> u64 {
        if !self.model.is_trained() {
            return crate::retry::compute_delay(&self.fallback, attempt).as_millis() as u64;
        }

        let inputs = [
            error_ordinal(err) / 8.0, // normalize to [0, 1]
            attempt as f64 / 10.0,    // normalize (typical max ~5-10)
            hour_of_day_normalized(),
        ];
        let predicted = self.model.predict_primary(&inputs);

        // Clamp to sane bounds
        let delay = predicted.clamp(100.0, 60_000.0);
        delay as u64
    }

    /// Record a retry outcome for training.
    ///
    /// # Arguments
    /// - `err`: The error that triggered the retry.
    /// - `attempt`: The attempt number (0-indexed).
    /// - `delay_ms`: The actual delay used before this attempt.
    /// - `succeeded`: Whether the subsequent attempt succeeded.
    pub fn record(&mut self, err: &ProviderError, attempt: u32, delay_ms: u64, succeeded: bool) {
        self.record_by_ordinal(error_ordinal(err), attempt, delay_ms, succeeded);
    }

    /// Record a retry outcome from a pre-computed error ordinal.
    ///
    /// Escape-hatch for callers who can't hold onto the original
    /// `ProviderError` through the retry loop (it isn't `Clone`
    /// because of `reqwest::Error` / `serde_json::Error`). Snapshot
    /// the ordinal via [`error_ordinal`] at retry time, pass it
    /// here when the outcome is known.
    pub fn record_by_ordinal(
        &mut self,
        ordinal: f64,
        attempt: u32,
        delay_ms: u64,
        succeeded: bool,
    ) {
        let inputs = [
            ordinal / 8.0,
            attempt as f64 / 10.0,
            hour_of_day_normalized(),
        ];

        // Target: if succeeded, the delay was good — record it.
        // If failed, the delay was too short — record 2x as target.
        let target = if succeeded {
            delay_ms as f64
        } else {
            (delay_ms as f64 * 2.0).min(60_000.0)
        };

        self.model.record(&inputs, &[Some(target)]);
    }

    /// Train the model. Returns true if converged.
    pub fn train(&mut self) -> bool {
        self.model.train()
    }

    /// Whether the model is trained.
    pub fn is_trained(&self) -> bool {
        self.model.is_trained()
    }

    /// Number of training samples.
    pub fn training_sample_count(&self) -> usize {
        self.model.training_sample_count()
    }

    /// Serialize to JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("RetryModel serialization should not fail")
    }

    /// Deserialize from JSON.
    pub fn from_json(json: &str) -> Option<Self> {
        serde_json::from_str(json).ok()
    }
}

impl Default for RetryModel {
    fn default() -> Self {
        Self::new()
    }
}

// ═══════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untrained_uses_exponential_backoff() {
        let model = RetryModel::new();
        assert!(!model.is_trained());

        let err = ProviderError::Timeout;
        let delay = model.delay_ms(&err, 0);
        // Default: base_delay=1000ms, attempt 0 => 2^0 * 1000 = 1000ms + jitter
        assert!(delay >= 1000, "delay should be >= 1000ms, got {delay}");
        assert!(
            delay <= 1500,
            "delay should be <= 1500ms (with jitter), got {delay}"
        );
    }

    #[test]
    fn untrained_delay_increases_with_attempt() {
        let model = RetryModel::new();
        let err = ProviderError::ServerError {
            status: 503,
            body: "unavailable".into(),
        };

        let d0 = model.delay_ms(&err, 0);
        let d1 = model.delay_ms(&err, 1);
        let d2 = model.delay_ms(&err, 2);

        // Exponential backoff: each attempt should be >= previous
        // (ignoring jitter randomness, we check the general trend)
        assert!(
            d1 >= d0 || d2 > d0,
            "delays should generally increase: {d0}, {d1}, {d2}"
        );
    }

    #[test]
    fn error_ordinal_mappings() {
        assert!(
            (error_ordinal(&ProviderError::RateLimited {
                retry_after_ms: 1000
            }))
            .abs()
                < 1e-9
        );
        assert!((error_ordinal(&ProviderError::Timeout) - 1.0).abs() < 1e-9);
        assert!(
            (error_ordinal(&ProviderError::ServerError {
                status: 503,
                body: String::new(),
            }) - 5.0)
                .abs()
                < 1e-9
        );
    }

    #[test]
    fn record_increments_count() {
        let mut model = RetryModel::new();
        assert_eq!(model.training_sample_count(), 0);

        model.record(&ProviderError::Timeout, 0, 1000, true);
        assert_eq!(model.training_sample_count(), 1);

        model.record(&ProviderError::Timeout, 1, 2000, false);
        assert_eq!(model.training_sample_count(), 2);
    }

    #[test]
    fn train_insufficient_data() {
        let mut model = RetryModel::new();
        for i in 0..10 {
            model.record(&ProviderError::Timeout, i, 1000 * (i as u64 + 1), true);
        }
        assert!(!model.train());
        assert!(!model.is_trained());
    }

    #[test]
    fn serialization_roundtrip() {
        let model = RetryModel::new();
        let json = model.to_json();
        let restored = RetryModel::from_json(&json).expect("should deserialize");
        assert!(!restored.is_trained());
    }

    #[test]
    fn from_json_invalid() {
        assert!(RetryModel::from_json("not json").is_none());
    }

    #[test]
    fn with_fallback_config() {
        use std::time::Duration;
        let config = RetryConfig {
            max_retries: 5,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(10),
            jitter_fraction: 0.0,
        };
        let model = RetryModel::with_fallback(config);
        let delay = model.delay_ms(&ProviderError::Timeout, 0);
        // base_delay=500ms, attempt 0, no jitter => exactly 500ms
        assert_eq!(delay, 500);
    }

    #[test]
    fn default_impl() {
        let model = RetryModel::default();
        assert!(!model.is_trained());
    }

    #[test]
    fn hour_of_day_in_range() {
        let h = hour_of_day_normalized();
        assert!(h >= 0.0 && h < 1.0, "hour should be in [0, 1), got {h}");
    }
}
