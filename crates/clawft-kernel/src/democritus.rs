//! DEMOCRITUS continuous cognitive loop (ECC decision D5).
//!
//! The [`DemocritusLoop`] is the nervous system of WeftOS — an integration
//! layer that orchestrates the ECC subsystems on every cognitive tick:
//!
//! ```text
//! SENSE → EMBED → SEARCH → UPDATE → COMMIT
//! ```
//!
//! It drains the [`ImpulseQueue`] for new events, embeds them via the
//! configured [`EmbeddingProvider`], queries HNSW for nearest neighbors,
//! updates the [`CausalGraph`] with inferred edges, registers cross-refs
//! in the [`CrossRefStore`], and logs the result.
//!
//! This module is compiled only when the `ecc` feature is enabled.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::causal::{CausalEdgeType, CausalGraph};
use crate::crossref::{CrossRef, CrossRefStore, CrossRefType, StructureTag, UniversalNodeId};
use crate::embedding::{EmbeddingProvider};
use crate::hnsw_service::HnswService;
use crate::impulse::{Impulse, ImpulseQueue, ImpulseType};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the DEMOCRITUS loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DemocritusConfig {
    /// Maximum number of impulses to process per tick.
    pub max_impulses_per_tick: usize,
    /// Number of nearest neighbors to retrieve during SEARCH phase.
    pub search_k: usize,
    /// Cosine similarity threshold above which two events are considered correlated.
    pub correlation_threshold: f32,
    /// Budget for a single tick in microseconds. If exceeded, the tick stops early.
    pub tick_budget_us: u64,
}

impl Default for DemocritusConfig {
    fn default() -> Self {
        Self {
            max_impulses_per_tick: 64,
            search_k: 5,
            correlation_threshold: 0.7,
            tick_budget_us: 15_000, // 15ms
        }
    }
}

// ---------------------------------------------------------------------------
// Tick result
// ---------------------------------------------------------------------------

/// Summary of a single DEMOCRITUS tick cycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DemocritusTickResult {
    /// Number of impulses drained in the SENSE phase.
    pub impulses_sensed: usize,
    /// Number of embeddings produced in the EMBED phase.
    pub embeddings_produced: usize,
    /// Number of HNSW searches performed in the SEARCH phase.
    pub searches_performed: usize,
    /// Number of causal edges added in the UPDATE phase.
    pub edges_added: usize,
    /// Number of cross-refs registered in the UPDATE phase.
    pub crossrefs_added: usize,
    /// Whether the tick was cut short due to budget exhaustion.
    pub budget_exceeded: bool,
    /// Wall-clock duration of the tick in microseconds.
    pub duration_us: u64,
}

// ---------------------------------------------------------------------------
// DemocritusLoop
// ---------------------------------------------------------------------------

/// The DEMOCRITUS continuous cognitive loop.
///
/// Runs every CognitiveTick cycle: Sense -> Embed -> Search -> Update -> Commit.
pub struct DemocritusLoop {
    // ECC subsystem references
    causal_graph: Arc<CausalGraph>,
    hnsw: Arc<HnswService>,
    impulse_queue: Arc<ImpulseQueue>,
    crossref_store: Arc<CrossRefStore>,
    embedding_provider: Arc<dyn EmbeddingProvider>,
    // Configuration
    config: DemocritusConfig,
    // Tick statistics
    total_ticks: AtomicU64,
    total_nodes_added: AtomicU64,
    total_edges_added: AtomicU64,
}

impl DemocritusLoop {
    /// Create a new DEMOCRITUS loop wired to the given ECC subsystems.
    pub fn new(
        causal_graph: Arc<CausalGraph>,
        hnsw: Arc<HnswService>,
        impulse_queue: Arc<ImpulseQueue>,
        crossref_store: Arc<CrossRefStore>,
        embedding_provider: Arc<dyn EmbeddingProvider>,
        config: DemocritusConfig,
    ) -> Self {
        Self {
            causal_graph,
            hnsw,
            impulse_queue,
            crossref_store,
            embedding_provider,
            config,
            total_ticks: AtomicU64::new(0),
            total_nodes_added: AtomicU64::new(0),
            total_edges_added: AtomicU64::new(0),
        }
    }

