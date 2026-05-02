//! HNSW-backed [`VectorBackend`] implementation.
//!
//! Wraps the existing [`HnswService`] behind the unified
//! [`VectorBackend`] trait so that it can be used standalone or as the
//! hot tier inside [`HybridBackend`](super::vector_hybrid::HybridBackend).
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

use crate::hnsw_service::{HnswService, HnswServiceConfig};
use crate::vector_backend::{SearchResult, VectorBackend, VectorError, VectorResult};

// ── ID ↔ key mapping ────────────────────────────────────────────────────

/// Internal mapping between numeric IDs (used by `VectorBackend`) and
/// the string keys used by `HnswService`.
struct IdMap {
    id_to_key: HashMap<u64, String>,
    key_to_id: HashMap<String, u64>,
}

impl IdMap {
    fn new() -> Self {
        Self {
            id_to_key: HashMap::new(),
            key_to_id: HashMap::new(),
        }
    }

    fn insert(&mut self, id: u64, key: String) {
        // Remove old key mapping if this id was previously used.
        if let Some(old_key) = self.id_to_key.insert(id, key.clone())
            && old_key != key {
                self.key_to_id.remove(&old_key);
            }
        self.key_to_id.insert(key, id);
    }

    fn contains_id(&self, id: u64) -> bool {
        self.id_to_key.contains_key(&id)
    }

    #[allow(dead_code)]
    fn key_for_id(&self, id: u64) -> Option<&str> {
        self.id_to_key.get(&id).map(|s| s.as_str())
    }

    fn remove(&mut self, id: u64) -> Option<String> {
        if let Some(key) = self.id_to_key.remove(&id) {
            self.key_to_id.remove(&key);
            Some(key)
        } else {
            None
        }
    }

    fn len(&self) -> usize {
        self.id_to_key.len()
    }
}

// ── Tombstone record ───────────────────────────────────────────────────

/// A soft-deleted vector. Keeps the ID and the epoch at which it was
/// deleted so that [`compact`] can purge old tombstones.
#[derive(Debug, Clone)]
struct Tombstone {
    /// Epoch at which the vector was soft-deleted.
    deleted_at_epoch: u64,
}

// ── Backend ─────────────────────────────────────────────────────────────

/// HNSW vector backend wrapping [`HnswService`].
pub struct HnswBackend {
    inner: HnswService,
    id_map: Mutex<IdMap>,
    /// Monotonic epoch counter -- bumped on every mutation.
    epoch: AtomicU64,
    /// Soft-deleted entries keyed by vector ID.
    tombstones: Mutex<HashMap<u64, Tombstone>>,
    /// Optional upper bound on stored vectors.
    max_vectors: Mutex<Option<usize>>,
}

impl HnswBackend {
    /// Create a new HNSW backend with the given configuration.
    pub fn new(config: HnswServiceConfig) -> Self {
        Self {
            inner: HnswService::new(config),
            id_map: Mutex::new(IdMap::new()),
            epoch: AtomicU64::new(0),
            tombstones: Mutex::new(HashMap::new()),
            max_vectors: Mutex::new(None),
        }
    }

    /// Create with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(HnswServiceConfig::default())
    }

    /// Create with a capacity limit.
    pub fn with_max_vectors(config: HnswServiceConfig, limit: usize) -> Self {
        Self {
            inner: HnswService::new(config),
            id_map: Mutex::new(IdMap::new()),
            epoch: AtomicU64::new(0),
            tombstones: Mutex::new(HashMap::new()),
            max_vectors: Mutex::new(Some(limit)),
        }
    }

    /// Access the underlying [`HnswService`] for legacy code paths.
    pub fn inner(&self) -> &HnswService {
        &self.inner
    }

    /// Bump the epoch and return the new value.
    fn bump_epoch(&self) -> u64 {
        self.epoch.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Number of live (non-tombstoned) vectors.
    fn live_count(&self) -> usize {
        let map = self.id_map.lock().expect("IdMap lock poisoned");
        let ts = self.tombstones.lock().expect("tombstones lock poisoned");
        map.len().saturating_sub(ts.len())
    }

    /// Check capacity and return an error if full.
    fn check_capacity(
        &self,
        id_map: &IdMap,
        tombstones: &HashMap<u64, Tombstone>,
        id: u64,
    ) -> VectorResult<()> {
        let limit = self.max_vectors.lock().expect("max_vectors lock poisoned");
        if let Some(max) = *limit {
            let live = id_map.len().saturating_sub(tombstones.len());
            // Allow upsert of existing id.
            if live >= max && !id_map.contains_id(id) {
                return Err(VectorError::StoreFull { max, current: live });
            }
        }
        Ok(())
    }
}

