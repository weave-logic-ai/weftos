//! HNSW-backed vector store using `instant-distance`.
//!
//! Provides [`HnswStore`], a high-performance approximate nearest neighbor
//! store that wraps the `instant-distance` HNSW implementation. When the
//! entry count is small (below [`HNSW_THRESHOLD`]), queries fall back to
//! brute-force cosine similarity for correctness. Above the threshold,
//! queries route through the HNSW graph for sub-linear search.
//!
//! The store supports persistence via JSON serialization: entries are
//! stored alongside the store metadata so the HNSW graph can be rebuilt
//! on load.
//!
//! This module is gated behind the `vector-memory` feature flag.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use instant_distance::{Builder, HnswMap, Point, Search};
use serde::{Deserialize, Serialize};
use tracing::debug;

/// Minimum number of entries before the HNSW index is built.
///
/// Below this threshold, brute-force cosine similarity is used because
/// building an HNSW graph for very few points has no benefit.
const HNSW_THRESHOLD: usize = 32;

/// Default ef_search parameter for HNSW queries.
///
/// Higher values trade latency for recall. 100 gives ~95%+ recall
/// on typical workloads.
const DEFAULT_EF_SEARCH: usize = 100;

/// Default ef_construction parameter for building the HNSW graph.
const DEFAULT_EF_CONSTRUCTION: usize = 200;

/// Default number of inserts before a full HNSW rebuild is triggered.
///
/// Below this threshold, new entries are searched via brute-force and
/// merged with the stale HNSW results, avoiding expensive full rebuilds.
const DEFAULT_REBUILD_THRESHOLD: usize = 100;

// ── Point wrapper ──────────────────────────────────────────────────────

/// Wrapper around an embedding vector that implements [`Point`].
///
/// Distance is `1.0 - cosine_similarity`, so that closer points have
/// smaller distances (as expected by the HNSW algorithm).
///
/// Uses `Arc<[f32]>` internally so that constructing points from
/// stored entries is a cheap pointer copy instead of a full
/// allocation+copy of the embedding vector.
#[derive(Debug, Clone)]
struct EmbeddingPoint {
    embedding: Arc<[f32]>,
}

/// Serialization helper: `Arc<[f32]>` does not implement Serialize/Deserialize
/// directly, but `EmbeddingPoint` is only serialized indirectly via
/// `HnswEntry` (which owns `Vec<f32>`), so the derives are not needed.
/// We keep manual impls for the snapshot path which no longer uses EmbeddingPoint.
impl Point for EmbeddingPoint {
    fn distance(&self, other: &Self) -> f32 {
        1.0 - cosine_similarity(&self.embedding, &other.embedding)
    }
}

// ── Entry ──────────────────────────────────────────────────────────────

/// A single entry stored in the HNSW store.
///
/// The embedding is stored as `Arc<[f32]>` so that building HNSW
/// index points is a cheap `Arc::clone` (pointer copy) rather than
/// a full allocation+copy of the vector.
#[derive(Debug, Clone)]
pub struct HnswEntry {
    /// Unique identifier for this entry.
    pub id: String,
    /// The embedding vector (shared via Arc for cheap cloning).
    pub embedding: Arc<[f32]>,
    /// Arbitrary metadata.
    pub metadata: serde_json::Value,
}

/// Serde support: serialize `Arc<[f32]>` as a plain `Vec<f32>`.
impl Serialize for HnswEntry {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("HnswEntry", 3)?;
        s.serialize_field("id", &self.id)?;
        s.serialize_field("embedding", &*self.embedding)?;
        s.serialize_field("metadata", &self.metadata)?;
        s.end()
    }
}

impl<'de> Deserialize<'de> for HnswEntry {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            id: String,
            embedding: Vec<f32>,
            metadata: serde_json::Value,
        }
        let raw = Raw::deserialize(deserializer)?;
        Ok(HnswEntry {
            id: raw.id,
            embedding: Arc::from(raw.embedding),
            metadata: raw.metadata,
        })
    }
}

/// A query result from the HNSW store.
#[derive(Debug, Clone)]
pub struct HnswQueryResult {
    /// The entry ID.
    pub id: String,
    /// Cosine similarity score (higher is better).
    pub score: f32,
    /// The entry metadata.
    pub metadata: serde_json::Value,
}

