//! 6-stage pluggable pipeline system.
//!
//! Stages: Classifier -> Router -> Assembler -> Transport -> Scorer -> Learner

pub mod assembler;
pub mod classifier;
pub mod cost_tracker;
pub mod learner;
#[cfg(feature = "native")]
pub mod llm_adapter;
pub mod mutation;
#[cfg(feature = "native")]
pub mod service_llm_adapter;
pub mod permissions;
pub mod rate_limiter;
pub mod router;
pub mod scorer;
pub mod tiered_router;
pub mod traits;
pub mod transport;

use std::sync::Arc;

use clawft_types::config::PipelineConfig;

use self::learner::{NoopLearner, TrajectoryLearner, TrajectoryLearnerConfig};
use self::scorer::{FitnessScorer, NoopScorer};
use self::traits::{LearningBackend, QualityScorer};

/// Build a quality scorer from configuration.
///
/// - `"fitness"` -> [`FitnessScorer`] (Level 1 multi-objective scorer)
/// - `"noop"` or anything else -> [`NoopScorer`] (Level 0 baseline)
pub fn build_scorer(config: &PipelineConfig) -> Arc<dyn QualityScorer> {
    match config.scorer.as_str() {
        "fitness" => Arc::new(FitnessScorer::new()),
        _ => Arc::new(NoopScorer::new()),
    }
}

/// Build a learning backend from configuration.
///
/// - `"trajectory"` -> [`TrajectoryLearner`] (Level 1 GEPA-inspired)
/// - `"noop"` or anything else -> [`NoopLearner`] (Level 0 baseline)
pub fn build_learner(config: &PipelineConfig) -> Arc<dyn LearningBackend> {
    match config.learner.as_str() {
        "trajectory" => Arc::new(TrajectoryLearner::new(TrajectoryLearnerConfig::default())),
        _ => Arc::new(NoopLearner::new()),
    }
}

#[cfg(test)]
mod factory_tests {
    use super::*;
    use clawft_types::config::PipelineConfig;

    #[test]
    fn build_scorer_noop_default() {
        let config = PipelineConfig::default();
        let scorer = build_scorer(&config);
        // NoopScorer returns 1.0 for everything
        let req = traits::ChatRequest {
            messages: vec![],
            tools: vec![],
            model: None,
            max_tokens: None,
            temperature: None,
            auth_context: None,
            complexity_boost: 0.0,
        };
        let resp = clawft_types::provider::LlmResponse {
            id: "test".into(),
            content: vec![],
            stop_reason: clawft_types::provider::StopReason::EndTurn,
            usage: clawft_types::provider::Usage {
                input_tokens: 0,
                output_tokens: 0,
                total_tokens: 0,
            },
            metadata: std::collections::HashMap::new(),
        };
        let score = scorer.score(&req, &resp);
        assert!((score.overall - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn build_scorer_fitness() {
        let config = PipelineConfig {
            scorer: "fitness".into(),
            learner: "noop".into(),
        };
        let scorer = build_scorer(&config);
        // FitnessScorer returns different scores for empty responses
        let req = traits::ChatRequest {
            messages: vec![],
            tools: vec![],
            model: None,
            max_tokens: None,
            temperature: None,
            auth_context: None,
            complexity_boost: 0.0,
        };
        let resp = clawft_types::provider::LlmResponse {
            id: "test".into(),
            content: vec![],
            stop_reason: clawft_types::provider::StopReason::EndTurn,
            usage: clawft_types::provider::Usage {
                input_tokens: 0,
                output_tokens: 0,
                total_tokens: 0,
            },
            metadata: std::collections::HashMap::new(),
        };
        let score = scorer.score(&req, &resp);
        // Empty response: FitnessScorer should return < 1.0
        assert!(score.overall < 1.0);
    }

    #[test]
    fn build_learner_noop_default() {
        let config = PipelineConfig::default();
        let _learner = build_learner(&config);
        // NoopLearner just works without panicking
    }

    #[test]
    fn build_learner_trajectory() {
        let config = PipelineConfig {
            scorer: "noop".into(),
            learner: "trajectory".into(),
        };
        let _learner = build_learner(&config);
        // TrajectoryLearner constructed without panicking
    }

    #[test]
    fn build_scorer_unknown_falls_to_noop() {
        let config = PipelineConfig {
            scorer: "unknown_thing".into(),
            learner: "noop".into(),
        };
        let scorer = build_scorer(&config);
        let req = traits::ChatRequest {
            messages: vec![],
            tools: vec![],
            model: None,
            max_tokens: None,
            temperature: None,
            auth_context: None,
            complexity_boost: 0.0,
        };
        let resp = clawft_types::provider::LlmResponse {
            id: "test".into(),
            content: vec![],
            stop_reason: clawft_types::provider::StopReason::EndTurn,
            usage: clawft_types::provider::Usage {
                input_tokens: 0,
                output_tokens: 0,
                total_tokens: 0,
            },
            metadata: std::collections::HashMap::new(),
        };
        let score = scorer.score(&req, &resp);
        assert!((score.overall - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn build_learner_unknown_falls_to_noop() {
        let config = PipelineConfig {
            scorer: "noop".into(),
            learner: "not_real".into(),
        };
        let _learner = build_learner(&config);
    }
}
