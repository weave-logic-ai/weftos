//! Quality scoring for LLM responses.
//!
//! Provides a [`QualityScorer`] trait for evaluating the quality of responses
//! and two implementations:
//!
//! - [`NoopScorer`] -- Always returns a neutral score (0.5). Use when the
//!   `rvf` feature is enabled but no real scorer is configured.
//! - [`BasicScorer`] -- Heuristic scorer that evaluates response quality based
//!   on length, error indicators, and tool usage patterns.
//!
//! These are *standalone* scorers for the RVF integration layer. They are
//! separate from the pipeline scorer in [`crate::pipeline::scorer`] which
//! operates on typed LLM request/response pairs.
//!
//! This module is gated behind the `rvf` feature flag.

// ── Trait ──────────────────────────────────────────────────────────────

/// Evaluate and record quality scores for LLM request/response pairs.
pub trait QualityScorer: Send + Sync {
    /// Score a response given its originating request.
    ///
    /// Returns a value in `0.0..=1.0` where higher is better.
    fn score(&self, request: &str, response: &str) -> f32;

    /// Record a scored observation for future reference.
    fn record(&mut self, request: &str, response: &str, score: f32);
}

// ── NoopScorer ─────────────────────────────────────────────────────────

/// No-op scorer that always returns `0.5`.
///
/// Suitable as a default placeholder when no real quality model is loaded.
pub struct NoopScorer;

impl NoopScorer {
    /// Create a new no-op scorer.
    pub fn new() -> Self {
        Self
    }
}

impl Default for NoopScorer {
    fn default() -> Self {
        Self::new()
    }
}

impl QualityScorer for NoopScorer {
    fn score(&self, _request: &str, _response: &str) -> f32 {
        0.5
    }

    fn record(&mut self, _request: &str, _response: &str, _score: f32) {
        // No-op.
    }
}

// ── BasicScorer ────────────────────────────────────────────────────────

/// Heuristic quality scorer.
///
/// Evaluates response quality using three signals:
///
/// 1. **Length** -- Longer responses score higher (up to 500 words), because
///    very short answers often indicate a refusal or lack of information.
/// 2. **Error indicators** -- Phrases like "I can't", "I don't know", or
///    "I'm unable" reduce the score.
/// 3. **Tool usage** -- Presence of tool invocation patterns (JSON objects
///    with `"tool_use"` or `"function"` keys) increases the score, as it
///    indicates the model is actively fulfilling the request.
pub struct BasicScorer {
    /// History of `(request_hash, score)` pairs for trend analysis.
    history: Vec<(u64, f32)>,
}

impl BasicScorer {
    /// Create a new basic scorer with an empty history.
    pub fn new() -> Self {
        Self {
            history: Vec::new(),
        }
    }

    /// Return a read-only view of the scoring history.
    pub fn history(&self) -> &[(u64, f32)] {
        &self.history
    }
}

impl Default for BasicScorer {
    fn default() -> Self {
        Self::new()
    }
}

impl QualityScorer for BasicScorer {
    fn score(&self, _request: &str, response: &str) -> f32 {
        let lower = response.to_lowercase();
        let word_count = lower.split_whitespace().count();

        // Length component: 0.0 for empty, up to 0.4 at 500+ words.
        let length_score = (word_count as f32 / 500.0).min(1.0) * 0.4;

        // Error-indicator penalty: up to -0.3.
        let error_phrases = [
            "i can't",
            "i cannot",
            "i don't know",
            "i'm unable",
            "i am unable",
            "as an ai",
            "i'm not able",
        ];
        let error_penalty = if error_phrases.iter().any(|p| lower.contains(p)) {
            0.3
        } else {
            0.0
        };

        // Tool-use bonus: +0.2 if the response contains tool-invocation patterns.
        let tool_bonus = if lower.contains("tool_use") || lower.contains("\"function\"") {
            0.2
        } else {
            0.0
        };

        // Base of 0.3 (non-empty response is worth something).
        let base = if word_count > 0 { 0.3 } else { 0.0 };

        (base + length_score + tool_bonus - error_penalty).clamp(0.0, 1.0)
    }