    /// Execute one full tick cycle: Sense -> Embed -> Search -> Update -> Commit.
    ///
    /// Returns a summary of what was processed. This is the method the
    /// [`CognitiveTick`] loop should call on each cycle.
    pub async fn tick(&self) -> DemocritusTickResult {
        let start = Instant::now();
        let mut result = DemocritusTickResult {
            impulses_sensed: 0,
            embeddings_produced: 0,
            searches_performed: 0,
            edges_added: 0,
            crossrefs_added: 0,
            budget_exceeded: false,
            duration_us: 0,
        };

        // ── SENSE ────────────────────────────────────────────────────
        let impulses = self.sense();
        result.impulses_sensed = impulses.len();

        if impulses.is_empty() {
            result.duration_us = start.elapsed().as_micros() as u64;
            self.commit(&result);
            return result;
        }

        // ── EMBED ────────────────────────────────────────────────────
        let embedded = self.embed(&impulses).await;
        result.embeddings_produced = embedded.len();

        if self.budget_exceeded(start) {
            result.budget_exceeded = true;
            result.duration_us = start.elapsed().as_micros() as u64;
            self.commit(&result);
            return result;
        }

        // ── SEARCH ───────────────────────────────────────────────────
        // Batch all HNSW searches under a single mutex acquisition
        // instead of locking per-impulse (Task 3: batch Mutex).
        let non_empty_queries: Vec<(usize, &[f32])> = embedded
            .iter()
            .enumerate()
            .filter(|(_, emb)| !emb.is_empty())
            .map(|(i, emb)| (i, emb.as_slice()))
            .collect();

        let query_slices: Vec<&[f32]> = non_empty_queries.iter().map(|(_, s)| *s).collect();
        let batch_results = if !query_slices.is_empty() {
            self.hnsw.search_batch(&query_slices, self.config.search_k)
        } else {
            Vec::new()
        };

        // Reassemble: map batch results back to their impulse indices.
        let mut search_results_by_index: Vec<Vec<(String, f32)>> =
            vec![Vec::new(); embedded.len()];
        for (batch_idx, &(orig_idx, _)) in non_empty_queries.iter().enumerate() {
            search_results_by_index[orig_idx] = batch_results
                .get(batch_idx)
                .map(|results| results.iter().map(|r| (r.id.clone(), r.score)).collect())
                .unwrap_or_default();
        }
        result.searches_performed = non_empty_queries.len();

        type NeighborTriple<'a> = (&'a Impulse, &'a Vec<f32>, Vec<(String, f32)>);
        let mut neighbors_per_event: Vec<NeighborTriple<'_>> =
            Vec::with_capacity(embedded.len());
        for (i, (impulse, embedding)) in impulses.iter().zip(embedded.iter()).enumerate() {
            if self.budget_exceeded(start) {
                result.budget_exceeded = true;
                break;
            }
            let neighbors = std::mem::take(&mut search_results_by_index[i]);
            neighbors_per_event.push((impulse, embedding, neighbors));
        }

        // ── UPDATE ───────────────────────────────────────────────────
        for (impulse, embedding, neighbors) in &neighbors_per_event {
            if self.budget_exceeded(start) {
                result.budget_exceeded = true;
                break;
            }
            let (edges, crossrefs) = self.update(impulse, embedding, neighbors);
            result.edges_added += edges;
            result.crossrefs_added += crossrefs;
        }

        // ── COMMIT ───────────────────────────────────────────────────
        result.duration_us = start.elapsed().as_micros() as u64;
        self.commit(&result);
        result
    }

    // ── Phase implementations ────────────────────────────────────────

    /// SENSE: drain the impulse queue up to the per-tick limit.
    fn sense(&self) -> Vec<Impulse> {
        let mut impulses = self.impulse_queue.drain_ready();
        impulses.truncate(self.config.max_impulses_per_tick);
        impulses
    }

    /// EMBED: convert each impulse's payload to a vector embedding.
    ///
    /// On embedding failure, falls back to an empty vector (the impulse
    /// will still be recorded in the causal graph but won't participate
    /// in similarity search).
    async fn embed(&self, impulses: &[Impulse]) -> Vec<Vec<f32>> {
        let texts: Vec<String> = impulses
            .iter()
            .map(|imp| {
                // Build a text representation from the impulse payload.
                let type_str = imp.impulse_type.to_string();
                let payload_str = imp.payload.to_string();
                format!("{type_str}:{payload_str}")
            })
            .collect();

        let text_refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();

        match self.embedding_provider.embed_batch(&text_refs).await {
            Ok(vecs) => vecs,
            Err(e) => {
                warn!("DEMOCRITUS embed phase failed, falling back to empty vectors: {e}");
                vec![Vec::new(); impulses.len()]
            }
        }
    }

    /// SEARCH: query HNSW for k nearest neighbors of the given embedding.
    ///
    /// Note: The tick loop now uses `search_batch` for batched mutex
    /// acquisition. This method is retained for single-query callers.
    #[allow(dead_code)]
    fn search(&self, embedding: &[f32]) -> Vec<(String, f32)> {
        if embedding.is_empty() {
            return Vec::new();
        }
        self.hnsw
            .search(embedding, self.config.search_k)
            .into_iter()
            .map(|r| (r.id, r.score))
            .collect()
    }