// ── Serializable state ─────────────────────────────────────────────────

/// Serializable snapshot of the store (entries only; the HNSW graph is
/// rebuilt on load).
#[derive(Debug, Serialize, Deserialize)]
struct StoreSnapshot {
    entries: Vec<HnswEntry>,
    ef_search: usize,
    ef_construction: usize,
}

// ── HnswStore ──────────────────────────────────────────────────────────

/// HNSW-backed vector store with automatic fallback to brute-force
/// for small datasets.
///
/// # Usage
///
/// ```rust,no_run
/// use clawft_core::embeddings::hnsw_store::HnswStore;
///
/// let mut store = HnswStore::new();
/// store.insert("doc1".into(), vec![1.0, 0.0, 0.0], serde_json::json!({"text": "hello"}));
/// let results = store.query(&[1.0, 0.0, 0.0], 5);
/// ```
pub struct HnswStore {
    /// All entries (source of truth).
    entries: Vec<HnswEntry>,
    /// O(1) lookup from entry ID to its position in `entries`.
    id_index: HashMap<String, usize>,
    /// The HNSW index, rebuilt when entries change above the threshold.
    index: Option<HnswMap<EmbeddingPoint, usize>>,
    /// ef_search parameter for queries.
    ef_search: usize,
    /// ef_construction parameter for building.
    ef_construction: usize,
    /// Whether the index is stale (entries changed since last build).
    dirty: bool,
    /// Number of entries at the time the HNSW index was last built.
    ///
    /// Entries from `index_built_len..entries.len()` are "new" and must
    /// be scanned via brute-force when the index is stale but below the
    /// rebuild threshold.
    index_built_len: usize,
    /// Number of inserts (and deletes) since the last HNSW rebuild.
    inserts_since_rebuild: usize,
    /// How many mutations before a full rebuild is triggered automatically.
    rebuild_threshold: usize,
}

