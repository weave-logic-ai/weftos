//! EML learned functions for benchmark scoring.
//!
//! Replaces the piecewise-linear scoring functions in [`bench_cmd`] with
//! [`eml_core::EmlModel`] learned approximations. Untrained models fall
//! back to the original hardcoded behavior so benchmarks remain usable
//! before any expert labels are collected.
//!
//! # Models
//!
//! - 5 per-dimension scorers (`EmlModel::new(2, 1, 1)`) — one metric in, one score out
//! - 1 composite scorer (`EmlModel::new(3, 5, 1)`) — all raw metrics to final score
//!
//! # Persistence
//!
//! Models are saved/loaded as JSON in the benchmark directory
//! (`$RUNTIME_DIR/benchmarks/eml/`).

use eml_core::EmlModel;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// ═══════════════════════════════════════════════════════════════════
// Per-dimension learned scorers
// ═══════════════════════════════════════════════════════════════════

/// A single-dimension learned scorer that maps one metric to a [0, 100] score.
///
/// When untrained, delegates to a provided fallback function so that
/// existing benchmark behavior is preserved.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DimensionScorer {
    name: String,
    model: EmlModel,
}

impl DimensionScorer {
    /// Create a new untrained dimension scorer.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            // depth=2, 1 input, 1 output — smallest viable EML model
            model: EmlModel::new(2, 1, 1),
        }
    }

    /// Score a single metric value using the learned model.
    ///
    /// Falls back to `fallback_fn` when untrained.
    pub fn score(&self, value: f64, fallback_fn: fn(f64) -> f64) -> f64 {
        if !self.model.is_trained() {
            return fallback_fn(value);
        }
        // Normalize input to a reasonable range for the model
        let normalized = self.normalize_input(value);
        let raw = self.model.predict_primary(&[normalized]);
        // Clamp to valid score range
        raw.clamp(0.0, 100.0)
    }

    /// Record a training sample: (raw_metric, expert_score).
    pub fn record(&mut self, value: f64, expert_score: f64) {
        let normalized = self.normalize_input(value);
        self.model.record(&[normalized], &[Some(expert_score)]);
    }

    /// Train the model. Returns true if converged.
    pub fn train(&mut self) -> bool {
        self.model.train()
    }

    /// Whether the model has been trained.
    pub fn is_trained(&self) -> bool {
        self.model.is_trained()
    }

    /// Number of training samples collected.
    pub fn training_sample_count(&self) -> usize {
        self.model.training_sample_count()
    }

    /// Normalize input to a [0, 1] range appropriate for each dimension.
    ///
    /// Uses the same ranges as the original piecewise breakpoints.
    fn normalize_input(&self, value: f64) -> f64 {
        match self.name.as_str() {
            "throughput" => value / 100_000.0,   // 0..100K ops/sec
            "latency" => value / 10_000.0,       // 0..10K us
            "scalability" => value,               // 0..1 coefficient
            "stability" => value / 20.0,          // 0..20 ratio
            "endurance" => value / 100.0,         // 0..100 drift %
            _ => value,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════
// Composite benchmark scorer
// ═══════════════════════════════════════════════════════════════════

/// Composite benchmark scorer that learns the mapping from 5 raw metrics
/// directly to an overall score, replacing the fixed dimension weights
/// (25/25/20/15/15).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkScorerModel {
    /// Per-dimension learned scorers.
    pub throughput: DimensionScorer,
    pub latency: DimensionScorer,
    pub scalability: DimensionScorer,
    pub stability: DimensionScorer,
    pub endurance: DimensionScorer,
    /// Composite: learns the overall score from 5 raw metrics.
    composite: EmlModel,
}

impl BenchmarkScorerModel {
    /// Create a new untrained benchmark scorer.
    pub fn new() -> Self {
        Self {
            throughput: DimensionScorer::new("throughput"),
            latency: DimensionScorer::new("latency"),
            scalability: DimensionScorer::new("scalability"),
            stability: DimensionScorer::new("stability"),
            endurance: DimensionScorer::new("endurance"),
            // depth=3, 5 inputs (raw metrics), 1 output (composite score)
            composite: EmlModel::new(3, 5, 1),
        }
    }

    /// Compute composite score from raw metrics using the learned model.
    ///
    /// Falls back to the weighted-average of per-dimension scores when
    /// the composite model is untrained.
    pub fn score(&self, metrics: &[f64; 5]) -> f64 {
        if self.composite.is_trained() {
            let normalized = self.normalize_metrics(metrics);
            return self.composite.predict_primary(&normalized).clamp(0.0, 100.0);
        }

        // Fallback: use per-dimension scorers (which themselves fall back
        // to the original piecewise-linear functions) with fixed weights.
        let scores = self.score_dimensions(metrics);
        let weights = [0.25, 0.25, 0.20, 0.15, 0.15];
        scores.iter().zip(weights.iter()).map(|(s, w)| s * w).sum()
    }

    /// Score each dimension individually.
    ///
    /// Returns `[throughput_score, latency_score, scalability_score,
    ///           stability_score, endurance_score]`.
    pub fn score_dimensions(&self, metrics: &[f64; 5]) -> [f64; 5] {
        [
            self.throughput.score(metrics[0], super::bench_cmd::score_throughput),
            self.latency.score(metrics[1], super::bench_cmd::score_latency),
            self.scalability.score(metrics[2], super::bench_cmd::score_scalability),
            self.stability.score(metrics[3], super::bench_cmd::score_stability),
            self.endurance.score(metrics[4], super::bench_cmd::score_endurance),
        ]
    }

    /// Record an expert-graded benchmark result for training.
    ///
    /// # Arguments
    /// - `metrics`: `[throughput_ops, latency_p95_us, scalability_coeff,
    ///               stability_ratio, endurance_drift_pct]`
    /// - `dimension_scores`: Optional per-dimension expert scores `[t, l, s, st, e]`.
    /// - `overall_score`: Expert overall score (0..100).
    pub fn record(
        &mut self,
        metrics: [f64; 5],
        dimension_scores: Option<[f64; 5]>,
        overall_score: f64,
    ) {
        // Record per-dimension if expert scores provided
        if let Some(ds) = dimension_scores {
            self.throughput.record(metrics[0], ds[0]);
            self.latency.record(metrics[1], ds[1]);
            self.scalability.record(metrics[2], ds[2]);
            self.stability.record(metrics[3], ds[3]);
            self.endurance.record(metrics[4], ds[4]);
        }

        // Record composite
        let normalized = self.normalize_metrics(&metrics);
        self.composite.record(&normalized, &[Some(overall_score)]);
    }

    /// Train all models. Returns true if the composite converged.
    pub fn train(&mut self) -> bool {
        let _ = self.throughput.train();
        let _ = self.latency.train();
        let _ = self.scalability.train();
        let _ = self.stability.train();
        let _ = self.endurance.train();
        self.composite.train()
    }

    /// Whether the composite model is trained.
    pub fn is_composite_trained(&self) -> bool {
        self.composite.is_trained()
    }

    /// Summary of training status across all models.
    pub fn training_summary(&self) -> String {
        format!(
            "composite: {} ({} samples), throughput: {}, latency: {}, \
             scalability: {}, stability: {}, endurance: {}",
            if self.composite.is_trained() { "trained" } else { "untrained" },
            self.composite.training_sample_count(),
            if self.throughput.is_trained() { "trained" } else { "untrained" },
            if self.latency.is_trained() { "trained" } else { "untrained" },
            if self.scalability.is_trained() { "trained" } else { "untrained" },
            if self.stability.is_trained() { "trained" } else { "untrained" },
            if self.endurance.is_trained() { "trained" } else { "untrained" },
        )
    }

    /// Normalize raw metrics to [0, 1] for the composite model.
    fn normalize_metrics(&self, metrics: &[f64; 5]) -> [f64; 5] {
        [
            metrics[0] / 100_000.0, // throughput: 0..100K
            metrics[1] / 10_000.0,  // latency: 0..10K us
            metrics[2],             // scalability: 0..1
            metrics[3] / 20.0,      // stability: 0..20
            metrics[4] / 100.0,     // endurance: 0..100%
        ]
    }

    // -------------------------------------------------------------------
    // Persistence
    // -------------------------------------------------------------------

    /// Default directory for saving/loading EML benchmark models.
    pub fn model_dir() -> PathBuf {
        clawft_rpc::runtime_dir().join("benchmarks").join("eml")
    }

    /// Save all models to the given directory.
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        std::fs::write(dir.join("scorer.json"), serde_json::to_string_pretty(self).unwrap())?;
        Ok(())
    }

    /// Load models from the given directory, or return a new untrained scorer.
    pub fn load(dir: &Path) -> Self {
        let path = dir.join("scorer.json");
        if path.exists()
            && let Ok(data) = std::fs::read_to_string(&path)
                && let Ok(model) = serde_json::from_str::<Self>(&data) {
                    return model;
                }
        Self::new()
    }
}

