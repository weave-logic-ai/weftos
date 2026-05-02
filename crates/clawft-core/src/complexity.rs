//! Task complexity analysis for model routing.
//!
//! Provides a [`TaskComplexityAnalyzer`] that estimates the complexity of a
//! task description using lightweight heuristics. The resulting score drives
//! the 3-tier routing decision in [`crate::intelligent_router`].
//!
//! This module is gated behind the `rvf` feature flag.

// ── TaskComplexityAnalyzer ─────────────────────────────────────────────

/// Estimates task complexity using surface-level text heuristics.
///
/// The analyser does **not** use an LLM or neural model. It relies on
/// properties of the input text that correlate with cognitive load:
///
/// - Task length (word count)
/// - Number of distinct requirements (sentence count)
/// - Technical-keyword density
/// - Multi-step indicators
///
/// All scores are returned in `0.0..=1.0`.
pub struct TaskComplexityAnalyzer;

impl TaskComplexityAnalyzer {
    /// Create a new analyser.
    pub fn new() -> Self {
        Self
    }

    /// Estimate the complexity of `task`.
    ///
    /// Returns a value in `0.0..=1.0`:
    /// - `0.0` -- trivial (e.g. "hello")
    /// - `0.3` -- moderate (short technical question)
    /// - `0.7+` -- very complex (multi-step, code-heavy, architectural)
    pub fn analyze(&self, task: &str) -> f32 {
        let lower = task.to_lowercase();
        let words: Vec<&str> = lower.split_whitespace().collect();
        let word_count = words.len();

        // Component 1: Length -- 0.0-0.25 based on word count.
        let length_score = (word_count as f32 / 200.0).min(1.0) * 0.25;

        // Component 2: Sentence count (proxy for requirement count).
        // We count sentence-ending punctuation.
        let sentence_count = lower
            .chars()
            .filter(|c| *c == '.' || *c == '?' || *c == '!' || *c == ';')
            .count()
            .max(1); // At least 1 sentence.
        let sentence_score = ((sentence_count as f32 - 1.0) / 5.0).clamp(0.0, 1.0) * 0.15;

        // Component 3: Technical keywords.
        let technical_keywords = [
            "api",
            "database",
            "schema",
            "authentication",
            "authorization",
            "deploy",
            "kubernetes",
            "docker",
            "algorithm",
            "concurrency",
            "async",
            "microservice",
            "architecture",
            "security",
            "encryption",
            "sql",
            "graphql",
            "websocket",
            "distributed",
            "cache",
            "index",
            "migration",
        ];
        let tech_hits = technical_keywords
            .iter()
            .filter(|kw| lower.contains(**kw))
            .count();
        let tech_score = (tech_hits as f32 / 4.0).min(1.0) * 0.25;

        // Component 4: Multi-step indicators.
        let step_indicators = [
            "first",
            "second",
            "third",
            "then",
            "finally",
            "next",
            "step 1",
            "step 2",
            "step 3",
            "after that",
            "followed by",
            "in addition",
        ];
        let step_hits = step_indicators
            .iter()
            .filter(|si| lower.contains(**si))
            .count();
        let step_score = (step_hits as f32 / 3.0).min(1.0) * 0.20;

        // Component 5: Code fence presence.
        let code_score = if lower.contains("```") { 0.15 } else { 0.0 };

        (length_score + sentence_score + tech_score + step_score + code_score).clamp(0.0, 1.0)
    }
}

impl Default for TaskComplexityAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_question_low_complexity() {
        let a = TaskComplexityAnalyzer::new();
        let score = a.analyze("What is 2 + 2?");
        assert!(
            score < 0.3,
            "simple question should be low complexity, got {score}"
        );
    }

    #[test]
    fn complex_multipart_request_higher() {
        let a = TaskComplexityAnalyzer::new();
        let simple = a.analyze("What is Rust?");
        let complex = a.analyze(
            "First, design the database schema for a distributed authentication \
             microservice. Then implement the API endpoints with proper authorization. \
             Next, add caching with Redis. Finally, deploy to Kubernetes with \
             encrypted secrets.",
        );
        assert!(
            complex > simple,
            "complex ({complex}) should beat simple ({simple})"
        );
    }

    #[test]
    fn technical_keywords_raise_complexity() {
        let a = TaskComplexityAnalyzer::new();
        let no_tech = a.analyze("Tell me a story about a cat.");
        let with_tech = a.analyze("Set up the database schema and deploy to kubernetes.");
        assert!(
            with_tech > no_tech,
            "technical ({with_tech}) should beat non-technical ({no_tech})"
        );
    }

    #[test]
    fn empty_task_returns_zero() {
        let a = TaskComplexityAnalyzer::new();
        let score = a.analyze("");
        assert!(
            score.abs() < f32::EPSILON,
            "empty task should be 0.0, got {score}"
        );
    }

    #[test]
    fn score_clamped_to_unit_range() {
        let a = TaskComplexityAnalyzer::new();
        // Throw everything at it.
        let score = a.analyze(
            "First, design the api architecture for a distributed database microservice \
             with authentication and authorization. Then implement concurrency with async \
             websocket connections. Next, add encryption and caching. Finally, deploy to \
             kubernetes with docker. Step 1: schema migration. Step 2: graphql index. \
             Step 3: security audit. ```code here``` After that, followed by more work. \
             In addition, add algorithm for sql cache.",
        );
        assert!(score <= 1.0, "score should be <= 1.0, got {score}");
        assert!(score >= 0.0, "score should be >= 0.0, got {score}");
    }

    #[test]
    fn code_fences_raise_complexity() {
        let a = TaskComplexityAnalyzer::new();
        let no_code = a.analyze("Explain this function.");
        let with_code = a.analyze("Explain this function: ```fn main() {}```");
        assert!(
            with_code > no_code,
            "with code ({with_code}) should beat without ({no_code})"
        );
    }

    #[test]
    fn step_indicators_raise_complexity() {
        let a = TaskComplexityAnalyzer::new();
        let no_steps = a.analyze("Do the thing.");
        let with_steps = a.analyze("First do A, then do B, finally do C.");
        assert!(
            with_steps > no_steps,
            "with steps ({with_steps}) should beat without ({no_steps})"
        );
    }

    #[test]
    fn default_trait_creates_valid_analyzer() {
        let a = TaskComplexityAnalyzer;
        let score = a.analyze("hello");
        assert!(score >= 0.0);
        assert!(score <= 1.0);
    }

    #[test]
    fn longer_text_scores_higher_than_shorter() {
        let a = TaskComplexityAnalyzer::new();
        let short = a.analyze("help");
        let long = a.analyze(&"word ".repeat(100));
        assert!(
            long > short,
            "long text ({long}) should beat short ({short})"
        );
    }

    #[test]
    fn multiple_sentences_increase_score() {
        let a = TaskComplexityAnalyzer::new();
        let one = a.analyze("Do the task.");
        let many = a.analyze("Do task A. Then task B. Also task C. Check task D. Review task E.");
        assert!(
            many > one,
            "many sentences ({many}) should beat one ({one})"
        );
    }
}
