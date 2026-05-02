//! Causal graph DAG with typed/weighted directed edges.
//!
//! Provides a concurrent, lock-free causal reasoning graph where nodes
//! represent events or observations and edges encode causal relationships
//! with weights and provenance metadata. Built on `DashMap` for safe
//! concurrent access from multiple agent threads.

// The later sections of this file host k-means / covariance / PCA math
// where indexed `for i in 0..n { a[i][j] = ... }` loops are more
// readable than iterator combinators. Scope the allow to the whole
// file so the math reads like math.
#![allow(clippy::needless_range_loop)]

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(feature = "exochain")]
use std::sync::Arc;

/// Numeric identifier for causal graph nodes.
///
/// This is local to the causal module and distinct from
/// [`crate::cluster::NodeId`] which is a `String`.
pub type NodeId = u64;

// ---------------------------------------------------------------------------
// CausalEdgeType
// ---------------------------------------------------------------------------

/// The kind of causal relationship an edge represents.
#[non_exhaustive]
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum CausalEdgeType {
    /// A directly causes B.
    Causes,
    /// A suppresses or prevents B.
    Inhibits,
    /// A and B are statistically correlated (non-directional semantics,
    /// but stored in the directed graph for traversal purposes).
    Correlates,
    /// A is a precondition that enables B.
    Enables,
    /// A temporally follows B.
    Follows,
    /// A provides evidence against B.
    Contradicts,
    /// Edge was created by a ClawStage trigger.
    TriggeredBy,
    /// A provides supporting evidence for B.
    EvidenceFor,
}

impl fmt::Display for CausalEdgeType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Causes => write!(f, "Causes"),
            Self::Inhibits => write!(f, "Inhibits"),
            Self::Correlates => write!(f, "Correlates"),
            Self::Enables => write!(f, "Enables"),
            Self::Follows => write!(f, "Follows"),
            Self::Contradicts => write!(f, "Contradicts"),
            Self::TriggeredBy => write!(f, "TriggeredBy"),
            Self::EvidenceFor => write!(f, "EvidenceFor"),
        }
    }
}

// ---------------------------------------------------------------------------
// CausalEdge
// ---------------------------------------------------------------------------

/// A weighted, typed directed edge between two causal graph nodes.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CausalEdge {
    /// Source node (tail of the arrow).
    pub source: NodeId,
    /// Target node (head of the arrow).
    pub target: NodeId,
    /// Semantic type of the relationship.
    pub edge_type: CausalEdgeType,
    /// Strength / confidence of the relationship (0.0 .. 1.0 typical).
    pub weight: f32,
    /// Hybrid logical clock timestamp at creation.
    pub timestamp: u64,
    /// ExoChain sequence number for provenance tracking.
    pub chain_seq: u64,
    /// Universal Node ID bytes for the source node.
    pub source_universal_id: [u8; 32],
    /// Universal Node ID bytes for the target node.
    pub target_universal_id: [u8; 32],
}

// ---------------------------------------------------------------------------
// CausalNode
// ---------------------------------------------------------------------------

/// A node in the causal graph representing an event, observation, or concept.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CausalNode {
    /// Local numeric identifier.
    pub id: NodeId,
    /// Human-readable label.
    pub label: String,
    /// HLC timestamp at creation.
    pub created_at: u64,
    /// Arbitrary JSON metadata attached to this node.
    pub metadata: serde_json::Value,
}

// ---------------------------------------------------------------------------
// CausalGraph
// ---------------------------------------------------------------------------

/// A concurrent directed acyclic graph for causal reasoning.
///
/// Internally backed by [`DashMap`] for lock-free concurrent reads and
/// fine-grained write locking. Edge lists are stored in both forward
/// (outgoing) and reverse (incoming) adjacency maps for efficient
/// bidirectional traversal.
pub struct CausalGraph {
    nodes: DashMap<NodeId, CausalNode>,
    forward_edges: DashMap<NodeId, Vec<CausalEdge>>,
    reverse_edges: DashMap<NodeId, Vec<CausalEdge>>,
    next_node_id: AtomicU64,
    node_count: AtomicU64,
    edge_count: AtomicU64,
    /// Optional chain manager for ExoChain event logging.
    #[cfg(feature = "exochain")]
    chain_manager: Option<Arc<crate::chain::ChainManager>>,
    /// Optional governance engine for gating destructive operations.
    #[cfg(feature = "exochain")]
    governance_engine: Option<Arc<crate::governance::GovernanceEngine>>,
}

impl CausalGraph {
    /// Create an empty causal graph.
    pub fn new() -> Self {
        Self {
            nodes: DashMap::new(),
            forward_edges: DashMap::new(),
            reverse_edges: DashMap::new(),
            next_node_id: AtomicU64::new(1),
            node_count: AtomicU64::new(0),
            edge_count: AtomicU64::new(0),
            #[cfg(feature = "exochain")]
            chain_manager: None,
            #[cfg(feature = "exochain")]
            governance_engine: None,
        }
    }

    /// Set the chain manager for ExoChain event logging.
    #[cfg(feature = "exochain")]
    pub fn set_chain_manager(&mut self, cm: Arc<crate::chain::ChainManager>) {
        self.chain_manager = Some(cm);
    }

    /// Set the governance engine for gating destructive operations.
    #[cfg(feature = "exochain")]
    pub fn set_governance_engine(&mut self, ge: Arc<crate::governance::GovernanceEngine>) {
        self.governance_engine = Some(ge);
    }

    /// Add a node with an auto-assigned ID.
    pub fn add_node(&self, label: String, metadata: serde_json::Value) -> NodeId {
        let id = self.next_node_id.fetch_add(1, Ordering::SeqCst);
        let node = CausalNode {
            id,
            label: label.clone(),
            created_at: 0, // caller may set via metadata; HLC not available here
            metadata,
        };
        self.nodes.insert(id, node);
        self.forward_edges.insert(id, Vec::new());
        self.reverse_edges.insert(id, Vec::new());
        self.node_count.fetch_add(1, Ordering::SeqCst);

        #[cfg(feature = "exochain")]
        if let Some(ref cm) = self.chain_manager {
            cm.append(
                "causal",
                crate::chain::EVENT_KIND_CAUSAL_NODE_ADD,
                Some(serde_json::json!({
                    "node_id": id,
                    "label": label,
                })),
            );
        }

        id
    }

    /// Retrieve a clone of the node with the given ID.
    pub fn get_node(&self, id: NodeId) -> Option<CausalNode> {
        self.nodes.get(&id).map(|r| r.value().clone())
    }

    /// Remove a node and all edges incident to it.
    ///
    /// Returns the removed node, or `None` if the ID was not found.
    pub fn remove_node(&self, id: NodeId) -> Option<CausalNode> {
        let (_, node) = self.nodes.remove(&id)?;

        // Remove forward edges from this node and update reverse adjacency.
        if let Some((_, fwd)) = self.forward_edges.remove(&id) {
            let removed = fwd.len() as u64;
            for edge in &fwd {
                if let Some(mut rev) = self.reverse_edges.get_mut(&edge.target) {
                    rev.retain(|e| e.source != id);
                }
            }
            self.edge_count.fetch_sub(removed, Ordering::SeqCst);
        }

        // Remove reverse edges to this node and update forward adjacency.
        if let Some((_, rev)) = self.reverse_edges.remove(&id) {
            let removed = rev.len() as u64;
            for edge in &rev {
                if let Some(mut fwd) = self.forward_edges.get_mut(&edge.source) {
                    fwd.retain(|e| e.target != id);
                }
            }
            self.edge_count.fetch_sub(removed, Ordering::SeqCst);
        }

        self.node_count.fetch_sub(1, Ordering::SeqCst);

        #[cfg(feature = "exochain")]
        if let Some(ref cm) = self.chain_manager {
            cm.append(
                "causal",
                crate::chain::EVENT_KIND_CAUSAL_NODE_REMOVE,
                Some(serde_json::json!({
                    "node_id": id,
                    "label": node.label,
                })),
            );
        }

        Some(node)
    }

    /// Create an edge from `source` to `target`.
    ///
    /// Returns `false` if either endpoint does not exist.
    pub fn link(
        &self,
        source: NodeId,
        target: NodeId,
        edge_type: CausalEdgeType,
        weight: f32,
        timestamp: u64,
        chain_seq: u64,
    ) -> bool {
        if !self.nodes.contains_key(&source) || !self.nodes.contains_key(&target) {
            return false;
        }

        let edge = CausalEdge {
            source,
            target,
            edge_type,
            weight,
            timestamp,
            chain_seq,
            source_universal_id: [0u8; 32],
            target_universal_id: [0u8; 32],
        };

        if let Some(mut fwd) = self.forward_edges.get_mut(&source) {
            fwd.push(edge.clone());
        }
        if let Some(mut rev) = self.reverse_edges.get_mut(&target) {
            rev.push(edge.clone());
        }

        self.edge_count.fetch_add(1, Ordering::SeqCst);

        #[cfg(feature = "exochain")]
        if let Some(ref cm) = self.chain_manager {
            cm.append(
                "causal",
                crate::chain::EVENT_KIND_CAUSAL_EDGE_ADD,
                Some(serde_json::json!({
                    "source": source,
                    "target": target,
                    "edge_type": edge.edge_type.to_string(),
                    "weight": edge.weight,
                })),
            );
        }

        true
    }

    /// Remove all edges between `source` and `target` (in that direction).
    ///
    /// Returns the number of edges removed.
    pub fn unlink(&self, source: NodeId, target: NodeId) -> usize {
        let mut count = 0usize;

        if let Some(mut fwd) = self.forward_edges.get_mut(&source) {
            let before = fwd.len();
            fwd.retain(|e| e.target != target);
            count = before - fwd.len();
        }

        if let Some(mut rev) = self.reverse_edges.get_mut(&target) {
            rev.retain(|e| e.source != source);
        }

        self.edge_count
            .fetch_sub(count as u64, Ordering::SeqCst);

        #[cfg(feature = "exochain")]
        if let Some(ref cm) = self.chain_manager {
            cm.append(
                "causal",
                crate::chain::EVENT_KIND_CAUSAL_EDGE_REMOVE,
                Some(serde_json::json!({
                    "source": source,
                    "target": target,
                    "removed_count": count,
                })),
            );
        }

        count
    }

    /// Return all edges originating from `id`.
    pub fn get_forward_edges(&self, id: NodeId) -> Vec<CausalEdge> {
        self.forward_edges
            .get(&id)
            .map(|r| r.value().clone())
            .unwrap_or_default()
    }

    /// Return all edges targeting `id`.
    pub fn get_reverse_edges(&self, id: NodeId) -> Vec<CausalEdge> {
        self.reverse_edges
            .get(&id)
            .map(|r| r.value().clone())
            .unwrap_or_default()
    }

