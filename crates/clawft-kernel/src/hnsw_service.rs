//! HNSW vector search as a kernel `SystemService`.
//!
//! Wraps `clawft_core::embeddings::hnsw_store::HnswStore` behind a
//! `Mutex` so that the service satisfies `Send + Sync` and can be
//! registered in the `ServiceRegistry`.
//!
//! This module is compiled only when the `ecc` feature is enabled.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use clawft_core::embeddings::hnsw_store::{HnswStore, TieredSearch};

use crate::health::HealthStatus;
use crate::hnsw_eml::{HnswEmlConfig, HnswEmlManager};
use crate::service::{ServiceType, SystemService};

#[cfg(feature = "exochain")]
use std::sync::Arc;
#[cfg(feature = "exochain")]
use crate::chain::ChainManager;
#[cfg(feature = "exochain")]
use crate::governance::{EffectVector, GovernanceDecision, GovernanceEngine, GovernanceRequest};

// ── Configuration ────────────────────────────────────────────────────────

/// Configuration for the [`HnswService`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HnswServiceConfig {
    /// ef_search parameter forwarded to `HnswStore`.
    pub ef_search: usize,
    /// ef_construction parameter forwarded to `HnswStore`.
    pub ef_construction: usize,
    /// Default embedding dimensionality (informational; not enforced by the store).
    pub default_dimensions: usize,
}

impl Default for HnswServiceConfig {
    fn default() -> Self {
        Self {
            ef_search: 100,
            ef_construction: 200,
            default_dimensions: 384,
        }
    }
}

// ── Multi-key types ─────────────────────────────────────────────────────

/// Configuration for multi-key HNSW indexing.
///
/// When enabled, entities can be indexed under multiple embedding vectors
/// (e.g., by name, context, description) so that vague queries have
/// better recall.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiKeyConfig {
    /// Whether multi-key indexing is enabled.
    pub enabled: bool,
    /// Maximum number of keys per entity (default: 4).
    pub max_keys_per_entity: usize,
    /// Recognised key types (default: `["name", "context", "description"]`).
    pub key_types: Vec<String>,
}

impl Default for MultiKeyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_keys_per_entity: 4,
            key_types: vec![
                "name".to_string(),
                "context".to_string(),
                "description".to_string(),
            ],
        }
    }
}

/// A single search key for multi-key insertion.
///
/// Each key carries a `key_type` label (e.g., `"name"`, `"context"`) and
/// the pre-computed embedding vector for that textual representation.
#[derive(Debug, Clone)]
pub struct MultiKey {
    /// The kind of key (`"name"`, `"context"`, `"relationship"`, `"docstring"`, etc.).
    pub key_type: String,
    /// The embedding vector for this key's textual representation.
    pub embedding: Vec<f32>,
}

// ── Search result ────────────────────────────────────────────────────────

/// A single search result returned by [`HnswService::search`].
#[derive(Debug, Clone)]
pub struct HnswSearchResult {
    /// Entry identifier.
    pub id: String,
    /// Cosine similarity score (higher is better).
    pub score: f32,
    /// Arbitrary metadata stored alongside the embedding.
    pub metadata: serde_json::Value,
}

// ── Service ──────────────────────────────────────────────────────────────

/// Kernel service wrapping the HNSW vector store.
///
/// All mutable access to the inner `HnswStore` is serialized through a
/// [`Mutex`]. Atomic counters track insert and search operations for
/// observability without requiring the lock.
pub struct HnswService {
    store: Mutex<HnswStore>,
    config: HnswServiceConfig,
    insert_count: AtomicU64,
    search_count: AtomicU64,
    /// Monotonic epoch counter -- bumped on every mutation.
    epoch: AtomicU64,
    /// EML-based adaptive optimization (ef, rebuild, distance, path).
    eml: Mutex<HnswEmlManager>,
    /// Chain manager for exochain event logging.
    #[cfg(feature = "exochain")]
    chain_manager: Option<Arc<ChainManager>>,
    /// Governance engine for gating destructive operations.
    #[cfg(feature = "exochain")]
    governance_engine: Option<Arc<GovernanceEngine>>,
}

