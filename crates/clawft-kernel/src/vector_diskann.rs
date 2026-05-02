//! DiskANN-backed [`VectorBackend`] implementation.
//!
//! When compiled with the `diskann` feature, this uses the real
//! `ruvector-diskann` crate (Vamana graph + PQ + mmap persistence).
//! Without it, a brute-force stub performs linear scans in memory.
//!
//! ## Hardening features (Cognitum Seed WS1)
//!
//! - **Epoch-based versioning**: monotonic epoch bumped on every mutation.
//! - **Optimistic concurrency**: `insert_with_epoch` rejects stale writes.
//! - **Soft-delete + compaction**: tombstone records excluded from search.
//! - **Capacity limits**: configurable `max_vectors` with `StoreFull` error.
//!
//! Compiled only when the `ecc` feature is enabled.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

#[cfg(feature = "diskann")]
use ruvector_diskann::{DiskAnnConfig as RealDiskAnnConfig, DiskAnnIndex};

use crate::vector_backend::{SearchResult, VectorBackend, VectorError, VectorResult};

// ── Configuration ────────────────────────────────────────────────────────

/// DiskANN backend configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskAnnConfig {
    /// Maximum number of points the index can hold.
    #[serde(default = "default_max_points")]
    pub max_points: usize,

    /// Vector dimensionality.
    #[serde(default = "default_dimensions")]
    pub dimensions: usize,

    /// Number of neighbors per node in the graph.
    #[serde(default = "default_num_neighbors")]
    pub num_neighbors: usize,

    /// Size of the search candidate list.
    #[serde(default = "default_search_list_size")]
    pub search_list_size: usize,

    /// Directory path for SSD-backed data files.
    #[serde(default = "default_data_path")]
    pub data_path: String,

    /// Whether to use product quantization for compression.
    #[serde(default = "default_use_pq")]
    pub use_pq: bool,

    /// Number of PQ sub-quantizer chunks.
    #[serde(default = "default_pq_num_chunks")]
    pub pq_num_chunks: usize,
}

fn default_max_points() -> usize {
    10_000_000
}
fn default_dimensions() -> usize {
    384
}
fn default_num_neighbors() -> usize {
    64
}
fn default_search_list_size() -> usize {
    100
}
fn default_data_path() -> String {
    ".weftos/diskann".to_owned()
}
fn default_use_pq() -> bool {
    true
}
fn default_pq_num_chunks() -> usize {
    48
}

impl Default for DiskAnnConfig {
    fn default() -> Self {
        Self {
            max_points: default_max_points(),
            dimensions: default_dimensions(),
            num_neighbors: default_num_neighbors(),
            search_list_size: default_search_list_size(),
            data_path: default_data_path(),
            use_pq: default_use_pq(),
            pq_num_chunks: default_pq_num_chunks(),
        }
    }
}

// ── Stored entry ─────────────────────────────────────────────────────────

/// A single vector entry stored in the brute-force stub.
#[cfg(not(feature = "diskann"))]
#[derive(Clone)]
struct StoredEntry {
    key: String,
    vector: Vec<f32>,
    metadata: serde_json::Value,
}

// ── Tombstone record ───────────────────────────────────────────────────

/// A soft-deleted vector.
#[derive(Debug, Clone)]
struct Tombstone {
    deleted_at_epoch: u64,
}

// ── Backend ─────────────────────────────────────────────────────────────

/// DiskANN vector backend.
///
/// With `diskann` feature: uses `ruvector-diskann` (Vamana + PQ + mmap).
/// Without: brute-force linear scan stub.
pub struct DiskAnnBackend {
    config: DiskAnnConfig,
    /// Stub storage (used when `diskann` feature is off).
    #[cfg(not(feature = "diskann"))]
    entries: Mutex<HashMap<u64, StoredEntry>>,
    /// Real DiskANN index (used when `diskann` feature is on).
    #[cfg(feature = "diskann")]
    index: Mutex<DiskAnnIndex>,
    /// ID map: u64 -> string key (needed for real index too).
    #[cfg(feature = "diskann")]
    id_map: Mutex<HashMap<u64, String>>,
    /// Monotonic epoch counter.
    epoch: AtomicU64,
    /// Soft-deleted entries.
    tombstones: Mutex<HashMap<u64, Tombstone>>,
    /// Optional capacity limit (overrides config.max_points when set).
    max_vectors_override: Mutex<Option<usize>>,
}