    /// Return forward edges from `id` that match `edge_type`.
    pub fn get_edges_by_type(&self, id: NodeId, edge_type: &CausalEdgeType) -> Vec<CausalEdge> {
        self.forward_edges
            .get(&id)
            .map(|r| {
                r.value()
                    .iter()
                    .filter(|e| &e.edge_type == edge_type)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Number of nodes currently in the graph.
    pub fn node_count(&self) -> u64 {
        self.node_count.load(Ordering::SeqCst)
    }

    /// Number of edges currently in the graph.
    pub fn edge_count(&self) -> u64 {
        self.edge_count.load(Ordering::SeqCst)
    }

    /// Remove all nodes and edges (used during calibration cleanup).
    ///
    /// When the `exochain` feature is enabled and a governance engine is
    /// configured, this operation is gated: it will return
    /// [`KernelError::GovernanceDenied`] if the governance check fails.
    pub fn clear(&self) -> Result<(), crate::error::KernelError> {
        // Governance gate: destructive wipe requires approval.
        #[cfg(feature = "exochain")]
        if let Some(ref ge) = self.governance_engine {
            let request = crate::governance::GovernanceRequest::new("causal", "causal.clear")
                .with_effect(crate::governance::EffectVector {
                    risk: 0.8,
                    ..Default::default()
                });
            let result = ge.evaluate(&request);
            if matches!(
                result.decision,
                crate::governance::GovernanceDecision::Deny(_)
                    | crate::governance::GovernanceDecision::EscalateToHuman(_)
            ) {
                return Err(crate::error::KernelError::GovernanceDenied(
                    format!("causal.clear denied: {}", result.decision),
                ));
            }
        }

        let prev_nodes = self.node_count.load(Ordering::SeqCst);
        let prev_edges = self.edge_count.load(Ordering::SeqCst);

        self.nodes.clear();
        self.forward_edges.clear();
        self.reverse_edges.clear();
        self.node_count.store(0, Ordering::SeqCst);
        self.edge_count.store(0, Ordering::SeqCst);
        // Note: next_node_id is intentionally NOT reset so IDs remain unique
        // across clear cycles.

        #[cfg(feature = "exochain")]
        if let Some(ref cm) = self.chain_manager {
            cm.append(
                "causal",
                crate::chain::EVENT_KIND_CAUSAL_CLEAR,
                Some(serde_json::json!({
                    "node_count": prev_nodes,
                    "edge_count": prev_edges,
                })),
            );
        }

        Ok(())
    }

    /// BFS traversal forward from `start` up to `depth` hops.
    ///
    /// Returns all discovered node IDs (excluding `start`).
    pub fn traverse_forward(&self, start: NodeId, depth: usize) -> Vec<NodeId> {
        self.bfs(start, depth, true)
    }

    /// BFS traversal backward (following reverse edges) from `start`
    /// up to `depth` hops.
    ///
    /// Returns all discovered node IDs (excluding `start`).
    pub fn traverse_reverse(&self, start: NodeId, depth: usize) -> Vec<NodeId> {
        self.bfs(start, depth, false)
    }

    /// Find the shortest path from `from` to `to` using BFS, limited
    /// to `max_depth` hops.
    ///
    /// Returns the node sequence including both endpoints, or `None`
    /// if no path exists within the depth limit.
    pub fn find_path(&self, from: NodeId, to: NodeId, max_depth: usize) -> Option<Vec<NodeId>> {
        if from == to {
            return Some(vec![from]);
        }
        if !self.nodes.contains_key(&from) || !self.nodes.contains_key(&to) {
            return None;
        }

        // BFS with parent tracking.
        let mut visited: std::collections::HashSet<NodeId> = std::collections::HashSet::new();
        let mut parent: std::collections::HashMap<NodeId, NodeId> =
            std::collections::HashMap::new();
        let mut queue: VecDeque<(NodeId, usize)> = VecDeque::new();

        visited.insert(from);
        queue.push_back((from, 0));

        while let Some((current, d)) = queue.pop_front() {
            if d >= max_depth {
                continue;
            }
            let edges = self.get_forward_edges(current);
            for edge in edges {
                if visited.contains(&edge.target) {
                    continue;
                }
                visited.insert(edge.target);
                parent.insert(edge.target, current);

                if edge.target == to {
                    // Reconstruct path.
                    let mut path = Vec::new();
                    let mut cur = to;
                    while cur != from {
                        path.push(cur);
                        cur = parent[&cur];
                    }
                    path.push(from);
                    path.reverse();
                    return Some(path);
                }

                queue.push_back((edge.target, d + 1));
            }
        }

        None
    }

    /// Trace a typed causal chain from `from` to `to`.
    ///
    /// Uses BFS over forward edges (like [`find_path`]) but records the
    /// edge type at each step and builds a human-readable explanation
    /// string (e.g., "A --Causes--> B --Enables--> C --EvidenceFor--> D").
    ///
    /// Returns `None` if no path exists within `max_depth` hops, or if
    /// either endpoint is missing from the graph.
    pub fn trace_causal_chain(
        &self,
        from: NodeId,
        to: NodeId,
        max_depth: usize,
    ) -> Option<CausalChain> {
        if from == to {
            return Some(CausalChain {
                path: Vec::new(),
                explanation: self
                    .get_node(from)
                    .map(|n| n.label.clone())
                    .unwrap_or_else(|| format!("node:{from}")),
                total_weight: 0.0,
            });
        }
        if !self.nodes.contains_key(&from) || !self.nodes.contains_key(&to) {
            return None;
        }

        // BFS with parent tracking that records the edge used.
        let mut visited: std::collections::HashSet<NodeId> = std::collections::HashSet::new();
        // parent[node] = (previous_node, edge_type, edge_weight)
        let mut parent: std::collections::HashMap<NodeId, (NodeId, CausalEdgeType, f32)> =
            std::collections::HashMap::new();
        let mut queue: VecDeque<(NodeId, usize)> = VecDeque::new();

        visited.insert(from);
        queue.push_back((from, 0));

        while let Some((current, d)) = queue.pop_front() {
            if d >= max_depth {
                continue;
            }
            let edges = self.get_forward_edges(current);
            for edge in edges {
                if visited.contains(&edge.target) {
                    continue;
                }
                visited.insert(edge.target);
                parent.insert(
                    edge.target,
                    (current, edge.edge_type.clone(), edge.weight),
                );

                if edge.target == to {
                    // Reconstruct the chain.
                    let mut steps: Vec<(NodeId, CausalEdgeType, NodeId)> = Vec::new();
                    let mut total_weight: f32 = 0.0;
                    let mut cur = to;
                    while cur != from {
                        let (prev, etype, w) = parent[&cur].clone();
                        steps.push((prev, etype, cur));
                        total_weight += w;
                        cur = prev;
                    }
                    steps.reverse();

                    // Build explanation string.
                    let label_for = |id: NodeId| -> String {
                        self.get_node(id)
                            .map(|n| n.label.clone())
                            .unwrap_or_else(|| format!("node:{id}"))
                    };

                    let mut explanation = String::new();
                    for (i, (src, etype, tgt)) in steps.iter().enumerate() {
                        if i == 0 {
                            explanation.push_str(&label_for(*src));
                        }
                        explanation.push_str(&format!(" --{}--> {}", etype, label_for(*tgt)));
                    }

                    return Some(CausalChain {
                        path: steps,
                        explanation,
                        total_weight,
                    });
                }

                queue.push_back((edge.target, d + 1));
            }
        }

        None
    }

    // -- private helpers --

    fn bfs(&self, start: NodeId, depth: usize, forward: bool) -> Vec<NodeId> {
        let mut visited: std::collections::HashSet<NodeId> = std::collections::HashSet::new();
        let mut result = Vec::new();
        let mut queue: VecDeque<(NodeId, usize)> = VecDeque::new();

        visited.insert(start);
        queue.push_back((start, 0));

        while let Some((current, d)) = queue.pop_front() {
            if d >= depth {
                continue;
            }
            let edges = if forward {
                self.get_forward_edges(current)
            } else {
                self.get_reverse_edges(current)
            };
            for edge in edges {
                let neighbor = if forward { edge.target } else { edge.source };
                if visited.contains(&neighbor) {
                    continue;
                }
                visited.insert(neighbor);
                result.push(neighbor);
                queue.push_back((neighbor, d + 1));
            }
        }

        result
    }

    /// List all node IDs currently in the graph.
    pub fn node_ids(&self) -> Vec<NodeId> {
        self.nodes.iter().map(|r| *r.key()).collect()
    }

    /// Degree of a node (in + out edges, treating graph as undirected).
    pub fn degree(&self, id: NodeId) -> usize {
        let fwd = self.forward_edges.get(&id).map_or(0, |e| e.len());
        let rev = self.reverse_edges.get(&id).map_or(0, |e| e.len());
        fwd + rev
    }

    /// In-degree (number of incoming edges).
    pub fn in_degree(&self, id: NodeId) -> usize {
        self.reverse_edges.get(&id).map_or(0, |e| e.len())
    }

    /// Out-degree (number of outgoing edges).
    pub fn out_degree(&self, id: NodeId) -> usize {
        self.forward_edges.get(&id).map_or(0, |e| e.len())
    }

    // -----------------------------------------------------------------------
    // Connected Components (undirected)
    // -----------------------------------------------------------------------

    /// Find connected components treating the graph as undirected.
    ///
    /// Returns a vec of components, each component being a vec of node IDs.
    /// Components are sorted largest-first.
    pub fn connected_components(&self) -> Vec<Vec<NodeId>> {
        let ids = self.node_ids();
        let mut visited: std::collections::HashSet<NodeId> = std::collections::HashSet::new();
        let mut components = Vec::new();

        for &id in &ids {
            if visited.contains(&id) {
                continue;
            }
            // BFS over both directions (undirected).
            let mut component = Vec::new();
            let mut queue: VecDeque<NodeId> = VecDeque::new();
            visited.insert(id);
            queue.push_back(id);

            while let Some(current) = queue.pop_front() {
                component.push(current);
                // Forward neighbors.
                for edge in self.get_forward_edges(current) {
                    if visited.insert(edge.target) {
                        queue.push_back(edge.target);
                    }
                }
                // Reverse neighbors.
                for edge in self.get_reverse_edges(current) {
                    if visited.insert(edge.source) {
                        queue.push_back(edge.source);
                    }
                }
            }
            component.sort();
            components.push(component);
        }

        components.sort_by_key(|b| std::cmp::Reverse(b.len()));
        components
    }

    // -----------------------------------------------------------------------
    // Community Detection (Label Propagation)
    // -----------------------------------------------------------------------

    /// Detect communities using label propagation on the undirected graph.
    ///
    /// Each node starts with its own label. In each iteration, every node
    /// adopts the most frequent label among its neighbors (weighted by edge
    /// weight). Converges when no labels change, or after `max_iterations`.
    ///
    /// Returns a map from community label (a NodeId) to the set of node IDs
    /// in that community.
    pub fn detect_communities(&self, max_iterations: usize) -> Vec<Vec<NodeId>> {
        let ids = self.node_ids();
        if ids.is_empty() {
            return Vec::new();
        }

        // Initialize: each node gets its own ID as label.
        let mut labels: std::collections::HashMap<NodeId, NodeId> = std::collections::HashMap::new();
        for &id in &ids {
            labels.insert(id, id);
        }

        for _iter in 0..max_iterations {
            let mut changed = false;

            // Process nodes in a deterministic order.
            let mut process_order = ids.clone();
            process_order.sort();

            for &id in &process_order {
                // Gather neighbor labels weighted by edge weight.
                let mut label_weights: std::collections::HashMap<NodeId, f32> =
                    std::collections::HashMap::new();

                for edge in self.get_forward_edges(id) {
                    if let Some(&lbl) = labels.get(&edge.target) {
                        *label_weights.entry(lbl).or_insert(0.0) += edge.weight;
                    }
                }
                for edge in self.get_reverse_edges(id) {
                    if let Some(&lbl) = labels.get(&edge.source) {
                        *label_weights.entry(lbl).or_insert(0.0) += edge.weight;
                    }
                }

                if label_weights.is_empty() {
                    continue; // isolated node keeps its label
                }

                // Pick the label with the highest total weight.
                // On ties, pick the smallest label for determinism.
                let best_label = label_weights
                    .iter()
                    .max_by(|a, b| {
                        a.1.partial_cmp(b.1)
                            .unwrap_or(std::cmp::Ordering::Equal)
                            .then_with(|| b.0.cmp(a.0)) // smaller ID wins ties
                    })
                    .map(|(&lbl, _)| lbl)
                    .unwrap();

                if labels[&id] != best_label {
                    labels.insert(id, best_label);
                    changed = true;
                }
            }

            if !changed {
                break;
            }
        }

        // Group nodes by label.
        let mut communities: std::collections::HashMap<NodeId, Vec<NodeId>> =
            std::collections::HashMap::new();
        for (&node, &label) in &labels {
            communities.entry(label).or_default().push(node);
        }

        let mut result: Vec<Vec<NodeId>> = communities.into_values().collect();
        for community in &mut result {
            community.sort();
        }
        result.sort_by_key(|b| std::cmp::Reverse(b.len()));
        result
    }

    // -----------------------------------------------------------------------
    // Spectral Analysis
    // -----------------------------------------------------------------------

    /// Compute the algebraic connectivity (lambda_2) of the graph.
    ///
    /// Lambda_2 is the second-smallest eigenvalue of the graph Laplacian.
    /// - lambda_2 = 0 means the graph is disconnected.
    /// - Higher values indicate stronger connectivity.
    ///
    /// Uses sparse Lanczos iteration at O(k*m) where m = number of edges
    /// and k = `max_iterations`. For typical ECC graphs with average degree
    /// ~10, this is ~200x faster than the dense O(k*n^2) approach.
    ///
    /// Returns `(lambda_2, fiedler_vector)` where the Fiedler vector can be
    /// used for spectral partitioning (sign of each component indicates which
    /// partition the node belongs to).
    pub fn spectral_analysis(&self, max_iterations: usize) -> SpectralResult {
        let ids = self.node_ids();
        let n = ids.len();

        if n < 2 {
            return SpectralResult {
                lambda_2: 0.0,
                fiedler_vector: Vec::new(),
                node_ids: ids,
            };
        }

        // Build index map: NodeId -> matrix index.
        let mut id_to_idx: std::collections::HashMap<NodeId, usize> =
            std::collections::HashMap::new();
        let mut sorted_ids = ids.clone();
        sorted_ids.sort();
        for (i, &id) in sorted_ids.iter().enumerate() {
            id_to_idx.insert(id, i);
        }

        // Build sparse adjacency: adj[i] = Vec<(j, weight)> (symmetric).
        // Also accumulate degrees.  O(m) space instead of O(n^2).
        let mut adj: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n];
        let mut degree: Vec<f64> = vec![0.0; n];

        for &id in &sorted_ids {
            let i = id_to_idx[&id];

            // Forward edges.
            for edge in self.get_forward_edges(id) {
                if let Some(&j) = id_to_idx.get(&edge.target)
                    && i != j {
                        let w = edge.weight as f64;
                        adj[i].push((j, w));
                        adj[j].push((i, w));
                        degree[i] += w;
                        degree[j] += w;
                    }
            }
            // Reverse edges — only upper triangle to avoid double-counting.
            for edge in self.get_reverse_edges(id) {
                if let Some(&j) = id_to_idx.get(&edge.source)
                    && i != j && j > i {
                        let w = edge.weight as f64;
                        adj[i].push((j, w));
                        adj[j].push((i, w));
                        degree[i] += w;
                        degree[j] += w;
                    }
            }
        }

        // Fix up degree: recompute from adjacency for correctness (handles
        // any double-adds from symmetric storage).
        for i in 0..n {
            let mut d = 0.0f64;
            for &(_, w) in &adj[i] {
                d += w;
            }
            degree[i] = d;
        }

        // Sparse Laplacian mat-vec: result = L * x = D*x - A*x
        let laplacian_mul = |x: &[f64], out: &mut [f64]| {
            for i in 0..n {
                let mut sum = degree[i] * x[i]; // D*x
                for &(j, w) in &adj[i] {
                    sum -= w * x[j]; // -A*x
                }
                out[i] = sum;
            }
        };

        // ── Lanczos iteration ──────────────────────────────────────────
        // Builds a k x k tridiagonal matrix T whose eigenvalues approximate
        // those of L restricted to the subspace orthogonal to the constant
        // (null-space) vector.  We then extract lambda_2 from T.
        //
        // The Fiedler vector is recovered by mapping the corresponding
        // eigenvector of T back through the Lanczos basis.

        let inv_sqrt_n = 1.0 / (n as f64).sqrt();

        // Initial vector: deterministic, orthogonal to the constant vector.
        let mut q: Vec<f64> = (0..n)
            .map(|i| (i as f64) - (n as f64 - 1.0) / 2.0)
            .collect();

        // Project out the constant (null-space) direction.
        let dot_ones: f64 = q.iter().sum::<f64>() * inv_sqrt_n;
        for qi in q.iter_mut() {
            *qi -= dot_ones * inv_sqrt_n;
        }
        normalize_vec(&mut q);

        let k = max_iterations.min(n - 1); // can't exceed n-1 Lanczos steps
        let mut alpha: Vec<f64> = Vec::with_capacity(k); // diagonal of T
        let mut beta: Vec<f64> = Vec::with_capacity(k);  // sub-diagonal of T
        let mut basis: Vec<Vec<f64>> = Vec::with_capacity(k); // Lanczos vectors

        let mut q_prev: Vec<f64> = vec![0.0; n];
        let mut w_buf: Vec<f64> = vec![0.0; n];

        for j in 0..k {
            basis.push(q.clone());

            // w = L * q_j
            laplacian_mul(&q, &mut w_buf);

            // alpha_j = q_j^T * w
            let aj: f64 = q.iter().zip(w_buf.iter()).map(|(a, b)| a * b).sum();
            alpha.push(aj);

            // w = w - alpha_j * q_j - beta_{j-1} * q_{j-1}
            let bj_prev = if j > 0 { beta[j - 1] } else { 0.0 };
            for i in 0..n {
                w_buf[i] -= aj * q[i] + bj_prev * q_prev[i];
            }

            // Re-orthogonalize against all previous basis vectors AND the
            // constant vector (full reorth for numerical stability).
            let dot_c: f64 = w_buf.iter().sum::<f64>() * inv_sqrt_n;
            for wi in w_buf.iter_mut() {
                *wi -= dot_c * inv_sqrt_n;
            }
            for prev in &basis {
                let dot: f64 = w_buf.iter().zip(prev.iter()).map(|(a, b)| a * b).sum();
                for i in 0..n {
                    w_buf[i] -= dot * prev[i];
                }
            }

            let bj: f64 = w_buf.iter().map(|x| x * x).sum::<f64>().sqrt();
            beta.push(bj);

            if bj < 1e-12 {
                // Invariant subspace found; stop early.
                break;
            }

            // q_{j+1} = w / beta_j
            q_prev = q.clone();
            q = w_buf.iter().map(|&x| x / bj).collect();
        }

        // ── Extract eigenvalues from the tridiagonal matrix T ──────────
        // Use the implicit-shift QR algorithm on the symmetric tridiagonal
        // matrix (alpha, beta).  We only need the smallest eigenvalue of T
        // (which approximates lambda_2 since we projected out the null space).
        let m = alpha.len();
        let (evals, evecs) = tridiag_eigen(&alpha, &beta[..m.saturating_sub(1).max(0).min(beta.len())], m);

        // Find the smallest eigenvalue (approximation to lambda_2).
        let mut min_idx = 0;
        let mut min_val = f64::MAX;
        for (i, &ev) in evals.iter().enumerate() {
            if ev < min_val {
                min_val = ev;
                min_idx = i;
            }
        }
        let lambda_2 = min_val.max(0.0);

        // Recover the Fiedler vector: v = Q * s, where Q is the n x m Lanczos
        // basis and s is the eigenvector of T corresponding to lambda_2.
        let s = &evecs[min_idx];
        let mut fiedler = vec![0.0f64; n];
        for (j, bvec) in basis.iter().enumerate() {
            if j < s.len() {
                let sj = s[j];
                for i in 0..n {
                    fiedler[i] += sj * bvec[i];
                }
            }
        }
        normalize_vec(&mut fiedler);

        SpectralResult {
            lambda_2,
            fiedler_vector: fiedler,
            node_ids: sorted_ids,
        }
    }

    /// Approximate spectral analysis using Random Fourier Features.
    ///
    /// O(m) per feature vector -- 3-6x faster than Lanczos on large graphs
    /// (>10K nodes) with approximately 5% accuracy loss. Uses random
    /// projections to estimate the graph Laplacian's eigenvalues without
    /// building an explicit Krylov subspace.
    ///
    /// # Algorithm
    ///
    /// 1. Generate `num_features` random Gaussian vectors.
    /// 2. For each random vector z, compute L*z (one sparse mat-vec, O(m)).
    /// 3. Build a `num_features x num_features` covariance matrix C where
    ///    C[i][j] = z_i^T * L * z_j.
    /// 4. Eigendecompose C (small dense matrix) to get approximate
    ///    eigenvalues of L.
    /// 5. Return the second-smallest eigenvalue as lambda_2.
    ///
    /// # Arguments
    ///
    /// - `num_features`: Number of random feature vectors (higher = more
    ///   accurate, but slower). 64-128 is typical.
    /// - `max_iter`: Maximum Jacobi iterations for the small dense eigensolve
    ///   (passed through to `tridiag_eigen`-style solver). Typically 200.
    pub fn spectral_analysis_rff(&self, num_features: usize, max_iter: usize) -> SpectralResult {
        let ids = self.node_ids();
        let n = ids.len();

        if n < 2 {
            return SpectralResult {
                lambda_2: 0.0,
                fiedler_vector: Vec::new(),
                node_ids: ids,
            };
        }

        // Build index map.
        let mut id_to_idx: std::collections::HashMap<NodeId, usize> =
            std::collections::HashMap::new();
        let mut sorted_ids = ids.clone();
        sorted_ids.sort();
        for (i, &id) in sorted_ids.iter().enumerate() {
            id_to_idx.insert(id, i);
        }

        // Build sparse adjacency + degrees (same as spectral_analysis).
        let mut adj: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n];
        let mut degree: Vec<f64> = vec![0.0; n];

        for &id in &sorted_ids {
            let i = id_to_idx[&id];
            for edge in self.get_forward_edges(id) {
                if let Some(&j) = id_to_idx.get(&edge.target)
                    && i != j {
                        let w = edge.weight as f64;
                        adj[i].push((j, w));
                        adj[j].push((i, w));
                        degree[i] += w;
                        degree[j] += w;
                    }
            }
            for edge in self.get_reverse_edges(id) {
                if let Some(&j) = id_to_idx.get(&edge.source)
                    && i != j && j > i {
                        let w = edge.weight as f64;
                        adj[i].push((j, w));
                        adj[j].push((i, w));
                        degree[i] += w;
                        degree[j] += w;
                    }
            }
        }
        // Recompute degree from adjacency for correctness.
        for i in 0..n {
            degree[i] = adj[i].iter().map(|&(_, w)| w).sum();
        }

        // Sparse Laplacian mat-vec.
        let laplacian_mul = |x: &[f64], out: &mut [f64]| {
            for i in 0..n {
                let mut sum = degree[i] * x[i];
                for &(j, w) in &adj[i] {
                    sum -= w * x[j];
                }
                out[i] = sum;
            }
        };

        let m = num_features.min(n - 1);
        let inv_sqrt_n = 1.0 / (n as f64).sqrt();

        // Generate deterministic pseudo-random vectors using a simple LCG.
        // We avoid pulling in rand to keep dependencies minimal.
        let mut seed: u64 = 0xDEAD_BEEF_CAFE_BABEu64;
        let next_gaussian = |seed: &mut u64| -> f64 {
            // Box-Muller from two uniform values via LCG.
            let uniform = |s: &mut u64| -> f64 {
                *s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                (*s >> 11) as f64 / (1u64 << 53) as f64
            };
            let u1 = uniform(seed).max(1e-15);
            let u2 = uniform(seed);
            (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
        };

        // Step 1: Generate random vectors, project out constant vector, apply L.
        let mut z_vecs: Vec<Vec<f64>> = Vec::with_capacity(m);
        let mut lz_vecs: Vec<Vec<f64>> = Vec::with_capacity(m);

        for _ in 0..m {
            let mut z: Vec<f64> = (0..n).map(|_| next_gaussian(&mut seed)).collect();
            // Project out the constant (null-space) direction.
            let dot_ones: f64 = z.iter().sum::<f64>() * inv_sqrt_n;
            for zi in z.iter_mut() {
                *zi -= dot_ones * inv_sqrt_n;
            }
            normalize_vec(&mut z);

            let mut lz = vec![0.0; n];
            laplacian_mul(&z, &mut lz);

            z_vecs.push(z);
            lz_vecs.push(lz);
        }

        // Step 2: Build the m x m covariance matrix C[i][j] = z_i^T * L * z_j.
        let mut c_mat = vec![vec![0.0f64; m]; m];
        for i in 0..m {
            for j in i..m {
                let dot: f64 = z_vecs[i].iter().zip(lz_vecs[j].iter()).map(|(a, b)| a * b).sum();
                c_mat[i][j] = dot;
                c_mat[j][i] = dot;
            }
        }

        // Step 3: Eigendecompose the small m x m matrix.
        let _diag: Vec<f64> = (0..m).map(|i| c_mat[i][i]).collect();
        let _off: Vec<f64> = if m > 1 {
            (0..m - 1).map(|i| c_mat[i][i + 1]).collect()
        } else {
            Vec::new()
        };

        // Use the Jacobi solver on the full matrix (not just tridiagonal).
        let (evals, evecs) = dense_jacobi_eigen(&c_mat, m, max_iter);

        // Step 4: Sort eigenvalues and find lambda_2 (second smallest).
        let mut eval_indices: Vec<(usize, f64)> = evals.iter().enumerate().map(|(i, &v)| (i, v)).collect();
        eval_indices.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        // The smallest eigenvalue should be ~0 (constant vector projected out).
        // lambda_2 is the second one.
        let (lambda2_idx, lambda_2) = if eval_indices.len() >= 2 {
            (eval_indices[1].0, eval_indices[1].1.max(0.0))
        } else {
            (eval_indices[0].0, eval_indices[0].1.max(0.0))
        };

        // Step 5: Recover approximate Fiedler vector.
        // v_approx = sum_j s[j] * z_j where s is the eigenvector of C for lambda_2.
        let s = &evecs[lambda2_idx];
        let mut fiedler = vec![0.0f64; n];
        for (j, z) in z_vecs.iter().enumerate() {
            if j < s.len() {
                let sj = s[j];
                for i in 0..n {
                    fiedler[i] += sj * z[i];
                }
            }
        }
        normalize_vec(&mut fiedler);

        SpectralResult {
            lambda_2,
            fiedler_vector: fiedler,
            node_ids: sorted_ids,
        }
    }

    /// Partition the graph into two halves using the Fiedler vector.
    ///
    /// Nodes with positive Fiedler vector components go to partition A,
    /// negative to partition B. This is spectral bisection — the
    /// minimum-cut balanced partition.
    pub fn spectral_partition(&self) -> (Vec<NodeId>, Vec<NodeId>) {
        let result = self.spectral_analysis(50);
        let mut a = Vec::new();
        let mut b = Vec::new();

        for (i, &id) in result.node_ids.iter().enumerate() {
            if i < result.fiedler_vector.len() && result.fiedler_vector[i] >= 0.0 {
                a.push(id);
            } else {
                b.push(id);
            }
        }
        (a, b)
    }

    // -----------------------------------------------------------------------
    // Predictive Analysis
    // -----------------------------------------------------------------------

    /// Compute co-modification coupling between nodes based on temporal
    /// co-occurrence patterns.
    ///
    /// Given a list of "change events" (each event is a set of node IDs that
    /// changed together, plus a timestamp), computes a coupling score for
    /// every pair of nodes that have been modified together.
    ///
    /// The coupling score for nodes (A, B) is:
    ///   coupling = co_changes(A,B) / max(changes(A), changes(B))
    ///
    /// Returns pairs sorted by coupling score descending.
    pub fn compute_coupling(
        &self,
        change_events: &[ChangeEvent],
    ) -> Vec<CouplingPair> {
        let mut change_count: std::collections::HashMap<NodeId, usize> =
            std::collections::HashMap::new();
        let mut co_change_count: std::collections::HashMap<(NodeId, NodeId), usize> =
            std::collections::HashMap::new();

        for event in change_events {
            let mut nodes: Vec<NodeId> = event.node_ids.clone();
            nodes.sort();
            nodes.dedup();

            for &id in &nodes {
                *change_count.entry(id).or_insert(0) += 1;
            }
            // Count co-occurrences.
            for i in 0..nodes.len() {
                for j in (i + 1)..nodes.len() {
                    let key = (nodes[i], nodes[j]);
                    *co_change_count.entry(key).or_insert(0) += 1;
                }
            }
        }

        let mut pairs: Vec<CouplingPair> = co_change_count
            .iter()
            .map(|(&(a, b), &co)| {
                let max_changes = change_count
                    .get(&a)
                    .copied()
                    .unwrap_or(1)
                    .max(change_count.get(&b).copied().unwrap_or(1));
                CouplingPair {
                    node_a: a,
                    node_b: b,
                    co_changes: co,
                    coupling_score: co as f64 / max_changes as f64,
                }
            })
            .collect();

        pairs.sort_by(|a, b| {
            b.coupling_score
                .partial_cmp(&a.coupling_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        pairs
    }

    /// Detect burst patterns in change events and predict which nodes are
    /// likely to change next.
    ///
    /// A "burst" is a period where a node has significantly more changes
    /// than its baseline rate. Nodes currently in a burst, or recently
    /// co-modified with nodes in a burst, are predicted to change next.
    ///
    /// `window_size` is the number of recent events to consider for the
    /// burst window. `baseline_factor` is the multiplier above which a
    /// node's activity is considered a burst (e.g., 2.0 = 2x baseline).
    ///
    /// Returns nodes sorted by prediction confidence (descending).
    pub fn predict_changes(
        &self,
        change_events: &[ChangeEvent],
        window_size: usize,
        baseline_factor: f64,
    ) -> Vec<ChangePrediction> {
        if change_events.is_empty() {
            return Vec::new();
        }

        // Sort events by timestamp.
        let mut sorted_events = change_events.to_vec();
        sorted_events.sort_by_key(|e| e.timestamp);

        let total = sorted_events.len();
        let window_start = total.saturating_sub(window_size);

        // Compute baseline rate (changes per event across all history).
        let mut total_counts: std::collections::HashMap<NodeId, usize> =
            std::collections::HashMap::new();
        for event in &sorted_events {
            for &id in &event.node_ids {
                *total_counts.entry(id).or_insert(0) += 1;
            }
        }

        // Compute window rate.
        let mut window_counts: std::collections::HashMap<NodeId, usize> =
            std::collections::HashMap::new();
        for event in &sorted_events[window_start..] {
            for &id in &event.node_ids {
                *window_counts.entry(id).or_insert(0) += 1;
            }
        }

        let window_len = total - window_start;

        // Identify nodes in burst.
        let mut burst_nodes: Vec<(NodeId, f64)> = Vec::new();
        for (&id, &window_count) in &window_counts {
            let total_count = total_counts.get(&id).copied().unwrap_or(0);
            let baseline_rate = total_count as f64 / total as f64;
            let window_rate = window_count as f64 / window_len as f64;

            if baseline_rate > 0.0 && window_rate / baseline_rate >= baseline_factor {
                burst_nodes.push((id, window_rate / baseline_rate));
            }
        }

        // Compute coupling to identify co-modification partners.
        let coupling = self.compute_coupling(change_events);
        let coupling_map: std::collections::HashMap<(NodeId, NodeId), f64> = coupling
            .iter()
            .map(|p| ((p.node_a, p.node_b), p.coupling_score))
            .collect();

        // Score all nodes.
        let mut predictions: std::collections::HashMap<NodeId, f64> =
            std::collections::HashMap::new();

        // Burst nodes get high base confidence.
        for &(id, burst_ratio) in &burst_nodes {
            *predictions.entry(id).or_insert(0.0) += burst_ratio * 0.6;
        }

        // Coupled partners of burst nodes get transitive confidence.
        for &(burst_id, burst_ratio) in &burst_nodes {
            for (&(a, b), &coupling_score) in &coupling_map {
                let partner = if a == burst_id {
                    Some(b)
                } else if b == burst_id {
                    Some(a)
                } else {
                    None
                };
                if let Some(partner_id) = partner {
                    *predictions.entry(partner_id).or_insert(0.0) +=
                        burst_ratio * coupling_score * 0.4;
                }
            }
        }

        // Recent activity boost: nodes that appeared in the last few events.
        let recency_window = (window_size / 3).max(1);
        let recency_start = total.saturating_sub(recency_window);
        for event in &sorted_events[recency_start..] {
            for &id in &event.node_ids {
                *predictions.entry(id).or_insert(0.0) += 0.1;
            }
        }

        let mut result: Vec<ChangePrediction> = predictions
            .into_iter()
            .map(|(id, confidence)| {
                let label = self
                    .get_node(id)
                    .map(|n| n.label.clone())
                    .unwrap_or_else(|| format!("node:{id}"));
                let in_burst = burst_nodes.iter().any(|&(bid, _)| bid == id);
                ChangePrediction {
                    node_id: id,
                    label,
                    confidence: confidence.min(1.0),
                    in_burst,
                    recent_changes: window_counts.get(&id).copied().unwrap_or(0),
                }
            })
            .collect();

        result.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        result
    }
}

// ---------------------------------------------------------------------------
// Supporting types
// ---------------------------------------------------------------------------

/// A traced causal chain from source to target through the graph.
///
/// Each step records the source node, the typed relationship, and the
/// target node.  The `explanation` field provides a human-readable
/// narrative like "A --Causes--> B --Enables--> C".
#[derive(Debug, Clone)]
pub struct CausalChain {
    /// Sequence of (source, edge_type, target) steps in the chain.
    pub path: Vec<(NodeId, CausalEdgeType, NodeId)>,
    /// Human-readable narrative of the chain.
    pub explanation: String,
    /// Sum of edge weights along the chain.
    pub total_weight: f32,
}

/// Result of spectral analysis on the causal graph.
#[derive(Debug, Clone)]
pub struct SpectralResult {
    /// Algebraic connectivity (second-smallest Laplacian eigenvalue).
    /// 0.0 means disconnected. Higher = more connected.
    pub lambda_2: f64,
    /// Fiedler vector — sign indicates spectral partition membership.
    pub fiedler_vector: Vec<f64>,
    /// Node IDs in the same order as the Fiedler vector.
    pub node_ids: Vec<NodeId>,
}

/// A temporal change event: a set of nodes that changed together.
#[derive(Debug, Clone)]
pub struct ChangeEvent {
    /// Nodes that changed in this event (e.g., modules modified in a commit).
    pub node_ids: Vec<NodeId>,
    /// Timestamp of the event.
    pub timestamp: u64,
}

/// Coupling between two nodes based on co-modification frequency.
#[derive(Debug, Clone)]
pub struct CouplingPair {
    /// First node.
    pub node_a: NodeId,
    /// Second node.
    pub node_b: NodeId,
    /// Number of times both changed in the same event.
    pub co_changes: usize,
    /// Coupling score: co_changes / max(changes_a, changes_b).
    pub coupling_score: f64,
}

/// A prediction that a node will change soon.
#[derive(Debug, Clone)]
pub struct ChangePrediction {
    /// Node ID.
    pub node_id: NodeId,
    /// Human-readable label.
    pub label: String,
    /// Prediction confidence (0.0 .. 1.0).
    pub confidence: f64,
    /// Whether this node is currently in a burst pattern.
    pub in_burst: bool,
    /// Number of changes in the recent window.
    pub recent_changes: usize,
}

/// L2-normalize a vector in place.
fn normalize_vec(v: &mut [f64]) {
    let norm: f64 = v.iter().map(|x| x * x).sum::<f64>().sqrt();
    if norm > 1e-12 {
        v.iter_mut().for_each(|x| *x /= norm);
    }
}

/// Compute all eigenvalues and eigenvectors of a symmetric tridiagonal matrix.
///
/// * `diag` — main diagonal (length m).
/// * `off`  — sub-diagonal (length m-1).
///
/// Returns `(eigenvalues, eigenvectors)` where `eigenvectors[i]` is the
/// eigenvector for `eigenvalues[i]`, each of length `m`.
///
/// Uses the Jacobi eigenvalue algorithm on the full m x m symmetric matrix
/// built from the tridiagonal.  Since m is small (Lanczos iteration count,
/// typically 20-50), the O(m^3) cost is negligible compared to the O(k*m)
/// sparse mat-vecs.
fn tridiag_eigen(diag: &[f64], off: &[f64], m: usize) -> (Vec<f64>, Vec<Vec<f64>>) {
    if m == 0 {
        return (Vec::new(), Vec::new());
    }
    if m == 1 {
        return (vec![diag[0]], vec![vec![1.0]]);
    }

    // Build full symmetric matrix from the tridiagonal.
    let mut a = vec![vec![0.0f64; m]; m];
    for i in 0..m {
        a[i][i] = diag[i];
    }
    let off_len = off.len().min(m - 1);
    for i in 0..off_len {
        a[i][i + 1] = off[i];
        a[i + 1][i] = off[i];
    }

    // Eigenvector matrix V (columns are eigenvectors), starts as identity.
    let mut v = vec![vec![0.0f64; m]; m];
    for i in 0..m {
        v[i][i] = 1.0;
    }

    // Jacobi cyclic sweeps.
    for _ in 0..100 * m {
        // Find the largest off-diagonal element.
        let mut max_off = 0.0f64;
        let mut p = 0usize;
        let mut q = 1usize;
        for i in 0..m {
            for j in (i + 1)..m {
                if a[i][j].abs() > max_off {
                    max_off = a[i][j].abs();
                    p = i;
                    q = j;
                }
            }
        }

        if max_off < 1e-15 {
            break;
        }

        // Compute Jacobi rotation angle to zero out a[p][q].
        let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
        let t = theta.signum() / (theta.abs() + (1.0 + theta * theta).sqrt());
        let c = 1.0 / (1.0 + t * t).sqrt();
        let s = t * c;

        // Apply similarity rotation to A.
        let app = a[p][p];
        let aqq = a[q][q];
        let apq = a[p][q];
        a[p][p] = c * c * app - 2.0 * s * c * apq + s * s * aqq;
        a[q][q] = s * s * app + 2.0 * s * c * apq + c * c * aqq;
        a[p][q] = 0.0;
        a[q][p] = 0.0;

        for r in 0..m {
            if r != p && r != q {
                let arp = a[r][p];
                let arq = a[r][q];
                a[r][p] = c * arp - s * arq;
                a[p][r] = a[r][p];
                a[r][q] = s * arp + c * arq;
                a[q][r] = a[r][q];
            }
        }

        // Accumulate rotation into eigenvector matrix.
        for r in 0..m {
            let vp = v[r][p];
            let vq = v[r][q];
            v[r][p] = c * vp - s * vq;
            v[r][q] = s * vp + c * vq;
        }
    }

    // Eigenvalues are the diagonal of A.
    let eigenvalues: Vec<f64> = (0..m).map(|i| a[i][i]).collect();

    // Eigenvectors: column j of V is the eigenvector for eigenvalue j.
    // Return as eigenvectors[j] = column j.
    let eigenvectors: Vec<Vec<f64>> = (0..m)
        .map(|j| (0..m).map(|i| v[i][j]).collect())
        .collect();

    (eigenvalues, eigenvectors)
}

/// Jacobi eigenvalue decomposition for a dense symmetric matrix.
///
/// Like `tridiag_eigen` but accepts an arbitrary symmetric matrix (not
/// just tridiagonal). Used by [`CausalGraph::spectral_analysis_rff`] for
/// the small `m x m` covariance matrix.
///
/// Returns `(eigenvalues, eigenvectors)` where `eigenvectors[i]` is the
/// eigenvector for `eigenvalues[i]`.
fn dense_jacobi_eigen(mat: &[Vec<f64>], m: usize, max_iter: usize) -> (Vec<f64>, Vec<Vec<f64>>) {
    if m == 0 {
        return (Vec::new(), Vec::new());
    }
    if m == 1 {
        return (vec![mat[0][0]], vec![vec![1.0]]);
    }

    let mut a = mat.to_vec();

    // Eigenvector matrix V, starts as identity.
    let mut v = vec![vec![0.0f64; m]; m];
    for i in 0..m {
        v[i][i] = 1.0;
    }

    for _ in 0..max_iter {
        // Find largest off-diagonal element.
        let mut max_off = 0.0f64;
        let mut p = 0usize;
        let mut q = 1usize;
        for i in 0..m {
            for j in (i + 1)..m {
                if a[i][j].abs() > max_off {
                    max_off = a[i][j].abs();
                    p = i;
                    q = j;
                }
            }
        }

        if max_off < 1e-15 {
            break;
        }

        let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
        let t = theta.signum() / (theta.abs() + (1.0 + theta * theta).sqrt());
        let c = 1.0 / (1.0 + t * t).sqrt();
        let s = t * c;

        let app = a[p][p];
        let aqq = a[q][q];
        let apq = a[p][q];
        a[p][p] = c * c * app - 2.0 * s * c * apq + s * s * aqq;
        a[q][q] = s * s * app + 2.0 * s * c * apq + c * c * aqq;
        a[p][q] = 0.0;
        a[q][p] = 0.0;

        for r in 0..m {
            if r != p && r != q {
                let arp = a[r][p];
                let arq = a[r][q];
                a[r][p] = c * arp - s * arq;
                a[p][r] = a[r][p];
                a[r][q] = s * arp + c * arq;
                a[q][r] = a[r][q];
            }
        }

        for r in 0..m {
            let vp = v[r][p];
            let vq = v[r][q];
            v[r][p] = c * vp - s * vq;
            v[r][q] = s * vp + c * vq;
        }
    }

    let eigenvalues: Vec<f64> = (0..m).map(|i| a[i][i]).collect();
    let eigenvectors: Vec<Vec<f64>> = (0..m)
        .map(|j| (0..m).map(|i| v[i][j]).collect())
        .collect();

    (eigenvalues, eigenvectors)
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

/// Serializable snapshot of a [`CausalGraph`] for JSON persistence.
#[derive(Serialize, Deserialize)]
struct CausalGraphSnapshot {
    next_node_id: u64,
    nodes: Vec<CausalNode>,
    forward_edges: std::collections::HashMap<NodeId, Vec<CausalEdge>>,
}

impl CausalGraph {
    /// Serialize the entire graph to a JSON writer.
    pub fn save_to_writer<W: std::io::Write>(&self, writer: W) -> Result<(), std::io::Error> {
        let snapshot = self.to_snapshot();
        serde_json::to_writer_pretty(writer, &snapshot)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
    }

    /// Deserialize a graph from a JSON reader.
    pub fn load_from_reader<R: std::io::Read>(reader: R) -> Result<Self, std::io::Error> {
        let snapshot: CausalGraphSnapshot = serde_json::from_reader(reader)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        Ok(Self::from_snapshot(snapshot))
    }

    /// Save the graph to a file path.
    pub fn save_to_file(&self, path: &std::path::Path) -> Result<(), std::io::Error> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = std::fs::File::create(path)?;
        let writer = std::io::BufWriter::new(file);
        self.save_to_writer(writer)
    }

    /// Load a graph from a file path.
    pub fn load_from_file(path: &std::path::Path) -> Result<Self, std::io::Error> {
        let file = std::fs::File::open(path)?;
        let reader = std::io::BufReader::new(file);
        Self::load_from_reader(reader)
    }

    fn to_snapshot(&self) -> CausalGraphSnapshot {
        let nodes: Vec<CausalNode> = self.nodes.iter().map(|r| r.value().clone()).collect();
        let mut forward_edges = std::collections::HashMap::new();
        for entry in self.forward_edges.iter() {
            if !entry.value().is_empty() {
                forward_edges.insert(*entry.key(), entry.value().clone());
            }
        }
        CausalGraphSnapshot {
            next_node_id: self.next_node_id.load(Ordering::SeqCst),
            nodes,
            forward_edges,
        }
    }

    fn from_snapshot(snapshot: CausalGraphSnapshot) -> Self {
        let graph = Self {
            nodes: DashMap::new(),
            forward_edges: DashMap::new(),
            reverse_edges: DashMap::new(),
            next_node_id: AtomicU64::new(snapshot.next_node_id),
            node_count: AtomicU64::new(0),
            edge_count: AtomicU64::new(0),
            #[cfg(feature = "exochain")]
            chain_manager: None,
            #[cfg(feature = "exochain")]
            governance_engine: None,
        };

        // Restore nodes.
        for node in &snapshot.nodes {
            graph.nodes.insert(node.id, node.clone());
            graph.forward_edges.insert(node.id, Vec::new());
            graph.reverse_edges.insert(node.id, Vec::new());
        }
        graph.node_count.store(snapshot.nodes.len() as u64, Ordering::SeqCst);

        // Restore edges from forward_edges map.
        let mut total_edges: u64 = 0;
        for (source_id, edges) in &snapshot.forward_edges {
            for edge in edges {
                if let Some(mut fwd) = graph.forward_edges.get_mut(source_id) {
                    fwd.push(edge.clone());
                }
                if let Some(mut rev) = graph.reverse_edges.get_mut(&edge.target) {
                    rev.push(edge.clone());
                }
                total_edges += 1;
            }
        }
        graph.edge_count.store(total_edges, Ordering::SeqCst);

        graph
    }
}

impl fmt::Debug for CausalGraph {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CausalGraph")
            .field("node_count", &self.node_count.load(Ordering::Relaxed))
            .field("edge_count", &self.edge_count.load(Ordering::Relaxed))
            .finish()
    }
}

impl Default for CausalGraph {
    fn default() -> Self {
        Self::new()
    }
}

// ===========================================================================
// KG-009: Geometric Shadowing for Memory Decay (RoMem)
// ===========================================================================

/// Configuration for geometric shadowing (memory decay).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShadowingConfig {
    /// Decay rate per epoch (0.0 = no decay, 1.0 = instant decay).
    pub decay_rate: f64,
    /// How many hops a node's shadow extends.
    pub shadow_radius: usize,
}

impl Default for ShadowingConfig {
    fn default() -> Self {
        Self {
            decay_rate: 0.1,
            shadow_radius: 3,
        }
    }
}

/// Per-edge-type volatility: some edge types decay faster than others.
/// EML-learned rates; these defaults encode domain knowledge.
fn edge_type_volatility(edge_type: &CausalEdgeType) -> f64 {
    match edge_type {
        // Correlates are ephemeral -- decay fast.
        CausalEdgeType::Correlates => 2.0,
        // Follows is temporal context -- moderately volatile.
        CausalEdgeType::Follows => 1.5,
        // EvidenceFor is mid-range.
        CausalEdgeType::EvidenceFor => 1.2,
        // TriggeredBy is event-driven, somewhat transient.
        CausalEdgeType::TriggeredBy => 1.3,
        // Contradicts: important to keep visible.
        CausalEdgeType::Contradicts => 0.5,
        // Causes and Enables are structural -- decay slowly.
        CausalEdgeType::Causes => 0.3,
        CausalEdgeType::Enables => 0.4,
        // Inhibits is a strong structural relationship.
        CausalEdgeType::Inhibits => 0.4,
    }
}

/// Maximum edge-type volatility across an iterator of edges, using a
/// [`clawft_treecalc::triage`] dispatch on the edge-kind stream so a
/// uniform-kind incident set collapses to a single `match` branch
/// (Sequence form) instead of one per edge.
///
/// Behaviour-equivalent to `edges.map(volatility).fold(1.0, max)` —
/// the hardcoded fallback semantics — but faster on the common case
/// where a node's incident edges are all the same kind (e.g. a
/// node touched only by `Causes` edges).
///
/// NOTE(treecalc-swap): wired — Finding #9 (CausalEdgeType decay
/// dispatch). The treecalc batching is the structural change; a
/// per-kind learned `DecayScheduleModel` is a follow-up.
pub fn max_volatility_batched<'a, I>(edges: I) -> f64
where
    I: IntoIterator<Item = &'a CausalEdge>,
{
    // Snapshot the edge-kind stream so we can triage and re-iterate.
    let kinds: Vec<&CausalEdgeType> =
        edges.into_iter().map(|e| &e.edge_type).collect();
    if kinds.is_empty() {
        return 1.0;
    }

    match clawft_treecalc::triage(kinds.iter().copied()) {
        // All edges share one kind — single match, no per-edge branching.
        clawft_treecalc::Form::Sequence => {
            // Safe to unwrap: kinds is non-empty by the guard above.
            edge_type_volatility(kinds[0]).max(1.0)
        }
        // Atom is unreachable (we guard non-empty), but treat as the
        // identity to keep behaviour aligned with the fallback.
        clawft_treecalc::Form::Atom => 1.0,
        // Mixed kinds — fold over deduped representatives so each
        // kind costs one match branch instead of one per occurrence.
        clawft_treecalc::Form::Branch => {
            let mut seen: std::collections::HashSet<std::mem::Discriminant<CausalEdgeType>> =
                std::collections::HashSet::new();
            let mut max = 1.0_f64;
            for k in &kinds {
                if seen.insert(std::mem::discriminant(*k)) {
                    max = max.max(edge_type_volatility(k));
                }
            }
            max
        }
    }
}

/// Compute shadow-adjusted relevance for each node in the graph.
///
/// Recent nodes cast "shadows" that suppress older redundant nodes.
/// A node's shadow weight is `decay_rate^(age * volatility)` where age
/// is `current_epoch - node.created_at` and volatility is the maximum
/// edge-type volatility among the node's incident edges.
///
/// Nodes within `shadow_radius` hops of a recent (high-weight) node
/// have their weight further reduced proportionally, simulating the
/// effect of newer evidence making older nearby evidence redundant.
///
/// Returns a map from NodeId to shadow-adjusted weight in [0.0, 1.0].
/// A weight of 0.0 means the node is fully shadowed and can be pruned.
pub fn compute_shadows(
    graph: &CausalGraph,
    config: &ShadowingConfig,
    current_epoch: u64,
) -> std::collections::HashMap<NodeId, f64> {
    let node_ids = graph.node_ids();
    let mut weights: std::collections::HashMap<NodeId, f64> = std::collections::HashMap::new();

    if config.decay_rate <= 0.0 || config.decay_rate > 1.0 {
        // No decay or invalid config: all nodes get weight 1.0.
        for &id in &node_ids {
            weights.insert(id, 1.0);
        }
        return weights;
    }

    // Phase 1: Compute base weight from age and edge-type volatility.
    //
    // NOTE(treecalc-swap): wired — Finding #9. Per-node incident-edge
    // dispatch goes through `max_volatility_batched`, which uses
    // `clawft_treecalc::triage` so a uniform-kind incident set
    // collapses to one match branch. Behaviour matches the previous
    // `map(volatility).fold(1.0, max)` formulation exactly.
    for &id in &node_ids {
        let node = match graph.get_node(id) {
            Some(n) => n,
            None => continue,
        };

        let age = current_epoch.saturating_sub(node.created_at) as f64;

        let fwd_edges = graph.get_forward_edges(id);
        let rev_edges = graph.get_reverse_edges(id);
        let max_volatility =
            max_volatility_batched(fwd_edges.iter().chain(rev_edges.iter()));

        // Geometric decay: weight = (1 - decay_rate)^(age * volatility).
        let base_weight = (1.0 - config.decay_rate).powf(age * max_volatility);
        weights.insert(id, base_weight.clamp(0.0, 1.0));
    }

    // Phase 2: Shadow suppression -- recent (high-weight) nodes suppress
    // nearby older nodes within shadow_radius hops.
    if config.shadow_radius > 0 {
        // Sort nodes by weight descending to process recent nodes first.
        let mut sorted: Vec<(NodeId, f64)> = weights.iter().map(|(&k, &v)| (k, v)).collect();
        sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // For each high-weight node, reduce weights of nearby older nodes.
        let shadow_reductions: Vec<(NodeId, f64)> = {
            let mut reductions = Vec::new();
            for &(caster_id, caster_weight) in &sorted {
                if caster_weight < 0.5 {
                    break; // Only recent nodes cast meaningful shadows.
                }

                // BFS from caster up to shadow_radius hops.
                let nearby = graph.traverse_forward(caster_id, config.shadow_radius);
                let nearby_rev = graph.traverse_reverse(caster_id, config.shadow_radius);

                for neighbor_id in nearby.iter().chain(nearby_rev.iter()) {
                    let neighbor_weight = weights.get(neighbor_id).copied().unwrap_or(1.0);
                    // Only suppress nodes that are older (lower weight).
                    if neighbor_weight < caster_weight {
                        // Shadow strength decays with hop distance.
                        // Approximate: since traverse returns all within radius,
                        // apply a flat suppression factor.
                        let suppression = caster_weight * config.decay_rate;
                        let new_weight = (neighbor_weight - suppression).max(0.0);
                        reductions.push((*neighbor_id, new_weight));
                    }
                }
            }
            reductions
        };

        for (id, new_weight) in shadow_reductions {
            let entry = weights.entry(id).or_insert(1.0);
            if new_weight < *entry {
                *entry = new_weight;
            }
        }
    }

    weights
}

// ===========================================================================
// KG-014: Codebook Cold-Start (TransFIR)
// ===========================================================================

/// Vector quantization codebook for bootstrapping new entity embeddings.
///
/// When a new entity type has no training data for embeddings, the codebook
/// provides a reasonable starting vector by mapping entity types to their
/// nearest centroid from previously seen embeddings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VqCodebook {
    /// Centroid vectors (k centroids, each of dimension `dim`).
    centroids: Vec<Vec<f32>>,
    /// Maps entity type strings to their assigned centroid index.
    assignments: std::collections::HashMap<String, usize>,
    /// Dimensionality of vectors.
    dim: usize,
}

impl VqCodebook {
    /// Create a new codebook with `k` centroids of dimension `dim`.
    ///
    /// Centroids are initialized to zero and must be trained before use.
    pub fn new(k: usize, dim: usize) -> Self {
        Self {
            centroids: vec![vec![0.0; dim]; k],
            assignments: std::collections::HashMap::new(),
            dim,
        }
    }

    /// Number of centroids in the codebook.
    pub fn k(&self) -> usize {
        self.centroids.len()
    }

    /// Dimensionality of the embedding space.
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// Train the codebook from existing embeddings using Lloyd's algorithm.
    ///
    /// Each embedding is a `(entity_type, vector)` pair. After training,
    /// each entity type is assigned to its nearest centroid.
    ///
    /// Uses K-means++ initialization for better convergence.
    pub fn train(&mut self, embeddings: &[(String, Vec<f32>)]) {
        if embeddings.is_empty() || self.centroids.is_empty() {
            return;
        }

        let k = self.centroids.len();
        let n = embeddings.len();

        // K-means++ initialization: pick first centroid randomly (use first
        // point), then pick subsequent centroids proportional to distance^2.
        self.centroids[0] = embeddings[0].1.clone();
        self.centroids[0].resize(self.dim, 0.0);

        for ci in 1..k {
            if ci >= n {
                // More centroids than points: duplicate last used embedding.
                self.centroids[ci] = self.centroids[ci - 1].clone();
                continue;
            }

            // Find the embedding farthest from all existing centroids.
            let mut best_dist = f32::NEG_INFINITY;
            let mut best_idx = ci;
            for (ei, (_etype, vec)) in embeddings.iter().enumerate() {
                let min_dist = (0..ci)
                    .map(|c| Self::distance_sq(vec, &self.centroids[c]))
                    .fold(f32::INFINITY, f32::min);
                if min_dist > best_dist {
                    best_dist = min_dist;
                    best_idx = ei;
                }
            }
            let mut v = embeddings[best_idx].1.clone();
            v.resize(self.dim, 0.0);
            self.centroids[ci] = v;
        }

        // Lloyd's algorithm: iterate assignment + update.
        let max_lloyd_iterations = 50;
        for _iter in 0..max_lloyd_iterations {
            // Assignment step: assign each embedding to nearest centroid.
            let mut cluster_sums: Vec<Vec<f64>> = vec![vec![0.0; self.dim]; k];
            let mut cluster_counts: Vec<usize> = vec![0; k];
            let mut assigns: Vec<usize> = Vec::with_capacity(n);

            for (_etype, vec) in embeddings {
                let (ci, _dist) = self.quantize(vec);
                assigns.push(ci);
                cluster_counts[ci] += 1;
                for (d, &val) in vec.iter().enumerate().take(self.dim) {
                    cluster_sums[ci][d] += val as f64;
                }
            }

            // Update step: recompute centroids.
            let mut changed = false;
            for ci in 0..k {
                if cluster_counts[ci] == 0 {
                    continue;
                }
                let count = cluster_counts[ci] as f64;
                for d in 0..self.dim {
                    let new_val = (cluster_sums[ci][d] / count) as f32;
                    if (new_val - self.centroids[ci][d]).abs() > 1e-6 {
                        changed = true;
                    }
                    self.centroids[ci][d] = new_val;
                }
            }

            if !changed {
                break;
            }
        }

        // Store final assignments: each entity type -> nearest centroid.
        self.assignments.clear();
        for (etype, vec) in embeddings {
            let (ci, _dist) = self.quantize(vec);
            self.assignments.insert(etype.clone(), ci);
        }
    }

    /// Look up the codebook vector for an entity type.
    ///
    /// Returns `None` if the entity type has not been seen during training.
    pub fn lookup(&self, entity_type: &str) -> Option<&[f32]> {
        self.assignments
            .get(entity_type)
            .map(|&ci| self.centroids[ci].as_slice())
    }

    /// Quantize a vector to its nearest centroid.
    ///
    /// Returns `(centroid_index, squared_distance)`.
    pub fn quantize(&self, vector: &[f32]) -> (usize, f32) {
        let mut best_ci = 0;
        let mut best_dist = f32::INFINITY;
        for (ci, centroid) in self.centroids.iter().enumerate() {
            let dist = Self::distance_sq(vector, centroid);
            if dist < best_dist {
                best_dist = dist;
                best_ci = ci;
            }
        }
        (best_ci, best_dist)
    }

    /// Squared Euclidean distance between two vectors.
    fn distance_sq(a: &[f32], b: &[f32]) -> f32 {
        let len = a.len().min(b.len());
        let mut sum = 0.0f32;
        for i in 0..len {
            let diff = a[i] - b[i];
            sum += diff * diff;
        }
        // Treat missing dimensions as 0.
        for val in a.iter().skip(len) {
            sum += val * val;
        }
        for val in b.iter().skip(len) {
            sum += val * val;
        }
        sum
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_graph() -> CausalGraph {
        CausalGraph::new()
    }

    // 1
    #[test]
    fn new_graph_empty() {
        let g = make_graph();
        assert_eq!(g.node_count(), 0);
        assert_eq!(g.edge_count(), 0);
    }

    // 2
    #[test]
    fn add_node_returns_id() {
        let g = make_graph();
        let id1 = g.add_node("A".into(), serde_json::json!({}));
        let id2 = g.add_node("B".into(), serde_json::json!({}));
        assert_ne!(id1, id2);
        assert_eq!(g.node_count(), 2);
    }

    // 3
    #[test]
    fn get_node() {
        let g = make_graph();
        let id = g.add_node("hello".into(), serde_json::json!({"key": "val"}));
        let node = g.get_node(id).unwrap();
        assert_eq!(node.label, "hello");
        assert_eq!(node.metadata["key"], "val");
    }

    // 4
    #[test]
    fn remove_node() {
        let g = make_graph();
        let id = g.add_node("X".into(), serde_json::json!({}));
        assert!(g.get_node(id).is_some());
        let removed = g.remove_node(id).unwrap();
        assert_eq!(removed.label, "X");
        assert!(g.get_node(id).is_none());
        assert_eq!(g.node_count(), 0);
    }

    // 5
    #[test]
    fn link_creates_edge() {
        let g = make_graph();
        let a = g.add_node("A".into(), serde_json::json!({}));
        let b = g.add_node("B".into(), serde_json::json!({}));
        assert!(g.link(a, b, CausalEdgeType::Causes, 0.9, 100, 1));
        assert_eq!(g.edge_count(), 1);
    }

    // 6
    #[test]
    fn link_invalid_source_returns_false() {
        let g = make_graph();
        let b = g.add_node("B".into(), serde_json::json!({}));
        assert!(!g.link(9999, b, CausalEdgeType::Causes, 0.5, 0, 0));
        assert_eq!(g.edge_count(), 0);
    }

    // 7
    #[test]
    fn link_invalid_target_returns_false() {
        let g = make_graph();
        let a = g.add_node("A".into(), serde_json::json!({}));
        assert!(!g.link(a, 9999, CausalEdgeType::Causes, 0.5, 0, 0));
        assert_eq!(g.edge_count(), 0);
    }

    // 8
    #[test]
    fn get_forward_edges() {
        let g = make_graph();
        let a = g.add_node("A".into(), serde_json::json!({}));
        let b = g.add_node("B".into(), serde_json::json!({}));
        let c = g.add_node("C".into(), serde_json::json!({}));
        g.link(a, b, CausalEdgeType::Causes, 1.0, 0, 0);
        g.link(a, c, CausalEdgeType::Enables, 0.5, 0, 0);
        let fwd = g.get_forward_edges(a);
        assert_eq!(fwd.len(), 2);
        assert!(fwd.iter().any(|e| e.target == b));
        assert!(fwd.iter().any(|e| e.target == c));
    }

    // 9
    #[test]
    fn get_reverse_edges() {
        let g = make_graph();
        let a = g.add_node("A".into(), serde_json::json!({}));
        let b = g.add_node("B".into(), serde_json::json!({}));
        g.link(a, b, CausalEdgeType::Causes, 1.0, 0, 0);
        let rev = g.get_reverse_edges(b);
        assert_eq!(rev.len(), 1);
        assert_eq!(rev[0].source, a);
    }

    // 10
    #[test]
    fn get_edges_by_type() {
        let g = make_graph();
        let a = g.add_node("A".into(), serde_json::json!({}));
        let b = g.add_node("B".into(), serde_json::json!({}));
        let c = g.add_node("C".into(), serde_json::json!({}));
        g.link(a, b, CausalEdgeType::Causes, 1.0, 0, 0);
        g.link(a, c, CausalEdgeType::Inhibits, 0.3, 0, 0);
        let causes = g.get_edges_by_type(a, &CausalEdgeType::Causes);
        assert_eq!(causes.len(), 1);
        assert_eq!(causes[0].target, b);
    }

    // 11
    #[test]
    fn unlink_removes_edges() {
        let g = make_graph();
        let a = g.add_node("A".into(), serde_json::json!({}));
        let b = g.add_node("B".into(), serde_json::json!({}));
        g.link(a, b, CausalEdgeType::Causes, 1.0, 0, 0);
        g.link(a, b, CausalEdgeType::Enables, 0.5, 0, 0);
        assert_eq!(g.edge_count(), 2);
        let removed = g.unlink(a, b);
        assert_eq!(removed, 2);
        assert_eq!(g.edge_count(), 0);
        assert!(g.get_forward_edges(a).is_empty());
        assert!(g.get_reverse_edges(b).is_empty());
    }

    // 12
    #[test]
    fn node_count() {
        let g = make_graph();
        assert_eq!(g.node_count(), 0);
        g.add_node("A".into(), serde_json::json!({}));
        assert_eq!(g.node_count(), 1);
        let id = g.add_node("B".into(), serde_json::json!({}));
        assert_eq!(g.node_count(), 2);
        g.remove_node(id);
        assert_eq!(g.node_count(), 1);
    }

    // 13
    #[test]
    fn edge_count() {
        let g = make_graph();
        let a = g.add_node("A".into(), serde_json::json!({}));
        let b = g.add_node("B".into(), serde_json::json!({}));
        let c = g.add_node("C".into(), serde_json::json!({}));
        g.link(a, b, CausalEdgeType::Causes, 1.0, 0, 0);
        g.link(b, c, CausalEdgeType::Follows, 0.8, 0, 0);
        assert_eq!(g.edge_count(), 2);
    }

    // 14
    #[test]
    fn clear_empties_graph() {
        let g = make_graph();
        let a = g.add_node("A".into(), serde_json::json!({}));
        let b = g.add_node("B".into(), serde_json::json!({}));
        g.link(a, b, CausalEdgeType::Causes, 1.0, 0, 0);
        g.clear().unwrap();
        assert_eq!(g.node_count(), 0);
        assert_eq!(g.edge_count(), 0);
    }

    // 15 — A -> B, traverse 1 hop forward from A
    #[test]
    fn traverse_forward_single_hop() {
        let g = make_graph();
        let a = g.add_node("A".into(), serde_json::json!({}));
        let b = g.add_node("B".into(), serde_json::json!({}));
        g.link(a, b, CausalEdgeType::Causes, 1.0, 0, 0);
        let reachable = g.traverse_forward(a, 1);
        assert_eq!(reachable, vec![b]);
    }

    // 16 — A -> B -> C, traverse 2 hops from A
    #[test]
    fn traverse_forward_multi_hop() {
        let g = make_graph();
        let a = g.add_node("A".into(), serde_json::json!({}));
        let b = g.add_node("B".into(), serde_json::json!({}));
        let c = g.add_node("C".into(), serde_json::json!({}));
        g.link(a, b, CausalEdgeType::Causes, 1.0, 0, 0);
        g.link(b, c, CausalEdgeType::Causes, 1.0, 0, 0);
        let reachable = g.traverse_forward(a, 2);
        assert!(reachable.contains(&b));
        assert!(reachable.contains(&c));
        assert_eq!(reachable.len(), 2);
    }

    // 17 — A -> B -> C, traverse only 1 hop from A (should NOT reach C)
    #[test]
    fn traverse_forward_depth_limit() {
        let g = make_graph();
        let a = g.add_node("A".into(), serde_json::json!({}));
        let b = g.add_node("B".into(), serde_json::json!({}));
        let c = g.add_node("C".into(), serde_json::json!({}));
        g.link(a, b, CausalEdgeType::Causes, 1.0, 0, 0);
        g.link(b, c, CausalEdgeType::Causes, 1.0, 0, 0);
        let reachable = g.traverse_forward(a, 1);
        assert_eq!(reachable, vec![b]);
        assert!(!reachable.contains(&c));
    }

    // 18 — A -> B -> C, traverse reverse from C
    #[test]
    fn traverse_reverse() {
        let g = make_graph();
        let a = g.add_node("A".into(), serde_json::json!({}));
        let b = g.add_node("B".into(), serde_json::json!({}));
        let c = g.add_node("C".into(), serde_json::json!({}));
        g.link(a, b, CausalEdgeType::Causes, 1.0, 0, 0);
        g.link(b, c, CausalEdgeType::Causes, 1.0, 0, 0);
        let reachable = g.traverse_reverse(c, 2);
        assert!(reachable.contains(&b));
        assert!(reachable.contains(&a));
    }

    // 19
    #[test]
    fn find_path_exists() {
        let g = make_graph();
        let a = g.add_node("A".into(), serde_json::json!({}));
        let b = g.add_node("B".into(), serde_json::json!({}));
        let c = g.add_node("C".into(), serde_json::json!({}));
        g.link(a, b, CausalEdgeType::Causes, 1.0, 0, 0);
        g.link(b, c, CausalEdgeType::Causes, 1.0, 0, 0);
        let path = g.find_path(a, c, 5).unwrap();
        assert_eq!(path, vec![a, b, c]);
    }

    // 20
    #[test]
    fn find_path_no_path() {
        let g = make_graph();
        let a = g.add_node("A".into(), serde_json::json!({}));
        let b = g.add_node("B".into(), serde_json::json!({}));
        // No edge between them.
        assert!(g.find_path(a, b, 5).is_none());
    }

    // 21
    #[test]
    fn concurrent_add_nodes() {
        use std::sync::Arc;
        use std::thread;

        let g = Arc::new(CausalGraph::new());
        let mut handles = Vec::new();

        for t in 0..4 {
            let g = Arc::clone(&g);
            handles.push(thread::spawn(move || {
                for i in 0..25 {
                    g.add_node(format!("t{t}-n{i}"), serde_json::json!({}));
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(g.node_count(), 100);
    }

    // 22
    #[test]
    fn causal_edge_type_display() {
        assert_eq!(CausalEdgeType::Causes.to_string(), "Causes");
        assert_eq!(CausalEdgeType::Inhibits.to_string(), "Inhibits");
        assert_eq!(CausalEdgeType::Correlates.to_string(), "Correlates");
        assert_eq!(CausalEdgeType::Enables.to_string(), "Enables");
        assert_eq!(CausalEdgeType::Follows.to_string(), "Follows");
        assert_eq!(CausalEdgeType::Contradicts.to_string(), "Contradicts");
        assert_eq!(CausalEdgeType::TriggeredBy.to_string(), "TriggeredBy");
        assert_eq!(CausalEdgeType::EvidenceFor.to_string(), "EvidenceFor");
    }

    // =====================================================================
    // Degree tests
    // =====================================================================

    // 23
    #[test]
    fn degree_computation() {
        let g = make_graph();
        let a = g.add_node("A".into(), serde_json::json!({}));
        let b = g.add_node("B".into(), serde_json::json!({}));
        let c = g.add_node("C".into(), serde_json::json!({}));
        g.link(a, b, CausalEdgeType::Causes, 1.0, 0, 0);
        g.link(a, c, CausalEdgeType::Enables, 0.5, 0, 0);
        g.link(b, c, CausalEdgeType::Follows, 0.8, 0, 0);
        assert_eq!(g.out_degree(a), 2);
        assert_eq!(g.in_degree(a), 0);
        assert_eq!(g.degree(a), 2);
        assert_eq!(g.in_degree(c), 2);
        assert_eq!(g.out_degree(c), 0);
        assert_eq!(g.degree(c), 2);
        assert_eq!(g.degree(b), 2); // 1 in + 1 out
    }

    // =====================================================================
    // Connected Components tests
    // =====================================================================

    // 24
    #[test]
    fn connected_components_single() {
        let g = make_graph();
        let a = g.add_node("A".into(), serde_json::json!({}));
        let b = g.add_node("B".into(), serde_json::json!({}));
        g.link(a, b, CausalEdgeType::Causes, 1.0, 0, 0);
        let cc = g.connected_components();
        assert_eq!(cc.len(), 1);
        assert_eq!(cc[0].len(), 2);
    }

    // 25
    #[test]
    fn connected_components_two_islands() {
        let g = make_graph();
        let a = g.add_node("A".into(), serde_json::json!({}));
        let b = g.add_node("B".into(), serde_json::json!({}));
        let c = g.add_node("C".into(), serde_json::json!({}));
        let d = g.add_node("D".into(), serde_json::json!({}));
        g.link(a, b, CausalEdgeType::Causes, 1.0, 0, 0);
        g.link(c, d, CausalEdgeType::Causes, 1.0, 0, 0);
        let cc = g.connected_components();
        assert_eq!(cc.len(), 2);
        assert_eq!(cc[0].len(), 2);
        assert_eq!(cc[1].len(), 2);
    }

    // 26
    #[test]
    fn connected_components_isolated_node() {
        let g = make_graph();
        let a = g.add_node("A".into(), serde_json::json!({}));
        let b = g.add_node("B".into(), serde_json::json!({}));
        g.add_node("isolated".into(), serde_json::json!({}));
        g.link(a, b, CausalEdgeType::Causes, 1.0, 0, 0);
        let cc = g.connected_components();
        assert_eq!(cc.len(), 2);
        assert_eq!(cc[0].len(), 2); // largest first
        assert_eq!(cc[1].len(), 1); // isolated
    }

    // =====================================================================
    // Community Detection tests
    // =====================================================================

    // 27
    #[test]
    fn community_detection_two_clusters() {
        let g = make_graph();
        // Cluster 1: A-B-C strongly connected.
        let a = g.add_node("A".into(), serde_json::json!({}));
        let b = g.add_node("B".into(), serde_json::json!({}));
        let c = g.add_node("C".into(), serde_json::json!({}));
        g.link(a, b, CausalEdgeType::Causes, 1.0, 0, 0);
        g.link(b, c, CausalEdgeType::Causes, 1.0, 0, 0);
        g.link(c, a, CausalEdgeType::Causes, 1.0, 0, 0);

        // Cluster 2: D-E-F strongly connected.
        let d = g.add_node("D".into(), serde_json::json!({}));
        let e = g.add_node("E".into(), serde_json::json!({}));
        let f = g.add_node("F".into(), serde_json::json!({}));
        g.link(d, e, CausalEdgeType::Causes, 1.0, 0, 0);
        g.link(e, f, CausalEdgeType::Causes, 1.0, 0, 0);
        g.link(f, d, CausalEdgeType::Causes, 1.0, 0, 0);

        // Weak bridge between clusters.
        g.link(c, d, CausalEdgeType::Correlates, 0.1, 0, 0);

        let communities = g.detect_communities(20);
        // Should find 2 communities (clusters) even with the weak bridge.
        // Label propagation may merge them due to the bridge, but the strong
        // internal edges should dominate.
        assert!(!communities.is_empty());
        // At minimum, isolated nodes shouldn't be their own community.
        assert!(communities.len() <= 3);
    }

    // 28
    #[test]
    fn community_detection_isolated_nodes() {
        let g = make_graph();
        g.add_node("A".into(), serde_json::json!({}));
        g.add_node("B".into(), serde_json::json!({}));
        let communities = g.detect_communities(10);
        // Each isolated node stays in its own community.
        assert_eq!(communities.len(), 2);
    }

    // 29
    #[test]
    fn community_detection_empty_graph() {
        let g = make_graph();
        let communities = g.detect_communities(10);
        assert!(communities.is_empty());
    }

    // =====================================================================
    // Spectral Analysis tests
    // =====================================================================

    // 30
    #[test]
    fn spectral_connected_graph_positive_lambda2() {
        let g = make_graph();
        let a = g.add_node("A".into(), serde_json::json!({}));
        let b = g.add_node("B".into(), serde_json::json!({}));
        let c = g.add_node("C".into(), serde_json::json!({}));
        g.link(a, b, CausalEdgeType::Causes, 1.0, 0, 0);
        g.link(b, c, CausalEdgeType::Causes, 1.0, 0, 0);
        g.link(a, c, CausalEdgeType::Causes, 1.0, 0, 0);

        let result = g.spectral_analysis(50);
        assert!(
            result.lambda_2 > 0.0,
            "connected graph should have lambda_2 > 0, got {}",
            result.lambda_2
        );
        assert_eq!(result.fiedler_vector.len(), 3);
        assert_eq!(result.node_ids.len(), 3);
    }

    // 31
    #[test]
    fn spectral_disconnected_graph_zero_lambda2() {
        let g = make_graph();
        let _a = g.add_node("A".into(), serde_json::json!({}));
        let _b = g.add_node("B".into(), serde_json::json!({}));
        // No edges — disconnected.
        let result = g.spectral_analysis(50);
        assert!(
            result.lambda_2 < 0.01,
            "disconnected graph should have lambda_2 ~ 0, got {}",
            result.lambda_2
        );
        assert_eq!(result.node_ids.len(), 2);
    }

    // 32
    #[test]
    fn spectral_partition_splits_graph() {
        let g = make_graph();
        let a = g.add_node("A".into(), serde_json::json!({}));
        let b = g.add_node("B".into(), serde_json::json!({}));
        let c = g.add_node("C".into(), serde_json::json!({}));
        let d = g.add_node("D".into(), serde_json::json!({}));
        // Two clusters with weak bridge.
        g.link(a, b, CausalEdgeType::Causes, 1.0, 0, 0);
        g.link(c, d, CausalEdgeType::Causes, 1.0, 0, 0);
        g.link(b, c, CausalEdgeType::Correlates, 0.1, 0, 0);

        let (part_a, part_b) = g.spectral_partition();
        assert!(!part_a.is_empty());
        assert!(!part_b.is_empty());
        assert_eq!(part_a.len() + part_b.len(), 4);
    }

    // 33
    #[test]
    fn spectral_single_node() {
        let g = make_graph();
        g.add_node("A".into(), serde_json::json!({}));
        let result = g.spectral_analysis(50);
        assert_eq!(result.lambda_2, 0.0);
    }

    // =====================================================================
    // Coupling / Predictive Analysis tests
    // =====================================================================

    // 34
    #[test]
    fn coupling_basic() {
        let g = make_graph();
        let a = g.add_node("A".into(), serde_json::json!({}));
        let b = g.add_node("B".into(), serde_json::json!({}));
        let c = g.add_node("C".into(), serde_json::json!({}));

        let events = vec![
            ChangeEvent { node_ids: vec![a, b], timestamp: 1 },
            ChangeEvent { node_ids: vec![a, b], timestamp: 2 },
            ChangeEvent { node_ids: vec![a, c], timestamp: 3 },
            ChangeEvent { node_ids: vec![b, c], timestamp: 4 },
        ];

        let coupling = g.compute_coupling(&events);
        assert!(!coupling.is_empty());

        // A-B co-changed 2 times out of max(3,3)=3 → 0.67
        let ab = coupling.iter().find(|p| {
            (p.node_a == a && p.node_b == b) || (p.node_a == b && p.node_b == a)
        });
        assert!(ab.is_some());
        let ab = ab.unwrap();
        assert_eq!(ab.co_changes, 2);
        assert!((ab.coupling_score - 2.0 / 3.0).abs() < 0.01);
    }

    // 35
    #[test]
    fn coupling_empty_events() {
        let g = make_graph();
        let coupling = g.compute_coupling(&[]);
        assert!(coupling.is_empty());
    }

    // 36
    #[test]
    fn predict_changes_burst_detection() {
        let g = make_graph();
        let a = g.add_node("module_a".into(), serde_json::json!({}));
        let b = g.add_node("module_b".into(), serde_json::json!({}));
        let c = g.add_node("module_c".into(), serde_json::json!({}));

        // History: 50 events. Module A changes rarely (every 10th event).
        // Module C fills the rest so there are plenty of events.
        let mut events = Vec::new();
        for i in 0..50 {
            events.push(ChangeEvent { node_ids: vec![c], timestamp: i });
            if i % 10 == 0 {
                events.push(ChangeEvent { node_ids: vec![a], timestamp: i });
            }
        }
        // Burst window: module A + B change together in every recent event.
        for i in 50..60 {
            events.push(ChangeEvent { node_ids: vec![a, b], timestamp: i });
        }
        // Module A baseline: ~5 changes in ~55 events before window (rate ~0.09).
        // Module A window: 10 changes in 10 events (rate 1.0).
        // Burst ratio: ~11x — well above 1.5 threshold.

        let predictions = g.predict_changes(&events, 10, 1.5);
        assert!(!predictions.is_empty());

        // Module A should be predicted (in burst).
        let pred_a = predictions.iter().find(|p| p.node_id == a);
        assert!(pred_a.is_some(), "module_a should be in predictions");
        assert!(pred_a.unwrap().in_burst, "module_a should be in burst");

        // Module B should be predicted (co-modified with A during burst).
        let pred_b = predictions.iter().find(|p| p.node_id == b);
        assert!(pred_b.is_some(), "module_b should be predicted via coupling");
    }

    // 37
    #[test]
    fn predict_changes_empty_events() {
        let g = make_graph();
        let predictions = g.predict_changes(&[], 5, 2.0);
        assert!(predictions.is_empty());
    }

    // 38
    #[test]
    fn node_ids_returns_all() {
        let g = make_graph();
        let a = g.add_node("A".into(), serde_json::json!({}));
        let b = g.add_node("B".into(), serde_json::json!({}));
        let ids = g.node_ids();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&a));
        assert!(ids.contains(&b));
    }

    // 39
    #[test]
    fn spectral_strongly_connected_high_lambda2() {
        // A complete graph of 4 nodes should have high lambda_2.
        let g = make_graph();
        let nodes: Vec<NodeId> = (0..4)
            .map(|i| g.add_node(format!("N{i}"), serde_json::json!({})))
            .collect();
        for i in 0..4 {
            for j in 0..4 {
                if i != j {
                    g.link(nodes[i], nodes[j], CausalEdgeType::Causes, 1.0, 0, 0);
                }
            }
        }
        let result = g.spectral_analysis(50);
        // For K4, lambda_2 should be 4.0 (all eigenvalues of Laplacian of K4 are 0,4,4,4).
        assert!(
            result.lambda_2 > 3.0,
            "complete graph K4 lambda_2 should be ~4.0, got {}",
            result.lambda_2
        );
    }

    // ── Persistence tests ────────────────────────────────────────────

    fn tmp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "causal_test_{name}_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    // 40
    #[test]
    fn persist_empty_graph_roundtrip() {
        let g = make_graph();
        let path = tmp_path("empty");
        g.save_to_file(&path).unwrap();
        let loaded = CausalGraph::load_from_file(&path).unwrap();
        assert_eq!(loaded.node_count(), 0);
        assert_eq!(loaded.edge_count(), 0);
        let _ = std::fs::remove_file(&path);
    }

    // 41
    #[test]
    fn persist_nodes_and_edges_roundtrip() {
        let g = make_graph();
        let a = g.add_node("Alpha".into(), serde_json::json!({"role": "source"}));
        let b = g.add_node("Beta".into(), serde_json::json!({"role": "target"}));
        let c = g.add_node("Gamma".into(), serde_json::json!({}));
        g.link(a, b, CausalEdgeType::Causes, 0.9, 100, 1);
        g.link(b, c, CausalEdgeType::Enables, 0.5, 200, 2);
        g.link(a, c, CausalEdgeType::Inhibits, 0.3, 300, 3);

        let path = tmp_path("nodes_edges");
        g.save_to_file(&path).unwrap();
        let loaded = CausalGraph::load_from_file(&path).unwrap();

        assert_eq!(loaded.node_count(), 3);
        assert_eq!(loaded.edge_count(), 3);

        // Verify node data.
        let na = loaded.get_node(a).unwrap();
        assert_eq!(na.label, "Alpha");
        assert_eq!(na.metadata["role"], "source");

        let nb = loaded.get_node(b).unwrap();
        assert_eq!(nb.label, "Beta");

        // Verify edges.
        let fwd_a = loaded.get_forward_edges(a);
        assert_eq!(fwd_a.len(), 2);
        assert!(fwd_a.iter().any(|e| e.target == b && e.edge_type == CausalEdgeType::Causes));
        assert!(fwd_a.iter().any(|e| e.target == c && e.edge_type == CausalEdgeType::Inhibits));

        let _ = std::fs::remove_file(&path);
    }

    // 42
    #[test]
    fn persist_node_metadata_survives() {
        let g = make_graph();
        let id = g.add_node("rich".into(), serde_json::json!({
            "tags": ["a", "b"],
            "count": 42,
            "nested": {"x": true}
        }));

        let path = tmp_path("metadata");
        g.save_to_file(&path).unwrap();
        let loaded = CausalGraph::load_from_file(&path).unwrap();
        let node = loaded.get_node(id).unwrap();
        assert_eq!(node.metadata["tags"][0], "a");
        assert_eq!(node.metadata["count"], 42);
        assert_eq!(node.metadata["nested"]["x"], true);
        let _ = std::fs::remove_file(&path);
    }

    // 43
    #[test]
    fn persist_edge_types_and_weights() {
        let g = make_graph();
        let a = g.add_node("A".into(), serde_json::json!({}));
        let b = g.add_node("B".into(), serde_json::json!({}));
        g.link(a, b, CausalEdgeType::Contradicts, 0.77, 555, 10);

        let path = tmp_path("edge_types");
        g.save_to_file(&path).unwrap();
        let loaded = CausalGraph::load_from_file(&path).unwrap();
        let edges = loaded.get_forward_edges(a);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].edge_type, CausalEdgeType::Contradicts);
        assert!((edges[0].weight - 0.77).abs() < 0.001);
        assert_eq!(edges[0].timestamp, 555);
        assert_eq!(edges[0].chain_seq, 10);
        let _ = std::fs::remove_file(&path);
    }

    // 44
    #[test]
    fn persist_next_node_id_preserved() {
        let g = make_graph();
        let _a = g.add_node("A".into(), serde_json::json!({}));
        let _b = g.add_node("B".into(), serde_json::json!({}));
        let _c = g.add_node("C".into(), serde_json::json!({}));
        // next_node_id should be 4 now (started at 1, added 3 nodes).

        let path = tmp_path("next_id");
        g.save_to_file(&path).unwrap();
        let loaded = CausalGraph::load_from_file(&path).unwrap();

        // Adding a new node should get id >= 4, not collide with existing.
        let new_id = loaded.add_node("D".into(), serde_json::json!({}));
        assert!(new_id >= 4, "new node should get id >= 4, got {new_id}");
        assert!(loaded.get_node(new_id).is_some());
        assert_eq!(loaded.node_count(), 4);
        let _ = std::fs::remove_file(&path);
    }

    // 45
    #[test]
    fn persist_writer_reader_roundtrip() {
        let g = make_graph();
        let a = g.add_node("X".into(), serde_json::json!({}));
        let b = g.add_node("Y".into(), serde_json::json!({}));
        g.link(a, b, CausalEdgeType::Follows, 1.0, 0, 0);

        let mut buf = Vec::new();
        g.save_to_writer(&mut buf).unwrap();

        let loaded = CausalGraph::load_from_reader(buf.as_slice()).unwrap();
        assert_eq!(loaded.node_count(), 2);
        assert_eq!(loaded.edge_count(), 1);
        let edges = loaded.get_forward_edges(a);
        assert_eq!(edges[0].edge_type, CausalEdgeType::Follows);
    }

    // 46
    #[test]
    fn persist_reverse_edges_rebuilt() {
        let g = make_graph();
        let a = g.add_node("A".into(), serde_json::json!({}));
        let b = g.add_node("B".into(), serde_json::json!({}));
        g.link(a, b, CausalEdgeType::Causes, 1.0, 0, 0);

        let path = tmp_path("reverse");
        g.save_to_file(&path).unwrap();
        let loaded = CausalGraph::load_from_file(&path).unwrap();

        // Reverse edges should be rebuilt from forward edges.
        let rev = loaded.get_reverse_edges(b);
        assert_eq!(rev.len(), 1);
        assert_eq!(rev[0].source, a);
        let _ = std::fs::remove_file(&path);
    }

    // =====================================================================
    // Sparse Lanczos vs dense reference comparison
    // =====================================================================

    /// Dense reference: compute lambda_2 via Jacobi eigendecomposition of the
    /// full Laplacian.  O(n^3) — only used in tests for correctness checking.
    fn dense_spectral_lambda2(g: &CausalGraph, _max_iterations: usize) -> f64 {
        let ids = g.node_ids();
        let n = ids.len();
        if n < 2 {
            return 0.0;
        }

        let mut id_to_idx: std::collections::HashMap<NodeId, usize> =
            std::collections::HashMap::new();
        let mut sorted_ids = ids.clone();
        sorted_ids.sort();
        for (i, &id) in sorted_ids.iter().enumerate() {
            id_to_idx.insert(id, i);
        }

        let mut laplacian = vec![vec![0.0f64; n]; n];
        for &id in &sorted_ids {
            let i = id_to_idx[&id];
            for edge in g.get_forward_edges(id) {
                if let Some(&j) = id_to_idx.get(&edge.target)
                    && i != j {
                        let w = edge.weight as f64;
                        laplacian[i][j] -= w;
                        laplacian[j][i] -= w;
                    }
            }
            for edge in g.get_reverse_edges(id) {
                if let Some(&j) = id_to_idx.get(&edge.source)
                    && i != j && j > i {
                        let w = edge.weight as f64;
                        laplacian[i][j] -= w;
                        laplacian[j][i] -= w;
                    }
            }
        }
        for i in 0..n {
            let off_sum: f64 = (0..n).filter(|&j| j != i).map(|j| -laplacian[i][j]).sum();
            laplacian[i][i] = off_sum;
        }

        // Jacobi eigendecomposition of the full Laplacian.
        let mut a = laplacian;
        for _ in 0..100 * n {
            let mut max_off = 0.0f64;
            let mut p = 0usize;
            let mut q = 1usize;
            for i in 0..n {
                for j in (i + 1)..n {
                    if a[i][j].abs() > max_off {
                        max_off = a[i][j].abs();
                        p = i;
                        q = j;
                    }
                }
            }
            if max_off < 1e-15 { break; }
            let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
            let t = theta.signum() / (theta.abs() + (1.0 + theta * theta).sqrt());
            let c = 1.0 / (1.0 + t * t).sqrt();
            let s = t * c;
            let app = a[p][p]; let aqq = a[q][q]; let apq = a[p][q];
            a[p][p] = c * c * app - 2.0 * s * c * apq + s * s * aqq;
            a[q][q] = s * s * app + 2.0 * s * c * apq + c * c * aqq;
            a[p][q] = 0.0;
            a[q][p] = 0.0;
            for r in 0..n {
                if r != p && r != q {
                    let arp = a[r][p]; let arq = a[r][q];
                    a[r][p] = c * arp - s * arq; a[p][r] = a[r][p];
                    a[r][q] = s * arp + c * arq; a[q][r] = a[r][q];
                }
            }
        }

        // Collect eigenvalues, sort, return second-smallest.
        let mut evals: Vec<f64> = (0..n).map(|i| a[i][i]).collect();
        evals.sort_by(|a, b| a.partial_cmp(b).unwrap());
        if n >= 2 { evals[1].max(0.0) } else { 0.0 }
    }

    // 47
    #[test]
    fn spectral_lanczos_matches_dense_triangle() {
        // Triangle graph (K3): known lambda_2 = 3.0 for unit weights.
        let g = make_graph();
        let a = g.add_node("A".into(), serde_json::json!({}));
        let b = g.add_node("B".into(), serde_json::json!({}));
        let c = g.add_node("C".into(), serde_json::json!({}));
        g.link(a, b, CausalEdgeType::Causes, 1.0, 0, 0);
        g.link(b, c, CausalEdgeType::Causes, 1.0, 0, 0);
        g.link(a, c, CausalEdgeType::Causes, 1.0, 0, 0);

        let sparse_result = g.spectral_analysis(50);
        let dense_lambda2 = dense_spectral_lambda2(&g, 200);

        assert!(
            (sparse_result.lambda_2 - dense_lambda2).abs() < 0.5,
            "Lanczos lambda_2={} vs dense lambda_2={} differ too much",
            sparse_result.lambda_2,
            dense_lambda2,
        );
        // Both should be close to 3.0 for K3 with symmetric unit edges.
        assert!(sparse_result.lambda_2 > 1.0, "lambda_2 should be > 1 for K3");
    }

    // 48
    #[test]
    fn spectral_lanczos_matches_dense_path() {
        // Path graph: A - B - C - D (lambda_2 ~ 0.586 for unit weights).
        let g = make_graph();
        let a = g.add_node("A".into(), serde_json::json!({}));
        let b = g.add_node("B".into(), serde_json::json!({}));
        let c = g.add_node("C".into(), serde_json::json!({}));
        let d = g.add_node("D".into(), serde_json::json!({}));
        g.link(a, b, CausalEdgeType::Causes, 1.0, 0, 0);
        g.link(b, c, CausalEdgeType::Causes, 1.0, 0, 0);
        g.link(c, d, CausalEdgeType::Causes, 1.0, 0, 0);

        let sparse_result = g.spectral_analysis(50);
        let dense_lambda2 = dense_spectral_lambda2(&g, 200);

        assert!(
            (sparse_result.lambda_2 - dense_lambda2).abs() < 0.5,
            "Lanczos lambda_2={} vs dense lambda_2={} differ too much",
            sparse_result.lambda_2,
            dense_lambda2,
        );
        assert!(sparse_result.lambda_2 > 0.0, "path graph should be connected");
    }

    // 49
    #[test]
    fn spectral_lanczos_matches_dense_k4() {
        // K4: lambda_2 = 4.0 for unit-weight complete graph on 4 nodes.
        let g = make_graph();
        let nodes: Vec<NodeId> = (0..4)
            .map(|i| g.add_node(format!("N{i}"), serde_json::json!({})))
            .collect();
        for i in 0..4 {
            for j in (i + 1)..4 {
                g.link(nodes[i], nodes[j], CausalEdgeType::Causes, 1.0, 0, 0);
            }
        }

        let sparse_result = g.spectral_analysis(50);
        let dense_lambda2 = dense_spectral_lambda2(&g, 200);

        assert!(
            (sparse_result.lambda_2 - dense_lambda2).abs() < 0.5,
            "K4: Lanczos lambda_2={} vs dense lambda_2={}",
            sparse_result.lambda_2,
            dense_lambda2,
        );
    }

    // 50
    #[test]
    fn spectral_lanczos_disconnected() {
        // Two isolated nodes — lambda_2 should be 0.
        let g = make_graph();
        g.add_node("A".into(), serde_json::json!({}));
        g.add_node("B".into(), serde_json::json!({}));

        let result = g.spectral_analysis(50);
        assert!(
            result.lambda_2 < 0.01,
            "disconnected graph should have lambda_2 ~ 0, got {}",
            result.lambda_2
        );
    }

    // ===================================================================
    // KG-003: Causal Chain Tracing
    // ===================================================================

    // 51
    #[test]
    fn trace_chain_simple_linear() {
        // A --Causes--> B --Enables--> C
        let g = make_graph();
        let a = g.add_node("A".into(), serde_json::json!({}));
        let b = g.add_node("B".into(), serde_json::json!({}));
        let c = g.add_node("C".into(), serde_json::json!({}));
        g.link(a, b, CausalEdgeType::Causes, 0.8, 0, 0);
        g.link(b, c, CausalEdgeType::Enables, 0.6, 0, 0);

        let chain = g.trace_causal_chain(a, c, 10).expect("path should exist");
        assert_eq!(chain.path.len(), 2);
        assert_eq!(chain.path[0], (a, CausalEdgeType::Causes, b));
        assert_eq!(chain.path[1], (b, CausalEdgeType::Enables, c));
        assert!((chain.total_weight - 1.4).abs() < 1e-5);
        assert!(chain.explanation.contains("Causes"));
        assert!(chain.explanation.contains("Enables"));
        assert!(chain.explanation.contains("A"));
        assert!(chain.explanation.contains("C"));
    }

    // 52
    #[test]
    fn trace_chain_same_node() {
        let g = make_graph();
        let a = g.add_node("A".into(), serde_json::json!({}));
        let chain = g.trace_causal_chain(a, a, 10).expect("same node => empty chain");
        assert!(chain.path.is_empty());
        assert_eq!(chain.total_weight, 0.0);
        assert_eq!(chain.explanation, "A");
    }

    // 53
    #[test]
    fn trace_chain_no_path() {
        let g = make_graph();
        let a = g.add_node("A".into(), serde_json::json!({}));
        let b = g.add_node("B".into(), serde_json::json!({}));
        // No edge between A and B.
        assert!(g.trace_causal_chain(a, b, 10).is_none());
    }

    // 54
    #[test]
    fn trace_chain_nonexistent_node() {
        let g = make_graph();
        let a = g.add_node("A".into(), serde_json::json!({}));
        assert!(g.trace_causal_chain(a, 9999, 10).is_none());
        assert!(g.trace_causal_chain(9999, a, 10).is_none());
    }

    // 55
    #[test]
    fn trace_chain_depth_limit() {
        // A -> B -> C -> D, but max_depth=2 cannot reach D from A.
        let g = make_graph();
        let a = g.add_node("A".into(), serde_json::json!({}));
        let b = g.add_node("B".into(), serde_json::json!({}));
        let c = g.add_node("C".into(), serde_json::json!({}));
        let d = g.add_node("D".into(), serde_json::json!({}));
        g.link(a, b, CausalEdgeType::Causes, 1.0, 0, 0);
        g.link(b, c, CausalEdgeType::Causes, 1.0, 0, 0);
        g.link(c, d, CausalEdgeType::Causes, 1.0, 0, 0);

        assert!(g.trace_causal_chain(a, d, 2).is_none());
        assert!(g.trace_causal_chain(a, d, 3).is_some());
    }

    // 56
    #[test]
    fn trace_chain_picks_shortest() {
        // A -> B -> D (len 2) and A -> C -> X -> D (len 3).
        // BFS should find the shorter path.
        let g = make_graph();
        let a = g.add_node("A".into(), serde_json::json!({}));
        let b = g.add_node("B".into(), serde_json::json!({}));
        let c = g.add_node("C".into(), serde_json::json!({}));
        let x = g.add_node("X".into(), serde_json::json!({}));
        let d = g.add_node("D".into(), serde_json::json!({}));
        g.link(a, b, CausalEdgeType::Causes, 1.0, 0, 0);
        g.link(b, d, CausalEdgeType::EvidenceFor, 1.0, 0, 0);
        g.link(a, c, CausalEdgeType::Causes, 1.0, 0, 0);
        g.link(c, x, CausalEdgeType::Causes, 1.0, 0, 0);
        g.link(x, d, CausalEdgeType::Causes, 1.0, 0, 0);

        let chain = g.trace_causal_chain(a, d, 10).unwrap();
        assert_eq!(chain.path.len(), 2, "should pick the 2-hop path");
    }

    // 57
    #[test]
    fn trace_chain_mixed_edge_types() {
        let g = make_graph();
        let a = g.add_node("auth_service".into(), serde_json::json!({}));
        let b = g.add_node("config_service".into(), serde_json::json!({}));
        let c = g.add_node("cache_ttl".into(), serde_json::json!({}));
        g.link(a, b, CausalEdgeType::Causes, 0.9, 0, 0);
        g.link(b, c, CausalEdgeType::EvidenceFor, 0.7, 0, 0);

        let chain = g.trace_causal_chain(a, c, 10).unwrap();
        assert!(chain.explanation.contains("auth_service"));
        assert!(chain.explanation.contains("--Causes-->"));
        assert!(chain.explanation.contains("--EvidenceFor-->"));
        assert!(chain.explanation.contains("cache_ttl"));
    }

    // ===================================================================
    // KG-004: Random Fourier Feature Spectral Analysis
    // ===================================================================

    // 58
    #[test]
    fn rff_single_node() {
        let g = make_graph();
        g.add_node("A".into(), serde_json::json!({}));
        let result = g.spectral_analysis_rff(32, 200);
        assert!(result.lambda_2.abs() < 1e-6);
    }

    // 59
    #[test]
    fn rff_disconnected_graph() {
        let g = make_graph();
        g.add_node("A".into(), serde_json::json!({}));
        g.add_node("B".into(), serde_json::json!({}));
        let result = g.spectral_analysis_rff(32, 200);
        assert!(
            result.lambda_2 < 0.1,
            "disconnected graph RFF lambda_2 should be ~0, got {}",
            result.lambda_2
        );
    }

    // 60
    #[test]
    fn rff_path_graph_positive_lambda2() {
        // Path: A -- B -- C -- D. lambda_2 should be positive.
        let g = make_graph();
        let a = g.add_node("A".into(), serde_json::json!({}));
        let b = g.add_node("B".into(), serde_json::json!({}));
        let c = g.add_node("C".into(), serde_json::json!({}));
        let d = g.add_node("D".into(), serde_json::json!({}));
        g.link(a, b, CausalEdgeType::Causes, 1.0, 0, 0);
        g.link(b, c, CausalEdgeType::Causes, 1.0, 0, 0);
        g.link(c, d, CausalEdgeType::Causes, 1.0, 0, 0);

        let result = g.spectral_analysis_rff(64, 200);
        assert!(
            result.lambda_2 > 0.01,
            "connected path graph should have lambda_2 > 0, got {}",
            result.lambda_2
        );
        assert_eq!(result.fiedler_vector.len(), 4);
    }

    // 61
    #[test]
    fn rff_agrees_with_lanczos_k4() {
        // K4: lambda_2 = 4.0 for unit-weight complete graph on 4 nodes.
        let g = make_graph();
        let nodes: Vec<NodeId> = (0..4)
            .map(|i| g.add_node(format!("N{i}"), serde_json::json!({})))
            .collect();
        for i in 0..4 {
            for j in (i + 1)..4 {
                g.link(nodes[i], nodes[j], CausalEdgeType::Causes, 1.0, 0, 0);
            }
        }

        let lanczos = g.spectral_analysis(50);
        let rff = g.spectral_analysis_rff(64, 200);

        // RFF should be within ~50% of Lanczos for a small graph.
        // (RFF is designed for large graphs; small graphs have higher variance.)
        assert!(
            rff.lambda_2 > 0.5,
            "K4 RFF lambda_2 should be significantly positive, got {}",
            rff.lambda_2
        );
        // Both should be in the same ballpark.
        let ratio = rff.lambda_2 / lanczos.lambda_2.max(1e-12);
        assert!(
            ratio > 0.3 && ratio < 3.0,
            "RFF/Lanczos ratio for K4 should be reasonable: RFF={}, Lanczos={}, ratio={}",
            rff.lambda_2,
            lanczos.lambda_2,
            ratio,
        );
    }

    // 62
    #[test]
    fn rff_returns_correct_node_count() {
        let g = make_graph();
        for i in 0..8 {
            g.add_node(format!("N{i}"), serde_json::json!({}));
        }
        let result = g.spectral_analysis_rff(16, 100);
        assert_eq!(result.node_ids.len(), 8);
        assert_eq!(result.fiedler_vector.len(), 8);
    }

    // 63
    #[test]
    fn rff_fiedler_vector_is_normalized() {
        let g = make_graph();
        let nodes: Vec<NodeId> = (0..6)
            .map(|i| g.add_node(format!("N{i}"), serde_json::json!({})))
            .collect();
        for i in 0..5 {
            g.link(nodes[i], nodes[i + 1], CausalEdgeType::Causes, 1.0, 0, 0);
        }

        let result = g.spectral_analysis_rff(32, 200);
        let norm: f64 = result.fiedler_vector.iter().map(|x| x * x).sum::<f64>().sqrt();
        assert!(
            (norm - 1.0).abs() < 0.05,
            "Fiedler vector should be approximately unit-normalized, got norm={}",
            norm
        );
    }

    // -- KG-009: Geometric Shadowing tests ---------------------------------

    #[test]
    fn shadowing_no_decay_gives_unit_weights() {
        let g = make_graph();
        let a = g.add_node("A".into(), serde_json::json!({}));
        let b = g.add_node("B".into(), serde_json::json!({}));
        g.link(a, b, CausalEdgeType::Causes, 1.0, 0, 0);

        let config = ShadowingConfig {
            decay_rate: 0.0,
            shadow_radius: 3,
        };
        let weights = compute_shadows(&g, &config, 100);
        assert_eq!(*weights.get(&a).unwrap(), 1.0);
        assert_eq!(*weights.get(&b).unwrap(), 1.0);
    }

    #[test]
    fn shadowing_recent_nodes_have_higher_weight() {
        let g = make_graph();
        // Node with created_at=0 (old) and created_at=90 (recent).
        let old_id = g.add_node("Old".into(), serde_json::json!({}));
        let recent_id = g.add_node("Recent".into(), serde_json::json!({}));

        // Manually set created_at via metadata workaround: directly modify nodes.
        if let Some(mut node) = g.nodes.get_mut(&old_id) {
            node.created_at = 0;
        }
        if let Some(mut node) = g.nodes.get_mut(&recent_id) {
            node.created_at = 90;
        }
        g.link(old_id, recent_id, CausalEdgeType::Causes, 1.0, 90, 0);

        let config = ShadowingConfig {
            decay_rate: 0.05,
            shadow_radius: 2,
        };
        let weights = compute_shadows(&g, &config, 100);

        let old_w = *weights.get(&old_id).unwrap();
        let recent_w = *weights.get(&recent_id).unwrap();
        assert!(recent_w > old_w,
            "Recent node (w={recent_w}) should have higher weight than old node (w={old_w})");
    }

    #[test]
    fn shadowing_correlates_decay_faster() {
        let g = make_graph();
        let a = g.add_node("A".into(), serde_json::json!({}));
        let b = g.add_node("B".into(), serde_json::json!({}));
        let c = g.add_node("C".into(), serde_json::json!({}));

        // a->b via Causes (low volatility), a->c via Correlates (high volatility).
        g.link(a, b, CausalEdgeType::Causes, 1.0, 0, 0);
        g.link(a, c, CausalEdgeType::Correlates, 1.0, 0, 0);

        // Both b and c have same age (created_at=0).
        let config = ShadowingConfig {
            decay_rate: 0.05,
            shadow_radius: 0, // no shadow suppression, just age decay
        };
        let weights = compute_shadows(&g, &config, 50);

        let w_b = *weights.get(&b).unwrap();
        let w_c = *weights.get(&c).unwrap();
        // Node connected via Correlates should decay faster.
        assert!(w_b > w_c,
            "Causes-connected node (w={w_b}) should decay slower than Correlates-connected (w={w_c})");
    }

    #[test]
    fn shadowing_empty_graph() {
        let g = make_graph();
        let config = ShadowingConfig::default();
        let weights = compute_shadows(&g, &config, 100);
        assert!(weights.is_empty());
    }

    #[test]
    fn shadowing_weights_in_range() {
        let g = make_graph();
        for i in 0..10 {
            let id = g.add_node(format!("N{i}"), serde_json::json!({}));
            if let Some(mut node) = g.nodes.get_mut(&id) {
                node.created_at = i * 10;
            }
        }
        let ids = g.node_ids();
        for i in 0..ids.len().saturating_sub(1) {
            g.link(ids[i], ids[i + 1], CausalEdgeType::Causes, 1.0, 0, 0);
        }

        let config = ShadowingConfig {
            decay_rate: 0.1,
            shadow_radius: 2,
        };
        let weights = compute_shadows(&g, &config, 100);
        for &w in weights.values() {
            assert!((0.0..=1.0).contains(&w), "weight {w} out of range");
        }
    }

    // -- Finding #9: treecalc-batched edge volatility ----------------------

    fn make_edge(source: NodeId, target: NodeId, kind: CausalEdgeType) -> CausalEdge {
        CausalEdge {
            source,
            target,
            edge_type: kind,
            weight: 1.0,
            timestamp: 0,
            chain_seq: 0,
            source_universal_id: [0u8; 32],
            target_universal_id: [0u8; 32],
        }
    }

    fn naive_max_volatility(edges: &[CausalEdge]) -> f64 {
        edges
            .iter()
            .map(|e| edge_type_volatility(&e.edge_type))
            .fold(1.0_f64, f64::max)
    }

    #[test]
    fn batched_max_volatility_empty_returns_one() {
        // Mirrors the original `fold(1.0, max)` identity.
        assert_eq!(max_volatility_batched(std::iter::empty()), 1.0);
    }

    #[test]
    fn batched_max_volatility_uniform_kind_matches_naive() {
        // Sequence form — all edges share one kind.
        let edges = vec![
            make_edge(1, 2, CausalEdgeType::Causes),
            make_edge(1, 3, CausalEdgeType::Causes),
            make_edge(1, 4, CausalEdgeType::Causes),
        ];
        let naive = naive_max_volatility(&edges);
        let batched = max_volatility_batched(edges.iter());
        assert_eq!(naive, batched);
        // Causes has volatility 0.3, but the fold floor is 1.0.
        assert_eq!(batched, 1.0);
    }

    #[test]
    fn batched_max_volatility_mixed_kinds_matches_naive() {
        // Branch form — heterogeneous kinds.
        let edges = vec![
            make_edge(1, 2, CausalEdgeType::Causes),
            make_edge(1, 3, CausalEdgeType::Correlates),
            make_edge(1, 4, CausalEdgeType::Follows),
            make_edge(1, 5, CausalEdgeType::Inhibits),
        ];
        let naive = naive_max_volatility(&edges);
        let batched = max_volatility_batched(edges.iter());
        assert_eq!(naive, batched);
        // Correlates volatility (2.0) wins.
        assert_eq!(batched, 2.0);
    }

    #[test]
    fn batched_max_volatility_high_volatility_uniform() {
        // Sequence form for a kind whose volatility > 1.0.
        let edges = [make_edge(1, 2, CausalEdgeType::Correlates),
            make_edge(1, 3, CausalEdgeType::Correlates)];
        assert_eq!(max_volatility_batched(edges.iter()), 2.0);
    }

    #[test]
    fn batched_max_volatility_dedups_repeated_kinds() {
        // Branch form with repeats — should still match the naive
        // fold because we max over the unique-kind volatilities.
        let edges = vec![
            make_edge(1, 2, CausalEdgeType::Causes),
            make_edge(1, 3, CausalEdgeType::Causes),
            make_edge(1, 4, CausalEdgeType::Correlates),
            make_edge(1, 5, CausalEdgeType::Correlates),
            make_edge(1, 6, CausalEdgeType::Follows),
        ];
        assert_eq!(
            max_volatility_batched(edges.iter()),
            naive_max_volatility(&edges)
        );
    }

    #[test]
    fn compute_shadows_post_batching_matches_pre_batching_invariants() {
        // High-level: compute_shadows with mixed-kind edges still
        // produces in-range weights and Correlates edges decay faster
        // than Causes — the original `shadowing_correlates_decay_faster`
        // invariant remains intact under the treecalc dispatch.
        let g = make_graph();
        let a = g.add_node("A".into(), serde_json::json!({}));
        let b = g.add_node("B".into(), serde_json::json!({}));
        let c = g.add_node("C".into(), serde_json::json!({}));
        g.link(a, b, CausalEdgeType::Causes, 1.0, 0, 0);
        g.link(a, c, CausalEdgeType::Correlates, 1.0, 0, 0);

        let cfg = ShadowingConfig {
            decay_rate: 0.05,
            shadow_radius: 0,
        };
        let weights = compute_shadows(&g, &cfg, 50);
        let w_b = *weights.get(&b).unwrap();
        let w_c = *weights.get(&c).unwrap();
        assert!(w_b > w_c, "Causes ({w_b}) should decay slower than Correlates ({w_c})");
    }

    // -- KG-014: VQ Codebook Cold-Start tests ------------------------------

    #[test]
    fn vq_codebook_new() {
        let cb = VqCodebook::new(4, 8);
        assert_eq!(cb.k(), 4);
        assert_eq!(cb.dim(), 8);
        assert_eq!(cb.centroids.len(), 4);
        assert!(cb.assignments.is_empty());
    }

    #[test]
    fn vq_codebook_quantize_to_nearest() {
        let mut cb = VqCodebook::new(2, 3);
        cb.centroids[0] = vec![1.0, 0.0, 0.0];
        cb.centroids[1] = vec![0.0, 1.0, 0.0];

        let (ci, dist) = cb.quantize(&[0.9, 0.1, 0.0]);
        assert_eq!(ci, 0, "should be nearest to centroid 0");
        assert!(dist < 0.1);

        let (ci2, _) = cb.quantize(&[0.1, 0.9, 0.0]);
        assert_eq!(ci2, 1, "should be nearest to centroid 1");
    }

    #[test]
    fn vq_codebook_train_and_lookup() {
        let mut cb = VqCodebook::new(2, 3);
        let embeddings = vec![
            ("Function".to_string(), vec![1.0, 0.0, 0.0]),
            ("Function".to_string(), vec![0.9, 0.1, 0.0]),
            ("Module".to_string(), vec![0.0, 1.0, 0.0]),
            ("Module".to_string(), vec![0.1, 0.9, 0.0]),
        ];

        cb.train(&embeddings);

        // After training, Function and Module should map to different centroids.
        let func_vec = cb.lookup("Function");
        let mod_vec = cb.lookup("Module");
        assert!(func_vec.is_some());
        assert!(mod_vec.is_some());

        // Centroids should be near the cluster means.
        let func_centroid = func_vec.unwrap();
        assert!(func_centroid[0] > 0.5, "Function centroid should be near [1,0,0]");

        let mod_centroid = mod_vec.unwrap();
        assert!(mod_centroid[1] > 0.5, "Module centroid should be near [0,1,0]");
    }

    #[test]
    fn vq_codebook_lookup_unknown_type() {
        let cb = VqCodebook::new(2, 3);
        assert!(cb.lookup("Unknown").is_none());
    }

    #[test]
    fn vq_codebook_train_empty() {
        let mut cb = VqCodebook::new(3, 4);
        cb.train(&[]); // should not panic
        assert!(cb.assignments.is_empty());
    }

    #[test]
    fn vq_codebook_train_single_embedding() {
        let mut cb = VqCodebook::new(2, 3);
        let embeddings = vec![("Type".to_string(), vec![0.5, 0.5, 0.0])];
        cb.train(&embeddings);
        let result = cb.lookup("Type");
        assert!(result.is_some());
    }

    #[test]
    fn vq_codebook_distance_sq_mismatched_dims() {
        // Shorter vector should be padded with zeros conceptually.
        let dist = VqCodebook::distance_sq(&[1.0, 2.0], &[1.0, 2.0, 3.0]);
        assert!((dist - 9.0).abs() < 1e-6, "missing dim treated as 0, so diff=3 -> sq=9");
    }
}