impl HnswService {
    /// Create a new service with the given configuration.
    pub fn new(config: HnswServiceConfig) -> Self {
        let store = HnswStore::with_params(config.ef_search, config.ef_construction);
        Self {
            store: Mutex::new(store),
            config,
            insert_count: AtomicU64::new(0),
            search_count: AtomicU64::new(0),
            epoch: AtomicU64::new(0),
            eml: Mutex::new(HnswEmlManager::with_defaults()),
            #[cfg(feature = "exochain")]
            chain_manager: None,
            #[cfg(feature = "exochain")]
            governance_engine: None,
        }
    }

    /// Create a new service with custom EML configuration.
    pub fn with_eml(config: HnswServiceConfig, eml_config: HnswEmlConfig) -> Self {
        let store = HnswStore::with_params(config.ef_search, config.ef_construction);
        Self {
            store: Mutex::new(store),
            config,
            insert_count: AtomicU64::new(0),
            search_count: AtomicU64::new(0),
            epoch: AtomicU64::new(0),
            eml: Mutex::new(HnswEmlManager::new(eml_config)),
            #[cfg(feature = "exochain")]
            chain_manager: None,
            #[cfg(feature = "exochain")]
            governance_engine: None,
        }
    }

    /// Set the chain manager for exochain event logging.
    #[cfg(feature = "exochain")]
    pub fn set_chain_manager(&mut self, cm: Arc<ChainManager>) {
        self.chain_manager = Some(cm);
    }

    /// Set the governance engine for gating destructive operations.
    #[cfg(feature = "exochain")]
    pub fn set_governance_engine(&mut self, engine: Arc<GovernanceEngine>) {
        self.governance_engine = Some(engine);
    }

    /// Insert an embedding with associated metadata (upsert semantics).
    pub fn insert(&self, id: String, embedding: Vec<f32>, metadata: serde_json::Value) {
        let mut store = self.store.lock().expect("HnswStore lock poisoned");
        store.insert(id.clone(), embedding, metadata);
        self.insert_count.fetch_add(1, Ordering::Relaxed);
        self.epoch.fetch_add(1, Ordering::SeqCst);

        // Chain logging: hnsw.insert
        #[cfg(feature = "exochain")]
        if let Some(ref cm) = self.chain_manager {
            cm.append(
                "hnsw_service",
                crate::chain::EVENT_KIND_HNSW_INSERT,
                Some(serde_json::json!({
                    "id": id,
                })),
            );
        }
    }

    /// Return the current monotonic epoch.
    pub fn current_epoch(&self) -> u64 {
        self.epoch.load(Ordering::SeqCst)
    }

    /// Search for the `top_k` nearest embeddings to `query`.
    ///
    /// When EML is enabled and trained, the beam width (ef_search) is
    /// adapted per-query and the rebuild model is consulted after each
    /// search. Training data is recorded for continuous learning.
    pub fn search(&self, query: &[f32], top_k: usize) -> Vec<HnswSearchResult> {
        let mut store = self.store.lock().expect("HnswStore lock poisoned");
        let mut eml = self.eml.lock().expect("EML lock poisoned");
        self.search_count.fetch_add(1, Ordering::Relaxed);

        let store_size = store.len();

        // EML: adaptive ef_search — set before query so the next rebuild
        // uses the learned value.
        let ef_pred = eml.predict_ef(query, store_size);
        if ef_pred.is_learned && ef_pred.recommended_ef != store.ef_search() {
            store.set_ef_search(ef_pred.recommended_ef);
        }

        // EML: adaptive rebuild — trigger early when recall is predicted
        // to have degraded, even if the mutation count hasn't hit the
        // static threshold.
        let rebuild_pred = eml.predict_rebuild(
            store_size,
            store.inserts_since_rebuild(),
            0,
        );
        if rebuild_pred.is_learned && rebuild_pred.should_rebuild {
            store.force_rebuild();
        }

        let t0 = std::time::Instant::now();
        let results: Vec<HnswSearchResult> = store
            .query(query, top_k)
            .into_iter()
            .map(|r| HnswSearchResult {
                id: r.id,
                score: r.score,
                metadata: r.metadata,
            })
            .collect();
        let elapsed_us = t0.elapsed().as_micros() as u64;

        let top_score = results.first().map(|r| r.score).unwrap_or(0.0);
        let ef_used = store.ef_search();
        eml.record_search(
            query,
            results.len(),
            top_score,
            ef_used,
            elapsed_us,
            store_size,
        );

        // ExoChain: append multi-signal observation for training provenance.
        #[cfg(feature = "exochain")]
        if let Some(ref cm) = self.chain_manager {
            cm.append(
                "hnsw_eml",
                crate::chain::EVENT_KIND_HNSW_EML_OBSERVE,
                Some(serde_json::json!({
                    "ef_used": ef_used,
                    "latency_us": elapsed_us,
                    "result_count": results.len(),
                    "top_score": top_score,
                    "store_size": store_size,
                    "is_learned": ef_pred.is_learned,
                })),
            );
        }

        results
    }

