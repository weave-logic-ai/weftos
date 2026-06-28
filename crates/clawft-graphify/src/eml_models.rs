//! EML learned-function wrappers for graphify heuristics.
//!
//! Each model wraps an [`eml_core::EmlModel`] and provides:
//! - A domain-specific prediction API
//! - Hardcoded fallback when untrained
//! - `record` + `train` for online learning from user feedback
//!
//! All models are opt-in: passing `None` to analysis functions keeps the
//! original hardcoded behaviour.

use eml_core::EmlModel;

// ---------------------------------------------------------------------------
// 1. SurpriseScorerModel  (analyze.rs)
// ---------------------------------------------------------------------------

/// Learned surprise scorer for graph edges.
///
/// Takes 7 features and produces a single surprise score:
/// `[confidence_ordinal, same_file_type, same_repo, same_community,
///   is_semantic, min_degree, max_degree]`
///
/// Training data: user ratings of edge surprise (1-5 scale).
pub struct SurpriseScorerModel {
    model: EmlModel,
    trained: bool,
}

impl SurpriseScorerModel {
    /// Create a new untrained model.
    pub fn new() -> Self {
        Self {
            model: EmlModel::new(3, 7, 1),
            trained: false,
        }
    }

    /// Score an edge given its feature vector.
    ///
    /// Returns a surprise score (higher = more surprising).
    /// Falls back to the original hardcoded logic when untrained.
    pub fn score(&self, features: &[f64; 7]) -> f64 {
        if !self.trained {
            return self.fallback_score(features);
        }
        self.model.predict_primary(features)
    }

    /// Original hardcoded surprise scoring logic.
    ///
    /// Features: `[confidence_ordinal, cross_file_type, cross_repo,
    ///   cross_community, is_semantic, min_degree, max_degree]`
    fn fallback_score(&self, f: &[f64; 7]) -> f64 {
        let mut score: f64 = 0.0;

        // 1. Confidence weight: Ambiguous=3, Inferred=2, Extracted=1
        let conf_ord = f[0];
        if conf_ord >= 2.5 {
            score += 3.0; // Ambiguous
        } else if conf_ord >= 1.5 {
            score += 2.0; // Inferred
        } else {
            score += 1.0; // Extracted
        }

        // 2. Cross file-type bonus
        if f[1] > 0.5 {
            score += 2.0;
        }

        // 3. Cross-repo bonus
        if f[2] > 0.5 {
            score += 2.0;
        }

        // 4. Cross-community bonus
        if f[3] > 0.5 {
            score += 1.0;
        }

        // 4b. Semantic similarity multiplier
        if f[4] > 0.5 {
            score *= 1.5;
        }

        // 5. Peripheral-to-hub bonus
        let min_deg = f[5];
        let max_deg = f[6];
        if min_deg <= 2.0 && max_deg >= 5.0 {
            score += 1.0;
        }

        score
    }

    /// Record a training sample: features + user surprise rating (1-5).
    pub fn record(&mut self, features: [f64; 7], user_rating: f64) {
        self.model.record(&features, &[Some(user_rating)]);
    }

    /// Train on accumulated samples. Returns `true` if converged.
    pub fn train(&mut self) -> bool {
        let converged = self.model.train();
        if converged {
            self.trained = true;
        }
        converged
    }

    /// Whether the model has been trained.
    pub fn is_trained(&self) -> bool {
        self.trained
    }

    /// Serialize to JSON for persistence.
    pub fn to_json(&self) -> String {
        self.model.to_json()
    }

    /// Deserialize from JSON.
    pub fn from_json(json: &str) -> Option<Self> {
        EmlModel::from_json(json).map(|model| {
            let trained = model.is_trained();
            Self { model, trained }
        })
    }
}

