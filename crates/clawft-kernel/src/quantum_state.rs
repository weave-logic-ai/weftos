//! Quantum-inspired cognitive state for the ECC substrate.
//!
//! Layers superposition and Born-rule probabilities on top of
//! the deterministic CausalGraph. The classical path is always
//! available via `collapse()` or by not using this module.
//!
//! Key concepts:
//! - State vector |psi> over graph nodes (superposition)
//! - Hamiltonian H = graph Laplacian L (same as spectral analysis)
//! - Unitary evolution exp(-i*dt*H) between measurements
//! - Born rule P(k) = |<k|psi>|^2 for measurement outcomes
//! - Projective collapse on evidence arrival

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Complex number (no external dep)
// ---------------------------------------------------------------------------

/// Complex number for quantum amplitudes (no external dep needed).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Complex {
    pub re: f64,
    pub im: f64,
}

impl Complex {
    pub fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }

    pub fn real(re: f64) -> Self {
        Self { re, im: 0.0 }
    }

    pub fn zero() -> Self {
        Self { re: 0.0, im: 0.0 }
    }

    pub fn norm_sq(&self) -> f64 {
        self.re * self.re + self.im * self.im
    }

    pub fn norm(&self) -> f64 {
        self.norm_sq().sqrt()
    }

    pub fn conj(&self) -> Self {
        Self {
            re: self.re,
            im: -self.im,
        }
    }

    pub fn add(&self, other: &Self) -> Self {
        Self {
            re: self.re + other.re,
            im: self.im + other.im,
        }
    }

    pub fn sub(&self, other: &Self) -> Self {
        Self {
            re: self.re - other.re,
            im: self.im - other.im,
        }
    }

    pub fn mul(&self, other: &Self) -> Self {
        Self {
            re: self.re * other.re - self.im * other.im,
            im: self.re * other.im + self.im * other.re,
        }
    }

    pub fn scale(&self, s: f64) -> Self {
        Self {
            re: self.re * s,
            im: self.im * s,
        }
    }
}

// ---------------------------------------------------------------------------
// QuantumCognitiveState
// ---------------------------------------------------------------------------

/// Quantum state vector over graph nodes.
///
/// |psi> = sum_i alpha_i |i> where alpha_i are complex amplitudes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantumCognitiveState {
    /// Complex amplitudes per node. Length = number of graph nodes.
    psi: Vec<Complex>,
    /// Node IDs corresponding to each amplitude index.
    node_ids: Vec<u64>,
    /// Number of measurements performed.
    measurement_count: u64,
    /// Entropy history for decoherence tracking.
    entropy_history: Vec<f64>,
}

impl QuantumCognitiveState {
    /// Initialize from the Fiedler vector (classical -> quantum).
    ///
    /// The Fiedler vector becomes the initial state with real amplitudes,
    /// normalized so that the Born-rule probabilities sum to 1.
    pub fn from_fiedler(fiedler: &[f64], node_ids: &[u64]) -> Self {
        let norm: f64 = fiedler.iter().map(|x| x * x).sum::<f64>().sqrt();
        let psi: Vec<Complex> = fiedler
            .iter()
            .map(|&x| Complex::real(x / norm.max(1e-15)))
            .collect();
        Self {
            psi,
            node_ids: node_ids.to_vec(),
            measurement_count: 0,
            entropy_history: Vec::new(),
        }
    }

    /// Initialize uniform superposition (maximum uncertainty).
    pub fn uniform(n: usize, node_ids: &[u64]) -> Self {
        let amp = 1.0 / (n as f64).sqrt();
        let psi = vec![Complex::real(amp); n];
        Self {
            psi,
            node_ids: node_ids.to_vec(),
            measurement_count: 0,
            entropy_history: Vec::new(),
        }
    }

    /// Born rule: probability of node `i` being in the "answer."
    pub fn probability(&self, i: usize) -> f64 {
        self.psi.get(i).map(|a| a.norm_sq()).unwrap_or(0.0)
    }

