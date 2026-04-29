//! Stub vector store with an RVF-compatible interface.
//!
//! Provides [`RvfStore`], an in-memory vector store that mirrors the API
//! shape of `rvf-runtime::RvfStore` (create, open, ingest, query, delete,
//! compact). This stub uses brute-force cosine similarity and serializes
//! to a JSON file for persistence.
//!
//! When the real `rvf-runtime` crate is integrated, this stub can be
//! replaced or wrapped while keeping the same interface.
//!
//! This module is gated behind the `rvf` feature flag.
//!
//! # Why this is the active path (and `rvf_io` is gone)
//!
//! Originally the embeddings module shipped two RVF shapes side by side:
//! this `rvf_stub` (brute-force, JSON-backed, used by
//! [`crate::memory_bootstrap`]) and a forward-compatible `rvf_io`
//! module modeling segment files with inline WITNESS chains. The
//! `rvf_io` shape was the planned target once the upstream
//! `rvf-runtime` 0.2 binary format stabilized, but it carried no
//! callers in-tree and risked becoming bit-rot.
//!
//! In WEFT-93 (release-gate audit `06-memory-workspace.md` row WS-O2 /
//! task MW-15) we picked one fate: keep `rvf_stub` as the live path
//! and delete `rvf_io`. The reasoning:
//!
//! 1. `rvf_stub` is exercised by `memory_bootstrap.rs` and its tests.
//!    `rvf_io` had no callers anywhere in the workspace.
//! 2. Dual implementations rot fast and confuse new contributors.
//! 3. The forward-compatible segment shape can return when there is a
//!    real consumer (e.g. once `rvf-runtime` >= 0.3 ships a stable
//!    on-disk format).
//!
//! The decision is recorded in
//! `.planning/development_notes/08-memory-workspace/h2-vector-memory/decisions.md`.
//! If you find yourself adding a second RVF shape: file a Plane item
//! for the migration plan first, don't ship them side by side.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::debug;

/// Errors from the RVF stub store.
#[non_exhaustive]
#[derive(Debug)]
pub enum RvfError {
    /// An I/O error occurred (reading/writing the store file).
    Io(std::io::Error),
    /// A serialization/deserialization error occurred.
    Serde(serde_json::Error),
    /// The entry was not found.
    NotFound(String),
}

impl std::fmt::Display for RvfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RvfError::Io(e) => write!(f, "rvf I/O error: {e}"),
            RvfError::Serde(e) => write!(f, "rvf serde error: {e}"),
            RvfError::NotFound(id) => write!(f, "rvf entry not found: {id}"),
        }
    }
}

impl std::error::Error for RvfError {}

impl From<std::io::Error> for RvfError {
    fn from(e: std::io::Error) -> Self {
        RvfError::Io(e)
    }
}

impl From<serde_json::Error> for RvfError {
    fn from(e: serde_json::Error) -> Self {
        RvfError::Serde(e)
    }
}

/// A single entry in the RVF stub store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RvfEntry {
    /// Unique identifier.
    pub id: String,
    /// The embedding vector.
    pub embedding: Vec<f32>,
    /// Arbitrary metadata.
    pub metadata: serde_json::Value,
}

/// A query result from the store.
#[derive(Debug, Clone)]
pub struct RvfQueryResult {
    /// The entry ID.
    pub id: String,
    /// Cosine similarity score.
    pub score: f32,
    /// The entry metadata.
    pub metadata: serde_json::Value,
}

/// Serializable store state.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoreState {
    entries: Vec<RvfEntry>,
}

/// In-memory vector store with an RVF-compatible interface.
///
/// The store holds entries in a `Vec` and performs brute-force cosine
/// similarity for queries. It can be persisted to a JSON file via
/// [`compact`](RvfStore::compact) and reloaded via [`open`](RvfStore::open).
///
/// # Thread Safety
///
/// This store is NOT thread-safe. Wrap in a `Mutex` or `RwLock` if
/// concurrent access is needed.
pub struct RvfStore {
    entries: Vec<RvfEntry>,
    path: Option<PathBuf>,
}

impl RvfStore {
    /// Create a new, empty in-memory store with an optional file path.
    ///
    /// If `path` is provided, [`compact`](RvfStore::compact) will persist
    /// the store to that file.
    pub fn create(path: Option<&Path>) -> Self {
        debug!(path = ?path, "creating new RvfStore");
        Self {
            entries: Vec::new(),
            path: path.map(|p| p.to_path_buf()),
        }
    }

    /// Open an existing store from a JSON file.
    ///
    /// If the file does not exist, creates a new empty store at that path.
    pub fn open(path: &Path) -> Result<Self, RvfError> {
        if path.exists() {
            debug!(path = %path.display(), "opening existing RvfStore");
            let data = std::fs::read_to_string(path)?;
            let state: StoreState = serde_json::from_str(&data)?;
            Ok(Self {
                entries: state.entries,
                path: Some(path.to_path_buf()),
            })
        } else {
            debug!(path = %path.display(), "no existing store, creating new");
            Ok(Self::create(Some(path)))
        }
    }

    /// Ingest (add) an entry into the store.
    pub fn ingest(&mut self, id: String, embedding: Vec<f32>, metadata: serde_json::Value) {
        // Remove existing entry with same ID (upsert semantics)
        self.entries.retain(|e| e.id != id);
        self.entries.push(RvfEntry {
            id,
            embedding,
            metadata,
        });
    }