impl Default for SurpriseScorerModel {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// 2. ClusterThresholdModel  (cluster.rs)
// ---------------------------------------------------------------------------

/// Learned community detection thresholds.
///
/// Takes 3 graph-topology features and produces 3 thresholds:
/// `(node_count, edge_density, current_community_count)`
///   -> `(max_fraction, min_split_size, cohesion_threshold)`
///
/// Training data: validated community quality ratings.
pub struct ClusterThresholdModel {
    model: EmlModel,
    trained: bool,
}

impl ClusterThresholdModel {
    /// Create a new untrained model.
    pub fn new() -> Self {
        Self {
            model: EmlModel::new(2, 3, 3),
            trained: false,
        }
    }

    /// Predict thresholds for the given graph topology.
    ///
    /// Returns `(max_community_fraction, min_split_size, cohesion_threshold)`.
    pub fn predict(
        &self,
        node_count: f64,
        edge_density: f64,
        community_count: f64,
    ) -> (f64, f64, f64) {
        if !self.trained {
            return self.fallback();
        }
        let inputs = [node_count, edge_density, community_count];
        let out = self.model.predict(&inputs);
        (
            out[0].clamp(0.05, 0.50),
            out[1].max(2.0),
            out[2].clamp(0.01, 0.50),
        )
    }

    /// Hardcoded defaults from the original constants.
    fn fallback(&self) -> (f64, f64, f64) {
        (0.25, 10.0, 0.15)
    }

    /// Record a training sample.
    pub fn record(
        &mut self,
        node_count: f64,
        edge_density: f64,
        community_count: f64,
        optimal_fraction: f64,
        optimal_min_split: f64,
        optimal_cohesion: f64,
    ) {
        self.model.record(
            &[node_count, edge_density, community_count],
            &[
                Some(optimal_fraction),
                Some(optimal_min_split),
                Some(optimal_cohesion),
            ],
        );
    }

    /// Train on accumulated samples.
    pub fn train(&mut self) -> bool {
        let converged = self.model.train();
        if converged {
            self.trained = true;
        }
        converged
    }

    /// Whether the model has been trained.
    pub fn is_trained(&self) -> bool {
        self.trained
    }

    /// Serialize to JSON.
    pub fn to_json(&self) -> String {
        self.model.to_json()
    }

    /// Deserialize from JSON.
    pub fn from_json(json: &str) -> Option<Self> {
        EmlModel::from_json(json).map(|model| {
            let trained = model.is_trained();
            Self { model, trained }
        })
    }
}

impl Default for ClusterThresholdModel {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// 3. LayoutModel  (export/html.rs)
// ---------------------------------------------------------------------------

/// Learned ForceAtlas2 physics parameter tuner.
///
/// Takes 3 graph-structure features and produces 6 physics params:
/// `(node_count, edge_count, density)`
///   -> `(gravity, spring_length, spring_constant, damping, central_gravity, node_distance)`
///
/// Training data: user layout quality ratings (1-5).
pub struct LayoutModel {
    model: EmlModel,
    trained: bool,
}

/// ForceAtlas2 physics parameters for vis.js.
#[derive(Debug, Clone)]
pub struct PhysicsParams {
    pub gravitational_constant: f64,
    pub spring_length: f64,
    pub spring_constant: f64,
    pub damping: f64,
    pub central_gravity: f64,
    pub avoid_overlap: f64,
}

impl PhysicsParams {
    /// Default hardcoded values from the original html.rs.
    pub fn default_params() -> Self {
        Self {
            gravitational_constant: -60.0,
            spring_length: 120.0,
            spring_constant: 0.08,
            damping: 0.4,
            central_gravity: 0.005,
            avoid_overlap: 0.8,
        }
    }
}

impl LayoutModel {
    /// Create a new untrained model.
    pub fn new() -> Self {
        Self {
            model: EmlModel::new(3, 3, 6),
            trained: false,
        }
    }

    /// Predict physics parameters for the given graph.
    pub fn predict(&self, node_count: f64, edge_count: f64, density: f64) -> PhysicsParams {
        if !self.trained {
            return PhysicsParams::default_params();
        }
        let inputs = [node_count, edge_count, density];
        let out = self.model.predict(&inputs);
        PhysicsParams {
            gravitational_constant: out[0].clamp(-200.0, -10.0),
            spring_length: out[1].clamp(50.0, 500.0),
            spring_constant: out[2].clamp(0.01, 0.5),
            damping: out[3].clamp(0.05, 0.95),
            central_gravity: out[4].clamp(0.001, 0.1),
            avoid_overlap: out[5].clamp(0.0, 1.0),
        }
    }

