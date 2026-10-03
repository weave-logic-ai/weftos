//! Graph operations harness for large in-memory knowledge graphs (GRAPH-041).
//!
//! Builds synthetic graphs via [`crate::bench::fixtures`] and times common
//! ops: neighbor scan, degree scan, cluster, god-nodes, subgraph, dedup.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::analyze;
use crate::cluster;
use crate::entity::EntityId;
use crate::model::{FlowDirection, KnowledgeGraph};

use super::fixtures::{
    estimate_graph_bytes, estimate_query_tokens, synthetic_graph, token_reduction_ratio,
};

/// Named graph operation measured by the harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphOp {
    /// Construct synthetic graph of N nodes.
    Construct,
    /// Scan neighbors for every node.
    NeighborScan,
    /// Sum degrees for every node.
    DegreeScan,
    /// Label-propagation community detection.
    Cluster,
    /// Top-N god nodes.
    GodNodes,
    /// Subgraph of a contiguous id slice.
    Subgraph,
    /// Near-duplicate entity merge pass.
    Dedup,
    /// Data-flow trace from a seed entity.
    TraceDataFlow,
}

impl GraphOp {
    /// Stable string name for reports / criterion ids.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Construct => "construct",
            Self::NeighborScan => "neighbor_scan",
            Self::DegreeScan => "degree_scan",
            Self::Cluster => "cluster",
            Self::GodNodes => "god_nodes",
            Self::Subgraph => "subgraph",
            Self::Dedup => "dedup",
            Self::TraceDataFlow => "trace_data_flow",
        }
    }
}

/// One timed graph-op observation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphOpsSample {
    /// Operation measured.
    pub op: GraphOp,
    /// Graph node count at measurement time.
    pub node_count: usize,
    /// Graph edge count at measurement time.
    pub edge_count: usize,
    /// Wall-clock duration.
    #[serde(with = "super::duration_ms")]
    pub duration: Duration,
    /// Optional note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl GraphOpsSample {
    /// Duration in whole milliseconds.
    pub fn as_millis(&self) -> u128 {
        self.duration.as_millis()
    }
}

/// Token-reduction sample (corpus tokens vs graph-query tokens).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TokenReductionSample {
    /// Estimated tokens if querying raw corpus.
    pub corpus_tokens: u64,
    /// Tokens for a graph neighbor-query answer surface.
    pub graph_query_tokens: u64,
    /// `1 - graph/corpus` (higher is better).
    pub reduction_ratio: f64,
}

/// Memory soft-check against the structural estimator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryEstimate {
    /// Node count used.
    pub node_count: usize,
    /// Edge count used.
    pub edge_count: usize,
    /// Estimated bytes.
    pub estimated_bytes: u64,
    /// Budget (default 500 MiB for 50K scale).
    pub budget_bytes: u64,
    /// Whether estimate is under budget.
    pub under_budget: bool,
}

/// Full suite result for one graph scale.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphOpsSuiteResult {
    /// Node count used for the suite.
    pub node_count: usize,
    /// Edge count after construction.
    pub edge_count: usize,
    /// Per-op samples.
    pub samples: Vec<GraphOpsSample>,
    /// Memory soft estimate.
    pub memory: MemoryEstimate,
    /// Optional token-reduction sample.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_reduction: Option<TokenReductionSample>,
    /// Sum of all op durations.
    #[serde(with = "super::duration_ms")]
    pub total_duration: Duration,
}

impl GraphOpsSuiteResult {
    /// Look up a sample by op.
    pub fn sample(&self, op: GraphOp) -> Option<&GraphOpsSample> {
        self.samples.iter().find(|s| s.op == op)
    }
}

/// Graph-ops harness.
#[derive(Debug, Default)]
pub struct GraphOpsHarness {
    samples: Vec<GraphOpsSample>,
}

impl GraphOpsHarness {
    /// Create an empty harness.
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a pre-measured sample.
    pub fn push_sample(&mut self, sample: GraphOpsSample) {
        self.samples.push(sample);
    }

    /// Borrow samples.
    pub fn samples(&self) -> &[GraphOpsSample] {
        &self.samples
    }

    /// Clear collected samples.
    pub fn clear(&mut self) {
        self.samples.clear();
    }

    fn record(
        &mut self,
        op: GraphOp,
        node_count: usize,
        edge_count: usize,
        duration: Duration,
    ) -> GraphOpsSample {
        let sample = GraphOpsSample {
            op,
            node_count,
            edge_count,
            duration,
            note: None,
        };
        self.samples.push(sample.clone());
        sample
    }

    /// Construct a synthetic graph and record the construct op.
    pub fn construct(&mut self, n_nodes: usize) -> (KnowledgeGraph, GraphOpsSample) {
        let start = Instant::now();
        let kg = synthetic_graph(n_nodes);
        let sample = self.record(
            GraphOp::Construct,
            kg.entity_count(),
            kg.relationship_count(),
            start.elapsed(),
        );
        (kg, sample)
    }

    /// Time a full neighbor scan.
    pub fn neighbor_scan(&mut self, kg: &KnowledgeGraph) -> GraphOpsSample {
        let ids: Vec<EntityId> = kg.entity_ids().cloned().collect();
        let start = Instant::now();
        let mut touch = 0usize;
        for id in &ids {
            touch = touch.wrapping_add(kg.neighbors(id).len());
        }
        // Prevent dead-code elimination without criterion black_box dep.
        std::hint::black_box(touch);
        self.record(
            GraphOp::NeighborScan,
            kg.entity_count(),
            kg.relationship_count(),
            start.elapsed(),
        )
    }

