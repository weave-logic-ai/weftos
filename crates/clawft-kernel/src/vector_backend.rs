//! Unified vector search backend trait.
//!
//! Provides a common interface for HNSW, DiskANN, and hybrid vector
//! search backends. The kernel creates the appropriate backend at boot
//! time based on [`VectorConfig`](clawft_types::config::VectorConfig).
//!
//! This module is compiled only when the `ecc` feature is enabled.

use serde::{Deserialize, Serialize};

// ── Search result ────────────────────────────────────────────────────────

/// A single search result returned by any [`VectorBackend`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// Entry identifier (numeric, for fast dedup/sorting).
    pub id: u64,
    /// String key originally associated with this vector (e.g., node id).
    pub key: String,
    /// Distance from the query vector (lower is closer).
    pub distance: f32,
    /// Arbitrary metadata stored alongside the embedding.
    #[serde(default)]
    pub metadata: serde_json::Value,
}

impl SearchResult {
    /// Create a new search result.
    pub fn new(id: u64, key: String, distance: f32, metadata: serde_json::Value) -> Self {
        Self {
            id,
            key,
            distance,
            metadata,
        }
    }
}

// ── Error ────────────────────────────────────────────────────────────────

/// Errors that can occur in vector backend operations.
#[derive(Debug, thiserror::Error)]
pub enum VectorError {
    /// The vector dimensions do not match the expected dimensionality.
    #[error("dimension mismatch: expected {expected}, got {got}")]
    DimensionMismatch { expected: usize, got: usize },

    /// I/O error during persistence operations.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// The backend capacity has been exceeded.
    #[error("capacity exceeded: max {max} entries")]
    CapacityExceeded { max: usize },

    /// The store has reached its configured vector limit.
    #[error("store full: {current}/{max} vectors")]
    StoreFull { max: usize, current: usize },

    /// Optimistic concurrency conflict: the caller's epoch is stale.
    #[error("epoch conflict: expected {expected}, actual {actual}")]
    EpochConflict { expected: u64, actual: u64 },

    /// Generic backend error.
    #[error("{0}")]
    Other(String),
}

/// Result alias for vector backend operations.
pub type VectorResult<T> = Result<T, VectorError>;

// ── Trait ────────────────────────────────────────────────────────────────

/// Unified vector search backend interface.
///
/// Implementations must be `Send + Sync` so they can be shared across
/// async tasks via `Arc`.
pub trait VectorBackend: Send + Sync {
    /// Insert a vector with the given numeric ID, string key, and metadata.
    ///
    /// If a vector with the same `id` already exists, it is replaced
    /// (upsert semantics).
    fn insert(
        &self,
        id: u64,
        key: &str,
        vector: &[f32],
        metadata: serde_json::Value,
    ) -> VectorResult<()>;

    /// Search for the `k` nearest vectors to `query`.
    ///
    /// Results are sorted by ascending distance (closest first).
    fn search(&self, query: &[f32], k: usize) -> Vec<SearchResult>;

    /// Return the number of vectors currently stored.
    fn len(&self) -> usize;

    /// Return `true` if the store contains no vectors.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Check whether a vector with the given ID exists.
    fn contains(&self, id: u64) -> bool;

    /// Remove a vector by ID. Returns `true` if it was present.
    fn remove(&self, id: u64) -> bool;

    /// Persist any in-memory state to durable storage.
    ///
    /// For purely in-memory backends (HNSW), this is a no-op.
    fn flush(&self) -> VectorResult<()>;

    /// Return a human-readable name for this backend (e.g. "hnsw", "diskann", "hybrid").
    fn backend_name(&self) -> &str;

    // ── Epoch-based versioning ──────────────────────────────────────────

    /// Return the current monotonic epoch.
    ///
    /// The epoch is bumped on every mutating operation (insert, delete,
    /// update, soft-delete, compact). Implementations should persist the
    /// epoch across restarts via [`flush`] / load.
    fn current_epoch(&self) -> u64 {
        0
    }

