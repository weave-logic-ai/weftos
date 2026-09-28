//! In-memory vector store with cosine similarity search.
//!
//! Provides a simple [`VectorStore`] that holds embedding vectors alongside
//! their text, tags, and metadata. Search is brute-force cosine similarity
//! over all entries, returning top-k results sorted by descending score.
//!
//! This module is gated behind the `vector-memory` feature flag.
//!
//! Frozen first-stage retriever in the RMM landing (arXiv:2503.08026):
//! Top-K comes from here (or HNSW/RVF); a lightweight SONA reranker
//! learns from generator citations. Do not RL-update these vectors from
//! ±1 citation rewards. See
//! `docs/research/rmm-reflective-memory-management.md`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// A single entry in the vector store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorEntry {
    /// Unique identifier for this entry.
    pub id: String,
    /// The original text that was embedded.
    pub text: String,
    /// The embedding vector.
    pub embedding: Vec<f32>,
    /// Tags for categorization and filtering.
    pub tags: Vec<String>,
    /// Arbitrary metadata.
    pub metadata: HashMap<String, serde_json::Value>,
    /// Unix timestamp (seconds since epoch) when the entry was added.
    pub timestamp: u64,
}

/// A search result from the vector store.
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// The entry ID.
    pub id: String,
    /// The original text.
    pub text: String,
    /// Cosine similarity score (higher = more similar).
    pub score: f32,
    /// Tags from the entry.
    pub tags: Vec<String>,
    /// Timestamp from the entry.
    pub timestamp: u64,
}

/// In-memory vector store with brute-force cosine similarity search.
///
/// Suitable for small to medium datasets (up to tens of thousands of entries).
/// For larger datasets, swap to an HNSW-based backend (e.g. RVF) when available.
#[derive(Debug, Default)]
pub struct VectorStore {
    entries: Vec<VectorEntry>,
}

impl VectorStore {
    /// Create a new, empty vector store.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Add an entry to the store.
    pub fn add(
        &mut self,
        id: String,
        text: String,
        embedding: Vec<f32>,
        tags: Vec<String>,
        metadata: HashMap<String, serde_json::Value>,
    ) {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        self.entries.push(VectorEntry {
            id,
            text,
            embedding,
            tags,
            metadata,
            timestamp,
        });
    }

    /// Add an entry with an explicit timestamp.
    pub fn add_with_timestamp(
        &mut self,
        id: String,
        text: String,
        embedding: Vec<f32>,
        tags: Vec<String>,
        metadata: HashMap<String, serde_json::Value>,
        timestamp: u64,
    ) {
        self.entries.push(VectorEntry {
            id,
            text,
            embedding,
            tags,
            metadata,
            timestamp,
        });
    }

    /// Search for the top-k most similar entries to the query embedding.
    ///
    /// Results are sorted by descending cosine similarity score.
    pub fn search(&self, query_embedding: &[f32], top_k: usize) -> Vec<SearchResult> {
        if self.entries.is_empty() || top_k == 0 {
            return Vec::new();
        }

        let mut scored: Vec<SearchResult> = self
            .entries
            .iter()
            .map(|entry| {
                let score = cosine_similarity(query_embedding, &entry.embedding);
                SearchResult {
                    id: entry.id.clone(),
                    text: entry.text.clone(),
                    score,
                    tags: entry.tags.clone(),
                    timestamp: entry.timestamp,
                }
            })
            .collect();

        // Sort by descending score.
        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        scored.truncate(top_k);
        scored
    }

    /// Remove an entry by ID. Returns `true` if the entry was found and removed.
    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.id != id);
        self.entries.len() < before
    }

    /// Return the number of entries in the store.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Return `true` if the store has no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Get all entries (for serialization / inspection).
    pub fn entries(&self) -> &[VectorEntry] {
        &self.entries
    }
}