impl DiskAnnBackend {
    /// Create a new DiskANN backend with the given configuration.
    pub fn new(config: DiskAnnConfig) -> Self {
        #[cfg(feature = "diskann")]
        {
            let real_config = RealDiskAnnConfig {
                dim: config.dimensions,
                max_degree: config.num_neighbors,
                build_beam: config.search_list_size,
                search_beam: config.search_list_size,
                alpha: 1.2,
                pq_subspaces: if config.use_pq {
                    config.pq_num_chunks
                } else {
                    0
                },
                pq_iterations: 10,
                storage_path: Some(std::path::PathBuf::from(&config.data_path)),
            };
            let index = DiskAnnIndex::new(real_config);
            Self {
                config,
                index: Mutex::new(index),
                id_map: Mutex::new(HashMap::new()),
                epoch: AtomicU64::new(0),
                tombstones: Mutex::new(HashMap::new()),
                max_vectors_override: Mutex::new(None),
            }
        }
        #[cfg(not(feature = "diskann"))]
        {
            Self {
                config,
                entries: Mutex::new(HashMap::new()),
                epoch: AtomicU64::new(0),
                tombstones: Mutex::new(HashMap::new()),
                max_vectors_override: Mutex::new(None),
            }
        }
    }

    /// Create with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(DiskAnnConfig::default())
    }

    /// Borrow the configuration.
    pub fn config(&self) -> &DiskAnnConfig {
        &self.config
    }

    /// Bump the epoch and return the new value.
    fn bump_epoch(&self) -> u64 {
        self.epoch.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Effective capacity limit: override > config.max_points.
    fn effective_max(&self) -> Option<usize> {
        let ov = self
            .max_vectors_override
            .lock()
            .expect("max_vectors lock poisoned");
        if ov.is_some() {
            return *ov;
        }
        Some(self.config.max_points)
    }
}

#[cfg(not(feature = "diskann"))]
/// Compute cosine distance between two vectors.
///
/// Returns `1.0 - cosine_similarity`. Handles zero-magnitude vectors
/// by returning `1.0` (maximum distance).
fn cosine_distance(a: &[f32], b: &[f32]) -> f32 {
    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom < f32::EPSILON {
        return 1.0;
    }
    1.0 - (dot / denom)
}

// ── Real DiskANN backend (feature = "diskann") ──────────────────────────

#[cfg(feature = "diskann")]
impl VectorBackend for DiskAnnBackend {
    fn insert(
        &self,
        id: u64,
        key: &str,
        vector: &[f32],
        _metadata: serde_json::Value,
    ) -> VectorResult<()> {
        let mut ts = self.tombstones.lock().expect("tombstones lock poisoned");
        ts.remove(&id);
        drop(ts);

        let mut index = self.index.lock().expect("DiskAnn lock poisoned");
        let mut id_map = self.id_map.lock().expect("DiskAnn id_map lock poisoned");
        index
            .insert(key.to_owned(), vector.to_vec())
            .map_err(|e| VectorError::Other(format!("diskann insert: {e}")))?;
        id_map.insert(id, key.to_owned());
        self.bump_epoch();
        Ok(())
    }