impl VectorBackend for HnswBackend {
    fn insert(
        &self,
        id: u64,
        key: &str,
        vector: &[f32],
        metadata: serde_json::Value,
    ) -> VectorResult<()> {
        let mut map = self.id_map.lock().expect("IdMap lock poisoned");
        let mut ts = self.tombstones.lock().expect("tombstones lock poisoned");

        // If this id was tombstoned, un-tombstone it (re-insert).
        ts.remove(&id);

        self.check_capacity(&map, &ts, id)?;

        map.insert(id, key.to_owned());
        drop(ts);
        drop(map);

        self.inner.insert(key.to_owned(), vector.to_vec(), metadata);
        self.bump_epoch();
        Ok(())
    }

    fn search(&self, query: &[f32], k: usize) -> Vec<SearchResult> {
        let results = self.inner.search(query, k);
        let map = self.id_map.lock().expect("IdMap lock poisoned");
        let ts = self.tombstones.lock().expect("tombstones lock poisoned");

        results
            .into_iter()
            .filter_map(|r| {
                // Reverse-lookup the numeric id from the string key.
                map.key_to_id.get(&r.id).and_then(|&numeric_id| {
                    // Exclude tombstoned entries.
                    if ts.contains_key(&numeric_id) {
                        return None;
                    }
                    // HnswService returns cosine similarity (1.0 = identical).
                    // Convert to distance: distance = 1.0 - similarity.
                    let distance = 1.0 - r.score;
                    Some(SearchResult::new(numeric_id, r.id, distance, r.metadata))
                })
            })
            .collect()
    }

    fn len(&self) -> usize {
        self.live_count()
    }

    fn contains(&self, id: u64) -> bool {
        let map = self.id_map.lock().expect("IdMap lock poisoned");
        let ts = self.tombstones.lock().expect("tombstones lock poisoned");
        map.contains_id(id) && !ts.contains_key(&id)
    }

    fn remove(&self, id: u64) -> bool {
        let mut map = self.id_map.lock().expect("IdMap lock poisoned");
        let mut ts = self.tombstones.lock().expect("tombstones lock poisoned");
        ts.remove(&id);
        let removed = map.remove(id).is_some();
        if removed {
            drop(ts);
            drop(map);
            self.bump_epoch();
        }
        removed
    }

    fn flush(&self) -> VectorResult<()> {
        // HNSW is in-memory only; flush is a no-op.
        // Epoch is persisted by callers via save_to_file when needed.
        Ok(())
    }

    fn backend_name(&self) -> &str {
        "hnsw"
    }

    // ── Epoch ───────────────────────────────────────────────────────────

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

    // ── Soft-delete + compaction ────────────────────────────────────────

    fn soft_delete(&self, id: u64) -> bool {
        let map = self.id_map.lock().expect("IdMap lock poisoned");
        let mut ts = self.tombstones.lock().expect("tombstones lock poisoned");

        if !map.contains_id(id) || ts.contains_key(&id) {
            return false;
        }

        let epoch = self.bump_epoch();
        ts.insert(id, Tombstone { deleted_at_epoch: epoch });
        true
    }

    fn compact(&self, older_than_epoch: u64) -> usize {
        let mut map = self.id_map.lock().expect("IdMap lock poisoned");
        let mut ts = self.tombstones.lock().expect("tombstones lock poisoned");

        let to_purge: Vec<u64> = ts
            .iter()
            .filter(|(_, t)| t.deleted_at_epoch < older_than_epoch)
            .map(|(&id, _)| id)
            .collect();

        let count = to_purge.len();
        for id in to_purge {
            ts.remove(&id);
            map.remove(id);
        }

        if count > 0 {
            drop(ts);
            drop(map);
            self.bump_epoch();
        }

        count
    }

    fn tombstone_count(&self) -> usize {
        let ts = self.tombstones.lock().expect("tombstones lock poisoned");
        ts.len()
    }

    // ── Capacity limits ────────────────────────────────────────────────

    fn max_vectors(&self) -> Option<usize> {
        *self.max_vectors.lock().expect("max_vectors lock poisoned")
    }

    fn set_max_vectors(&self, limit: Option<usize>) {
        *self.max_vectors.lock().expect("max_vectors lock poisoned") = limit;
    }
}