    /// Born rule: probability distribution over all nodes.
    pub fn probabilities(&self) -> Vec<f64> {
        self.psi.iter().map(|a| a.norm_sq()).collect()
    }

    /// Von Neumann entropy: S = -sum p_i ln(p_i).
    ///
    /// - 0 = fully collapsed (one node dominates)
    /// - ln(n) = maximum uncertainty (uniform superposition)
    pub fn entropy(&self) -> f64 {
        -self
            .psi
            .iter()
            .map(|a| {
                let p = a.norm_sq();
                if p > 1e-15 { p * p.ln() } else { 0.0 }
            })
            .sum::<f64>()
    }

    /// Coherent evolution: |psi(t+dt)> = exp(-i dt H)|psi(t)>.
    ///
    /// Uses first-order approximation: |psi'> ~= (I - i dt H)|psi>.
    /// H = graph Laplacian L (sparse, computed from CausalGraph adjacency).
    ///
    /// `laplacian_action` computes H|psi> given |psi>.
    pub fn evolve(&mut self, laplacian_action: impl Fn(&[Complex]) -> Vec<Complex>, dt: f64) {
        // |psi'> = |psi> - i*dt * H|psi>
        let h_psi = laplacian_action(&self.psi);
        for (i, hp) in h_psi.iter().enumerate() {
            // -i * dt * hp = dt * (hp.im, -hp.re)
            self.psi[i].re += dt * hp.im;
            self.psi[i].im -= dt * hp.re;
        }
        self.normalize();
    }

    /// Measure evidence impact via expectation value: <psi|DeltaL|psi>.
    ///
    /// This is the quantum generalization of delta_lambda_2 = w*(phi[u]-phi[v])^2.
    pub fn evidence_impact(&self, u_idx: usize, v_idx: usize, weight: f64) -> f64 {
        let diff = self.psi[u_idx].sub(&self.psi[v_idx]);
        weight * diff.norm_sq()
    }