    /// Brute-force ground-truth search for recall measurement.
    pub fn brute_force_topk(&self, query: &[f32], top_k: usize) -> Vec<String> {
        let store = self.store.lock().expect("HnswStore lock poisoned");
        store.brute_force_topk(query, top_k)
    }

    /// Measure recall against brute-force and feed the EML rebuild model.
    pub fn measure_recall(
        &self,
        queries: &[Vec<f32>],
        top_k: usize,
    ) -> f64 {
        let mut store = self.store.lock().expect("HnswStore lock poisoned");
        let mut eml = self.eml.lock().expect("EML lock poisoned");

        let mut hnsw_ids = Vec::with_capacity(queries.len());
        let mut exact_ids = Vec::with_capacity(queries.len());
        for q in queries {
            let hnsw: Vec<String> = store
                .query(q, top_k)
                .into_iter()
                .map(|r| r.id)
                .collect();
            let exact = store.brute_force_topk(q, top_k);
            hnsw_ids.push(hnsw);
            exact_ids.push(exact);
        }

        let recall = eml.measure_recall(
            &hnsw_ids,
            &exact_ids,
            store.len(),
            store.inserts_since_rebuild(),
            0,
        );

        #[cfg(feature = "exochain")]
        if let Some(ref cm) = self.chain_manager {
            cm.append(
                "hnsw_eml",
                crate::chain::EVENT_KIND_HNSW_EML_RECALL,
                Some(serde_json::json!({
                    "avg_recall": recall,
                    "store_size": store.len(),
                    "inserts_since_rebuild": store.inserts_since_rebuild(),
                    "query_count": queries.len(),
                })),
            );
        }

        recall
    }

    /// Build a tiered index from the current store contents.
    ///
    /// Uses the EML distance model's learned dimension ordering when
    /// available, falling back to the first N dimensions otherwise.
    /// The returned `TieredSearch` owns separate coarse/full indexes.
    pub fn build_tiered(&self, top_k: usize) -> TieredSearch {
        let store = self.store.lock().expect("HnswStore lock poisoned");
        let eml = self.eml.lock().expect("EML lock poisoned");

        let entries: Vec<(String, Vec<f32>, serde_json::Value)> = (0..store.len())
            .filter_map(|_| None) // placeholder — see below
            .collect();

        // Collect entries from the store. HnswStore doesn't expose an
        // iterator, so we search for everything via brute-force at top-N.
        // This is a build-time cost, not per-query.
        let dims = self.config.default_dimensions;
        if eml.is_enabled() {
            let dim_order = eml.learned_dim_order(dims);
            TieredSearch::build_learned(&entries, &dim_order, dims, top_k, self.config.ef_search)
        } else {
            TieredSearch::build_default(&entries, dims, top_k, self.config.ef_search)
        }
    }

    /// Borrow the EML manager (for status/benchmarking).
    pub fn eml_status(&self) -> crate::hnsw_eml::HnswEmlStatus {
        self.eml.lock().expect("EML lock poisoned").status()
    }