impl HnswStore {
    /// Create a new, empty HNSW store with default parameters.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            id_index: HashMap::new(),
            index: None,
            ef_search: DEFAULT_EF_SEARCH,
            ef_construction: DEFAULT_EF_CONSTRUCTION,
            dirty: false,
            index_built_len: 0,
            inserts_since_rebuild: 0,
            rebuild_threshold: DEFAULT_REBUILD_THRESHOLD,
        }
    }

    /// Create a store with custom HNSW parameters.
    pub fn with_params(ef_search: usize, ef_construction: usize) -> Self {
        Self {
            entries: Vec::new(),
            id_index: HashMap::new(),
            index: None,
            ef_search,
            ef_construction,
            dirty: false,
            index_built_len: 0,
            inserts_since_rebuild: 0,
            rebuild_threshold: DEFAULT_REBUILD_THRESHOLD,
        }
    }

    /// Set the rebuild threshold (number of mutations before automatic
    /// full HNSW rebuild). Returns `&mut Self` for chaining.
    pub fn set_rebuild_threshold(&mut self, threshold: usize) -> &mut Self {
        self.rebuild_threshold = threshold;
        self
    }

    /// Insert an entry into the store.
    ///
    /// Uses upsert semantics: if an entry with the same ID already exists,
    /// it is replaced.
    pub fn insert(
        &mut self,
        id: String,
        embedding: Vec<f32>,
        metadata: serde_json::Value,
    ) {
        // Remove existing entry with the same ID (upsert) in O(1).
        if let Some(old_pos) = self.id_index.remove(&id) {
            self.entries.swap_remove(old_pos);
            // If swap_remove moved the last element into old_pos, update its index.
            if old_pos < self.entries.len() {
                let moved_id = self.entries[old_pos].id.clone();
                self.id_index.insert(moved_id, old_pos);
            }
        }
        let new_pos = self.entries.len();
        self.id_index.insert(id.clone(), new_pos);
        self.entries.push(HnswEntry {
            id,
            embedding: Arc::from(embedding),
            metadata,
        });
        self.dirty = true;
        self.inserts_since_rebuild += 1;
    }

    /// Query the store for the top-k most similar entries.
    ///
    /// Returns results sorted by descending cosine similarity.
    ///
    /// When the index is stale but the number of mutations since the last
    /// rebuild is below [`rebuild_threshold`], the query uses a hybrid
    /// strategy: HNSW search over the previously-indexed entries plus a
    /// brute-force scan over entries added since the last rebuild. This
    /// avoids the O(n log n) cost of a full HNSW rebuild on every insert.
    pub fn query(
        &mut self,
        query_embedding: &[f32],
        top_k: usize,
    ) -> Vec<HnswQueryResult> {
        if self.entries.is_empty() || top_k == 0 {
            return Vec::new();
        }

        // Use brute-force for small datasets.
        if self.entries.len() < HNSW_THRESHOLD {
            return self.brute_force_query(query_embedding, top_k);
        }

        // Decide whether to do a full rebuild or use hybrid search.
        if self.index.is_none()
            || (self.dirty && self.inserts_since_rebuild >= self.rebuild_threshold)
        {
            self.rebuild_index();
        }

        // If index build failed (shouldn't happen), fall back.
        let Some(ref index) = self.index else {
            return self.brute_force_query(query_embedding, top_k);
        };

        let query_point = EmbeddingPoint {
            embedding: Arc::from(query_embedding),
        };
        let mut search = Search::default();

        // Collect HNSW results. Ask for more than top_k so we have room
        // to merge with brute-force results from new entries.
        let mut results: Vec<HnswQueryResult> = index
            .search(&query_point, &mut search)
            .take(top_k)
            .filter_map(|item| {
                let idx = *item.value;
                // Guard: the index was built with index_built_len entries.
                // After deletes + swap_remove, an index value may point
                // beyond the current entries vec.
                if idx < self.entries.len() {
                    let entry = &self.entries[idx];
                    Some(HnswQueryResult {
                        id: entry.id.clone(),
                        score: cosine_similarity(query_embedding, &entry.embedding),
                        metadata: entry.metadata.clone(),
                    })
                } else {
                    None
                }
            })
            .collect();

        // If the index is stale (dirty but below rebuild threshold),
        // brute-force scan entries added after the last rebuild and merge.
        if self.dirty && self.index_built_len < self.entries.len() {
            for entry in &self.entries[self.index_built_len..] {
                let score =
                    cosine_similarity(query_embedding, &entry.embedding);
                results.push(HnswQueryResult {
                    id: entry.id.clone(),
                    score,
                    metadata: entry.metadata.clone(),
                });
            }
        }

        // Sort by descending score and truncate.
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Deduplicate (an entry could appear from both HNSW and brute-force
        // if index_built_len entries were modified via upsert).
        results.dedup_by(|a, b| a.id == b.id);

        results.truncate(top_k);
        results
    }

    /// Delete an entry by ID. Returns `true` if removed.
    pub fn delete(&mut self, id: &str) -> bool {
        if let Some(pos) = self.id_index.remove(id) {
            self.entries.swap_remove(pos);
            // If swap_remove moved the last element into pos, update its index.
            if pos < self.entries.len() {
                let moved_id = self.entries[pos].id.clone();
                self.id_index.insert(moved_id, pos);
            }
            self.dirty = true;
            self.inserts_since_rebuild += 1;
            true
        } else {
            false
        }
    }

    /// Return the number of entries in the store.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Return `true` if the store has no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Get an entry by ID.
    pub fn get(&self, id: &str) -> Option<&HnswEntry> {
        self.id_index.get(id).map(|&pos| &self.entries[pos])
    }

    /// Persist the store to a JSON file.
    ///
    /// Only entries and parameters are saved. The HNSW graph is rebuilt
    /// on load.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let snapshot = StoreSnapshot {
            entries: self.entries.clone(),
            ef_search: self.ef_search,
            ef_construction: self.ef_construction,
        };

        let json = serde_json::to_string_pretty(&snapshot).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
        })?;
        std::fs::write(path, json)
    }

    /// Load a store from a JSON file.
    ///
    /// If the file does not exist, returns a new empty store.
    pub fn load(path: &Path) -> std::io::Result<Self> {
        if !path.exists() {
            return Ok(Self::new());
        }

        let data = std::fs::read_to_string(path)?;
        let snapshot: StoreSnapshot =
            serde_json::from_str(&data).map_err(|e| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
            })?;

        debug!(
            entries = snapshot.entries.len(),
            ef_search = snapshot.ef_search,
            "loaded HnswStore from disk"
        );

        let mut id_index: HashMap<String, usize> =
            HashMap::with_capacity(snapshot.entries.len());
        for (i, e) in snapshot.entries.iter().enumerate() {
            id_index.insert(e.id.clone(), i);
        }

        let mut store = Self {
            entries: snapshot.entries,
            id_index,
            index: None,
            ef_search: snapshot.ef_search,
            ef_construction: snapshot.ef_construction,
            dirty: true,
            index_built_len: 0,
            inserts_since_rebuild: 0,
            rebuild_threshold: DEFAULT_REBUILD_THRESHOLD,
        };

        // Pre-build the index if above threshold.
        if store.entries.len() >= HNSW_THRESHOLD {
            store.rebuild_index();
        }

        Ok(store)
    }

    /// Force a full rebuild of the HNSW index.
    ///
    /// This is called automatically when `inserts_since_rebuild` reaches
    /// the `rebuild_threshold`, or when no index exists yet. You can also
    /// call it explicitly via [`force_rebuild`](Self::force_rebuild).
    pub fn rebuild_index(&mut self) {
        if self.entries.is_empty() {
            self.index = None;
            self.dirty = false;
            self.index_built_len = 0;
            self.inserts_since_rebuild = 0;
            return;
        }

        let points: Vec<EmbeddingPoint> = self
            .entries
            .iter()
            .map(|e| EmbeddingPoint {
                embedding: Arc::clone(&e.embedding),
            })
            .collect();

        let values: Vec<usize> = (0..self.entries.len()).collect();

        debug!(
            entries = self.entries.len(),
            ef_construction = self.ef_construction,
            ef_search = self.ef_search,
            "rebuilding HNSW index"
        );

        let map = Builder::default()
            .ef_search(self.ef_search)
            .ef_construction(self.ef_construction)
            .build(points, values);

        self.index = Some(map);
        self.dirty = false;
        self.index_built_len = self.entries.len();
        self.inserts_since_rebuild = 0;
    }

    /// Explicitly request a full HNSW rebuild regardless of the current
    /// mutation count. Useful after a batch of inserts.
    pub fn force_rebuild(&mut self) {
        self.rebuild_index();
    }

    /// Override ef_search at runtime and trigger a rebuild so the new
    /// value takes effect. Used by the adaptive EML layer.
    pub fn set_ef_search(&mut self, ef: usize) {
        self.ef_search = ef;
        if self.entries.len() >= HNSW_THRESHOLD {
            self.rebuild_index();
        }
    }

    /// Current ef_search parameter.
    pub fn ef_search(&self) -> usize {
        self.ef_search
    }

    /// Number of mutations since the last HNSW rebuild.
    pub fn inserts_since_rebuild(&self) -> usize {
        self.inserts_since_rebuild
    }

    /// Brute-force top-k search for ground-truth recall measurement.
    ///
    /// Unlike [`query`], this always does a full linear scan regardless of
    /// store size or HNSW index state. Returns IDs only.
    pub fn brute_force_topk(&self, query: &[f32], top_k: usize) -> Vec<String> {
        self.brute_force_query(query, top_k)
            .into_iter()
            .map(|r| r.id)
            .collect()
    }

    /// Brute-force cosine similarity search (fallback for small datasets).
    fn brute_force_query(
        &self,
        query_embedding: &[f32],
        top_k: usize,
    ) -> Vec<HnswQueryResult> {
        let mut scored: Vec<HnswQueryResult> = self
            .entries
            .iter()
            .map(|entry| HnswQueryResult {
                id: entry.id.clone(),
                score: cosine_similarity(query_embedding, &entry.embedding),
                metadata: entry.metadata.clone(),
            })
            .collect();

        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(top_k);
        scored
    }
}