    // ── Optimistic concurrency control ──────────────────────────────────

    /// Insert a vector only if the store's current epoch matches
    /// `parent_epoch`. If the epoch has advanced past `parent_epoch`,
    /// the call is rejected with [`VectorError::EpochConflict`].
    ///
    /// This enables optimistic concurrency control for multi-agent
    /// writes: each agent reads the epoch, performs its work, then
    /// submits with the epoch it observed.
    fn insert_with_epoch(
        &self,
        id: u64,
        key: &str,
        vector: &[f32],
        metadata: serde_json::Value,
        parent_epoch: u64,
    ) -> VectorResult<()> {
        let current = self.current_epoch();
        if parent_epoch < current {
            return Err(VectorError::EpochConflict {
                expected: parent_epoch,
                actual: current,
            });
        }
        self.insert(id, key, vector, metadata)
    }

    // ── Soft-delete + compaction ────────────────────────────────────────

    /// Mark a vector as tombstoned instead of physically removing it.
    ///
    /// Tombstoned vectors are excluded from search results but remain
    /// in storage until [`compact`] is called. Returns `true` if the
    /// vector existed and was not already tombstoned.
    fn soft_delete(&self, id: u64) -> bool {
        // Default: fall back to hard delete for backends that do not
        // implement tombstoning.
        self.remove(id)
    }

    /// Physically remove tombstones whose deletion epoch is older than
    /// `older_than_epoch`. Returns the number of entries purged.
    fn compact(&self, _older_than_epoch: u64) -> usize {
        0
    }

    /// Return the number of tombstoned (soft-deleted) entries.
    fn tombstone_count(&self) -> usize {
        0
    }

    // ── Capacity limits ────────────────────────────────────────────────

    /// Return the configured maximum number of vectors, or `None` if
    /// unbounded.
    fn max_vectors(&self) -> Option<usize> {
        None
    }

    /// Set the maximum number of vectors. Pass `None` for unbounded.
    fn set_max_vectors(&self, _limit: Option<usize>) {
        // Default: no-op. Implementations that support runtime
        // reconfiguration should override.
    }
}

// ── Backend configuration dispatch ───────────────────────────────────────

/// Which backend to construct, read from configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum VectorBackendKind {
    /// In-memory HNSW (default, fast, suitable for <1M vectors).
    #[default]
    Hnsw,
    /// SSD-backed DiskANN (large scale, 1M+ vectors).
    DiskAnn,
    /// Hot HNSW cache + cold DiskANN store.
    Hybrid,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_result_new() {
        let sr = SearchResult::new(1, "foo".into(), 0.5, serde_json::json!({"a": 1}));
        assert_eq!(sr.id, 1);
        assert_eq!(sr.key, "foo");
        assert!((sr.distance - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn backend_kind_default_is_hnsw() {
        assert_eq!(VectorBackendKind::default(), VectorBackendKind::Hnsw);
    }

    #[test]
    fn backend_kind_deserialize() {
        let kind: VectorBackendKind = serde_json::from_str(r#""hnsw""#).unwrap();
        assert_eq!(kind, VectorBackendKind::Hnsw);
        let kind: VectorBackendKind = serde_json::from_str(r#""diskann""#).unwrap();
        assert_eq!(kind, VectorBackendKind::DiskAnn);
        let kind: VectorBackendKind = serde_json::from_str(r#""hybrid""#).unwrap();
        assert_eq!(kind, VectorBackendKind::Hybrid);
    }

    #[test]
    fn error_store_full_display() {
        let err = VectorError::StoreFull {
            max: 100,
            current: 100,
        };
        assert!(format!("{err}").contains("100"));
    }

    #[test]
    fn error_epoch_conflict_display() {
        let err = VectorError::EpochConflict {
            expected: 5,
            actual: 10,
        };
        let msg = format!("{err}");
        assert!(msg.contains("5"));
        assert!(msg.contains("10"));
    }
}