    fn search(&self, query: &[f32], k: usize) -> Vec<SearchResult> {
        let _ts = self.tombstones.lock().expect("tombstones lock poisoned");
        let index = self.index.lock().expect("DiskAnn lock poisoned");
        match index.search(query, k) {
            Ok(results) => results
                .into_iter()
                .map(|r| SearchResult::new(0, r.id, r.distance, serde_json::Value::Null))
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    fn len(&self) -> usize {
        let index = self.index.lock().expect("DiskAnn lock poisoned");
        let ts = self.tombstones.lock().expect("tombstones lock poisoned");
        index.count().saturating_sub(ts.len())
    }

    fn contains(&self, id: u64) -> bool {
        let id_map = self.id_map.lock().expect("DiskAnn id_map lock poisoned");
        let ts = self.tombstones.lock().expect("tombstones lock poisoned");
        id_map.contains_key(&id) && !ts.contains_key(&id)
    }

    fn remove(&self, id: u64) -> bool {
        let mut ts = self.tombstones.lock().expect("tombstones lock poisoned");
        ts.remove(&id);
        drop(ts);

        let id_map = self.id_map.lock().expect("DiskAnn id_map lock poisoned");
        if let Some(key) = id_map.get(&id) {
            let mut index = self.index.lock().expect("DiskAnn lock poisoned");
            let removed = index.delete(key).unwrap_or(false);
            if removed {
                self.bump_epoch();
            }
            removed
        } else {
            false
        }
    }

    fn flush(&self) -> VectorResult<()> {
        let mut index = self.index.lock().expect("DiskAnn lock poisoned");
        index
            .build()
            .map_err(|e| VectorError::Other(format!("diskann build: {e}")))?;
        let path = std::path::Path::new(&self.config.data_path);
        std::fs::create_dir_all(path)
            .map_err(|e| VectorError::Other(format!("diskann mkdir: {e}")))?;
        index
            .save(path)
            .map_err(|e| VectorError::Other(format!("diskann save: {e}")))?;
        Ok(())
    }

    fn backend_name(&self) -> &str {
        "diskann (ruvector)"
    }

    fn current_epoch(&self) -> u64 {
        self.epoch.load(Ordering::SeqCst)
    }

    fn tombstone_count(&self) -> usize {
        self.tombstones
            .lock()
            .expect("tombstones lock poisoned")
            .len()
    }

    fn max_vectors(&self) -> Option<usize> {
        self.effective_max()
    }

    fn set_max_vectors(&self, limit: Option<usize>) {
        *self
            .max_vectors_override
            .lock()
            .expect("max_vectors lock poisoned") = limit;
    }
}

// ── Stub backend (no diskann feature) ───────────────────────────────────

#[cfg(not(feature = "diskann"))]
impl VectorBackend for DiskAnnBackend {
    fn insert(
        &self,
        id: u64,
        key: &str,
        vector: &[f32],
        metadata: serde_json::Value,
    ) -> VectorResult<()> {
        let mut entries = self.entries.lock().expect("DiskAnn lock poisoned");
        let mut ts = self.tombstones.lock().expect("tombstones lock poisoned");

        // Un-tombstone on re-insert.
        ts.remove(&id);

        // Effective capacity check.
        let live = entries.len().saturating_sub(ts.len());
        if let Some(max) = self.effective_max()
            && live >= max && !entries.contains_key(&id) {
                return Err(VectorError::StoreFull { max, current: live });
            }

        drop(ts);

        entries.insert(
            id,
            StoredEntry {
                key: key.to_owned(),
                vector: vector.to_vec(),
                metadata,
            },
        );
        drop(entries);
        self.bump_epoch();
        Ok(())
    }

    fn search(&self, query: &[f32], k: usize) -> Vec<SearchResult> {
        let entries = self.entries.lock().expect("DiskAnn lock poisoned");
        let ts = self.tombstones.lock().expect("tombstones lock poisoned");

        let mut scored: Vec<(u64, &StoredEntry, f32)> = entries
            .iter()
            .filter(|(id, _)| !ts.contains_key(id))
            .map(|(&id, entry)| {
                let dist = cosine_distance(query, &entry.vector);
                (id, entry, dist)
            })
            .collect();

        scored.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(k);

        scored
            .into_iter()
            .map(|(id, entry, dist)| {
                SearchResult::new(id, entry.key.clone(), dist, entry.metadata.clone())
            })
            .collect()
    }

    fn len(&self) -> usize {
        let entries = self.entries.lock().expect("DiskAnn lock poisoned");
        let ts = self.tombstones.lock().expect("tombstones lock poisoned");
        entries.len().saturating_sub(ts.len())
    }

    fn contains(&self, id: u64) -> bool {
        let entries = self.entries.lock().expect("DiskAnn lock poisoned");
        let ts = self.tombstones.lock().expect("tombstones lock poisoned");
        entries.contains_key(&id) && !ts.contains_key(&id)
    }

    fn remove(&self, id: u64) -> bool {
        let mut entries = self.entries.lock().expect("DiskAnn lock poisoned");
        let mut ts = self.tombstones.lock().expect("tombstones lock poisoned");
        ts.remove(&id);
        let removed = entries.remove(&id).is_some();
        if removed {
            drop(entries);
            drop(ts);
            self.bump_epoch();
        }
        removed
    }

    fn flush(&self) -> VectorResult<()> {
        Ok(())
    }

    fn backend_name(&self) -> &str {
        "diskann (stub)"
    }

    fn current_epoch(&self) -> u64 {
        self.epoch.load(Ordering::SeqCst)
    }

    fn insert_with_epoch(
        &self,
        id: u64,
        key: &str,
        vector: &[f32],
        metadata: serde_json::Value,
        parent_epoch: u64,
    ) -> VectorResult<()> {
        let current = self.epoch.load(Ordering::SeqCst);
        if parent_epoch < current {
            return Err(VectorError::EpochConflict {
                expected: parent_epoch,
                actual: current,
            });
        }
        self.insert(id, key, vector, metadata)
    }

    fn soft_delete(&self, id: u64) -> bool {
        let entries = self.entries.lock().expect("DiskAnn lock poisoned");
        let mut ts = self.tombstones.lock().expect("tombstones lock poisoned");
        if !entries.contains_key(&id) || ts.contains_key(&id) {
            return false;
        }
        let epoch = self.bump_epoch();
        ts.insert(id, Tombstone { deleted_at_epoch: epoch });
        true
    }

    fn compact(&self, older_than_epoch: u64) -> usize {
        let mut entries = self.entries.lock().expect("DiskAnn lock poisoned");
        let mut ts = self.tombstones.lock().expect("tombstones lock poisoned");

        let to_purge: Vec<u64> = ts
            .iter()
            .filter(|(_, t)| t.deleted_at_epoch < older_than_epoch)
            .map(|(&id, _)| id)
            .collect();

        let count = to_purge.len();
        for id in to_purge {
            ts.remove(&id);
            entries.remove(&id);
        }

        if count > 0 {
            drop(ts);
            drop(entries);
            self.bump_epoch();
        }

        count
    }

    fn tombstone_count(&self) -> usize {
        self.tombstones
            .lock()
            .expect("tombstones lock poisoned")
            .len()
    }

    fn max_vectors(&self) -> Option<usize> {
        self.effective_max()
    }

    fn set_max_vectors(&self, limit: Option<usize>) {
        *self
            .max_vectors_override
            .lock()
            .expect("max_vectors lock poisoned") = limit;
    }
}

impl std::fmt::Debug for DiskAnnBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiskAnnBackend")
            .field("config", &self.config)
            .field("len", &self.len())
            .field("epoch", &self.current_epoch())
            .field("tombstones", &self.tombstone_count())
            .finish()
    }
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_backend() -> DiskAnnBackend {
        DiskAnnBackend::new(DiskAnnConfig {
            max_points: 100,
            ..DiskAnnConfig::default()
        })
    }