impl std::fmt::Debug for HnswBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HnswBackend")
            .field("len", &self.len())
            .field("epoch", &self.current_epoch())
            .field("tombstones", &self.tombstone_count())
            .field("max_vectors", &self.max_vectors())
            .finish()
    }
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_backend() -> HnswBackend {
        HnswBackend::with_defaults()
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

        assert_eq!(b.len(), 3);
        assert!(!b.is_empty());

        let results = b.search(&[1.0, 0.0, 0.0], 2);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, 1);
        assert_eq!(results[0].key, "a");
        assert!(results[0].distance < 0.01);
    }

    #[test]
    fn contains_and_remove() {
        let b = make_backend();
        b.insert(10, "x", &[1.0, 0.0], serde_json::json!({}))
            .unwrap();
        assert!(b.contains(10));
        assert!(!b.contains(99));

        assert!(b.remove(10));
        assert!(!b.contains(10));
        assert!(!b.remove(10));
    }

    #[test]
    fn flush_is_noop() {
        let b = make_backend();
        b.flush().unwrap();
    }

    #[test]
    fn backend_name() {
        let b = make_backend();
        assert_eq!(b.backend_name(), "hnsw");
    }

    #[test]
    fn empty_search() {
        let b = make_backend();
        let results = b.search(&[1.0, 0.0], 5);
        assert!(results.is_empty());
    }

    // ── Epoch tests ─────────────────────────────────────────────────────

    #[test]
    fn epoch_starts_at_zero() {
        let b = make_backend();
        assert_eq!(b.current_epoch(), 0);
    }

    #[test]
    fn epoch_increments_on_insert() {
        let b = make_backend();
        b.insert(1, "a", &[1.0], serde_json::json!({})).unwrap();
        assert_eq!(b.current_epoch(), 1);
        b.insert(2, "b", &[0.0], serde_json::json!({})).unwrap();
        assert_eq!(b.current_epoch(), 2);
    }

    #[test]
    fn epoch_increments_on_remove() {
        let b = make_backend();
        b.insert(1, "a", &[1.0], serde_json::json!({})).unwrap();
        assert_eq!(b.current_epoch(), 1);
        b.remove(1);
        assert_eq!(b.current_epoch(), 2);
    }

    // ── Optimistic concurrency tests ────────────────────────────────────

    #[test]
    fn insert_with_epoch_succeeds_when_current() {
        let b = make_backend();
        let epoch = b.current_epoch();
        b.insert_with_epoch(1, "a", &[1.0], serde_json::json!({}), epoch)
            .unwrap();
        assert_eq!(b.len(), 1);
    }

    #[test]
    fn insert_with_epoch_rejects_stale() {
        let b = make_backend();
        b.insert(1, "a", &[1.0], serde_json::json!({})).unwrap();
        // Epoch is now 1. Trying with parent_epoch=0 should fail.
        let result = b.insert_with_epoch(2, "b", &[0.0], serde_json::json!({}), 0);
        assert!(result.is_err());
        match result.unwrap_err() {
            VectorError::EpochConflict { expected, actual } => {
                assert_eq!(expected, 0);
                assert_eq!(actual, 1);
            }
            other => panic!("expected EpochConflict, got: {other}"),
        }
    }

    #[test]
    fn insert_with_epoch_accepts_current_epoch() {
        let b = make_backend();
        b.insert(1, "a", &[1.0], serde_json::json!({})).unwrap();
        let epoch = b.current_epoch(); // 1
        b.insert_with_epoch(2, "b", &[0.0], serde_json::json!({}), epoch)
            .unwrap();
        assert_eq!(b.len(), 2);
    }

    // ── Soft-delete tests ───────────────────────────────────────────────

    #[test]
    fn soft_delete_hides_from_search() {
        let b = make_backend();
        b.insert(1, "a", &[1.0, 0.0, 0.0], serde_json::json!({}))
            .unwrap();
        b.insert(2, "b", &[0.9, 0.1, 0.0], serde_json::json!({}))
            .unwrap();

        assert!(b.soft_delete(1));
        assert_eq!(b.tombstone_count(), 1);
        assert!(!b.contains(1));

        // Search should not return tombstoned vector.
        let results = b.search(&[1.0, 0.0, 0.0], 5);
        for r in &results {
            assert_ne!(r.id, 1, "tombstoned vector should not appear in search");
        }
    }

    #[test]
    fn soft_delete_nonexistent_returns_false() {
        let b = make_backend();
        assert!(!b.soft_delete(999));
    }

    #[test]
    fn soft_delete_already_tombstoned_returns_false() {
        let b = make_backend();
        b.insert(1, "a", &[1.0], serde_json::json!({})).unwrap();
        assert!(b.soft_delete(1));
        assert!(!b.soft_delete(1));
    }

    #[test]
    fn soft_delete_reduces_len() {
        let b = make_backend();
        b.insert(1, "a", &[1.0], serde_json::json!({})).unwrap();
        b.insert(2, "b", &[0.0], serde_json::json!({})).unwrap();
        assert_eq!(b.len(), 2);
        b.soft_delete(1);
        assert_eq!(b.len(), 1);
    }

    #[test]
    fn reinsert_after_soft_delete() {
        let b = make_backend();
        b.insert(1, "a", &[1.0], serde_json::json!({})).unwrap();
        b.soft_delete(1);
        assert_eq!(b.len(), 0);
        assert_eq!(b.tombstone_count(), 1);

        // Re-insert same id clears the tombstone.
        b.insert(1, "a", &[0.5], serde_json::json!({})).unwrap();
        assert_eq!(b.len(), 1);
        assert_eq!(b.tombstone_count(), 0);
    }

    // ── Compaction tests ────────────────────────────────────────────────

    #[test]
    fn compact_purges_old_tombstones() {
        let b = make_backend();
        b.insert(1, "a", &[1.0], serde_json::json!({})).unwrap();
        b.insert(2, "b", &[0.0], serde_json::json!({})).unwrap();

        b.soft_delete(1);
        let delete_epoch = b.current_epoch();

        b.soft_delete(2);

        // Compact tombstones older than the second soft-delete epoch.
        // Only tombstone for id=1 should be purged.
        let purged = b.compact(delete_epoch + 1);
        assert_eq!(purged, 1);
        assert_eq!(b.tombstone_count(), 1);
    }

    #[test]
    fn compact_returns_zero_when_nothing_to_purge() {
        let b = make_backend();
        b.insert(1, "a", &[1.0], serde_json::json!({})).unwrap();
        assert_eq!(b.compact(100), 0);
    }

    // ── Capacity limit tests ────────────────────────────────────────────

    #[test]
    fn capacity_limit_enforced() {
        let b = HnswBackend::with_max_vectors(HnswServiceConfig::default(), 2);
        b.insert(1, "a", &[1.0], serde_json::json!({})).unwrap();
        b.insert(2, "b", &[0.0], serde_json::json!({})).unwrap();

        let result = b.insert(3, "c", &[0.5], serde_json::json!({}));
        assert!(result.is_err());
        match result.unwrap_err() {
            VectorError::StoreFull { max, current } => {
                assert_eq!(max, 2);
                assert_eq!(current, 2);
            }
            other => panic!("expected StoreFull, got: {other}"),
        }
    }

    #[test]
    fn capacity_upsert_does_not_count_double() {
        let b = HnswBackend::with_max_vectors(HnswServiceConfig::default(), 2);
        b.insert(1, "a", &[1.0], serde_json::json!({})).unwrap();
        b.insert(2, "b", &[0.0], serde_json::json!({})).unwrap();
        // Upsert existing id=1 should succeed.
        b.insert(1, "a-v2", &[0.5], serde_json::json!({})).unwrap();
    }

    #[test]
    fn capacity_default_is_none() {
        let b = make_backend();
        assert_eq!(b.max_vectors(), None);
    }

    #[test]
    fn set_max_vectors_at_runtime() {
        let b = make_backend();
        assert_eq!(b.max_vectors(), None);
        b.set_max_vectors(Some(10));
        assert_eq!(b.max_vectors(), Some(10));
        b.set_max_vectors(None);
        assert_eq!(b.max_vectors(), None);
    }

    #[test]
    fn soft_deleted_vectors_do_not_count_against_capacity() {
        let b = HnswBackend::with_max_vectors(HnswServiceConfig::default(), 2);
        b.insert(1, "a", &[1.0], serde_json::json!({})).unwrap();
        b.insert(2, "b", &[0.0], serde_json::json!({})).unwrap();

        // Soft-delete one -- frees a slot.
        b.soft_delete(1);

        // Should now accept a new insert.
        b.insert(3, "c", &[0.5], serde_json::json!({})).unwrap();
        assert_eq!(b.len(), 2);
    }
}