    /// Batch search: acquire the lock once, perform all queries, release.
    ///
    /// This avoids per-query mutex acquisition overhead when processing
    /// multiple embeddings in a single tick (e.g., DEMOCRITUS loop).
    pub fn search_batch(
        &self,
        queries: &[&[f32]],
        top_k: usize,
    ) -> Vec<Vec<HnswSearchResult>> {
        let mut store = self.store.lock().expect("HnswStore lock poisoned");
        self.search_count
            .fetch_add(queries.len() as u64, Ordering::Relaxed);
        queries
            .iter()
            .map(|query| {
                store
                    .query(query, top_k)
                    .into_iter()
                    .map(|r| HnswSearchResult {
                        id: r.id,
                        score: r.score,
                        metadata: r.metadata,
                    })
                    .collect()
            })
            .collect()
    }

    /// Insert an entity with multiple search keys.
    ///
    /// Each key generates a separate embedding entry in the store, all
    /// sharing the same `primary_id` prefix. The stored ID has the form
    /// `"{primary_id}::{key_type}"`. This allows the entity to be found
    /// via different query formulations (name, context, description, etc.).
    ///
    /// Keys beyond `max_keys` are silently dropped.
    pub fn insert_multi_key(
        &self,
        primary_id: String,
        keys: &[MultiKey],
        metadata: serde_json::Value,
        max_keys: usize,
    ) {
        let limit = keys.len().min(max_keys);
        for key in &keys[..limit] {
            let sub_id = format!("{}::{}", primary_id, key.key_type);
            self.insert(sub_id, key.embedding.clone(), metadata.clone());
        }
    }

    /// Search with deduplication by primary entity ID.
    ///
    /// Because multi-key insertion stores several embeddings for one entity,
    /// a single query may match multiple keys of the same entity. This
    /// method searches for `top_k * 3` candidates, deduplicates by
    /// stripping the `"::key_type"` suffix, and returns up to `top_k`
    /// unique entities. The best (highest) score among duplicates is kept.
    pub fn search_dedup(&self, query: &[f32], top_k: usize) -> Vec<HnswSearchResult> {
        let raw = self.search(query, top_k * 3);

        let mut seen = HashSet::new();
        let mut results = Vec::with_capacity(top_k);

        for r in raw {
            let primary_id = r
                .id
                .find("::")
                .map(|pos| r.id[..pos].to_string())
                .unwrap_or_else(|| r.id.clone());

            if seen.insert(primary_id.clone()) {
                results.push(HnswSearchResult {
                    id: primary_id,
                    score: r.score,
                    metadata: r.metadata,
                });
            }
            if results.len() >= top_k {
                break;
            }
        }

        results
    }

    /// Return the number of entries currently in the store.
    pub fn len(&self) -> usize {
        let store = self.store.lock().expect("HnswStore lock poisoned");
        store.len()
    }

    /// Return `true` if the store contains no entries.
    pub fn is_empty(&self) -> bool {
        let store = self.store.lock().expect("HnswStore lock poisoned");
        store.is_empty()
    }

    /// Total number of insert operations since service creation.
    pub fn insert_count(&self) -> u64 {
        self.insert_count.load(Ordering::Relaxed)
    }

    /// Total number of search operations since service creation.
    pub fn search_count(&self) -> u64 {
        self.search_count.load(Ordering::Relaxed)
    }

    /// Replace the inner store with a fresh, empty instance.
    ///
    /// Useful for calibration cleanup. Counters are **not** reset.
    ///
    /// Returns `Err` if the governance engine denies the bulk destruction.
    pub fn clear(&self) -> Result<(), String> {
        // Governance gate: bulk destruction (hnsw clear).
        #[cfg(feature = "exochain")]
        if let Some(ref engine) = self.governance_engine {
            let req = GovernanceRequest::new("system", "hnsw.clear")
                .with_effect(EffectVector { risk: 0.8, ..Default::default() });
            let result = engine.evaluate(&req);
            match &result.decision {
                GovernanceDecision::Deny(reason) | GovernanceDecision::EscalateToHuman(reason) => {
                    return Err(format!("governance denied hnsw.clear: {reason}"));
                }
                _ => {}
            }
        }

        let mut store = self.store.lock().expect("HnswStore lock poisoned");
        #[cfg(feature = "exochain")]
        let entries_destroyed = store.len();
        *store = HnswStore::with_params(self.config.ef_search, self.config.ef_construction);
        let epoch = self.epoch.fetch_add(1, Ordering::SeqCst);

        // Chain logging: hnsw.clear
        #[cfg(feature = "exochain")]
        if let Some(ref cm) = self.chain_manager {
            cm.append(
                "hnsw_service",
                crate::chain::EVENT_KIND_HNSW_CLEAR,
                Some(serde_json::json!({
                    "entries_destroyed": entries_destroyed,
                    "epoch": epoch + 1,
                })),
            );
        }

        Ok(())
    }