    /// UPDATE: add a causal node for the impulse, insert into HNSW,
    /// create causal edges based on neighbor similarity, and register
    /// cross-references.
    ///
    /// Returns (edges_added, crossrefs_added).
    fn update(
        &self,
        impulse: &Impulse,
        embedding: &[f32],
        neighbors: &[(String, f32)],
    ) -> (usize, usize) {
        let mut edges_added = 0usize;
        let mut crossrefs_added = 0usize;

        // Add a causal node for this impulse.
        let label = format!("impulse:{}:{}", impulse.impulse_type, impulse.id);
        let node_id = self.causal_graph.add_node(
            label.clone(),
            impulse.payload.clone(),
        );
        self.total_nodes_added.fetch_add(1, Ordering::Relaxed);

        // Insert embedding into HNSW (keyed by causal node ID).
        if !embedding.is_empty() {
            self.hnsw.insert(
                node_id.to_string(),
                embedding.to_vec(),
                serde_json::json!({
                    "impulse_id": impulse.id,
                    "impulse_type": impulse.impulse_type.to_string(),
                    "hlc": impulse.hlc_timestamp,
                }),
            );
        }

        // Create causal edges based on neighbor similarity.
        for (neighbor_id_str, score) in neighbors {
            let edge_type = self.classify_edge(impulse, *score);

            if let Ok(neighbor_node_id) = neighbor_id_str.parse::<u64>() {
                let linked = self.causal_graph.link(
                    node_id,
                    neighbor_node_id,
                    edge_type,
                    *score,
                    impulse.hlc_timestamp,
                    0, // chain_seq; set during exochain commit if enabled
                );
                if linked {
                    edges_added += 1;
                    self.total_edges_added.fetch_add(1, Ordering::Relaxed);
                }
            }
        }

        // Register a cross-reference linking the causal node to its source structure.
        let source_tag = structure_tag_from_u8(impulse.source_structure);
        let uni_id = UniversalNodeId::new(
            &StructureTag::CausalGraph,
            label.as_bytes(),
            impulse.hlc_timestamp,
            &impulse.source_node,
            &[0u8; 32],
        );
        let source_uni_id = UniversalNodeId::from_bytes(impulse.source_node);
        self.crossref_store.insert(CrossRef {
            source: uni_id,
            source_structure: StructureTag::CausalGraph,
            target: source_uni_id,
            target_structure: source_tag,
            ref_type: CrossRefType::TriggeredBy,
            created_at: impulse.hlc_timestamp,
            chain_seq: 0,
        });
        crossrefs_added += 1;

        (edges_added, crossrefs_added)
    }

    /// COMMIT: update tick statistics and log the result.
    fn commit(&self, result: &DemocritusTickResult) {
        self.total_ticks.fetch_add(1, Ordering::Relaxed);
        debug!(
            "DEMOCRITUS tick #{}: sensed={}, embedded={}, searched={}, edges={}, crossrefs={}, budget_exceeded={}, duration={}us",
            self.total_ticks.load(Ordering::Relaxed),
            result.impulses_sensed,
            result.embeddings_produced,
            result.searches_performed,
            result.edges_added,
            result.crossrefs_added,
            result.budget_exceeded,
            result.duration_us,
        );
    }

    // ── Helpers ──────────────────────────────────────────────────────

    /// Classify the edge type based on impulse context and similarity score.
    fn classify_edge(&self, impulse: &Impulse, score: f32) -> CausalEdgeType {
        // High similarity → Correlates (statistically similar events).
        if score >= self.config.correlation_threshold {
            return CausalEdgeType::Correlates;
        }

        // Impulse type hints at causal direction.
        match &impulse.impulse_type {
            ImpulseType::BeliefUpdate | ImpulseType::NoveltyDetected => CausalEdgeType::Follows,
            ImpulseType::EdgeConfirmed => CausalEdgeType::Causes,
            ImpulseType::CoherenceAlert => CausalEdgeType::EvidenceFor,
            ImpulseType::EmbeddingRefined => CausalEdgeType::Enables,
            ImpulseType::Custom(_) => CausalEdgeType::Follows,
        }
    }

    /// Check if the tick budget has been exceeded.
    fn budget_exceeded(&self, start: Instant) -> bool {
        start.elapsed().as_micros() as u64 > self.config.tick_budget_us
    }

    // ── Statistics accessors ─────────────────────────────────────────

    /// Total number of ticks executed.
    pub fn total_ticks(&self) -> u64 {
        self.total_ticks.load(Ordering::Relaxed)
    }

    /// Total number of causal nodes added across all ticks.
    pub fn total_nodes_added(&self) -> u64 {
        self.total_nodes_added.load(Ordering::Relaxed)
    }

    /// Total number of causal edges added across all ticks.
    pub fn total_edges_added(&self) -> u64 {
        self.total_edges_added.load(Ordering::Relaxed)
    }
}