impl Default for BenchmarkScorerModel {
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
    fn dimension_scorer_untrained_uses_fallback() {
        let scorer = DimensionScorer::new("throughput");
        assert!(!scorer.is_trained());

        // Should delegate to the fallback function
        fn mock_fallback(x: f64) -> f64 { x * 2.0 }
        let result = scorer.score(25.0, mock_fallback);
        assert!((result - 50.0).abs() < 1e-9, "untrained should use fallback");
    }

    #[test]
    fn composite_scorer_untrained_uses_piecewise_linear() {
        let scorer = BenchmarkScorerModel::new();
        assert!(!scorer.is_composite_trained());

        // 100K ops, 50us latency, 0.9 scalability, 1.5 stability, 1% drift
        // All perfect scores => expected: 100 * (0.25 + 0.25 + 0.20 + 0.15 + 0.15) = 100
        let metrics = [100_000.0, 50.0, 0.9, 1.5, 1.0];
        let score = scorer.score(&metrics);
        assert!(
            (score - 100.0).abs() < 1.0,
            "perfect metrics should score ~100, got {score}"
        );
    }

    #[test]
    fn composite_scorer_zero_metrics() {
        let scorer = BenchmarkScorerModel::new();
        let metrics = [0.0, 10_000.0, 0.0, 20.0, 100.0];
        let score = scorer.score(&metrics);
        assert!(
            score.abs() < 1.0,
            "worst metrics should score ~0, got {score}"
        );
    }