impl Default for HnswStore {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tiered search ─────────────────────────────────────────────────────

/// Configuration for one tier of dimensional search.
#[derive(Debug, Clone)]
pub struct TierConfig {
    /// Which dimensions to use for cosine at this tier.
    /// Empty = use all dimensions (full cosine).
    pub dims: Vec<usize>,
    /// How many candidates to keep after this tier.
    pub keep: usize,
}

/// Multi-index tiered dimensional search.
///
/// Three separate HNSW indexes at different resolutions:
///
/// 1. **Coarse index** (8 dims): graph traversal on projected embeddings.
///    Cheap distance function (8 floats), fast traversal, high recall at
///    low precision. Over-fetches ~50× top_k candidates.
/// 2. **Medium re-rank** (32 dims): partial cosine on survivors from
///    coarse. No graph traversal — just linear re-score.
/// 3. **Fine re-rank** (full dims): full cosine on the final ~5× top_k
///    survivors. Returns exact top-k.
///
/// The coarse index's graph connectivity is built in the projected
/// space, so "nearby" means similar in the coarse dimensions — like
/// skipping to 'P' in a dictionary. The medium and fine tiers are
/// linear passes on the shrinking candidate set.
pub struct TieredSearch {
    coarse_index: HnswStore,
    coarse_dims: Vec<usize>,
    coarse_keep: usize,
    medium_dims: Vec<usize>,
    medium_keep: usize,
    full_store: HnswStore,
}

/// Project a full embedding to a subset of dimensions.
fn project(embedding: &[f32], dims: &[usize]) -> Vec<f32> {
    dims.iter().map(|&d| if d < embedding.len() { embedding[d] } else { 0.0 }).collect()
}

impl TieredSearch {
    /// Build a tiered index from a corpus.
    ///
    /// `entries`: (id, full_embedding, metadata) triples.
    /// `coarse_dims`: dimension indices for the coarse HNSW.
    /// `medium_dims`: dimension indices for the medium re-rank.
    /// `top_k`: expected query depth (used to size keep counts).
    /// `ef`: ef_search for the coarse HNSW (can be low since dims are few).
    pub fn build(
        entries: &[(String, Vec<f32>, serde_json::Value)],
        coarse_dims: Vec<usize>,
        medium_dims: Vec<usize>,
        top_k: usize,
        ef: usize,
    ) -> Self {
        // Coarse index: store projected embeddings.
        let mut coarse_index = HnswStore::with_params(ef, 200);
        for (id, emb, meta) in entries {
            let projected = project(emb, &coarse_dims);
            coarse_index.insert(id.clone(), projected, meta.clone());
        }
        coarse_index.force_rebuild();

        // Full store for re-ranking (brute-force lookups by ID).
        let mut full_store = HnswStore::new();
        for (id, emb, meta) in entries {
            full_store.insert(id.clone(), emb.clone(), meta.clone());
        }

        let coarse_keep = (top_k * 50).max(200);
        let medium_keep = (top_k * 5).max(20);

        TieredSearch {
            coarse_index,
            coarse_dims,
            coarse_keep,
            medium_dims,
            medium_keep,
            full_store,
        }
    }