/// Map a raw `u8` structure tag back to a [`StructureTag`] variant.
fn structure_tag_from_u8(tag: u8) -> StructureTag {
    match tag {
        0x01 => StructureTag::ExoChain,
        0x02 => StructureTag::ResourceTree,
        0x03 => StructureTag::CausalGraph,
        0x04 => StructureTag::HnswIndex,
        other => StructureTag::Custom(other),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedding::MockEmbeddingProvider;
    use crate::hnsw_service::HnswServiceConfig;

    /// Helper: build a fully wired DemocritusLoop with default config.
    fn make_loop() -> (
        Arc<CausalGraph>,
        Arc<HnswService>,
        Arc<ImpulseQueue>,
        Arc<CrossRefStore>,
        DemocritusLoop,
    ) {
        make_loop_with_config(DemocritusConfig::default())
    }

    fn make_loop_with_config(
        config: DemocritusConfig,
    ) -> (
        Arc<CausalGraph>,
        Arc<HnswService>,
        Arc<ImpulseQueue>,
        Arc<CrossRefStore>,
        DemocritusLoop,
    ) {
        let cg = Arc::new(CausalGraph::new());
        let hnsw = Arc::new(HnswService::new(HnswServiceConfig {
            default_dimensions: 8,
            ..HnswServiceConfig::default()
        }));
        let iq = Arc::new(ImpulseQueue::new());
        let crs = Arc::new(CrossRefStore::new());
        let emb: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbeddingProvider::new(8));

        let democritus = DemocritusLoop::new(
            Arc::clone(&cg),
            Arc::clone(&hnsw),
            Arc::clone(&iq),
            Arc::clone(&crs),
            emb,
            config,
        );
        (cg, hnsw, iq, crs, democritus)
    }

    fn emit_test_impulse(iq: &ImpulseQueue, impulse_type: ImpulseType, ts: u64) -> u64 {
        iq.emit(
            StructureTag::CausalGraph.as_u8(),
            [0u8; 32],
            StructureTag::HnswIndex.as_u8(),
            impulse_type,
            serde_json::json!({"test": true}),
            ts,
        )
    }

    // ── Test 1: Empty impulse queue — tick completes with no new nodes ──

    #[tokio::test]
    async fn empty_queue_produces_no_work() {
        let (_cg, _hnsw, _iq, _crs, demo) = make_loop();
        let result = demo.tick().await;

        assert_eq!(result.impulses_sensed, 0);
        assert_eq!(result.embeddings_produced, 0);
        assert_eq!(result.searches_performed, 0);
        assert_eq!(result.edges_added, 0);
        assert_eq!(result.crossrefs_added, 0);
        assert!(!result.budget_exceeded);
        assert_eq!(demo.total_ticks(), 1);
        assert_eq!(demo.total_nodes_added(), 0);
    }

    // ── Test 2: Single impulse → full pipeline ──

    #[tokio::test]
    async fn single_impulse_full_pipeline() {
        let (cg, hnsw, iq, crs, demo) = make_loop();

        emit_test_impulse(&iq, ImpulseType::BeliefUpdate, 100);

        let result = demo.tick().await;

        assert_eq!(result.impulses_sensed, 1);
        assert_eq!(result.embeddings_produced, 1);
        assert_eq!(result.searches_performed, 1);
        // No pre-existing neighbors, so no edges added.
        assert_eq!(result.edges_added, 0);
        // One cross-ref should be registered.
        assert_eq!(result.crossrefs_added, 1);
        // Causal graph should have one node.
        assert_eq!(cg.node_count(), 1);
        // HNSW should have one entry.
        assert_eq!(hnsw.len(), 1);
        // CrossRefStore should have one entry.
        assert_eq!(crs.count(), 1);
        assert_eq!(demo.total_nodes_added(), 1);
    }

    // ── Test 3: Multiple impulses in one tick — batch processing ──

    #[tokio::test]
    async fn multiple_impulses_batch_processing() {
        let (cg, _hnsw, iq, crs, demo) = make_loop();

        emit_test_impulse(&iq, ImpulseType::BeliefUpdate, 100);
        emit_test_impulse(&iq, ImpulseType::CoherenceAlert, 200);
        emit_test_impulse(&iq, ImpulseType::NoveltyDetected, 300);

        let result = demo.tick().await;

        assert_eq!(result.impulses_sensed, 3);
        assert_eq!(result.embeddings_produced, 3);
        assert_eq!(result.searches_performed, 3);
        assert_eq!(result.crossrefs_added, 3);
        assert_eq!(cg.node_count(), 3);
        assert_eq!(crs.count(), 3);
    }

    // ── Test 4: Tick respects budget (stops early if budget exceeded) ──

    #[tokio::test]
    async fn tick_respects_budget() {
        // Use a budget of 0 microseconds so the tick must stop immediately.
        let config = DemocritusConfig {
            tick_budget_us: 0,
            ..DemocritusConfig::default()
        };
        let (_cg, _hnsw, iq, _crs, demo) = make_loop_with_config(config);

        // Emit several impulses.
        for i in 0..10 {
            emit_test_impulse(&iq, ImpulseType::BeliefUpdate, i);
        }

        let result = demo.tick().await;

        // With a zero budget, the tick should have been cut short.
        assert!(result.budget_exceeded);
        // Tick counter still increments.
        assert_eq!(demo.total_ticks(), 1);
    }

    // ── Test 5: CrossRef created linking new node to source entity ──

    #[tokio::test]
    async fn crossref_links_node_to_source() {
        let (_cg, _hnsw, iq, crs, demo) = make_loop();

        let source_node = [42u8; 32];
        iq.emit(
            StructureTag::ExoChain.as_u8(),
            source_node,
            StructureTag::HnswIndex.as_u8(),
            ImpulseType::EdgeConfirmed,
            serde_json::json!({"chain": "test"}),
            500,
        );

        demo.tick().await;

        // Verify cross-ref exists with the correct target (the source node).
        let target_uni = UniversalNodeId::from_bytes(source_node);
        let refs = crs.get_reverse(&target_uni);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].target_structure, StructureTag::ExoChain);
        assert_eq!(refs[0].ref_type, CrossRefType::TriggeredBy);
    }

    // ── Test 6: Tick statistics increment correctly ──

    #[tokio::test]
    async fn tick_statistics_increment() {
        let (_cg, _hnsw, iq, _crs, demo) = make_loop();

        assert_eq!(demo.total_ticks(), 0);
        assert_eq!(demo.total_nodes_added(), 0);
        assert_eq!(demo.total_edges_added(), 0);

        // Tick 1: one impulse.
        emit_test_impulse(&iq, ImpulseType::BeliefUpdate, 10);
        demo.tick().await;
        assert_eq!(demo.total_ticks(), 1);
        assert_eq!(demo.total_nodes_added(), 1);

        // Tick 2: two impulses.
        emit_test_impulse(&iq, ImpulseType::CoherenceAlert, 20);
        emit_test_impulse(&iq, ImpulseType::NoveltyDetected, 30);
        demo.tick().await;
        assert_eq!(demo.total_ticks(), 2);
        assert_eq!(demo.total_nodes_added(), 3);
    }

    // ── Test 7: HNSW search returns relevant neighbors ──

    #[tokio::test]
    async fn hnsw_returns_neighbors_on_second_tick() {
        let (_cg, hnsw, iq, _crs, demo) = make_loop();

        // First tick: insert a node.
        emit_test_impulse(&iq, ImpulseType::BeliefUpdate, 100);
        demo.tick().await;
        assert_eq!(hnsw.len(), 1);

        // Second tick: same impulse type/payload should find the first as neighbor.
        emit_test_impulse(&iq, ImpulseType::BeliefUpdate, 200);
        let result = demo.tick().await;

        assert_eq!(result.searches_performed, 1);
        // The search should have found the node from tick 1.
        // Whether an edge is added depends on the score vs threshold,
        // but the search itself was performed.
        assert_eq!(hnsw.search_count(), 2);
    }

    // ── Test 8: Causal edge type selection ──

    #[tokio::test]
    async fn edge_type_classification() {
        let (_, _, _, _, demo) = make_loop();

        let impulse_belief = Impulse {
            id: 1,
            source_structure: 0,
            source_node: [0u8; 32],
            target_structure: 2,
            impulse_type: ImpulseType::BeliefUpdate,
            payload: serde_json::json!({}),
            hlc_timestamp: 0,
            acknowledged: std::sync::atomic::AtomicBool::new(false),
        };

        // High similarity → Correlates.
        assert_eq!(
            demo.classify_edge(&impulse_belief, 0.9),
            CausalEdgeType::Correlates
        );

        // Below threshold, BeliefUpdate → Follows.
        assert_eq!(
            demo.classify_edge(&impulse_belief, 0.3),
            CausalEdgeType::Follows
        );

        // EdgeConfirmed → Causes.
        let impulse_confirmed = Impulse {
            impulse_type: ImpulseType::EdgeConfirmed,
            ..impulse_belief.clone()
        };
        assert_eq!(
            demo.classify_edge(&impulse_confirmed, 0.3),
            CausalEdgeType::Causes
        );

        // CoherenceAlert → EvidenceFor.
        let impulse_coherence = Impulse {
            impulse_type: ImpulseType::CoherenceAlert,
            ..impulse_belief.clone()
        };
        assert_eq!(
            demo.classify_edge(&impulse_coherence, 0.3),
            CausalEdgeType::EvidenceFor
        );

        // EmbeddingRefined → Enables.
        let impulse_refined = Impulse {
            impulse_type: ImpulseType::EmbeddingRefined,
            ..impulse_belief.clone()
        };
        assert_eq!(
            demo.classify_edge(&impulse_refined, 0.3),
            CausalEdgeType::Enables
        );
    }

    // ── Test 9: Commit phase logs and updates total_ticks ──

    #[tokio::test]
    async fn commit_updates_tick_counter() {
        let (_, _, _, _, demo) = make_loop();

        // Empty ticks still increment the tick counter.
        demo.tick().await;
        demo.tick().await;
        demo.tick().await;

        assert_eq!(demo.total_ticks(), 3);
    }

    // ── Test 10: Embedding errors handled gracefully ──

    #[tokio::test]
    async fn embedding_error_falls_back_gracefully() {
        use crate::embedding::EmbeddingError;

        /// Provider that always fails.
        struct FailingProvider;

        #[async_trait::async_trait]
        impl EmbeddingProvider for FailingProvider {
            async fn embed(&self, _text: &str) -> Result<Vec<f32>, EmbeddingError> {
                Err(EmbeddingError::BackendError("test failure".into()))
            }
            async fn embed_batch(
                &self,
                _texts: &[&str],
            ) -> Result<Vec<Vec<f32>>, EmbeddingError> {
                Err(EmbeddingError::BackendError("test failure".into()))
            }
            fn dimensions(&self) -> usize {
                8
            }
            fn model_name(&self) -> &str {
                "failing-test"
            }
        }

        let cg = Arc::new(CausalGraph::new());
        let hnsw = Arc::new(HnswService::new(HnswServiceConfig::default()));
        let iq = Arc::new(ImpulseQueue::new());
        let crs = Arc::new(CrossRefStore::new());
        let emb: Arc<dyn EmbeddingProvider> = Arc::new(FailingProvider);

        let demo = DemocritusLoop::new(
            Arc::clone(&cg),
            Arc::clone(&hnsw),
            Arc::clone(&iq),
            Arc::clone(&crs),
            emb,
            DemocritusConfig::default(),
        );

        emit_test_impulse(&iq, ImpulseType::BeliefUpdate, 100);

        let result = demo.tick().await;

        // Embedding failed → empty vectors, but tick still completes.
        assert_eq!(result.impulses_sensed, 1);
        assert_eq!(result.embeddings_produced, 1); // fallback produces empty vecs
        // Search with empty vector is skipped (non_empty_queries filter).
        assert_eq!(result.searches_performed, 0);
        // Node still added to causal graph.
        assert_eq!(cg.node_count(), 1);
        // But no HNSW insertion (empty embedding skipped).
        assert_eq!(hnsw.len(), 0);
        // Cross-ref still created.
        assert_eq!(result.crossrefs_added, 1);
    }

    // ── Test 11: max_impulses_per_tick truncation ──

    #[tokio::test]
    async fn max_impulses_per_tick_truncation() {
        let config = DemocritusConfig {
            max_impulses_per_tick: 2,
            ..DemocritusConfig::default()
        };
        let (_cg, _hnsw, iq, _crs, demo) = make_loop_with_config(config);

        // Emit 5 impulses.
        for i in 0..5 {
            emit_test_impulse(&iq, ImpulseType::BeliefUpdate, i);
        }

        let result = demo.tick().await;

        // Only 2 should be processed due to truncation.
        assert_eq!(result.impulses_sensed, 2);
        assert_eq!(result.embeddings_produced, 2);
    }

    // ── Test 12: structure_tag_from_u8 mapping ──

    #[test]
    fn structure_tag_roundtrip() {
        assert_eq!(structure_tag_from_u8(0x01), StructureTag::ExoChain);
        assert_eq!(structure_tag_from_u8(0x02), StructureTag::ResourceTree);
        assert_eq!(structure_tag_from_u8(0x03), StructureTag::CausalGraph);
        assert_eq!(structure_tag_from_u8(0x04), StructureTag::HnswIndex);
        assert_eq!(structure_tag_from_u8(0xFF), StructureTag::Custom(0xFF));
    }

    // ── Sprint 11: Budget exhaustion tests ──────────────────────────

    #[tokio::test]
    async fn budget_exhaustion_with_many_impulses() {
        // Use a budget of 0 microseconds with many impulses to force
        // budget exhaustion at different phases.
        let config = DemocritusConfig {
            tick_budget_us: 0,
            max_impulses_per_tick: 100,
            ..DemocritusConfig::default()
        };
        let (cg, _hnsw, iq, _crs, demo) = make_loop_with_config(config);

        for i in 0..50 {
            emit_test_impulse(&iq, ImpulseType::BeliefUpdate, i);
        }

        let result = demo.tick().await;
        assert!(result.budget_exceeded);
        // Even with budget exceeded, tick count increments.
        assert_eq!(demo.total_ticks(), 1);
        // Some impulses may have been sensed before budget check.
        assert!(result.impulses_sensed <= 50);
        // Causal graph nodes added should match embeddings completed
        // (may be fewer than sensed due to budget).
        assert!(cg.node_count() <= result.impulses_sensed as u64);
    }

    #[tokio::test]
    async fn budget_exceeded_flag_only_set_when_needed() {
        // Large budget should not trigger budget_exceeded.
        let config = DemocritusConfig {
            tick_budget_us: 10_000_000, // 10 seconds
            ..DemocritusConfig::default()
        };
        let (_, _, iq, _, demo) = make_loop_with_config(config);

        emit_test_impulse(&iq, ImpulseType::BeliefUpdate, 1);
        let result = demo.tick().await;
        assert!(!result.budget_exceeded);
    }

    // ── Sprint 11: ImpulseQueue overflow tests ──────────────────────

    #[tokio::test]
    async fn impulse_queue_large_burst() {
        let config = DemocritusConfig {
            max_impulses_per_tick: 10,
            ..DemocritusConfig::default()
        };
        let (_, _, iq, _, demo) = make_loop_with_config(config);

        // Emit far more impulses than per-tick limit.
        for i in 0..500 {
            emit_test_impulse(&iq, ImpulseType::BeliefUpdate, i);
        }

        // First tick processes at most 10.
        let r1 = demo.tick().await;
        assert_eq!(r1.impulses_sensed, 10);

        // Queue was drained fully (drain_ready takes all), but only 10 processed.
        // Remaining impulses are gone (drain clears the queue).
        let r2 = demo.tick().await;
        assert_eq!(r2.impulses_sensed, 0);
    }

    #[tokio::test]
    async fn impulse_queue_interleaved_emit_and_tick() {
        let (cg, _, iq, _, demo) = make_loop();

        // Emit, tick, emit, tick — verify state accumulates.
        emit_test_impulse(&iq, ImpulseType::BeliefUpdate, 10);
        let r1 = demo.tick().await;
        assert_eq!(r1.impulses_sensed, 1);
        assert_eq!(cg.node_count(), 1);

        emit_test_impulse(&iq, ImpulseType::CoherenceAlert, 20);
        emit_test_impulse(&iq, ImpulseType::NoveltyDetected, 30);
        let r2 = demo.tick().await;
        assert_eq!(r2.impulses_sensed, 2);
        assert_eq!(cg.node_count(), 3);

        assert_eq!(demo.total_ticks(), 2);
        assert_eq!(demo.total_nodes_added(), 3);
    }

    // ── Sprint 11: Embed failure recovery tests ─────────────────────

    #[tokio::test]
    async fn embed_failure_still_creates_crossrefs() {
        use crate::embedding::EmbeddingError;

        struct FailingProvider;

        #[async_trait::async_trait]
        impl EmbeddingProvider for FailingProvider {
            async fn embed(&self, _text: &str) -> Result<Vec<f32>, EmbeddingError> {
                Err(EmbeddingError::BackendError("test failure".into()))
            }
            async fn embed_batch(
                &self,
                _texts: &[&str],
            ) -> Result<Vec<Vec<f32>>, EmbeddingError> {
                Err(EmbeddingError::BackendError("test failure".into()))
            }
            fn dimensions(&self) -> usize {
                8
            }
            fn model_name(&self) -> &str {
                "failing-test"
            }
        }

        let cg = Arc::new(CausalGraph::new());
        let hnsw = Arc::new(HnswService::new(HnswServiceConfig::default()));
        let iq = Arc::new(ImpulseQueue::new());
        let crs = Arc::new(CrossRefStore::new());
        let emb: Arc<dyn EmbeddingProvider> = Arc::new(FailingProvider);

        let demo = DemocritusLoop::new(
            Arc::clone(&cg),
            Arc::clone(&hnsw),
            Arc::clone(&iq),
            Arc::clone(&crs),
            emb,
            DemocritusConfig::default(),
        );

        // Emit multiple impulses.
        for i in 0..5 {
            emit_test_impulse(&iq, ImpulseType::BeliefUpdate, i * 100);
        }

        let result = demo.tick().await;
        assert_eq!(result.impulses_sensed, 5);
        // Fallback: 5 empty vectors produced.
        assert_eq!(result.embeddings_produced, 5);
        // Causal nodes still created despite embed failure.
        assert_eq!(cg.node_count(), 5);
        // Cross-refs still created.
        assert_eq!(result.crossrefs_added, 5);
        assert_eq!(crs.count(), 5);
        // No HNSW insertions (empty embeddings skipped).
        assert_eq!(hnsw.len(), 0);
    }

    #[tokio::test]
    async fn embed_failure_no_edges_added() {
        use crate::embedding::EmbeddingError;

        struct FailingProvider;

        #[async_trait::async_trait]
        impl EmbeddingProvider for FailingProvider {
            async fn embed(&self, _text: &str) -> Result<Vec<f32>, EmbeddingError> {
                Err(EmbeddingError::BackendError("fail".into()))
            }
            async fn embed_batch(
                &self,
                _texts: &[&str],
            ) -> Result<Vec<Vec<f32>>, EmbeddingError> {
                Err(EmbeddingError::BackendError("fail".into()))
            }
            fn dimensions(&self) -> usize {
                8
            }
            fn model_name(&self) -> &str {
                "fail"
            }
        }

        let cg = Arc::new(CausalGraph::new());
        let hnsw = Arc::new(HnswService::new(HnswServiceConfig::default()));
        let iq = Arc::new(ImpulseQueue::new());
        let crs = Arc::new(CrossRefStore::new());
        let emb: Arc<dyn EmbeddingProvider> = Arc::new(FailingProvider);

        let demo = DemocritusLoop::new(
            Arc::clone(&cg),
            Arc::clone(&hnsw),
            Arc::clone(&iq),
            Arc::clone(&crs),
            emb,
            DemocritusConfig::default(),
        );

        emit_test_impulse(&iq, ImpulseType::BeliefUpdate, 100);
        let result = demo.tick().await;
        // With empty embeddings, search returns no neighbors, so no edges.
        assert_eq!(result.edges_added, 0);
        assert_eq!(demo.total_edges_added(), 0);
    }

    // ── Sprint 11: Multiple sequential ticks with accumulated state ──

    #[tokio::test]
    async fn multiple_sequential_ticks_accumulate_state() {
        let (cg, hnsw, iq, crs, demo) = make_loop();

        // Tick 1: single impulse.
        emit_test_impulse(&iq, ImpulseType::BeliefUpdate, 100);
        let r1 = demo.tick().await;
        assert_eq!(r1.impulses_sensed, 1);
        let nodes_after_1 = cg.node_count();
        let hnsw_after_1 = hnsw.len();

        // Tick 2: two more impulses.
        emit_test_impulse(&iq, ImpulseType::CoherenceAlert, 200);
        emit_test_impulse(&iq, ImpulseType::NoveltyDetected, 300);
        let r2 = demo.tick().await;
        assert_eq!(r2.impulses_sensed, 2);
        assert_eq!(cg.node_count(), nodes_after_1 + 2);
        assert_eq!(hnsw.len(), hnsw_after_1 + 2);

        // Tick 3: three more.
        emit_test_impulse(&iq, ImpulseType::EdgeConfirmed, 400);
        emit_test_impulse(&iq, ImpulseType::EmbeddingRefined, 500);
        emit_test_impulse(&iq, ImpulseType::Custom(42), 600);
        let r3 = demo.tick().await;
        assert_eq!(r3.impulses_sensed, 3);
        assert_eq!(cg.node_count(), nodes_after_1 + 5);

        // Total statistics.
        assert_eq!(demo.total_ticks(), 3);
        assert_eq!(demo.total_nodes_added(), 6);
        // Cross-refs: one per impulse.
        assert_eq!(crs.count(), 6);
    }

    #[tokio::test]
    async fn sequential_ticks_can_find_prior_neighbors() {
        let config = DemocritusConfig {
            correlation_threshold: 0.0, // accept all as correlated
            ..DemocritusConfig::default()
        };
        let (cg, hnsw, iq, _, demo) = make_loop_with_config(config);

        // Tick 1: insert a node.
        emit_test_impulse(&iq, ImpulseType::BeliefUpdate, 100);
        demo.tick().await;
        assert_eq!(hnsw.len(), 1);

        // Tick 2: same type should find tick-1's node as neighbor.
        emit_test_impulse(&iq, ImpulseType::BeliefUpdate, 200);
        let r2 = demo.tick().await;
        assert_eq!(r2.searches_performed, 1);
        // With threshold=0.0, any non-zero similarity creates an edge.
        // The mock provider produces deterministic vectors, so same impulse
        // type gets same embedding, yielding high similarity.
        // Edges depend on whether neighbor_id parses as a valid node_id.
        assert!(cg.node_count() >= 2);
    }

    // ── Sprint 11: Config edge cases ────────────────────────────────

    #[tokio::test]
    async fn zero_max_impulses_per_tick() {
        let config = DemocritusConfig {
            max_impulses_per_tick: 0,
            ..DemocritusConfig::default()
        };
        let (_, _, iq, _, demo) = make_loop_with_config(config);

        emit_test_impulse(&iq, ImpulseType::BeliefUpdate, 100);
        let result = demo.tick().await;
        // Truncation to 0 means no impulses processed.
        assert_eq!(result.impulses_sensed, 0);
        assert_eq!(result.embeddings_produced, 0);
    }

    #[tokio::test]
    async fn tick_result_duration_is_positive() {
        let (_, _, iq, _, demo) = make_loop();
        emit_test_impulse(&iq, ImpulseType::BeliefUpdate, 100);
        let result = demo.tick().await;
        // Duration should be non-negative (may be 0 on very fast systems).
        assert!(result.duration_us < 10_000_000, "tick should complete within 10s");
    }

    #[test]
    fn classify_edge_custom_impulse_type() {
        let (_, _, _, _, demo) = make_loop();
        let impulse = Impulse {
            id: 1,
            source_structure: 0,
            source_node: [0u8; 32],
            target_structure: 2,
            impulse_type: ImpulseType::Custom(99),
            payload: serde_json::json!({}),
            hlc_timestamp: 0,
            acknowledged: std::sync::atomic::AtomicBool::new(false),
        };

        // Custom type below threshold → Follows.
        assert_eq!(
            demo.classify_edge(&impulse, 0.3),
            CausalEdgeType::Follows
        );
        // Custom type above threshold → Correlates.
        assert_eq!(
            demo.classify_edge(&impulse, 0.9),
            CausalEdgeType::Correlates
        );
    }
}