    /// Record a training sample with the physics params that led to a good layout.
    pub fn record(
        &mut self,
        node_count: f64,
        edge_count: f64,
        density: f64,
        params: &PhysicsParams,
    ) {
        self.model.record(
            &[node_count, edge_count, density],
            &[
                Some(params.gravitational_constant),
                Some(params.spring_length),
                Some(params.spring_constant),
                Some(params.damping),
                Some(params.central_gravity),
                Some(params.avoid_overlap),
            ],
        );
    }

    /// Train on accumulated samples.
    pub fn train(&mut self) -> bool {
        let converged = self.model.train();
        if converged {
            self.trained = true;
        }
        converged
    }

    /// Whether the model has been trained.
    pub fn is_trained(&self) -> bool {
        self.trained
    }

    /// Serialize to JSON.
    pub fn to_json(&self) -> String {
        self.model.to_json()
    }

    /// Deserialize from JSON.
    pub fn from_json(json: &str) -> Option<Self> {
        EmlModel::from_json(json).map(|model| {
            let trained = model.is_trained();
            Self { model, trained }
        })
    }
}

impl Default for LayoutModel {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// 4. ForensicCoherenceModel  (domain/forensic.rs)
// ---------------------------------------------------------------------------

/// Learned coherence scoring for forensic knowledge graphs.
///
/// Takes 4 features and produces a coherence score:
/// `(density, avg_confidence, node_count, edge_count)` -> coherence
///
/// Training data: expert coherence ratings on forensic case graphs.
pub struct ForensicCoherenceModel {
    model: EmlModel,
    trained: bool,
}

impl ForensicCoherenceModel {
    /// Create a new untrained model.
    pub fn new() -> Self {
        Self {
            model: EmlModel::new(3, 4, 1),
            trained: false,
        }
    }

    /// Predict coherence given graph statistics.
    ///
    /// Falls back to `density * avg_confidence` when untrained.
    pub fn predict(
        &self,
        density: f64,
        avg_confidence: f64,
        node_count: f64,
        edge_count: f64,
    ) -> f64 {
        if !self.trained {
            return self.fallback(density, avg_confidence);
        }
        let inputs = [density, avg_confidence, node_count, edge_count];
        self.model.predict_primary(&inputs).clamp(0.0, 1.0)
    }

    /// Original linear formula: `density * avg_confidence`.
    fn fallback(&self, density: f64, avg_confidence: f64) -> f64 {
        density * avg_confidence
    }

    /// Record a training sample.
    pub fn record(
        &mut self,
        density: f64,
        avg_confidence: f64,
        node_count: f64,
        edge_count: f64,
        expert_coherence: f64,
    ) {
        self.model.record(
            &[density, avg_confidence, node_count, edge_count],
            &[Some(expert_coherence)],
        );
    }

    /// Train on accumulated samples.
    pub fn train(&mut self) -> bool {
        let converged = self.model.train();
        if converged {
            self.trained = true;
        }
        converged
    }

    /// Whether the model has been trained.
    pub fn is_trained(&self) -> bool {
        self.trained
    }

    /// Serialize to JSON.
    pub fn to_json(&self) -> String {
        self.model.to_json()
    }

    /// Deserialize from JSON.
    pub fn from_json(json: &str) -> Option<Self> {
        EmlModel::from_json(json).map(|model| {
            let trained = model.is_trained();
            Self { model, trained }
        })
    }
}

impl Default for ForensicCoherenceModel {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// 5. QueryFusionModel  (graphify_cmd.rs / KG-001)
// ---------------------------------------------------------------------------

/// Learned score fusion for hybrid graph search.
///
/// Takes 4 features and produces a single fused relevance score:
/// `[keyword_score, graph_proximity_score, community_match_score,
///   entity_type_relevance]` -> fused_score
///
/// Training data: user marks results as relevant (1.0) or irrelevant (0.0).
pub struct QueryFusionModel {
    model: EmlModel,
    trained: bool,
}

impl QueryFusionModel {
    /// Create a new untrained model.
    pub fn new() -> Self {
        Self {
            model: EmlModel::new(3, 4, 1),
            trained: false,
        }
    }