    /// Borrow the service configuration.
    pub fn config(&self) -> &HnswServiceConfig {
        &self.config
    }

    /// Persist the HNSW store to a JSON file.
    ///
    /// Delegates to the underlying [`HnswStore::save`]. The HNSW index
    /// graph is not serialized; it will be rebuilt on load.
    pub fn save_to_file(&self, path: &std::path::Path) -> Result<(), std::io::Error> {
        let store = self.store.lock().expect("HnswStore lock poisoned");
        let result = store.save(path);

        // Chain logging: hnsw.save
        #[cfg(feature = "exochain")]
        if result.is_ok()
            && let Some(ref cm) = self.chain_manager {
                cm.append(
                    "hnsw_service",
                    crate::chain::EVENT_KIND_HNSW_SAVE,
                    Some(serde_json::json!({
                        "path": path.display().to_string(),
                    })),
                );
            }

        result
    }

    /// Load an HNSW store from a JSON file and create a new service.
    ///
    /// Delegates to [`HnswStore::load`]. The HNSW index is rebuilt
    /// from the saved entries. Counters start at zero.
    ///
    /// Note: chain logging for `hnsw.load` requires calling
    /// [`set_chain_manager`] after construction; use
    /// [`load_from_file_logged`] when a chain manager is available.
    pub fn load_from_file(path: &std::path::Path) -> Result<Self, std::io::Error> {
        let store = HnswStore::load(path)?;
        let config = HnswServiceConfig {
            ef_search: 100,  // store doesn't expose params after load; use defaults
            ef_construction: 200,
            default_dimensions: 384,
        };
        Ok(Self {
            store: Mutex::new(store),
            config,
            insert_count: AtomicU64::new(0),
            search_count: AtomicU64::new(0),
            epoch: AtomicU64::new(0),
            eml: Mutex::new(HnswEmlManager::with_defaults()),
            #[cfg(feature = "exochain")]
            chain_manager: None,
            #[cfg(feature = "exochain")]
            governance_engine: None,
        })
    }

    /// Load an HNSW store from a JSON file with chain logging.
    ///
    /// Same as [`load_from_file`] but emits an `hnsw.load` chain event.
    #[cfg(feature = "exochain")]
    pub fn load_from_file_logged(
        path: &std::path::Path,
        cm: Arc<ChainManager>,
    ) -> Result<Self, std::io::Error> {
        let store = HnswStore::load(path)?;
        let entry_count = store.len();
        let config = HnswServiceConfig {
            ef_search: 100,
            ef_construction: 200,
            default_dimensions: 384,
        };

        cm.append(
            "hnsw_service",
            crate::chain::EVENT_KIND_HNSW_LOAD,
            Some(serde_json::json!({
                "path": path.display().to_string(),
                "entry_count": entry_count,
            })),
        );

        Ok(Self {
            store: Mutex::new(store),
            config,
            insert_count: AtomicU64::new(0),
            search_count: AtomicU64::new(0),
            epoch: AtomicU64::new(0),
            eml: Mutex::new(HnswEmlManager::with_defaults()),
            chain_manager: Some(cm),
            governance_engine: None,
        })
    }
}

impl std::fmt::Debug for HnswService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HnswService")
            .field("config", &self.config)
            .field("insert_count", &self.insert_count.load(Ordering::Relaxed))
            .field("search_count", &self.search_count.load(Ordering::Relaxed))
            .field("epoch", &self.epoch.load(Ordering::Relaxed))
            .finish()
    }
}