    /// Rank evidence by quantum impact (Born-rule weighted).
    ///
    /// This generalizes `rank_evidence_by_impact` to use amplitudes
    /// instead of the Fiedler vector.
    pub fn rank_evidence_quantum(
        &self,
        candidates: &[(usize, usize, f64)], // (u_idx, v_idx, weight)
    ) -> Vec<QuantumEvidenceRanking> {
        let mut rankings: Vec<_> = candidates
            .iter()
            .map(|&(u, v, w)| {
                let impact = self.evidence_impact(u, v, w);
                let p_u = self.probability(u);
                let p_v = self.probability(v);
                QuantumEvidenceRanking {
                    u_idx: u,
                    v_idx: v,
                    weight: w,
                    impact,
                    prob_u: p_u,
                    prob_v: p_v,
                    explanation: format!("Impact={:.4}, P(u)={:.3}, P(v)={:.3}", impact, p_u, p_v),
                }
            })
            .collect();
        rankings.sort_by(|a, b| {
            b.impact
                .partial_cmp(&a.impact)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        rankings
    }

    /// Projective measurement: collapse onto evidence outcome.
    ///
    /// After observing that edge (u,v) IS part of the causal chain,
    /// amplitudes at u and v are boosted, others are suppressed.
    pub fn observe_evidence(&mut self, u_idx: usize, v_idx: usize, weight: f64) {
        let boost = (1.0 + weight).sqrt();
        self.psi[u_idx] = self.psi[u_idx].scale(boost);
        self.psi[v_idx] = self.psi[v_idx].scale(boost);
        self.normalize();
        self.measurement_count += 1;
        self.entropy_history.push(self.entropy());
    }

    /// Partial collapse: suppress hypotheses inconsistent with evidence.
    ///
    /// Nodes contradicting the evidence have amplitudes dampened by `factor`.
    pub fn suppress(&mut self, node_idx: usize, factor: f64) {
        self.psi[node_idx] = self.psi[node_idx].scale(factor);
        self.normalize();
    }

    /// Full collapse: return the classical deterministic answer.
    ///
    /// Projects |psi> onto the most probable basis state.
    /// Returns `(index, probability)`.
    pub fn collapse(&self) -> (usize, f64) {
        self.psi
            .iter()
            .enumerate()
            .map(|(i, a)| (i, a.norm_sq()))
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap_or((0, 0.0))
    }

    /// Get the classical Fiedler-like vector (real parts of amplitudes).
    ///
    /// This is the "classical projection" -- always available.
    pub fn to_classical(&self) -> Vec<f64> {
        self.psi.iter().map(|a| a.re).collect()
    }

    /// Normalize: ensure sum of |alpha_i|^2 = 1.
    fn normalize(&mut self) {
        let norm: f64 = self.psi.iter().map(|a| a.norm_sq()).sum::<f64>().sqrt();
        if norm > 1e-15 {
            for a in &mut self.psi {
                a.re /= norm;
                a.im /= norm;
            }
        }
    }

    /// Is the state close to collapsed? (entropy near zero)
    pub fn is_collapsed(&self, threshold: f64) -> bool {
        self.entropy() < threshold
    }

    /// How many measurements until collapse?
    ///
    /// Based on entropy decay rate from history.
    pub fn estimated_measurements_to_collapse(&self) -> Option<usize> {
        if self.entropy_history.len() < 2 {
            return None;
        }
        let recent = &self.entropy_history[self.entropy_history.len().saturating_sub(5)..];
        if recent.len() < 2 {
            return None;
        }
        let rate = (recent.first().unwrap() - recent.last().unwrap()) / recent.len() as f64;
        if rate <= 0.0 {
            return None; // not converging
        }
        let remaining = self.entropy() / rate;
        Some(remaining.ceil() as usize)
    }

    /// Number of nodes in the state vector.
    pub fn dimension(&self) -> usize {
        self.psi.len()
    }

    /// Number of measurements performed so far.
    pub fn measurement_count(&self) -> u64 {
        self.measurement_count
    }

    /// Node IDs corresponding to each amplitude index.
    pub fn node_ids(&self) -> &[u64] {
        &self.node_ids
    }
}

// ---------------------------------------------------------------------------
// QuantumEvidenceRanking
// ---------------------------------------------------------------------------

/// Result of ranking a candidate evidence edge by quantum impact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantumEvidenceRanking {
    pub u_idx: usize,
    pub v_idx: usize,
    pub weight: f64,
    pub impact: f64,
    pub prob_u: f64,
    pub prob_v: f64,
    pub explanation: String,
}

// ---------------------------------------------------------------------------
// Hypothesis Superposition
// ---------------------------------------------------------------------------

/// A named hypothesis with an amplitude.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hypothesis {
    pub id: String,
    pub amplitude: f64,
    pub description: String,
    /// Edges (as `(source_id, target_id)`) that support this hypothesis.
    pub supporting_edges: Vec<(u64, u64)>,
}

/// Multiple competing hypotheses in superposition.
///
/// Each hypothesis carries an amplitude; the Born rule gives probabilities.
/// Evidence observations boost supporting hypotheses and dampen others.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HypothesisSuperposition {
    pub hypotheses: Vec<Hypothesis>,
}

impl Default for HypothesisSuperposition {
    fn default() -> Self {
        Self::new()
    }
}

impl HypothesisSuperposition {
    pub fn new() -> Self {
        Self {
            hypotheses: Vec::new(),
        }
    }

    /// Add a hypothesis. Amplitudes are redistributed uniformly.
    pub fn add_hypothesis(&mut self, id: &str, description: &str, edges: Vec<(u64, u64)>) {
        let n = self.hypotheses.len() + 1;
        let amp = 1.0 / (n as f64).sqrt();
        // Redistribute amplitudes uniformly
        for h in &mut self.hypotheses {
            h.amplitude = amp;
        }
        self.hypotheses.push(Hypothesis {
            id: id.to_string(),
            amplitude: amp,
            description: description.to_string(),
            supporting_edges: edges,
        });
    }