/// Compute cosine similarity between two vectors.
///
/// Returns 0.0 if either vector has zero norm.
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

    fn make_embedding(vals: &[f32]) -> Vec<f32> {
        vals.to_vec()
    }

    #[test]
    fn add_and_search() {
        let mut store = VectorStore::new();
        store.add(
            "doc1".into(),
            "hello world".into(),
            make_embedding(&[1.0, 0.0, 0.0]),
            vec!["greeting".into()],
            HashMap::new(),
        );
        store.add(
            "doc2".into(),
            "goodbye moon".into(),
            make_embedding(&[0.0, 1.0, 0.0]),
            vec!["farewell".into()],
            HashMap::new(),
        );

        let results = store.search(&[1.0, 0.0, 0.0], 1);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "doc1");
        assert!((results[0].score - 1.0).abs() < 0.01);
    }

    #[test]
    fn top_k_ordering() {
        let mut store = VectorStore::new();
        store.add(
            "a".into(),
            "text a".into(),
            make_embedding(&[1.0, 0.0]),
            vec![],
            HashMap::new(),
        );
        store.add(
            "b".into(),
            "text b".into(),
            make_embedding(&[0.7, 0.7]),
            vec![],
            HashMap::new(),
        );
        store.add(
            "c".into(),
            "text c".into(),
            make_embedding(&[0.0, 1.0]),
            vec![],
            HashMap::new(),
        );

        let results = store.search(&[1.0, 0.0], 3);
        assert_eq!(results.len(), 3);
        // "a" should be first (exact match), then "b" (partial), then "c" (orthogonal)
        assert_eq!(results[0].id, "a");
        assert_eq!(results[1].id, "b");
        assert_eq!(results[2].id, "c");
        assert!(results[0].score >= results[1].score);
        assert!(results[1].score >= results[2].score);
    }

    #[test]
    fn remove_entry() {
        let mut store = VectorStore::new();
        store.add(
            "doc1".into(),
            "hello".into(),
            make_embedding(&[1.0, 0.0]),
            vec![],
            HashMap::new(),
        );
        store.add(
            "doc2".into(),
            "world".into(),
            make_embedding(&[0.0, 1.0]),
            vec![],
            HashMap::new(),
        );

        assert_eq!(store.len(), 2);
        assert!(store.remove("doc1"));
        assert_eq!(store.len(), 1);
        assert!(!store.remove("doc1")); // Already removed.
    }

    #[test]
    fn empty_store_search_returns_empty() {
        let store = VectorStore::new();
        let results = store.search(&[1.0, 0.0], 5);
        assert!(results.is_empty());
    }

    #[test]
    fn identical_text_high_score() {
        let mut store = VectorStore::new();
        let emb = make_embedding(&[0.5, 0.5, 0.5]);
        store.add(
            "same".into(),
            "same text".into(),
            emb.clone(),
            vec![],
            HashMap::new(),
        );

        let results = store.search(&emb, 1);
        assert_eq!(results.len(), 1);
        assert!(
            (results[0].score - 1.0).abs() < 0.01,
            "identical embedding should score ~1.0, got {}",
            results[0].score
        );
    }

    #[test]
    fn dissimilar_text_low_score() {
        let mut store = VectorStore::new();
        store.add(
            "pos".into(),
            "positive".into(),
            make_embedding(&[1.0, 0.0, 0.0]),
            vec![],
            HashMap::new(),
        );

        // Opposite direction.
        let results = store.search(&[-1.0, 0.0, 0.0], 1);
        assert_eq!(results.len(), 1);
        assert!(
            results[0].score < 0.0,
            "opposite embeddings should have negative cosine, got {}",
            results[0].score
        );
    }

    #[test]
    fn len_and_is_empty() {
        let mut store = VectorStore::new();
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);

        store.add(
            "x".into(),
            "text".into(),
            make_embedding(&[1.0]),
            vec![],
            HashMap::new(),
        );
        assert!(!store.is_empty());
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn search_with_top_k_zero_returns_empty() {
        let mut store = VectorStore::new();
        store.add(
            "x".into(),
            "text".into(),
            make_embedding(&[1.0]),
            vec![],
            HashMap::new(),
        );

        let results = store.search(&[1.0], 0);
        assert!(results.is_empty());
    }

    #[test]
    fn search_top_k_larger_than_store() {
        let mut store = VectorStore::new();
        store.add(
            "only".into(),
            "only entry".into(),
            make_embedding(&[1.0, 0.0]),
            vec![],
            HashMap::new(),
        );

        let results = store.search(&[1.0, 0.0], 100);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn cosine_similarity_zero_vectors() {
        let score = cosine_similarity(&[0.0, 0.0], &[1.0, 0.0]);
        assert!((score - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn cosine_similarity_different_lengths() {
        let score = cosine_similarity(&[1.0, 0.0], &[1.0]);
        assert!((score - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn cosine_similarity_orthogonal() {
        let score = cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]);
        assert!(
            score.abs() < 0.01,
            "orthogonal vectors should have ~0 cosine, got {score}"
        );
    }

    #[test]
    fn add_with_timestamp_works() {
        let mut store = VectorStore::new();
        store.add_with_timestamp(
            "ts".into(),
            "timestamped".into(),
            make_embedding(&[1.0]),
            vec![],
            HashMap::new(),
            1234567890,
        );

        let results = store.search(&[1.0], 1);
        assert_eq!(results[0].timestamp, 1234567890);
    }

    #[test]
    fn search_preserves_tags() {
        let mut store = VectorStore::new();
        store.add(
            "tagged".into(),
            "tagged entry".into(),
            make_embedding(&[1.0, 0.0]),
            vec!["important".into(), "test".into()],
            HashMap::new(),
        );

        let results = store.search(&[1.0, 0.0], 1);
        assert_eq!(results[0].tags, vec!["important", "test"]);
    }

    #[test]
    fn default_creates_empty_store() {
        let store = VectorStore::default();
        assert!(store.is_empty());
    }

    #[test]
    fn entries_accessor() {
        let mut store = VectorStore::new();
        store.add(
            "e1".into(),
            "entry 1".into(),
            make_embedding(&[1.0]),
            vec![],
            HashMap::new(),
        );
        assert_eq!(store.entries().len(), 1);
        assert_eq!(store.entries()[0].id, "e1");
    }
}
