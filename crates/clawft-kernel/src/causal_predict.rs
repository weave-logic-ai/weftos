//! Causal collapse prediction via O(1) analytical perturbation + EML correction.
//!
//! Predicts how adding a new edge will change the causal graph's algebraic
//! connectivity (lambda_2) **without** recomputing the full O(k*m) Lanczos
//! eigenvalue iteration. The core insight comes from first-order eigenvalue
//! perturbation theory:
//!
//! ```text
//! delta_lambda_2 = w * (phi[u] - phi[v])^2
//! ```
//!
//! where phi is the Fiedler vector and w is the edge weight. Edges that
//! bridge the spectral partition (phi[u] and phi[v] have opposite signs)
//! produce the largest coherence gains.
//!
//! # Components
//!
//! - [`predict_delta_lambda2`] -- O(1) analytical perturbation formula
//! - [`rank_evidence_by_impact`] -- rank candidate edges by predicted impact
//! - [`detect_conversation_cycle`] -- detect stuck/oscillating conversations
//! - [`CausalCollapseModel`] -- EML-enhanced prediction with learned corrections
//! - [`CausalGraph::rank_candidates`] -- convenience method on the causal graph
//!
//! # References
//!
//! See `.planning/development_notes/eml-causal-collapse-research.md` for the
//! full mathematical derivation and design rationale.

use serde::{Deserialize, Serialize};

use crate::causal::CausalGraph;

// ---------------------------------------------------------------------------
// Analytical delta-lambda_2 prediction
// ---------------------------------------------------------------------------

/// First-order perturbation prediction: delta_lambda_2 = w * (phi[u] - phi[v])^2
///
/// O(1) per candidate -- just a multiplication and squared difference.
///
/// # Arguments
///
/// - `fiedler_u`: Fiedler vector component for node u.
/// - `fiedler_v`: Fiedler vector component for node v.
/// - `edge_weight`: Weight of the candidate edge.
///
/// # Returns
///
/// Predicted change in algebraic connectivity. Always non-negative because
/// adding edges can only increase lambda_2.
pub fn predict_delta_lambda2(fiedler_u: f64, fiedler_v: f64, edge_weight: f64) -> f64 {
    edge_weight * (fiedler_u - fiedler_v).powi(2)
}

// ---------------------------------------------------------------------------
// Evidence ranking
// ---------------------------------------------------------------------------

/// Result of ranking a candidate evidence addition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceRanking {
    /// Source node ID.
    pub source: u64,
    /// Target node ID.
    pub target: u64,
    /// Edge weight.
    pub weight: f32,
    /// Predicted change in algebraic connectivity.
    pub predicted_delta: f64,
    /// Human-readable explanation of why this edge matters.
    pub explanation: String,
}