    /// Born rule: probability of each hypothesis.
    pub fn probabilities(&self) -> Vec<(String, f64)> {
        self.hypotheses
            .iter()
            .map(|h| (h.id.clone(), h.amplitude.powi(2)))
            .collect()
    }

    /// Update amplitudes based on evidence.
    ///
    /// Evidence that supports a hypothesis boosts its amplitude;
    /// unsupported hypotheses are dampened.
    pub fn observe(&mut self, evidence_edge: (u64, u64)) {
        for h in &mut self.hypotheses {
            if h.supporting_edges.contains(&evidence_edge) {
                h.amplitude *= 1.5; // boost
            } else {
                h.amplitude *= 0.8; // dampen
            }
        }
        self.normalize();
    }

    /// Von Neumann entropy of the hypothesis distribution.
    pub fn entropy(&self) -> f64 {
        -self
            .hypotheses
            .iter()
            .map(|h| {
                let p = h.amplitude.powi(2);
                if p > 1e-15 { p * p.ln() } else { 0.0 }
            })
            .sum::<f64>()
    }

    /// Collapse to the most probable hypothesis.
    pub fn collapse(&self) -> &Hypothesis {
        self.hypotheses
            .iter()
            .max_by(|a, b| {
                a.amplitude
                    .abs()
                    .partial_cmp(&b.amplitude.abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .expect("cannot collapse empty superposition")
    }

    fn normalize(&mut self) {
        let norm: f64 = self
            .hypotheses
            .iter()
            .map(|h| h.amplitude.powi(2))
            .sum::<f64>()
            .sqrt();
        if norm > 1e-15 {
            for h in &mut self.hypotheses {
                h.amplitude /= norm;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── Complex arithmetic ──────────────────────────────────────────

    #[test]
    fn complex_add() {
        let a = Complex::new(1.0, 2.0);
        let b = Complex::new(3.0, 4.0);
        let c = a.add(&b);
        assert!((c.re - 4.0).abs() < 1e-15);
        assert!((c.im - 6.0).abs() < 1e-15);
    }

    #[test]
    fn complex_sub() {
        let a = Complex::new(5.0, 3.0);
        let b = Complex::new(2.0, 1.0);
        let c = a.sub(&b);
        assert!((c.re - 3.0).abs() < 1e-15);
        assert!((c.im - 2.0).abs() < 1e-15);
    }

    #[test]
    fn complex_mul() {
        // (1+2i)*(3+4i) = 3+4i+6i+8i^2 = (3-8)+(4+6)i = -5+10i
        let a = Complex::new(1.0, 2.0);
        let b = Complex::new(3.0, 4.0);
        let c = a.mul(&b);
        assert!((c.re - (-5.0)).abs() < 1e-15);
        assert!((c.im - 10.0).abs() < 1e-15);
    }

    #[test]
    fn complex_conj() {
        let a = Complex::new(3.0, -4.0);
        let c = a.conj();
        assert!((c.re - 3.0).abs() < 1e-15);
        assert!((c.im - 4.0).abs() < 1e-15);
    }

    #[test]
    fn complex_norm() {
        let a = Complex::new(3.0, 4.0);
        assert!((a.norm() - 5.0).abs() < 1e-15);
        assert!((a.norm_sq() - 25.0).abs() < 1e-15);
    }

    #[test]
    fn complex_scale() {
        let a = Complex::new(2.0, 3.0);
        let c = a.scale(2.0);
        assert!((c.re - 4.0).abs() < 1e-15);
        assert!((c.im - 6.0).abs() < 1e-15);
    }

    #[test]
    fn complex_zero() {
        let z = Complex::zero();
        assert!((z.re).abs() < 1e-15);
        assert!((z.im).abs() < 1e-15);
    }

    #[test]
    fn complex_real() {
        let r = Complex::real(42.0);
        assert!((r.re - 42.0).abs() < 1e-15);
        assert!((r.im).abs() < 1e-15);
    }

    // ── from_fiedler ────────────────────────────────────────────────

    #[test]
    fn from_fiedler_normalizes() {
        let fiedler = vec![1.0, -1.0, 0.5];
        let ids = vec![10, 20, 30];
        let qs = QuantumCognitiveState::from_fiedler(&fiedler, &ids);

        // Probabilities should sum to 1
        let total: f64 = qs.probabilities().iter().sum();
        assert!(
            (total - 1.0).abs() < 1e-12,
            "probabilities should sum to 1.0, got {total}"
        );
    }

    #[test]
    fn from_fiedler_preserves_signs() {
        let fiedler = vec![1.0, -1.0, 0.5];
        let ids = vec![10, 20, 30];
        let qs = QuantumCognitiveState::from_fiedler(&fiedler, &ids);
        let classical = qs.to_classical();
        assert!(classical[0] > 0.0);
        assert!(classical[1] < 0.0);
        assert!(classical[2] > 0.0);
    }

    // ── uniform superposition ───────────────────────────────────────

    #[test]
    fn uniform_equal_probabilities() {
        let n = 4;
        let ids = vec![1, 2, 3, 4];
        let qs = QuantumCognitiveState::uniform(n, &ids);
        let probs = qs.probabilities();
        for p in &probs {
            assert!(
                (p - 0.25).abs() < 1e-12,
                "uniform: each probability should be 0.25, got {p}"
            );
        }
    }

    #[test]
    fn uniform_probabilities_sum_to_one() {
        let qs = QuantumCognitiveState::uniform(5, &[1, 2, 3, 4, 5]);
        let total: f64 = qs.probabilities().iter().sum();
        assert!(
            (total - 1.0).abs() < 1e-12,
            "uniform probabilities should sum to 1.0, got {total}"
        );
    }

    // ── entropy ─────────────────────────────────────────────────────

    #[test]
    fn entropy_uniform_is_ln_n() {
        let n = 4;
        let qs = QuantumCognitiveState::uniform(n, &[1, 2, 3, 4]);
        let expected = (n as f64).ln();
        let actual = qs.entropy();
        assert!(
            (actual - expected).abs() < 1e-12,
            "entropy of uniform should be ln({n})={expected}, got {actual}"
        );
    }

    #[test]
    fn entropy_collapsed_is_zero() {
        // State with all amplitude on one node
        let mut qs = QuantumCognitiveState::uniform(3, &[1, 2, 3]);
        qs.psi[0] = Complex::real(1.0);
        qs.psi[1] = Complex::zero();
        qs.psi[2] = Complex::zero();
        let s = qs.entropy();
        assert!(
            s.abs() < 1e-12,
            "entropy of collapsed state should be ~0, got {s}"
        );
    }

    // ── evidence_impact matches classical delta_lambda2 ─────────────

    #[test]
    fn evidence_impact_matches_classical() {
        // When the state is the normalized Fiedler vector (all real),
        // evidence_impact should equal w * (phi[u] - phi[v])^2.
        let fiedler = vec![0.5, -0.5, 0.0];
        let norm: f64 = fiedler.iter().map(|x| x * x).sum::<f64>().sqrt();
        let ids = vec![0, 1, 2];
        let qs = QuantumCognitiveState::from_fiedler(&fiedler, &ids);

        let w = 1.0;
        let quantum_impact = qs.evidence_impact(0, 1, w);
        // Classical: w * ((0.5/norm) - (-0.5/norm))^2 = w * (1.0/norm)^2
        let classical = w * ((0.5 / norm) - (-0.5 / norm)).powi(2);
        assert!(
            (quantum_impact - classical).abs() < 1e-12,
            "quantum impact {quantum_impact} should match classical {classical}"
        );
    }

    // ── observe_evidence ────────────────────────────────────────────

    #[test]
    fn observe_evidence_boosts_targets() {
        let qs_before = QuantumCognitiveState::uniform(4, &[1, 2, 3, 4]);
        let p_before = qs_before.probability(0);

        let mut qs = qs_before;
        qs.observe_evidence(0, 1, 1.0);
        let p_after = qs.probability(0);

        assert!(
            p_after > p_before,
            "observe_evidence should boost target probability: {p_before} -> {p_after}"
        );
    }

    #[test]
    fn observe_evidence_keeps_normalization() {
        let mut qs = QuantumCognitiveState::uniform(4, &[1, 2, 3, 4]);
        qs.observe_evidence(0, 1, 2.0);
        qs.observe_evidence(1, 2, 0.5);
        let total: f64 = qs.probabilities().iter().sum();
        assert!(
            (total - 1.0).abs() < 1e-12,
            "probabilities should sum to 1 after observations, got {total}"
        );
    }

    #[test]
    fn observe_evidence_increments_count() {
        let mut qs = QuantumCognitiveState::uniform(3, &[1, 2, 3]);
        assert_eq!(qs.measurement_count(), 0);
        qs.observe_evidence(0, 1, 1.0);
        assert_eq!(qs.measurement_count(), 1);
        qs.observe_evidence(1, 2, 1.0);
        assert_eq!(qs.measurement_count(), 2);
    }

    // ── collapse ────────────────────────────────────────────────────

    #[test]
    fn collapse_returns_highest_probability() {
        let mut qs = QuantumCognitiveState::uniform(3, &[1, 2, 3]);
        // Make node 1 dominant
        qs.psi[1] = Complex::real(10.0);
        qs.normalize();
        let (idx, prob) = qs.collapse();
        assert_eq!(idx, 1);
        assert!(prob > 0.9);
    }

    #[test]
    fn collapse_uniform_returns_valid_index() {
        let qs = QuantumCognitiveState::uniform(5, &[10, 20, 30, 40, 50]);
        let (idx, prob) = qs.collapse();
        assert!(idx < 5);
        assert!(prob > 0.0);
    }

    // ── to_classical ────────────────────────────────────────────────

    #[test]
    fn to_classical_returns_real_parts() {
        let fiedler = vec![1.0, -1.0, 0.5];
        let ids = vec![0, 1, 2];
        let qs = QuantumCognitiveState::from_fiedler(&fiedler, &ids);
        let classical = qs.to_classical();
        assert_eq!(classical.len(), 3);
        // All imaginary parts should be zero for a Fiedler-initialized state
        for a in &qs.psi {
            assert!(a.im.abs() < 1e-15);
        }
    }

    // ── is_collapsed ────────────────────────────────────────────────

    #[test]
    fn is_collapsed_detects_low_entropy() {
        let mut qs = QuantumCognitiveState::uniform(3, &[1, 2, 3]);
        assert!(
            !qs.is_collapsed(0.1),
            "uniform state should not be collapsed"
        );

        qs.psi[0] = Complex::real(1.0);
        qs.psi[1] = Complex::zero();
        qs.psi[2] = Complex::zero();
        assert!(
            qs.is_collapsed(0.1),
            "single-node state should be collapsed"
        );
    }

    // ── estimated_measurements_to_collapse ──────────────────────────

    #[test]
    fn estimated_measurements_none_without_history() {
        let qs = QuantumCognitiveState::uniform(3, &[1, 2, 3]);
        assert!(qs.estimated_measurements_to_collapse().is_none());
    }

    #[test]
    fn estimated_measurements_converging() {
        let mut qs = QuantumCognitiveState::uniform(4, &[1, 2, 3, 4]);
        // Simulate a series of observations that reduce entropy
        for _ in 0..5 {
            qs.observe_evidence(0, 1, 1.0);
        }
        // With enough history and converging entropy, should return Some
        if qs.entropy_history.len() >= 2 {
            let first = qs.entropy_history.first().unwrap();
            let last = qs.entropy_history.last().unwrap();
            if first > last {
                assert!(qs.estimated_measurements_to_collapse().is_some());
            }
        }
    }

    // ── rank_evidence_quantum ───────────────────────────────────────

    #[test]
    fn rank_evidence_quantum_ordered_by_impact() {
        let fiedler = vec![0.7, -0.7, 0.1];
        let ids = vec![0, 1, 2];
        let qs = QuantumCognitiveState::from_fiedler(&fiedler, &ids);

        let candidates = vec![(0, 1, 1.0), (0, 2, 1.0), (1, 2, 1.0)];
        let ranked = qs.rank_evidence_quantum(&candidates);

        // Should be sorted descending by impact
        for i in 1..ranked.len() {
            assert!(
                ranked[i - 1].impact >= ranked[i].impact,
                "rankings should be sorted descending"
            );
        }
    }

    #[test]
    fn rank_evidence_quantum_contains_probabilities() {
        let qs = QuantumCognitiveState::uniform(3, &[0, 1, 2]);
        let candidates = vec![(0, 1, 1.0)];
        let ranked = qs.rank_evidence_quantum(&candidates);
        assert_eq!(ranked.len(), 1);
        assert!(ranked[0].prob_u > 0.0);
        assert!(ranked[0].prob_v > 0.0);
    }

    // ── evolve ──────────────────────────────────────────────────────

    #[test]
    fn evolve_identity_laplacian_no_change() {
        let qs_before = QuantumCognitiveState::uniform(3, &[1, 2, 3]);
        let probs_before = qs_before.probabilities();

        let mut qs = qs_before;
        // Identity Laplacian action returns zeros (H|psi> = 0 for uniform
        // on a complete graph with Laplacian having uniform eigenvector as
        // eigenvalue 0). Simulate zero Hamiltonian.
        qs.evolve(|psi| vec![Complex::zero(); psi.len()], 1.0);
        let probs_after = qs.probabilities();

        for (pb, pa) in probs_before.iter().zip(probs_after.iter()) {
            assert!(
                (pb - pa).abs() < 1e-12,
                "zero Hamiltonian should not change probabilities"
            );
        }
    }

    #[test]
    fn evolve_preserves_normalization() {
        let mut qs = QuantumCognitiveState::from_fiedler(&[1.0, -1.0, 0.5], &[0, 1, 2]);
        // Simple diagonal Laplacian action
        qs.evolve(
            |psi| {
                psi.iter()
                    .enumerate()
                    .map(|(i, a)| a.scale(i as f64))
                    .collect()
            },
            0.1,
        );
        let total: f64 = qs.probabilities().iter().sum();
        assert!(
            (total - 1.0).abs() < 1e-10,
            "evolve should preserve normalization, got {total}"
        );
    }

    // ── suppress ────────────────────────────────────────────────────

    #[test]
    fn suppress_dampens_amplitude() {
        let mut qs = QuantumCognitiveState::uniform(3, &[1, 2, 3]);
        let p_before = qs.probability(0);
        qs.suppress(0, 0.1);
        let p_after = qs.probability(0);
        assert!(
            p_after < p_before,
            "suppress should reduce probability: {p_before} -> {p_after}"
        );
    }

    #[test]
    fn suppress_keeps_normalization() {
        let mut qs = QuantumCognitiveState::uniform(4, &[1, 2, 3, 4]);
        qs.suppress(2, 0.01);
        let total: f64 = qs.probabilities().iter().sum();
        assert!(
            (total - 1.0).abs() < 1e-12,
            "suppress should preserve normalization, got {total}"
        );
    }

    // ── Born rule: probabilities always sum to 1 ────────────────────

    #[test]
    fn born_rule_sum_after_all_operations() {
        let mut qs = QuantumCognitiveState::from_fiedler(&[1.0, -0.5, 0.3, -0.2], &[0, 1, 2, 3]);

        // Evolve
        qs.evolve(|psi| psi.iter().map(|a| a.scale(0.5)).collect(), 0.1);

        // Observe
        qs.observe_evidence(0, 1, 0.8);

        // Suppress
        qs.suppress(3, 0.2);

        // Observe again
        qs.observe_evidence(1, 2, 1.5);

        let total: f64 = qs.probabilities().iter().sum();
        assert!(
            (total - 1.0).abs() < 1e-12,
            "probabilities must sum to 1.0 after mixed operations, got {total}"
        );
    }

    // ── HypothesisSuperposition ─────────────────────────────────────

    #[test]
    fn hypothesis_add_redistributes() {
        let mut hs = HypothesisSuperposition::new();
        hs.add_hypothesis("h1", "First", vec![(0, 1)]);
        assert_eq!(hs.hypotheses.len(), 1);
        assert!((hs.hypotheses[0].amplitude - 1.0).abs() < 1e-12);

        hs.add_hypothesis("h2", "Second", vec![(1, 2)]);
        assert_eq!(hs.hypotheses.len(), 2);
        let expected = 1.0 / 2.0f64.sqrt();
        for h in &hs.hypotheses {
            assert!(
                (h.amplitude - expected).abs() < 1e-12,
                "amplitude should be 1/sqrt(2), got {}",
                h.amplitude
            );
        }
    }

    #[test]
    fn hypothesis_probabilities_sum_to_one() {
        let mut hs = HypothesisSuperposition::new();
        hs.add_hypothesis("h1", "A", vec![(0, 1)]);
        hs.add_hypothesis("h2", "B", vec![(1, 2)]);
        hs.add_hypothesis("h3", "C", vec![(2, 3)]);
        let total: f64 = hs.probabilities().iter().map(|(_, p)| p).sum();
        assert!(
            (total - 1.0).abs() < 1e-12,
            "hypothesis probabilities should sum to 1, got {total}"
        );
    }

    #[test]
    fn hypothesis_observe_boosts_supported() {
        let mut hs = HypothesisSuperposition::new();
        hs.add_hypothesis("h1", "Supported", vec![(0, 1)]);
        hs.add_hypothesis("h2", "Unsupported", vec![(2, 3)]);

        let probs_before = hs.probabilities();
        let p1_before = probs_before[0].1;

        hs.observe((0, 1)); // supports h1
        let probs_after = hs.probabilities();
        let p1_after = probs_after[0].1;

        assert!(
            p1_after > p1_before,
            "h1 probability should increase: {p1_before} -> {p1_after}"
        );
    }

    #[test]
    fn hypothesis_observe_keeps_normalization() {
        let mut hs = HypothesisSuperposition::new();
        hs.add_hypothesis("h1", "A", vec![(0, 1)]);
        hs.add_hypothesis("h2", "B", vec![(1, 2)]);
        hs.observe((0, 1));
        hs.observe((1, 2));
        hs.observe((0, 1));
        let total: f64 = hs.probabilities().iter().map(|(_, p)| p).sum();
        assert!(
            (total - 1.0).abs() < 1e-12,
            "hypothesis probabilities should sum to 1 after observations, got {total}"
        );
    }

    #[test]
    fn hypothesis_collapse_returns_most_probable() {
        let mut hs = HypothesisSuperposition::new();
        hs.add_hypothesis("h1", "Winner", vec![(0, 1)]);
        hs.add_hypothesis("h2", "Loser", vec![(2, 3)]);
        // Boost h1 several times
        hs.observe((0, 1));
        hs.observe((0, 1));
        hs.observe((0, 1));
        let winner = hs.collapse();
        assert_eq!(winner.id, "h1");
    }

    #[test]
    fn hypothesis_entropy_decreases_with_evidence() {
        let mut hs = HypothesisSuperposition::new();
        hs.add_hypothesis("h1", "A", vec![(0, 1)]);
        hs.add_hypothesis("h2", "B", vec![(2, 3)]);
        let s_before = hs.entropy();
        hs.observe((0, 1));
        let s_after = hs.entropy();
        assert!(
            s_after < s_before,
            "entropy should decrease with evidence: {s_before} -> {s_after}"
        );
    }

    #[test]
    fn hypothesis_default_is_empty() {
        let hs = HypothesisSuperposition::default();
        assert!(hs.hypotheses.is_empty());
    }
}