    /// Query the store for the top-k most similar entries.
    ///
    /// Returns results sorted by descending cosine similarity.
    pub fn query(&self, query_embedding: &[f32], top_k: usize) -> Vec<RvfQueryResult> {
        if self.entries.is_empty() || top_k == 0 {
            return Vec::new();
        }

        let mut scored: Vec<RvfQueryResult> = self
            .entries
            .iter()
            .map(|entry| {
                let score = cosine_similarity(query_embedding, &entry.embedding);
                RvfQueryResult {
                    id: entry.id.clone(),
                    score,
                    metadata: entry.metadata.clone(),
                }
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

    /// Delete an entry by ID. Returns `true` if an entry was removed.
    pub fn delete(&mut self, id: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.id != id);
        self.entries.len() < before
    }

    /// Persist the store to its file path (if set).
    ///
    /// This is analogous to RVF compaction -- in the stub, it simply
    /// serializes all entries to JSON.
    pub fn compact(&self) -> Result<(), RvfError> {
        let Some(ref path) = self.path else {
            return Ok(()); // no path, nothing to persist
        };

        debug!(path = %path.display(), entries = self.entries.len(), "compacting RvfStore");

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let state = StoreState {
            entries: self.entries.clone(),
        };
        let json = serde_json::to_string_pretty(&state)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Return the number of entries in the store.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Return `true` if the store has no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Get the file path (if set).
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Get an entry by ID.
    pub fn get(&self, id: &str) -> Option<&RvfEntry> {
        self.entries.iter().find(|e| e.id == id)
    }
}

/// Compute cosine similarity between two vectors.
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

    fn temp_path(label: &str) -> PathBuf {
        let n = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        std::env::temp_dir().join(format!("clawft_rvf_test_{label}_{pid}_{n}.json"))
    }

    #[test]
    fn create_empty_store() {
        let store = RvfStore::create(None);
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn ingest_and_query() {
        let mut store = RvfStore::create(None);
        store.ingest(
            "doc1".into(),
            vec![1.0, 0.0, 0.0],
            serde_json::json!({"text": "hello"}),
        );
        store.ingest(
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
        let mut store = RvfStore::create(None);
        store.ingest("a".into(), vec![1.0, 0.0], serde_json::json!({}));
        store.ingest("b".into(), vec![0.7, 0.7], serde_json::json!({}));
        store.ingest("c".into(), vec![0.0, 1.0], serde_json::json!({}));

        let results = store.query(&[1.0, 0.0], 3);
        assert_eq!(results[0].id, "a");
        assert_eq!(results[1].id, "b");
        assert_eq!(results[2].id, "c");
    }

    #[test]
    fn delete_entry() {
        let mut store = RvfStore::create(None);
        store.ingest("doc1".into(), vec![1.0], serde_json::json!({}));
        store.ingest("doc2".into(), vec![0.0], serde_json::json!({}));

        assert!(store.delete("doc1"));
        assert_eq!(store.len(), 1);
        assert!(!store.delete("doc1")); // already gone
    }

    #[test]
    fn upsert_semantics() {
        let mut store = RvfStore::create(None);
        store.ingest("doc1".into(), vec![1.0, 0.0], serde_json::json!({"v": 1}));
        store.ingest("doc1".into(), vec![0.0, 1.0], serde_json::json!({"v": 2}));

        assert_eq!(store.len(), 1);
        let entry = store.get("doc1").unwrap();
        assert_eq!(entry.metadata["v"], 2);
        assert_eq!(entry.embedding, vec![0.0, 1.0]);
    }

    #[test]
    fn compact_and_open_roundtrip() {
        let path = temp_path("roundtrip");

        // Create, ingest, compact
        {
            let mut store = RvfStore::create(Some(&path));
            store.ingest(
                "doc1".into(),
                vec![1.0, 0.0, 0.0],
                serde_json::json!({"text": "hello"}),
            );
            store.ingest(
                "doc2".into(),
                vec![0.0, 1.0, 0.0],
                serde_json::json!({"text": "world"}),
            );
            store.compact().unwrap();
        }

        // Reopen and verify
        let store = RvfStore::open(&path).unwrap();
        assert_eq!(store.len(), 2);

        let results = store.query(&[1.0, 0.0, 0.0], 1);
        assert_eq!(results[0].id, "doc1");

        // Cleanup
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn open_nonexistent_creates_empty() {
        let path = temp_path("nonexist");
        let _ = std::fs::remove_file(&path); // ensure it doesn't exist

        let store = RvfStore::open(&path).unwrap();
        assert!(store.is_empty());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn query_empty_store() {
        let store = RvfStore::create(None);
        let results = store.query(&[1.0, 0.0], 5);
        assert!(results.is_empty());
    }

    #[test]
    fn query_top_k_zero() {
        let mut store = RvfStore::create(None);
        store.ingest("x".into(), vec![1.0], serde_json::json!({}));
        let results = store.query(&[1.0], 0);
        assert!(results.is_empty());
    }

    #[test]
    fn get_entry_by_id() {
        let mut store = RvfStore::create(None);
        store.ingest(
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
    fn cosine_similarity_opposite() {
        let score = cosine_similarity(&[1.0, 0.0], &[-1.0, 0.0]);
        assert!((score + 1.0).abs() < 0.01);
    }

    #[test]
    fn cosine_similarity_different_lengths() {
        let score = cosine_similarity(&[1.0, 0.0], &[1.0]);
        assert!((score - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn rvf_error_display() {
        let err = RvfError::NotFound("xyz".into());
        assert!(format!("{err}").contains("xyz"));

        let err = RvfError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "test"));
        assert!(format!("{err}").contains("test"));
    }

    #[test]
    fn store_path_accessor() {
        let path = temp_path("path_acc");
        let store = RvfStore::create(Some(&path));
        assert_eq!(store.path(), Some(path.as_path()));

        let store2 = RvfStore::create(None);
        assert!(store2.path().is_none());
    }

    #[test]
    fn compact_without_path_is_noop() {
        let store = RvfStore::create(None);
        assert!(store.compact().is_ok());
    }
}