/// Rank candidate evidence by predicted coherence impact.
///
/// Uses the Fiedler vector from the most recent spectral analysis to predict
/// how each candidate edge would change lambda_2, without actually adding any
/// edges.
///
/// Returns sorted by `predicted_delta` descending (biggest impact first).
///
/// # Arguments
///
/// - `fiedler`: The current Fiedler vector (indexed by node position).
/// - `current_lambda2`: Current algebraic connectivity (unused in first-order,
///   but available for logging and higher-order corrections).
/// - `candidates`: Slice of `(source_node_id, target_node_id, weight)` tuples.
pub fn rank_evidence_by_impact(
    fiedler: &[f64],
    _current_lambda2: f64,
    candidates: &[(u64, u64, f32)],
) -> Vec<EvidenceRanking> {
    let mut rankings: Vec<EvidenceRanking> = candidates
        .iter()
        .map(|&(src, tgt, w)| {
            let phi_u = fiedler.get(src as usize).copied().unwrap_or(0.0);
            let phi_v = fiedler.get(tgt as usize).copied().unwrap_or(0.0);
            let delta = predict_delta_lambda2(phi_u, phi_v, w as f64);

            let explanation = if phi_u.signum() != phi_v.signum()
                && phi_u.abs() > 1e-12
                && phi_v.abs() > 1e-12
            {
                format!(
                    "Bridges the graph partition (phi[{}]={:.3}, phi[{}]={:.3})",
                    src, phi_u, tgt, phi_v
                )
            } else if delta > 0.1 {
                format!("Strengthens weak connection (delta_lambda2={:.4})", delta)
            } else {
                format!("Reinforces existing cluster (delta_lambda2={:.4})", delta)
            };

            EvidenceRanking {
                source: src,
                target: tgt,
                weight: w,
                predicted_delta: delta,
                explanation,
            }
        })
        .collect();

    rankings.sort_by(|a, b| {
        b.predicted_delta
            .partial_cmp(&a.predicted_delta)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    rankings
}

// ---------------------------------------------------------------------------
// Conversation cycle detection
// ---------------------------------------------------------------------------

/// State of a conversation's coherence trajectory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConversationState {
    /// Not enough data to determine state.
    TooEarly,
    /// Coherence is increasing at a measurable rate.
    Converging {
        /// Rate of convergence (delta per tick).
        rate: f64,
    },
    /// Coherence is decreasing at a measurable rate.
    Diverging {
        /// Rate of divergence (delta per tick).
        rate: f64,
    },
    /// Coherence is not changing meaningfully.
    Stuck {
        /// Net change over the window.
        net_change: f64,
    },
    /// Coherence is bouncing up and down without net progress.
    Oscillating {
        /// Net change over the window.
        net_change: f64,
        /// Maximum swing between consecutive ticks.
        max_swing: f64,
    },
}

/// Detect if coherence changes indicate a circular/stuck conversation.
///
/// Examines the last `window` entries of the coherence history and classifies
/// the trajectory as converging, diverging, stuck, or oscillating.
///
/// # Arguments
///
/// - `coherence_history`: Full history of coherence values (one per tick).
/// - `window`: Number of recent entries to examine.
/// - `threshold`: Minimum meaningful change (below this = stuck).
///
/// # Implementation
///
/// Dispatches through [`clawft_treecalc::triage_trajectory`] first
/// (Finding #8 in `docs/eml-treecalc-swap-sites.md`): the trajectory
/// is classified as `Atom` (flat), `Sequence` (monotone), or
/// `Branch` (oscillating) by its sign-of-difference pattern. The
/// treecalc form then drives which `ConversationState` is returned,
/// and the per-state metrics (rate, net_change, max_swing) are
/// computed from the same recent window. This replaces the nested
/// threshold arithmetic with a structural dispatch that's easier to
/// extend (e.g. a future `Atom` sub-classifier distinguishing
/// "never-started" from "plateaued").
pub fn detect_conversation_cycle(
    coherence_history: &[f64],
    window: usize,
    threshold: f64,
) -> ConversationState {
    if coherence_history.len() < window || window < 2 {
        return ConversationState::TooEarly;
    }

    let recent = &coherence_history[coherence_history.len() - window..];
    let first = recent[0];
    let last = recent[recent.len() - 1];
    let total_change = (last - first).abs();
    let max_swing = recent
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0f64, f64::max);

    // Structural dispatch on the trajectory's form.
    match clawft_treecalc::triage_trajectory(recent, threshold) {
        clawft_treecalc::Form::Atom => ConversationState::Stuck {
            net_change: total_change,
        },
        clawft_treecalc::Form::Sequence => {
            // Monotone. Sign of `last - first` tells us up vs. down.
            let rate = total_change / window as f64;
            if last > first {
                ConversationState::Converging { rate }
            } else {
                ConversationState::Diverging { rate }
            }
        }
        clawft_treecalc::Form::Branch => {
            // Oscillating means the sign-of-difference flipped at
            // least once. If the net change is still large we call
            // it Converging/Diverging with noise; if small, we call
            // it Oscillating (net flat with visible swings).
            if total_change >= threshold {
                let rate = total_change / window as f64;
                if last > first {
                    ConversationState::Converging { rate }
                } else {
                    ConversationState::Diverging { rate }
                }
            } else {
                ConversationState::Oscillating {
                    net_change: total_change,
                    max_swing,
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Information Gain Pruning (KG-005)
// ---------------------------------------------------------------------------

/// Check if a candidate edge is redundant given recent additions.
///
/// An edge is considered redundant (low marginal information gain) when
/// **all three** of the following hold:
///
/// 1. Both endpoints are on the same side of the Fiedler partition
///    (same sign in the Fiedler vector), meaning they are already in the
///    same connected region.
/// 2. Its predicted `delta_lambda_2` is below `threshold`, indicating
///    the area is already well-connected and the edge adds negligible
///    algebraic connectivity.
/// 3. A recent addition already bridged the same Fiedler partition --
///    i.e., at least one edge in `recent_additions` has endpoints with
///    opposite Fiedler signs and a non-trivial predicted delta.
///
/// This is intended for use in the DEMOCRITUS update loop: before
/// processing each candidate edge, call `is_redundant` to decide
/// whether to skip it, achieving 50-70% token reduction in dense graphs.
///
/// # Arguments
///
/// - `candidate`: The `(source, target, weight)` edge being considered.
/// - `recent_additions`: Edges that have already been accepted in this
///   DEMOCRITUS cycle.
/// - `fiedler`: The current Fiedler vector (indexed by node position).
/// - `threshold`: Minimum delta_lambda_2 for an edge to be considered
///   non-redundant (e.g., 0.01).
///
/// # Returns
///
/// `true` if the candidate should be skipped (redundant).
pub fn is_redundant(
    candidate: &(u64, u64, f32),
    recent_additions: &[(u64, u64, f32)],
    fiedler: &[f64],
    threshold: f64,
) -> bool {
    let (src, tgt, w) = *candidate;

    let phi_src = fiedler.get(src as usize).copied().unwrap_or(0.0);
    let phi_tgt = fiedler.get(tgt as usize).copied().unwrap_or(0.0);

    // Condition 1: same side of the partition.
    // If they bridge the partition (opposite signs), never redundant.
    let same_side = phi_src.signum() == phi_tgt.signum()
        || phi_src.abs() < 1e-12
        || phi_tgt.abs() < 1e-12;
    if !same_side {
        return false;
    }

    // Condition 2: low predicted delta.
    let delta = predict_delta_lambda2(phi_src, phi_tgt, w as f64);
    if delta >= threshold {
        return false;
    }

    // Condition 3: a recent addition already bridged the partition.
    

    recent_additions.iter().any(|&(rs, rt, rw)| {
        let phi_rs = fiedler.get(rs as usize).copied().unwrap_or(0.0);
        let phi_rt = fiedler.get(rt as usize).copied().unwrap_or(0.0);
        let opposite_signs = phi_rs.signum() != phi_rt.signum()
            && phi_rs.abs() > 1e-12
            && phi_rt.abs() > 1e-12;
        let recent_delta = predict_delta_lambda2(phi_rs, phi_rt, rw as f64);
        opposite_signs && recent_delta > threshold
    })
}

// ---------------------------------------------------------------------------
// EML correction model for multi-edge batching
// ---------------------------------------------------------------------------

/// Feature vector for the causal collapse correction model.
#[derive(Debug, Clone)]
pub struct CollapseFeatures {
    /// Fiedler vector component for the source node.
    pub fiedler_u: f64,
    /// Fiedler vector component for the target node.
    pub fiedler_v: f64,
    /// Edge weight.
    pub edge_weight: f64,
    /// Current algebraic connectivity.
    pub current_lambda2: f64,
    /// Spectral gap (lambda_3 - lambda_2).
    pub spectral_gap: f64,
    /// Graph edge density.
    pub graph_density: f64,
    /// Number of nodes in the graph.
    pub node_count: f64,
    /// Degree of the source node.
    pub degree_u: f64,
    /// Degree of the target node.
    pub degree_v: f64,
}

impl CollapseFeatures {
    /// Convert to a normalized feature vector for the EML model.
    fn to_vec(&self) -> Vec<f64> {
        vec![
            self.fiedler_u.clamp(-1.0, 1.0),
            self.fiedler_v.clamp(-1.0, 1.0),
            self.edge_weight / 10.0,
            self.current_lambda2 / 100.0,
            self.spectral_gap / 100.0,
            self.graph_density,
            self.node_count / 10000.0,
            self.degree_u / 1000.0,
            self.degree_v / 1000.0,
        ]
    }
}

/// Internal training record storing both features and the residual ground truth.
#[derive(Debug, Clone)]
#[allow(dead_code)] // populated by training flow that lands later in the EML-collapse work
struct CollapseTrainingPoint {
    features: Vec<f64>,
    residual: f64,
}

/// EML-enhanced delta prediction for when the analytical formula is insufficient.
///
/// Learns corrections for: multi-edge batching, Fiedler staleness, phase transitions.
/// The model trains on the **residual** (actual - analytical) so the analytical
/// formula provides the baseline and the EML tree only needs to learn the error.
///
/// # Architecture
///
/// - Depth 3, 9 inputs, 1 head
/// - Inputs: fiedler_u, fiedler_v, edge_weight, current_lambda2, spectral_gap,
///   graph_density, node_count, degree_u, degree_v
/// - Output: correction to add to the analytical prediction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CausalCollapseModel {
    /// First-order correction model.
    model: eml_core::EmlModel,
    /// Training data (not serialized -- transient buffer).
    #[serde(skip)]
    training_data: Vec<CollapseTrainingPoint>,
}

impl Default for CausalCollapseModel {
    fn default() -> Self {
        Self::new()
    }
}

impl CausalCollapseModel {
    /// Create a new untrained collapse prediction model.
    pub fn new() -> Self {
        let mut model = eml_core::EmlModel::new(3, 9, 1);
        model.set_model_name("causal_collapse");
        Self {
            model,
            training_data: Vec::new(),
        }
    }

    /// Whether the underlying EML model has been trained.
    pub fn is_trained(&self) -> bool {
        self.model.is_trained()
    }

    /// Number of training samples collected.
    pub fn training_sample_count(&self) -> usize {
        self.training_data.len()
    }

    /// Predict with EML correction applied to analytical formula.
    ///
    /// When the model is untrained, returns the pure analytical prediction.
    /// When trained, adds a learned correction that accounts for higher-order
    /// effects like Fiedler staleness and multi-edge batching.
    pub fn predict(&self, features: &CollapseFeatures) -> f64 {
        let analytical =
            predict_delta_lambda2(features.fiedler_u, features.fiedler_v, features.edge_weight);

        if !self.model.is_trained() {
            return analytical;
        }

        // EML learns the CORRECTION to the analytical formula
        let input_vec = features.to_vec();
        let correction = self.model.predict_primary(&input_vec);
        analytical + correction
    }

    /// Record a ground-truth observation for future training.
    ///
    /// `actual_delta` is the real change in lambda_2 measured after adding
    /// the edge. The model trains on the residual (actual - analytical).
    pub fn record(&mut self, features: CollapseFeatures, actual_delta: f64) {
        let analytical =
            predict_delta_lambda2(features.fiedler_u, features.fiedler_v, features.edge_weight);
        let residual = actual_delta - analytical;
        let input_vec = features.to_vec();

        // Also record in the inner EML model's training buffer
        self.model.record(&input_vec, &[Some(residual)]);

        self.training_data.push(CollapseTrainingPoint {
            features: input_vec,
            residual,
        });
    }

    /// Train the EML correction model.
    ///
    /// Requires at least 50 training samples. Returns `true` if the model
    /// converged (MSE < 0.01).
    pub fn train(&mut self) -> bool {
        self.model.train()
    }

    /// Drain accumulated EML lifecycle events for ExoChain forwarding.
    pub fn drain_events(&mut self) -> Vec<eml_core::EmlEvent> {
        self.model.drain_events()
    }
}

// ---------------------------------------------------------------------------
// CausalGraph integration
// ---------------------------------------------------------------------------

impl CausalGraph {
    /// Rank candidate edges by predicted coherence impact.
    ///
    /// Uses the Fiedler vector from a recent spectral analysis to predict
    /// how each candidate edge would affect algebraic connectivity, without
    /// actually modifying the graph.
    ///
    /// # Arguments
    ///
    /// - `candidates`: Slice of `(source_node_id, target_node_id, weight)`.
    /// - `fiedler`: Fiedler vector (indexed by node position, not node ID).
    /// - `lambda2`: Current algebraic connectivity.
    ///
    /// # Returns
    ///
    /// Rankings sorted by predicted delta descending (biggest impact first).
    pub fn rank_candidates(
        &self,
        candidates: &[(u64, u64, f32)],
        fiedler: &[f64],
        lambda2: f64,
    ) -> Vec<EvidenceRanking> {
        rank_evidence_by_impact(fiedler, lambda2, candidates)
    }
}

// ---------------------------------------------------------------------------
// RPC types for daemon integration
// ---------------------------------------------------------------------------

/// Request payload for the `causal.rank` RPC endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CausalRankRequest {
    /// Candidate edges to rank: `[(source_id, target_id, weight), ...]`
    pub candidates: Vec<(u64, u64, f32)>,
}

/// Response payload for the `causal.rank` RPC endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CausalRankResponse {
    /// Current algebraic connectivity before any additions.
    pub current_lambda2: f64,
    /// Ranked evidence by predicted coherence impact (descending).
    pub rankings: Vec<EvidenceRanking>,
}

// ---------------------------------------------------------------------------
// Coherence history tracker (for DEMOCRITUS integration)
// ---------------------------------------------------------------------------

/// Tracks coherence history for conversation cycle detection.
///
/// Intended to be held alongside the DEMOCRITUS loop state. Call
/// [`push`] after each coherence measurement and [`check`] periodically
/// to detect stuck or oscillating conversations.
#[derive(Debug, Clone, Default)]
pub struct CoherenceTracker {
    history: Vec<f64>,
}

impl CoherenceTracker {
    /// Create a new empty tracker.
    pub fn new() -> Self {
        Self {
            history: Vec::new(),
        }
    }

    /// Record a coherence measurement.
    pub fn push(&mut self, coherence: f64) {
        self.history.push(coherence);
    }

    /// Number of measurements recorded.
    pub fn len(&self) -> usize {
        self.history.len()
    }

    /// Whether no measurements have been recorded.
    pub fn is_empty(&self) -> bool {
        self.history.is_empty()
    }

    /// Check for conversation cycle.
    ///
    /// Returns `None` if there are fewer than `window` measurements.
    pub fn check(&self, window: usize, threshold: f64) -> ConversationState {
        detect_conversation_cycle(&self.history, window, threshold)
    }

    /// Access the raw history.
    pub fn history(&self) -> &[f64] {
        &self.history
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::causal::{CausalEdgeType, CausalGraph};

    // -- predict_delta_lambda2 on known graphs ----------------------------------

    #[test]
    fn delta_lambda2_same_side_is_zero() {
        // Two nodes with the same Fiedler component: delta should be 0.
        let delta = predict_delta_lambda2(0.5, 0.5, 1.0);
        assert!(delta.abs() < 1e-12, "same Fiedler components => delta=0");
    }

    #[test]
    fn delta_lambda2_opposite_sides() {
        // Nodes on opposite sides of the partition.
        let delta = predict_delta_lambda2(0.5, -0.5, 1.0);
        assert!(
            (delta - 1.0).abs() < 1e-12,
            "delta should be 1.0 * (0.5 - (-0.5))^2 = 1.0, got {delta}"
        );
    }

    #[test]
    fn delta_lambda2_weight_scaling() {
        let delta = predict_delta_lambda2(1.0, 0.0, 2.0);
        assert!(
            (delta - 2.0).abs() < 1e-12,
            "delta should scale with weight: 2.0 * 1.0^2 = 2.0, got {delta}"
        );
    }

    #[test]
    fn delta_lambda2_zero_weight() {
        let delta = predict_delta_lambda2(1.0, -1.0, 0.0);
        assert!(delta.abs() < 1e-12, "zero weight => zero delta");
    }

    #[test]
    fn delta_lambda2_path_graph_fiedler() {
        // Path graph P_4: Fiedler vector components are proportional to
        // cos(pi * i / 4) for i = 0..3, normalized. For the perturbation
        // formula test, use unnormalized values.
        let phi = [0.653, 0.271, -0.271, -0.653]; // approximate P_4 Fiedler
        // Edge bridging the cut (1->2) should have largest delta.
        let delta_bridge = predict_delta_lambda2(phi[1], phi[2], 1.0);
        // Edge within same side (0->1) should have smaller delta.
        let delta_same = predict_delta_lambda2(phi[0], phi[1], 1.0);
        assert!(
            delta_bridge > delta_same,
            "bridge edge should have larger delta: bridge={delta_bridge}, same={delta_same}"
        );
    }

    #[test]
    fn delta_lambda2_star_graph() {
        // Star graph S_5: center node has Fiedler component 0, leaves are
        // symmetric. Adding any edge to center has same delta.
        let center = 0.0;
        let leaf = 0.5;
        let delta = predict_delta_lambda2(center, leaf, 1.0);
        assert!(
            (delta - 0.25).abs() < 1e-12,
            "center-leaf delta = 1.0 * 0.5^2 = 0.25, got {delta}"
        );
    }

    #[test]
    fn delta_lambda2_complete_graph() {
        // Complete graph K_n: all Fiedler components are equal (proportional to
        // constant vector for the fully-connected case). Adding an edge has
        // delta = 0 because all components are the same.
        let phi_val = 1.0 / 5.0_f64.sqrt();
        let delta = predict_delta_lambda2(phi_val, phi_val, 1.0);
        assert!(
            delta.abs() < 1e-12,
            "K_n: all components equal => delta=0, got {delta}"
        );
    }

    // -- rank_evidence_by_impact ------------------------------------------------

    #[test]
    fn ranking_order_descending() {
        let fiedler = vec![0.5, -0.5, 0.1, -0.1];
        let candidates = vec![(0, 1, 1.0f32), (2, 3, 1.0f32), (0, 2, 1.0f32)];
        let rankings = rank_evidence_by_impact(&fiedler, 1.0, &candidates);

        // (0, 1) bridges the cut with components 0.5 and -0.5 => delta = 1.0
        // (2, 3) bridges a small gap: 0.1 and -0.1 => delta = 0.04
        // (0, 2) same side-ish: 0.5 and 0.1 => delta = 0.16
        assert_eq!(rankings.len(), 3);
        assert!(
            rankings[0].predicted_delta >= rankings[1].predicted_delta,
            "rankings should be descending"
        );
        assert!(
            rankings[1].predicted_delta >= rankings[2].predicted_delta,
            "rankings should be descending"
        );
    }

    #[test]
    fn ranking_bridge_explanation() {
        let fiedler = vec![0.5, -0.5];
        let candidates = vec![(0, 1, 1.0f32)];
        let rankings = rank_evidence_by_impact(&fiedler, 1.0, &candidates);
        assert_eq!(rankings.len(), 1);
        assert!(
            rankings[0].explanation.contains("Bridges"),
            "opposite-sign nodes should get bridge explanation: {}",
            rankings[0].explanation
        );
    }

    #[test]
    fn ranking_empty_candidates() {
        let fiedler = vec![0.5, -0.5];
        let rankings = rank_evidence_by_impact(&fiedler, 1.0, &[]);
        assert!(rankings.is_empty());
    }

    #[test]
    fn ranking_out_of_bounds_node_ids() {
        // Node IDs beyond Fiedler vector length default to phi=0.0.
        let fiedler = vec![0.5, -0.5];
        let candidates = vec![(0, 99, 1.0f32)];
        let rankings = rank_evidence_by_impact(&fiedler, 1.0, &candidates);
        assert_eq!(rankings.len(), 1);
        // phi[0]=0.5, phi[99]=0.0 => delta = 1.0 * 0.25 = 0.25
        assert!(
            (rankings[0].predicted_delta - 0.25).abs() < 1e-12,
            "out-of-bounds defaults to 0.0"
        );
    }

    // -- detect_conversation_cycle all 4 states ---------------------------------

    #[test]
    fn cycle_too_early() {
        let history = vec![1.0, 2.0];
        let state = detect_conversation_cycle(&history, 5, 0.01);
        assert!(matches!(state, ConversationState::TooEarly));
    }

    #[test]
    fn cycle_converging() {
        // Steadily increasing coherence.
        let history: Vec<f64> = (0..20).map(|i| i as f64 * 0.1).collect();
        let state = detect_conversation_cycle(&history, 10, 0.01);
        match state {
            ConversationState::Converging { rate } => {
                assert!(rate > 0.0, "converging rate should be positive");
            }
            other => panic!("expected Converging, got {:?}", other),
        }
    }

    #[test]
    fn cycle_diverging() {
        // Steadily decreasing coherence.
        let history: Vec<f64> = (0..20).map(|i| 2.0 - i as f64 * 0.1).collect();
        let state = detect_conversation_cycle(&history, 10, 0.01);
        match state {
            ConversationState::Diverging { rate } => {
                assert!(rate > 0.0, "diverging rate should be positive");
            }
            other => panic!("expected Diverging, got {:?}", other),
        }
    }

    #[test]
    fn cycle_stuck() {
        // Flat coherence.
        let history = vec![1.0; 20];
        let state = detect_conversation_cycle(&history, 10, 0.01);
        match state {
            ConversationState::Stuck { net_change } => {
                assert!(
                    net_change < 0.01,
                    "stuck net_change should be below threshold"
                );
            }
            other => panic!("expected Stuck, got {:?}", other),
        }
    }

    #[test]
    fn cycle_oscillating() {
        // Alternating high/low values with near-zero net change.
        // Window of 10 starts and ends on the same value (even indices = 1.0)
        // so net_change is 0.0, but max_swing is 0.1 which exceeds 2*threshold.
        let mut history = Vec::new();
        for i in 0..21 {
            if i % 2 == 0 {
                history.push(1.0);
            } else {
                history.push(1.1);
            }
        }
        // Window = 11 entries: starts at 1.0, ends at 1.0 => net_change = 0
        let state = detect_conversation_cycle(&history, 11, 0.01);
        match state {
            ConversationState::Oscillating {
                net_change,
                max_swing,
            } => {
                assert!(net_change < 0.01, "net change should be below threshold");
                assert!(max_swing > 0.02, "max swing should exceed 2*threshold");
            }
            other => panic!("expected Oscillating, got {:?}", other),
        }
    }

    // -- CausalCollapseModel untrained fallback ---------------------------------

    #[test]
    fn collapse_model_untrained_fallback() {
        let model = CausalCollapseModel::new();
        assert!(!model.is_trained());

        let features = CollapseFeatures {
            fiedler_u: 0.5,
            fiedler_v: -0.5,
            edge_weight: 1.0,
            current_lambda2: 2.0,
            spectral_gap: 1.0,
            graph_density: 0.3,
            node_count: 10.0,
            degree_u: 3.0,
            degree_v: 4.0,
        };

        let predicted = model.predict(&features);
        let analytical = predict_delta_lambda2(0.5, -0.5, 1.0);
        assert!(
            (predicted - analytical).abs() < 1e-12,
            "untrained model should return pure analytical: expected {analytical}, got {predicted}"
        );
    }

    #[test]
    fn collapse_model_record_and_count() {
        let mut model = CausalCollapseModel::new();
        assert_eq!(model.training_sample_count(), 0);

        let features = CollapseFeatures {
            fiedler_u: 0.3,
            fiedler_v: -0.2,
            edge_weight: 0.8,
            current_lambda2: 1.5,
            spectral_gap: 0.5,
            graph_density: 0.4,
            node_count: 20.0,
            degree_u: 5.0,
            degree_v: 3.0,
        };

        model.record(features, 0.3);
        assert_eq!(model.training_sample_count(), 1);
    }

    #[test]
    fn collapse_model_trained_produces_finite() {
        let mut model = CausalCollapseModel::new();

        // Generate enough training data
        for i in 0..80 {
            let u = (i as f64 / 80.0) - 0.5;
            let v = -u;
            let w = 0.5 + (i as f64 / 160.0);
            let analytical = predict_delta_lambda2(u, v, w);
            // Simulate a small correction (10% error)
            let actual = analytical * 1.1;

            let features = CollapseFeatures {
                fiedler_u: u,
                fiedler_v: v,
                edge_weight: w,
                current_lambda2: 2.0,
                spectral_gap: 1.0,
                graph_density: 0.3,
                node_count: 50.0,
                degree_u: 5.0,
                degree_v: 5.0,
            };
            model.record(features, actual);
        }

        // Training should not panic
        let _ = model.train();

        // Prediction should be finite
        let features = CollapseFeatures {
            fiedler_u: 0.3,
            fiedler_v: -0.3,
            edge_weight: 1.0,
            current_lambda2: 2.0,
            spectral_gap: 1.0,
            graph_density: 0.3,
            node_count: 50.0,
            degree_u: 5.0,
            degree_v: 5.0,
        };
        let predicted = model.predict(&features);
        assert!(predicted.is_finite(), "trained prediction should be finite");
    }

    // -- CausalGraph::rank_candidates convenience method -----------------------

    #[test]
    fn causal_graph_rank_candidates() {
        let g = CausalGraph::new();
        let a = g.add_node("A".into(), serde_json::json!({}));
        let b = g.add_node("B".into(), serde_json::json!({}));
        let c = g.add_node("C".into(), serde_json::json!({}));
        g.link(a, b, CausalEdgeType::Causes, 1.0, 0, 0);

        // Synthetic Fiedler vector: A and B on same side, C on other.
        let fiedler = vec![0.0; a as usize]
            .into_iter()
            .chain(std::iter::once(0.5))  // a
            .chain(std::iter::once(0.3))  // b
            .chain(std::iter::once(-0.6)) // c
            .collect::<Vec<f64>>();

        let candidates = vec![(a, c, 1.0f32), (b, c, 1.0f32), (a, b, 1.0f32)];
        let rankings = g.rank_candidates(&candidates, &fiedler, 1.0);

        assert_eq!(rankings.len(), 3);
        // (a,c) delta = (0.5-(-0.6))^2 = 1.21
        // (b,c) delta = (0.3-(-0.6))^2 = 0.81
        // (a,b) delta = (0.5-0.3)^2 = 0.04
        assert_eq!(rankings[0].source, a);
        assert_eq!(rankings[0].target, c);
    }

    // -- CoherenceTracker -------------------------------------------------------

    #[test]
    fn coherence_tracker_basic() {
        let mut tracker = CoherenceTracker::new();
        assert!(tracker.is_empty());
        assert_eq!(tracker.len(), 0);

        tracker.push(1.0);
        tracker.push(1.1);
        tracker.push(1.2);
        assert_eq!(tracker.len(), 3);

        let state = tracker.check(3, 0.01);
        assert!(matches!(state, ConversationState::Converging { .. }));
    }

    // -- Serialization roundtrip ------------------------------------------------

    #[test]
    fn evidence_ranking_serde() {
        let r = EvidenceRanking {
            source: 1,
            target: 2,
            weight: 0.5,
            predicted_delta: 0.42,
            explanation: "test".into(),
        };
        let json = serde_json::to_string(&r).unwrap();
        let restored: EvidenceRanking = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.source, 1);
        assert_eq!(restored.target, 2);
        assert!((restored.predicted_delta - 0.42).abs() < 1e-12);
    }

    #[test]
    fn conversation_state_serde() {
        let state = ConversationState::Oscillating {
            net_change: 0.01,
            max_swing: 0.15,
        };
        let json = serde_json::to_string(&state).unwrap();
        let _restored: ConversationState = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn causal_rank_request_serde() {
        let req = CausalRankRequest {
            candidates: vec![(1, 2, 0.5), (3, 4, 0.8)],
        };
        let json = serde_json::to_string(&req).unwrap();
        let restored: CausalRankRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.candidates.len(), 2);
    }

    #[test]
    fn collapse_model_serde() {
        let model = CausalCollapseModel::new();
        let json = serde_json::to_string(&model).unwrap();
        let restored: CausalCollapseModel = serde_json::from_str(&json).unwrap();
        assert!(!restored.is_trained());
    }

    // ===================================================================
    // KG-005: Information Gain Pruning — is_redundant
    // ===================================================================

    #[test]
    fn redundant_same_side_low_delta_bridge_exists() {
        // Fiedler: nodes 0,1 on positive side; nodes 2,3 on negative side.
        let fiedler = vec![0.5, 0.3, -0.4, -0.6];
        // Recent addition bridged the partition: (1, 2) with opposite signs.
        let recent = vec![(1u64, 2u64, 1.0f32)];
        // Candidate: (0, 1) — same side, low delta.
        let candidate = (0u64, 1u64, 0.1f32);
        assert!(
            is_redundant(&candidate, &recent, &fiedler, 0.05),
            "same-side, low-delta, bridge exists => redundant"
        );
    }

    #[test]
    fn not_redundant_bridges_partition() {
        let fiedler = vec![0.5, 0.3, -0.4, -0.6];
        let recent: Vec<(u64, u64, f32)> = vec![];
        // Candidate: (0, 2) — opposite sides.
        let candidate = (0u64, 2u64, 1.0f32);
        assert!(
            !is_redundant(&candidate, &recent, &fiedler, 0.01),
            "bridge edge should never be redundant"
        );
    }

    #[test]
    fn not_redundant_high_delta() {
        let fiedler = vec![0.5, 0.3, -0.4, -0.6];
        let recent = vec![(1u64, 2u64, 1.0f32)];
        // Candidate: (0, 1) same side but with high weight => high delta.
        let candidate = (0u64, 1u64, 10.0f32);
        let phi_u = fiedler[0];
        let phi_v = fiedler[1];
        let delta = predict_delta_lambda2(phi_u, phi_v, 10.0);
        // With threshold lower than delta, delta >= threshold so not redundant.
        assert!(
            !is_redundant(&candidate, &recent, &fiedler, delta - 0.001),
            "high-delta edge should not be redundant when delta >= threshold"
        );
    }

    #[test]
    fn not_redundant_no_recent_bridge() {
        let fiedler = vec![0.5, 0.3, -0.4, -0.6];
        // No recent additions bridged the partition.
        let recent = vec![(0u64, 1u64, 1.0f32)]; // same side
        let candidate = (0u64, 1u64, 0.01f32);
        assert!(
            !is_redundant(&candidate, &recent, &fiedler, 0.05),
            "no bridge in recent => not redundant even if same side + low delta"
        );
    }

    #[test]
    fn not_redundant_empty_recent() {
        let fiedler = vec![0.5, 0.3, -0.4, -0.6];
        let recent: Vec<(u64, u64, f32)> = vec![];
        let candidate = (0u64, 1u64, 0.01f32);
        assert!(
            !is_redundant(&candidate, &recent, &fiedler, 0.05),
            "empty recent => not redundant"
        );
    }

    #[test]
    fn redundant_with_zero_fiedler_components() {
        // phi=0 treated as "same side" — candidates connecting to unknown
        // nodes are conservative (potentially redundant).
        let fiedler = vec![0.5, 0.0];
        let recent = vec![(0u64, 99u64, 2.0f32)]; // won't bridge since phi[99]=0
        let candidate = (0u64, 1u64, 0.001f32);
        // phi[0]=0.5, phi[1]=0.0 => same side (zero treated as no info).
        // delta = 0.001 * 0.25 = 0.00025 < threshold.
        // But recent (0,99) has phi[99]=0 => not opposite signs => no bridge.
        assert!(
            !is_redundant(&candidate, &recent, &fiedler, 0.01),
            "zero-component recent doesn't count as bridge"
        );
    }

    #[test]
    fn redundant_dense_graph_scenario() {
        // Simulate a dense region where many edges are redundant.
        // Positive cluster: 0,1,2,3. Negative: 4,5.
        let fiedler = vec![0.4, 0.3, 0.2, 0.1, -0.5, -0.6];
        // One edge already bridged the partition.
        let recent = vec![(3u64, 4u64, 1.0f32)];
        // Many within-cluster candidates should be redundant.
        let c1 = (0u64, 1u64, 0.01f32);
        let c2 = (1u64, 2u64, 0.01f32);
        let c3 = (2u64, 3u64, 0.01f32);
        assert!(is_redundant(&c1, &recent, &fiedler, 0.01));
        assert!(is_redundant(&c2, &recent, &fiedler, 0.01));
        assert!(is_redundant(&c3, &recent, &fiedler, 0.01));
    }
}