    fn record(&mut self, request: &str, _response: &str, score: f32) {
        let hash = simple_hash(request);
        self.history.push((hash, score));
    }
}

/// Cheap non-cryptographic hash for request deduplication.
fn simple_hash(s: &str) -> u64 {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── NoopScorer ─────────────────────────────────────────────────

    #[test]
    fn noop_always_returns_half() {
        let scorer = NoopScorer::new();
        assert!((scorer.score("any request", "any response") - 0.5).abs() < f32::EPSILON);
        assert!((scorer.score("", "") - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn noop_record_does_not_panic() {
        let mut scorer = NoopScorer::new();
        scorer.record("req", "resp", 0.5);
    }

    #[test]
    fn noop_default_trait() {
        let scorer = NoopScorer;
        assert!((scorer.score("x", "y") - 0.5).abs() < f32::EPSILON);
    }

    // ── BasicScorer ────────────────────────────────────────────────

    #[test]
    fn basic_long_response_scores_higher_than_short() {
        let scorer = BasicScorer::new();
        let short_score = scorer.score("question", "yes");
        let long_score = scorer.score("question", &"word ".repeat(200));
        assert!(
            long_score > short_score,
            "long ({long_score}) should beat short ({short_score})"
        );
    }

    #[test]
    fn basic_error_response_scores_lower() {
        let scorer = BasicScorer::new();
        let good_score = scorer.score("question", "Here is the answer with details.");
        let error_score = scorer.score("question", "I can't help with that request.");
        assert!(
            good_score > error_score,
            "good ({good_score}) should beat error ({error_score})"
        );
    }

    #[test]
    fn basic_tool_use_gets_bonus() {
        let scorer = BasicScorer::new();
        let no_tool = scorer.score("question", "Here is the answer.");
        let with_tool = scorer.score("question", r#"Here is the answer. {"tool_use": "search"}"#);
        assert!(
            with_tool > no_tool,
            "tool_use ({with_tool}) should beat no tool ({no_tool})"
        );
    }

    #[test]
    fn basic_empty_response_scores_zero() {
        let scorer = BasicScorer::new();
        let score = scorer.score("question", "");
        assert!(
            score.abs() < f32::EPSILON,
            "empty response should score 0.0, got {score}"
        );
    }

    #[test]
    fn basic_score_clamped_to_unit_range() {
        let scorer = BasicScorer::new();
        // Very long response with tool use.
        let response = format!("{} tool_use \"function\"", "word ".repeat(1000));
        let score = scorer.score("q", &response);
        assert!(score <= 1.0, "score should be <= 1.0, got {score}");
        assert!(score >= 0.0, "score should be >= 0.0, got {score}");
    }

    #[test]
    fn basic_record_stores_history() {
        let mut scorer = BasicScorer::new();
        scorer.record("req1", "resp1", 0.8);
        scorer.record("req2", "resp2", 0.6);
        assert_eq!(scorer.history().len(), 2);
        assert!((scorer.history()[0].1 - 0.8).abs() < f32::EPSILON);
        assert!((scorer.history()[1].1 - 0.6).abs() < f32::EPSILON);
    }

    #[test]
    fn basic_default_trait() {
        let scorer = BasicScorer::default();
        assert!(scorer.history().is_empty());
    }

    #[test]
    fn basic_multiple_error_phrases_detected() {
        let scorer = BasicScorer::new();
        let phrases = [
            "I cannot do that.",
            "I don't know the answer.",
            "I'm unable to help.",
            "I am unable to process.",
            "As an AI, I can't.",
            "I'm not able to assist.",
        ];
        for phrase in &phrases {
            let score = scorer.score("question", phrase);
            let clean_score = scorer.score("question", "Here is the answer.");
            assert!(
                clean_score > score,
                "clean ({clean_score}) should beat error phrase '{phrase}' ({score})"
            );
        }
    }
}