    #[test]
    fn insert_and_search() {
        let b = make_backend();
        b.insert(1, "a", &[1.0, 0.0, 0.0], serde_json::json!({}))
            .unwrap();
        b.insert(2, "b", &[0.0, 1.0, 0.0], serde_json::json!({}))
            .unwrap();
        b.insert(3, "c", &[0.0, 0.0, 1.0], serde_json::json!({}))
            .unwrap();

        let results = b.search(&[1.0, 0.0, 0.0], 2);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, 1);
        assert!(results[0].distance < 0.01);
    }

    #[test]
    fn remove_entry() {
        let b = make_backend();
        b.insert(1, "a", &[1.0, 0.0], serde_json::json!({}))
            .unwrap();
        assert!(b.contains(1));
        assert!(b.remove(1));
        assert!(!b.contains(1));
        assert_eq!(b.len(), 0);
    }

    #[test]
    fn capacity_exceeded() {
        let b = DiskAnnBackend::new(DiskAnnConfig {
            max_points: 2,
            ..DiskAnnConfig::default()
        });
        b.insert(1, "a", &[1.0], serde_json::json!({})).unwrap();
        b.insert(2, "b", &[0.0], serde_json::json!({})).unwrap();
        let result = b.insert(3, "c", &[0.5], serde_json::json!({}));
        assert!(result.is_err());
    }

    #[test]
    fn upsert_does_not_exceed_capacity() {
        let b = DiskAnnBackend::new(DiskAnnConfig {
            max_points: 2,
            ..DiskAnnConfig::default()
        });
        b.insert(1, "a", &[1.0], serde_json::json!({})).unwrap();
        b.insert(2, "b", &[0.0], serde_json::json!({})).unwrap();
        // Updating existing id=1 should succeed.
        b.insert(1, "a-updated", &[0.5], serde_json::json!({}))
            .unwrap();
        assert_eq!(b.len(), 2);
    }

    #[test]
    fn cosine_distance_identical() {
        let d = cosine_distance(&[1.0, 0.0], &[1.0, 0.0]);
        assert!(d.abs() < 0.001);
    }

    #[test]
    fn cosine_distance_orthogonal() {
        let d = cosine_distance(&[1.0, 0.0], &[0.0, 1.0]);
        assert!((d - 1.0).abs() < 0.001);
    }

    #[test]
    fn backend_name() {
        let b = make_backend();
        #[cfg(not(feature = "diskann"))]
        assert_eq!(b.backend_name(), "diskann (stub)");
        #[cfg(feature = "diskann")]
        assert_eq!(b.backend_name(), "diskann");
    }

    #[test]
    fn flush_noop() {
        let b = make_backend();
        b.flush().unwrap();
    }

    // ── Epoch tests ─────────────────────────────────────────────────────

    #[test]
    fn epoch_increments_on_insert() {
        let b = make_backend();
        assert_eq!(b.current_epoch(), 0);
        b.insert(1, "a", &[1.0], serde_json::json!({})).unwrap();
        assert_eq!(b.current_epoch(), 1);
    }

    #[test]
    fn insert_with_epoch_rejects_stale() {
        let b = make_backend();
        b.insert(1, "a", &[1.0], serde_json::json!({})).unwrap();
        let result = b.insert_with_epoch(2, "b", &[0.0], serde_json::json!({}), 0);
        assert!(matches!(result, Err(VectorError::EpochConflict { .. })));
    }

    // ── Soft-delete tests ───────────────────────────────────────────────

    #[test]
    fn soft_delete_hides_from_search() {
        let b = make_backend();
        b.insert(1, "a", &[1.0, 0.0], serde_json::json!({}))
            .unwrap();
        b.insert(2, "b", &[0.0, 1.0], serde_json::json!({}))
            .unwrap();

        assert!(b.soft_delete(1));
        assert_eq!(b.tombstone_count(), 1);
        assert_eq!(b.len(), 1);

        let results = b.search(&[1.0, 0.0], 5);
        for r in &results {
            assert_ne!(r.id, 1);
        }
    }

    #[test]
    fn compact_purges_old_tombstones() {
        let b = make_backend();
        b.insert(1, "a", &[1.0], serde_json::json!({})).unwrap();
        b.soft_delete(1);
        let epoch = b.current_epoch();
        let purged = b.compact(epoch + 1);
        assert_eq!(purged, 1);
        assert_eq!(b.tombstone_count(), 0);
    }

    // ── Capacity limit tests ────────────────────────────────────────────

    #[test]
    fn set_max_vectors_override() {
        let b = make_backend();
        // Default effective max comes from config.max_points = 100.
        assert_eq!(b.max_vectors(), Some(100));
        b.set_max_vectors(Some(5));
        assert_eq!(b.max_vectors(), Some(5));
        b.set_max_vectors(None);
        // Falls back to config.max_points.
        assert_eq!(b.max_vectors(), Some(100));
    }

    #[test]
    fn soft_delete_frees_capacity_slot() {
        let b = DiskAnnBackend::new(DiskAnnConfig {
            max_points: 2,
            ..DiskAnnConfig::default()
        });
        b.insert(1, "a", &[1.0], serde_json::json!({})).unwrap();
        b.insert(2, "b", &[0.0], serde_json::json!({})).unwrap();

        b.soft_delete(1);
        // Live count is now 1, so a new insert should succeed.
        b.insert(3, "c", &[0.5], serde_json::json!({})).unwrap();
    }
}