    /// Fuse individual scores into a single relevance score.
    ///
    /// Inputs: `[keyword_score, graph_proximity_score,
    ///   community_match_score, entity_type_relevance]`
    ///
    /// Falls back to a simple normalized sum when untrained.
    pub fn fuse(&self, features: &[f64; 4]) -> f64 {
        if !self.trained {
            return self.fallback_fuse(features);
        }
        self.model.predict_primary(features).max(0.0)
    }

    /// Simple weighted-sum fallback.
    ///
    /// Weights: keyword=0.4, proximity=0.3, community=0.2, entity_type=0.1
    fn fallback_fuse(&self, f: &[f64; 4]) -> f64 {
        f[0] * 0.4 + f[1] * 0.3 + f[2] * 0.2 + f[3] * 0.1
    }

    /// Record a training sample: features + user relevance (0.0 or 1.0).
    pub fn record(&mut self, features: [f64; 4], relevance: f64) {
        self.model.record(&features, &[Some(relevance)]);
    }

    /// Train on accumulated samples. Returns `true` if converged.
    pub fn train(&mut self) -> bool {
        let converged = self.model.train();
        if converged {
            self.trained = true;
        }
        converged
    }

    /// Whether the model has been trained.
    pub fn is_trained(&self) -> bool {
        self.trained
    }

    /// Serialize to JSON for persistence.
    pub fn to_json(&self) -> String {
        self.model.to_json()
    }

    /// Deserialize from JSON.
    pub fn from_json(json: &str) -> Option<Self> {
        EmlModel::from_json(json).map(|model| {
            let trained = model.is_trained();
            Self { model, trained }
        })
    }
}

impl Default for QueryFusionModel {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- SurpriseScorerModel ------------------------------------------------

    #[test]
    fn surprise_fallback_ambiguous_cross_file_type() {
        let model = SurpriseScorerModel::new();
        // Ambiguous (3.0), cross file type, same repo, same community, not semantic, low/high degree
        let features = [3.0, 1.0, 0.0, 0.0, 0.0, 1.0, 8.0];
        let score = model.score(&features);
        // 3 (ambiguous) + 2 (cross file type) + 1 (peripheral-hub) = 6
        assert!((score - 6.0).abs() < f64::EPSILON, "got {score}");
    }

    #[test]
    fn surprise_fallback_extracted_same_everything() {
        let model = SurpriseScorerModel::new();
        // Extracted (1.0), all same, not semantic, similar degrees
        let features = [1.0, 0.0, 0.0, 0.0, 0.0, 4.0, 4.0];
        let score = model.score(&features);
        // 1 (extracted) only
        assert!((score - 1.0).abs() < f64::EPSILON, "got {score}");
    }

    #[test]
    fn surprise_fallback_semantic_multiplier() {
        let model = SurpriseScorerModel::new();
        // Inferred, same types, same repo, cross community, semantic
        let features = [2.0, 0.0, 0.0, 1.0, 1.0, 4.0, 4.0];
        let score = model.score(&features);
        // 2 (inferred) + 1 (cross community) = 3, then * 1.5 = 4.5
        assert!((score - 4.5).abs() < f64::EPSILON, "got {score}");
    }

    #[test]
    fn surprise_untrained_flag() {
        let model = SurpriseScorerModel::new();
        assert!(!model.is_trained());
    }

    #[test]
    fn surprise_serialization_roundtrip() {
        let model = SurpriseScorerModel::new();
        let json = model.to_json();
        let restored = SurpriseScorerModel::from_json(&json).unwrap();
        assert!(!restored.is_trained());
    }

    // -- ClusterThresholdModel -----------------------------------------------