    /// Time a full degree scan.
    pub fn degree_scan(&mut self, kg: &KnowledgeGraph) -> GraphOpsSample {
        let ids: Vec<EntityId> = kg.entity_ids().cloned().collect();
        let start = Instant::now();
        let mut sum = 0usize;
        for id in &ids {
            sum = sum.wrapping_add(kg.degree(id));
        }
        std::hint::black_box(sum);
        self.record(
            GraphOp::DegreeScan,
            kg.entity_count(),
            kg.relationship_count(),
            start.elapsed(),
        )
    }

    /// Time community detection.
    pub fn cluster(&mut self, kg: &KnowledgeGraph) -> GraphOpsSample {
        let start = Instant::now();
        let communities = cluster::cluster(kg);
        std::hint::black_box(communities.len());
        self.record(
            GraphOp::Cluster,
            kg.entity_count(),
            kg.relationship_count(),
            start.elapsed(),
        )
    }

    /// Time god-node ranking.
    pub fn god_nodes(&mut self, kg: &KnowledgeGraph, top_n: usize) -> GraphOpsSample {
        let start = Instant::now();
        let gods = analyze::god_nodes(kg, top_n);
        std::hint::black_box(gods.len());
        self.record(
            GraphOp::GodNodes,
            kg.entity_count(),
            kg.relationship_count(),
            start.elapsed(),
        )
    }

    /// Time subgraph extraction for first `take` entities.
    pub fn subgraph(&mut self, kg: &KnowledgeGraph, take: usize) -> GraphOpsSample {
        let ids: Vec<EntityId> = kg.entity_ids().take(take).cloned().collect();
        let start = Instant::now();
        let sub = kg.subgraph(&ids);
        std::hint::black_box(sub.entity_count());
        self.record(
            GraphOp::Subgraph,
            kg.entity_count(),
            kg.relationship_count(),
            start.elapsed(),
        )
    }

    /// Time dedup pass (threshold 0.99 — near-identical only).
    pub fn dedup(&mut self, kg: &mut KnowledgeGraph) -> GraphOpsSample {
        let start = Instant::now();
        let removed = kg.dedup(0.99);
        std::hint::black_box(removed);
        self.record(
            GraphOp::Dedup,
            kg.entity_count(),
            kg.relationship_count(),
            start.elapsed(),
        )
    }

    /// Time data-flow trace from the first entity (if any).
    pub fn trace_data_flow(&mut self, kg: &KnowledgeGraph) -> GraphOpsSample {
        let seed = kg.entity_ids().next().cloned();
        let start = Instant::now();
        if let Some(id) = seed {
            let path = kg.trace_data_flow(&id, FlowDirection::Forward, 8);
            std::hint::black_box(path.len());
        }
        self.record(
            GraphOp::TraceDataFlow,
            kg.entity_count(),
            kg.relationship_count(),
            start.elapsed(),
        )
    }

    /// Run the standard suite at `n_nodes` and return a structured result.
    ///
    /// Ops: construct → neighbor → degree → cluster → god_nodes → subgraph
    /// → trace → dedup (last, mutates). Also fills memory + token reduction.
    pub fn run_suite(&mut self, n_nodes: usize) -> GraphOpsSuiteResult {
        self.clear();
        let (mut kg, _) = self.construct(n_nodes);
        let edge_count = kg.relationship_count();
        let node_count = kg.entity_count();

        self.neighbor_scan(&kg);
        self.degree_scan(&kg);
        self.cluster(&kg);
        self.god_nodes(&kg, 10);
        let take = (n_nodes / 10).max(1).min(n_nodes);
        self.subgraph(&kg, take);
        self.trace_data_flow(&kg);
        self.dedup(&mut kg);

        let estimated_bytes = estimate_graph_bytes(node_count, edge_count);
        let budget = 500 * 1024 * 1024;
        let memory = MemoryEstimate {
            node_count,
            edge_count,
            estimated_bytes,
            budget_bytes: budget,
            under_budget: estimated_bytes < budget,
        };

        // Token reduction: pretend corpus is n_nodes * 80 tokens; query is
        // neighbor labels of one hub.
        let corpus_tokens = (node_count as u64).saturating_mul(80);
        let hub = kg.entity_ids().next().cloned();
        let graph_query_tokens = if let Some(id) = hub {
            let labels: Vec<String> = kg
                .neighbors(&id)
                .into_iter()
                .map(|e| e.label.clone())
                .collect();
            estimate_query_tokens(&labels).max(1)
        } else {
            0
        };
        let token_reduction = Some(TokenReductionSample {
            corpus_tokens,
            graph_query_tokens,
            reduction_ratio: token_reduction_ratio(corpus_tokens, graph_query_tokens),
        });

        let total_duration = self.samples.iter().map(|s| s.duration).sum();

        GraphOpsSuiteResult {
            node_count,
            edge_count,
            samples: self.samples.clone(),
            memory,
            token_reduction,
            total_duration,
        }
    }
}

/// CI-scale node count (fast).
pub const CI_NODE_COUNT: usize = 128;
/// Criterion mid scale.
pub const BENCH_NODE_COUNT_1K: usize = 1_000;
/// Criterion large scale (GRAPH-041 target).
pub const BENCH_NODE_COUNT_10K: usize = 10_000;
/// GRAPH-041 acceptance scale (criterion / ignored soak).
pub const BENCH_NODE_COUNT_50K: usize = 50_000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suite_ci_scale_runs() {
        let mut h = GraphOpsHarness::new();
        let result = h.run_suite(CI_NODE_COUNT);
        assert_eq!(result.node_count, CI_NODE_COUNT);
        assert!(result.samples.len() >= 6);
        assert!(result.memory.under_budget);
        assert!(result.sample(GraphOp::Cluster).is_some());
        let tr = result.token_reduction.unwrap();
        assert!(tr.reduction_ratio > 0.0);
    }
}