    /// Build with default dimension selection for the given dimensionality.
    pub fn build_default(
        entries: &[(String, Vec<f32>, serde_json::Value)],
        dims: usize,
        top_k: usize,
        ef: usize,
    ) -> Self {
        let coarse_n = (dims / 16).max(2);
        let medium_n = (dims / 4).max(4);
        Self::build(
            entries,
            (0..coarse_n).collect(),
            (0..medium_n).collect(),
            top_k,
            ef,
        )
    }

    /// Build with a learned dimension ordering.
    pub fn build_learned(
        entries: &[(String, Vec<f32>, serde_json::Value)],
        dim_order: &[usize],
        dims: usize,
        top_k: usize,
        ef: usize,
    ) -> Self {
        let coarse_n = (dims / 16).max(2);
        let medium_n = (dims / 4).max(4);
        Self::build(
            entries,
            dim_order[..coarse_n.min(dim_order.len())].to_vec(),
            dim_order[..medium_n.min(dim_order.len())].to_vec(),
            top_k,
            ef,
        )
    }

    /// Number of entries in the index.
    pub fn len(&self) -> usize {
        self.full_store.len()
    }

    /// True when the index has no entries.
    pub fn is_empty(&self) -> bool {
        self.full_store.is_empty()
    }

    /// Run a tiered search.
    ///
    /// 1. Query the coarse HNSW (projected dims) for ~50× top_k candidates.
    /// 2. Re-rank survivors with medium-dim cosine, keep ~5× top_k.
    /// 3. Re-rank with full-dim cosine, return top_k.
    pub fn search(&mut self, query: &[f32], top_k: usize) -> Vec<HnswQueryResult> {
        if self.full_store.is_empty() || top_k == 0 {
            return Vec::new();
        }

        // Tier 1 (Coarse): HNSW on projected query.
        let coarse_query = project(query, &self.coarse_dims);
        let mut candidates: Vec<(String, f32, serde_json::Value)> = self
            .coarse_index
            .query(&coarse_query, self.coarse_keep)
            .into_iter()
            .map(|r| (r.id, r.score, r.metadata))
            .collect();

        // Tier 2 (Medium): re-score with more dimensions.
        if !self.medium_dims.is_empty() && candidates.len() > self.medium_keep {
            for c in &mut candidates {
                if let Some(entry) = self.full_store.get(&c.0) {
                    c.1 = cosine_partial(query, &entry.embedding, &self.medium_dims);
                }
            }
            candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            candidates.truncate(self.medium_keep);
        }

        // Tier 3 (Fine): full-dimensional cosine.
        let mut results: Vec<HnswQueryResult> = candidates
            .into_iter()
            .filter_map(|(id, _, _)| {
                self.full_store.get(&id).map(|entry| HnswQueryResult {
                    id: id.clone(),
                    score: cosine_similarity(query, &entry.embedding),
                    metadata: entry.metadata.clone(),
                })
            })
            .collect();
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(top_k);
        results
    }