    #[test]
    fn cluster_fallback_values() {
        let model = ClusterThresholdModel::new();
        let (frac, split, cohesion) = model.predict(100.0, 0.1, 5.0);
        assert!((frac - 0.25).abs() < f64::EPSILON);
        assert!((split - 10.0).abs() < f64::EPSILON);
        assert!((cohesion - 0.15).abs() < f64::EPSILON);
    }

    #[test]
    fn cluster_untrained_flag() {
        let model = ClusterThresholdModel::new();
        assert!(!model.is_trained());
    }

    #[test]
    fn cluster_serialization_roundtrip() {
        let model = ClusterThresholdModel::new();
        let json = model.to_json();
        let restored = ClusterThresholdModel::from_json(&json).unwrap();
        assert!(!restored.is_trained());
    }

    // -- LayoutModel --------------------------------------------------------

    #[test]
    fn layout_fallback_values() {
        let model = LayoutModel::new();
        let params = model.predict(100.0, 200.0, 0.04);
        assert!((params.gravitational_constant - (-60.0)).abs() < f64::EPSILON);
        assert!((params.spring_length - 120.0).abs() < f64::EPSILON);
        assert!((params.spring_constant - 0.08).abs() < f64::EPSILON);
        assert!((params.damping - 0.4).abs() < f64::EPSILON);
        assert!((params.central_gravity - 0.005).abs() < f64::EPSILON);
        assert!((params.avoid_overlap - 0.8).abs() < f64::EPSILON);
    }

    #[test]
    fn layout_untrained_flag() {
        let model = LayoutModel::new();
        assert!(!model.is_trained());
    }

    #[test]
    fn layout_serialization_roundtrip() {
        let model = LayoutModel::new();
        let json = model.to_json();
        let restored = LayoutModel::from_json(&json).unwrap();
        assert!(!restored.is_trained());
    }

    // -- ForensicCoherenceModel ----------------------------------------------

    #[test]
    fn forensic_fallback_linear_formula() {
        let model = ForensicCoherenceModel::new();
        let score = model.predict(0.5, 0.8, 10.0, 20.0);
        assert!((score - 0.4).abs() < f64::EPSILON, "got {score}");
    }

    #[test]
    fn forensic_fallback_zero_density() {
        let model = ForensicCoherenceModel::new();
        let score = model.predict(0.0, 0.9, 5.0, 0.0);
        assert!((score - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn forensic_untrained_flag() {
        let model = ForensicCoherenceModel::new();
        assert!(!model.is_trained());
    }

    #[test]
    fn forensic_serialization_roundtrip() {
        let model = ForensicCoherenceModel::new();
        let json = model.to_json();
        let restored = ForensicCoherenceModel::from_json(&json).unwrap();
        assert!(!restored.is_trained());
    }

    // -- QueryFusionModel ----------------------------------------------------

    #[test]
    fn query_fusion_fallback_weighted_sum() {
        let model = QueryFusionModel::new();
        // keyword=1.0, proximity=0.5, community=0.5, entity_type=1.0
        let score = model.fuse(&[1.0, 0.5, 0.5, 1.0]);
        // 1.0*0.4 + 0.5*0.3 + 0.5*0.2 + 1.0*0.1 = 0.4+0.15+0.10+0.10 = 0.75
        assert!((score - 0.75).abs() < f64::EPSILON, "got {score}");
    }

    #[test]
    fn query_fusion_fallback_zeros() {
        let model = QueryFusionModel::new();
        let score = model.fuse(&[0.0, 0.0, 0.0, 0.0]);
        assert!((score - 0.0).abs() < f64::EPSILON, "got {score}");
    }

    #[test]
    fn query_fusion_untrained_flag() {
        let model = QueryFusionModel::new();
        assert!(!model.is_trained());
    }

    #[test]
    fn query_fusion_serialization_roundtrip() {
        let model = QueryFusionModel::new();
        let json = model.to_json();
        let restored = QueryFusionModel::from_json(&json).unwrap();
        assert!(!restored.is_trained());
    }
}