    #[test]
    fn score_dimensions_returns_five() {
        let scorer = BenchmarkScorerModel::new();
        let metrics = [50_000.0, 500.0, 0.7, 3.0, 10.0];
        let dims = scorer.score_dimensions(&metrics);
        assert_eq!(dims.len(), 5);
        for (i, &d) in dims.iter().enumerate() {
            assert!(
                (0.0..=100.0).contains(&d),
                "dimension {i} out of range: {d}"
            );
        }
    }

    #[test]
    fn record_increments_counts() {
        let mut scorer = BenchmarkScorerModel::new();
        scorer.record(
            [50_000.0, 500.0, 0.7, 3.0, 10.0],
            Some([80.0, 60.0, 70.0, 60.0, 60.0]),
            68.0,
        );
        assert_eq!(scorer.throughput.training_sample_count(), 1);
        assert_eq!(scorer.latency.training_sample_count(), 1);
    }

    #[test]
    fn record_without_dimension_scores() {
        let mut scorer = BenchmarkScorerModel::new();
        scorer.record([50_000.0, 500.0, 0.7, 3.0, 10.0], None, 68.0);
        // Per-dimension should have 0 samples
        assert_eq!(scorer.throughput.training_sample_count(), 0);
    }

    #[test]
    fn train_insufficient_data() {
        let mut scorer = BenchmarkScorerModel::new();
        for i in 0..10 {
            scorer.record(
                [i as f64 * 10_000.0, 500.0, 0.5, 3.0, 10.0],
                None,
                50.0,
            );
        }
        // Not enough data (need >= 50)
        assert!(!scorer.train());
    }

    #[test]
    fn serialization_roundtrip() {
        let scorer = BenchmarkScorerModel::new();
        let json = serde_json::to_string(&scorer).unwrap();
        let restored: BenchmarkScorerModel = serde_json::from_str(&json).unwrap();
        assert!(!restored.is_composite_trained());
        assert_eq!(
            restored.throughput.name, scorer.throughput.name,
            "name should survive roundtrip"
        );
    }

    #[test]
    fn training_summary_format() {
        let scorer = BenchmarkScorerModel::new();
        let summary = scorer.training_summary();
        assert!(summary.contains("composite: untrained"));
        assert!(summary.contains("throughput: untrained"));
    }

    #[test]
    fn default_impl() {
        let scorer = BenchmarkScorerModel::default();
        assert!(!scorer.is_composite_trained());
    }
}