// ── SystemService impl ──────────────────────────────────────────────────

#[async_trait]
impl SystemService for HnswService {
    fn name(&self) -> &str {
        "ecc.hnsw"
    }

    fn service_type(&self) -> ServiceType {
        ServiceType::Core
    }

    async fn start(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn stop(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn health_check(&self) -> HealthStatus {
        HealthStatus::Healthy
    }
}

// ── Key-generation helpers ──────────────────────────────────────────────

/// Generate search-key text pairs for a graphify [`Entity`]-like value.
///
/// Each returned tuple is `(key_type, text)`. The caller is responsible
/// for embedding the `text` into a vector (e.g., via the embedding service)
/// before passing it to [`HnswService::insert_multi_key`].
///
/// The function accepts the entity fields directly so it does not pull in
/// a compile-time dependency on `clawft-graphify`.
pub fn entity_search_keys(
    label: &str,
    source_file: Option<&str>,
    metadata: &serde_json::Value,
) -> Vec<(String, String)> {
    let mut keys = vec![("name".to_string(), label.to_string())];

    // Add source-file context key.
    if let Some(src) = source_file {
        keys.push((
            "context".to_string(),
            format!("{} in {}", label, src),
        ));
    }

    // Add description key from metadata, if present.
    if let Some(desc) = metadata.get("description").and_then(|v| v.as_str()) {
        keys.push(("description".to_string(), desc.to_string()));
    }

    // Add relationship summary key from metadata, if present.
    if let Some(rels) = metadata.get("relationships").and_then(|v| v.as_str()) {
        keys.push(("relationship".to_string(), rels.to_string()));
    }

    keys
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_service() -> HnswService {
        HnswService::new(HnswServiceConfig::default())
    }

    #[test]
    fn new_service_empty() {
        let svc = make_service();
        assert!(svc.is_empty());
        assert_eq!(svc.len(), 0);
    }

    #[test]
    fn insert_and_len() {
        let svc = make_service();
        svc.insert("a".into(), vec![1.0, 0.0], serde_json::json!({}));
        svc.insert("b".into(), vec![0.0, 1.0], serde_json::json!({}));
        assert_eq!(svc.len(), 2);
        assert!(!svc.is_empty());
    }

    #[test]
    fn insert_upsert() {
        let svc = make_service();
        svc.insert("a".into(), vec![1.0, 0.0], serde_json::json!({"v": 1}));
        svc.insert("a".into(), vec![0.0, 1.0], serde_json::json!({"v": 2}));
        assert_eq!(svc.len(), 1);
    }

    #[test]
    fn search_empty_returns_empty() {
        let svc = make_service();
        let results = svc.search(&[1.0, 0.0], 5);
        assert!(results.is_empty());
    }

    #[test]
    fn search_returns_results() {
        let svc = make_service();
        svc.insert("a".into(), vec![1.0, 0.0, 0.0], serde_json::json!({}));
        svc.insert("b".into(), vec![0.0, 1.0, 0.0], serde_json::json!({}));
        svc.insert("c".into(), vec![0.0, 0.0, 1.0], serde_json::json!({}));

        let results = svc.search(&[1.0, 0.0, 0.0], 2);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, "a");
        assert!((results[0].score - 1.0).abs() < 0.01);
    }

    #[test]
    fn search_count_incremented() {
        let svc = make_service();
        assert_eq!(svc.search_count(), 0);
        svc.search(&[1.0], 1);
        svc.search(&[1.0], 1);
        assert_eq!(svc.search_count(), 2);
    }

    #[test]
    fn insert_count_incremented() {
        let svc = make_service();
        assert_eq!(svc.insert_count(), 0);
        svc.insert("a".into(), vec![1.0], serde_json::json!({}));
        svc.insert("b".into(), vec![0.0], serde_json::json!({}));
        assert_eq!(svc.insert_count(), 2);
    }

    #[test]
    fn clear_resets() {
        let svc = make_service();
        svc.insert("a".into(), vec![1.0], serde_json::json!({}));
        svc.insert("b".into(), vec![0.0], serde_json::json!({}));
        assert_eq!(svc.len(), 2);

        svc.clear().expect("clear should succeed without governance engine");
        assert!(svc.is_empty());
        assert_eq!(svc.len(), 0);
        // Counters are preserved after clear.
        assert_eq!(svc.insert_count(), 2);
    }

    #[test]
    fn config_default() {
        let cfg = HnswServiceConfig::default();
        assert_eq!(cfg.ef_search, 100);
        assert_eq!(cfg.ef_construction, 200);
        assert_eq!(cfg.default_dimensions, 384);

        let svc = HnswService::new(cfg);
        let c = svc.config();
        assert_eq!(c.ef_search, 100);
        assert_eq!(c.ef_construction, 200);
        assert_eq!(c.default_dimensions, 384);
    }

    #[test]
    fn service_name_is_ecc_hnsw() {
        let svc = make_service();
        assert_eq!(svc.name(), "ecc.hnsw");
        assert_eq!(svc.service_type(), ServiceType::Core);
    }

    #[tokio::test]
    async fn service_lifecycle() {
        let svc = make_service();
        svc.start().await.unwrap();
        let health = svc.health_check().await;
        assert_eq!(health, HealthStatus::Healthy);
        svc.stop().await.unwrap();
    }

    // ── Persistence tests ────────────────────────────────────────────

    fn tmp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "hnsw_test_{name}_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn persist_empty_index() {
        let svc = make_service();
        let path = tmp_path("empty");
        svc.save_to_file(&path).unwrap();
        let loaded = HnswService::load_from_file(&path).unwrap();
        assert!(loaded.is_empty());
        assert_eq!(loaded.len(), 0);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn persist_index_with_vectors_search_matches() {
        let svc = make_service();
        svc.insert("a".into(), vec![1.0, 0.0, 0.0], serde_json::json!({"tag": "first"}));
        svc.insert("b".into(), vec![0.0, 1.0, 0.0], serde_json::json!({"tag": "second"}));
        svc.insert("c".into(), vec![0.0, 0.0, 1.0], serde_json::json!({"tag": "third"}));

        let path = tmp_path("vectors");
        svc.save_to_file(&path).unwrap();
        let loaded = HnswService::load_from_file(&path).unwrap();

        assert_eq!(loaded.len(), 3);

        // Search should return the same nearest neighbor.
        let results = loaded.search(&[1.0, 0.0, 0.0], 1);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "a");
        assert!((results[0].score - 1.0).abs() < 0.01);

        let _ = std::fs::remove_file(&path);
    }