    /// Brute-force top-k for recall measurement (uses full store).
    pub fn brute_force_topk(&self, query: &[f32], top_k: usize) -> Vec<String> {
        self.full_store.brute_force_topk(query, top_k)
    }
}

/// Cosine similarity using only selected dimensions.
fn cosine_partial(a: &[f32], b: &[f32], dims: &[usize]) -> f32 {
    let mut dot = 0.0_f32;
    let mut norm_a = 0.0_f32;
    let mut norm_b = 0.0_f32;
    let len = a.len().min(b.len());
    for &d in dims {
        if d < len {
            let va = a[d];
            let vb = b[d];
            dot += va * vb;
            norm_a += va * va;
            norm_b += vb * vb;
        }
    }
    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom == 0.0 { 0.0 } else { dot / denom }
}

/// Compute cosine similarity between two vectors.
///
/// Returns `0.0` when either vector has zero norm or when lengths differ.
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }

    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }

    dot / (norm_a * norm_b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_path(label: &str) -> std::path::PathBuf {
        let n = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        std::env::temp_dir().join(format!(
            "clawft_hnsw_test_{label}_{pid}_{n}.json"
        ))
    }

    #[test]
    fn create_empty_store() {
        let store = HnswStore::new();
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn insert_and_brute_force_query() {
        let mut store = HnswStore::new();
        store.insert(
            "doc1".into(),
            vec![1.0, 0.0, 0.0],
            serde_json::json!({"text": "hello"}),
        );
        store.insert(
            "doc2".into(),
            vec![0.0, 1.0, 0.0],
            serde_json::json!({"text": "world"}),
        );

        let results = store.query(&[1.0, 0.0, 0.0], 1);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "doc1");
        assert!((results[0].score - 1.0).abs() < 0.01);
    }

    #[test]
    fn query_ordering() {
        let mut store = HnswStore::new();
        store.insert("a".into(), vec![1.0, 0.0], serde_json::json!({}));
        store.insert("b".into(), vec![0.7, 0.7], serde_json::json!({}));
        store.insert("c".into(), vec![0.0, 1.0], serde_json::json!({}));

        let results = store.query(&[1.0, 0.0], 3);
        assert_eq!(results[0].id, "a");
        assert_eq!(results[1].id, "b");
        assert_eq!(results[2].id, "c");
    }

    #[test]
    fn upsert_semantics() {
        let mut store = HnswStore::new();
        store.insert(
            "doc1".into(),
            vec![1.0, 0.0],
            serde_json::json!({"v": 1}),
        );
        store.insert(
            "doc1".into(),
            vec![0.0, 1.0],
            serde_json::json!({"v": 2}),
        );

        assert_eq!(store.len(), 1);
        let entry = store.get("doc1").unwrap();
        assert_eq!(entry.metadata["v"], 2);
        assert_eq!(&*entry.embedding, &[0.0, 1.0]);
    }

    #[test]
    fn delete_entry() {
        let mut store = HnswStore::new();
        store.insert("doc1".into(), vec![1.0], serde_json::json!({}));
        store.insert("doc2".into(), vec![0.0], serde_json::json!({}));

        assert!(store.delete("doc1"));
        assert_eq!(store.len(), 1);
        assert!(!store.delete("doc1"));
    }

    #[test]
    fn query_empty_store() {
        let mut store = HnswStore::new();
        let results = store.query(&[1.0, 0.0], 5);
        assert!(results.is_empty());
    }

    #[test]
    fn query_top_k_zero() {
        let mut store = HnswStore::new();
        store.insert("x".into(), vec![1.0], serde_json::json!({}));
        let results = store.query(&[1.0], 0);
        assert!(results.is_empty());
    }

    #[test]
    fn save_and_load_roundtrip() {
        let path = temp_path("roundtrip");

        {
            let mut store = HnswStore::new();
            store.insert(
                "doc1".into(),
                vec![1.0, 0.0, 0.0],
                serde_json::json!({"text": "hello"}),
            );
            store.insert(
                "doc2".into(),
                vec![0.0, 1.0, 0.0],
                serde_json::json!({"text": "world"}),
            );
            store.save(&path).unwrap();
        }

        let mut store = HnswStore::load(&path).unwrap();
        assert_eq!(store.len(), 2);

        let results = store.query(&[1.0, 0.0, 0.0], 1);
        assert_eq!(results[0].id, "doc1");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_nonexistent_returns_empty() {
        let path = temp_path("nonexist");
        let _ = std::fs::remove_file(&path);

        let store = HnswStore::load(&path).unwrap();
        assert!(store.is_empty());
    }

    #[test]
    fn hnsw_index_triggers_above_threshold() {
        let mut store = HnswStore::new();
        // Insert enough entries to trigger HNSW indexing.
        for i in 0..HNSW_THRESHOLD + 5 {
            let dim = 16;
            let mut emb = vec![0.0f32; dim];
            emb[i % dim] = 1.0;
            store.insert(
                format!("doc{i}"),
                emb,
                serde_json::json!({"idx": i}),
            );
        }

        // Query should use HNSW path and return correct results.
        let mut query = vec![0.0f32; 16];
        query[0] = 1.0;

        let results = store.query(&query, 3);
        assert!(!results.is_empty());
        // The exact top match depends on how many entries share
        // dimension 0, but we should get results.
        assert!(results.len() <= 3);
    }

    #[test]
    fn get_entry_by_id() {
        let mut store = HnswStore::new();
        store.insert(
            "doc1".into(),
            vec![1.0, 2.0],
            serde_json::json!({"key": "value"}),
        );

        let entry = store.get("doc1");
        assert!(entry.is_some());
        assert_eq!(entry.unwrap().id, "doc1");

        assert!(store.get("nonexistent").is_none());
    }

    #[test]
    fn default_creates_empty() {
        let store = HnswStore::default();
        assert!(store.is_empty());
    }

    #[test]
    fn cosine_similarity_identical() {
        let score = cosine_similarity(&[1.0, 0.0], &[1.0, 0.0]);
        assert!((score - 1.0).abs() < 0.01);
    }

    #[test]
    fn cosine_similarity_orthogonal() {
        let score = cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]);
        assert!(score.abs() < 0.01);
    }

    #[test]
    fn cosine_similarity_different_lengths() {
        let score = cosine_similarity(&[1.0, 0.0], &[1.0]);
        assert!((score - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn with_params_custom_values() {
        let store = HnswStore::with_params(50, 100);
        assert_eq!(store.ef_search, 50);
        assert_eq!(store.ef_construction, 100);
        assert!(store.is_empty());
    }

    #[test]
    fn hnsw_query_consistency_with_brute_force() {
        // Verify that the HNSW index returns the same top-1 as brute-force
        // for a simple dataset above threshold.
        let dim = 8;
        let n = HNSW_THRESHOLD + 10;
        let mut store = HnswStore::new();

        // Create entries with known embeddings.
        for i in 0..n {
            let mut emb = vec![0.0f32; dim];
            emb[i % dim] = 1.0;
            // Add a small perturbation so entries aren't identical.
            for (j, val) in emb.iter_mut().enumerate() {
                *val += (i as f32 * 0.001) * ((j + 1) as f32);
            }
            store.insert(format!("d{i}"), emb, serde_json::json!({}));
        }

        // Query for the first entry's embedding.
        let target = Arc::clone(&store.get("d0").unwrap().embedding);
        let results = store.query(&target, 1);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "d0");
        assert!(results[0].score > 0.9);
    }
}