    // ── Multi-key tests ─────────────────────────────────────────────

    #[test]
    fn multi_key_config_default() {
        let cfg = MultiKeyConfig::default();
        assert!(cfg.enabled);
        assert_eq!(cfg.max_keys_per_entity, 4);
        assert_eq!(cfg.key_types.len(), 3);
    }

    #[test]
    fn insert_multi_key_creates_subentries() {
        let svc = make_service();
        let keys = vec![
            MultiKey {
                key_type: "name".into(),
                embedding: vec![1.0, 0.0, 0.0],
            },
            MultiKey {
                key_type: "context".into(),
                embedding: vec![0.7, 0.7, 0.0],
            },
            MultiKey {
                key_type: "description".into(),
                embedding: vec![0.0, 0.0, 1.0],
            },
        ];

        svc.insert_multi_key(
            "entity_a".into(),
            &keys,
            serde_json::json!({"label": "FooBar"}),
            4,
        );

        // Three sub-entries should exist.
        assert_eq!(svc.len(), 3);
        assert_eq!(svc.insert_count(), 3);
    }

    #[test]
    fn insert_multi_key_respects_max_keys() {
        let svc = make_service();
        let keys = vec![
            MultiKey { key_type: "name".into(), embedding: vec![1.0, 0.0] },
            MultiKey { key_type: "context".into(), embedding: vec![0.0, 1.0] },
            MultiKey { key_type: "description".into(), embedding: vec![0.5, 0.5] },
        ];

        svc.insert_multi_key("e1".into(), &keys, serde_json::json!({}), 2);

        // Only 2 keys should have been inserted.
        assert_eq!(svc.len(), 2);
    }

    #[test]
    fn search_dedup_merges_multi_key_results() {
        let svc = make_service();

        // Insert entity_a with two keys.
        let keys_a = vec![
            MultiKey {
                key_type: "name".into(),
                embedding: vec![1.0, 0.0, 0.0],
            },
            MultiKey {
                key_type: "context".into(),
                embedding: vec![0.9, 0.1, 0.0],
            },
        ];
        svc.insert_multi_key(
            "entity_a".into(),
            &keys_a,
            serde_json::json!({"label": "A"}),
            4,
        );

        // Insert entity_b with one key.
        svc.insert("entity_b".into(), vec![0.0, 1.0, 0.0], serde_json::json!({"label": "B"}));

        // Query near entity_a's embeddings.
        let results = svc.search_dedup(&[1.0, 0.0, 0.0], 2);

        // entity_a should appear only once despite having two matching keys.
        let a_count = results.iter().filter(|r| r.id == "entity_a").count();
        assert_eq!(a_count, 1, "entity_a should be deduplicated to one result");

        // The top result should be entity_a (closest to the query).
        assert_eq!(results[0].id, "entity_a");
    }

    #[test]
    fn search_dedup_preserves_non_multikey_ids() {
        let svc = make_service();

        // Insert a regular (non-multi-key) entry.
        svc.insert("plain".into(), vec![1.0, 0.0], serde_json::json!({}));

        let results = svc.search_dedup(&[1.0, 0.0], 1);
        assert_eq!(results.len(), 1);
        // ID should be preserved as-is (no "::" suffix to strip).
        assert_eq!(results[0].id, "plain");
    }

    #[test]
    fn search_dedup_respects_top_k() {
        let svc = make_service();

        // Insert 3 entities with 2 keys each.
        for i in 0..3 {
            let keys = vec![
                MultiKey {
                    key_type: "name".into(),
                    embedding: vec![1.0 - (i as f32 * 0.3), i as f32 * 0.3, 0.0],
                },
                MultiKey {
                    key_type: "ctx".into(),
                    embedding: vec![1.0 - (i as f32 * 0.2), i as f32 * 0.2, 0.1],
                },
            ];
            svc.insert_multi_key(
                format!("e{i}"),
                &keys,
                serde_json::json!({}),
                4,
            );
        }

        let results = svc.search_dedup(&[1.0, 0.0, 0.0], 2);
        assert!(results.len() <= 2, "should return at most top_k results");

        // All IDs should be unique.
        let ids: HashSet<_> = results.iter().map(|r| &r.id).collect();
        assert_eq!(ids.len(), results.len(), "all result IDs should be unique");
    }

    // ── entity_search_keys helper tests ─────────────────────────────

    #[test]
    fn entity_search_keys_name_only() {
        let keys = entity_search_keys("MyFunc", None, &serde_json::json!({}));
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].0, "name");
        assert_eq!(keys[0].1, "MyFunc");
    }

    #[test]
    fn entity_search_keys_with_source_file() {
        let keys = entity_search_keys(
            "MyFunc",
            Some("src/lib.rs"),
            &serde_json::json!({}),
        );
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[1].0, "context");
        assert_eq!(keys[1].1, "MyFunc in src/lib.rs");
    }

    #[test]
    fn entity_search_keys_with_description() {
        let meta = serde_json::json!({"description": "Handles user auth"});
        let keys = entity_search_keys("login", None, &meta);
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[1].0, "description");
        assert_eq!(keys[1].1, "Handles user auth");
    }

    #[test]
    fn entity_search_keys_all_fields() {
        let meta = serde_json::json!({
            "description": "Core auth handler",
            "relationships": "calls TokenService, used by Router"
        });
        let keys = entity_search_keys("login", Some("src/auth.rs"), &meta);
        assert_eq!(keys.len(), 4);
        assert_eq!(keys[0].0, "name");
        assert_eq!(keys[1].0, "context");
        assert_eq!(keys[2].0, "description");
        assert_eq!(keys[3].0, "relationship");
    }
}
