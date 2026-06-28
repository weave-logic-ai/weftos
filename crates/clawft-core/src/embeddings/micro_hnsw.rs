//! Micro-HNSW -- minimal approximate nearest neighbor search (H2.8).
//!
//! A compact HNSW implementation designed for the WASM micro-HNSW module
//! with an 8KB compiled size budget. This module provides the core
//! algorithm that can be compiled to WASM independently.
//!
//! # Design
//!
//! - Single-layer HNSW graph (no multi-layer hierarchy for size).
//! - Maximum 1024 vectors.
//! - Fixed-width neighbor lists (max 16 neighbors per node).
//! - Communication with the main WASM agent via message passing.
//! - Cosine similarity as the distance metric.
//!
//! # Message Passing Protocol
//!
//! The micro-HNSW module communicates via serializable messages:
//!
//! - `MicroHnswRequest::Insert` -- add a vector.
//! - `MicroHnswRequest::Query` -- find k nearest neighbors.
//! - `MicroHnswRequest::Delete` -- remove a vector by ID.
//! - `MicroHnswResponse::Results` -- query results.
//! - `MicroHnswResponse::Ok` -- operation succeeded.
//! - `MicroHnswResponse::Error` -- operation failed.
//!
//! This module is gated behind the `vector-memory` feature flag.

use serde::{Deserialize, Serialize};

/// Maximum number of neighbors per node.
const MAX_NEIGHBORS: usize = 16;

/// Maximum number of vectors in the micro index.
const MAX_VECTORS: usize = 1024;

// ── Message passing protocol ────────────────────────────────────────

/// Request messages from the main agent to the micro-HNSW module.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MicroHnswRequest {
    /// Insert a vector with the given ID.
    Insert {
        /// Vector ID.
        id: u32,
        /// The embedding vector.
        embedding: Vec<f32>,
    },
    /// Query for the k nearest neighbors.
    Query {
        /// The query embedding.
        embedding: Vec<f32>,
        /// Number of results to return.
        k: usize,
    },
    /// Delete a vector by ID.
    Delete {
        /// Vector ID to delete.
        id: u32,
    },
    /// Get the number of vectors in the index.
    Count,
}

/// Response messages from the micro-HNSW module to the main agent.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MicroHnswResponse {
    /// Query results: list of (id, score) pairs.
    Results(Vec<(u32, f32)>),
    /// Operation succeeded.
    Ok,
    /// Operation failed with an error message.
    Error(String),
    /// Count response.
    Count(usize),
}

// ── Node ────────────────────────────────────────────────────────────

/// A node in the micro-HNSW graph.
#[derive(Debug, Clone)]
struct MicroNode {
    id: u32,
    embedding: Vec<f32>,
    /// Neighbor IDs (indices into the nodes array).
    neighbors: Vec<usize>,
}

// ── MicroHnsw ───────────────────────────────────────────────────────

/// A minimal HNSW index for WASM deployment.
///
/// Uses a single-layer graph with greedy search. Designed for small
/// datasets (<= 1024 vectors) with compact binary size.
pub struct MicroHnsw {
    nodes: Vec<MicroNode>,
    dimension: usize,
}

impl MicroHnsw {
    /// Create a new, empty micro-HNSW index.
    pub fn new(dimension: usize) -> Self {
        Self {
            nodes: Vec::new(),
            dimension,
        }
    }

    /// Process a request message and return a response.
    pub fn process(&mut self, request: MicroHnswRequest) -> MicroHnswResponse {
        match request {
            MicroHnswRequest::Insert { id, embedding } => self.insert(id, embedding),
            MicroHnswRequest::Query { embedding, k } => self.query(&embedding, k),
            MicroHnswRequest::Delete { id } => self.delete(id),
            MicroHnswRequest::Count => MicroHnswResponse::Count(self.nodes.len()),
        }
    }

    /// Insert a vector into the index.
    fn insert(&mut self, id: u32, embedding: Vec<f32>) -> MicroHnswResponse {
        if self.nodes.len() >= MAX_VECTORS {
            return MicroHnswResponse::Error("micro-HNSW capacity exceeded".into());
        }

        if embedding.len() != self.dimension {
            return MicroHnswResponse::Error(format!(
                "dimension mismatch: expected {}, got {}",
                self.dimension,
                embedding.len()
            ));
        }

        // Remove existing node with same ID (upsert).
        self.remove_node(id);

        let new_idx = self.nodes.len();

        // Find nearest neighbors for the new node.
        let neighbors = self.find_nearest_indices(&embedding, MAX_NEIGHBORS);

        self.nodes.push(MicroNode {
            id,
            embedding,
            neighbors: neighbors.clone(),
        });

        // Add back-links from neighbors to the new node.
        for &neighbor_idx in &neighbors {
            if neighbor_idx < self.nodes.len() - 1 {
                let node = &mut self.nodes[neighbor_idx];
                if node.neighbors.len() < MAX_NEIGHBORS {
                    node.neighbors.push(new_idx);
                }
            }
        }

        MicroHnswResponse::Ok
    }

    /// Query the index for the k nearest neighbors.
    fn query(&self, embedding: &[f32], k: usize) -> MicroHnswResponse {
        if self.nodes.is_empty() || k == 0 {
            return MicroHnswResponse::Results(Vec::new());
        }

        // For small indices, brute-force is fine.
        if self.nodes.len() <= 64 {
            return self.brute_force_query(embedding, k);
        }

        // Greedy graph search.
        let mut visited = vec![false; self.nodes.len()];
        let mut results: Vec<(u32, f32)> = Vec::new();

        // Start from a random entry point (first node).
        let mut candidates = vec![0usize];
        visited[0] = true;

        while let Some(idx) = candidates.pop() {
            let node = &self.nodes[idx];
            let score = cosine_similarity(embedding, &node.embedding);
            results.push((node.id, score));

            // Expand neighbors.
            for &neighbor_idx in &node.neighbors {
                if neighbor_idx < self.nodes.len() && !visited[neighbor_idx] {
                    visited[neighbor_idx] = true;
                    candidates.push(neighbor_idx);
                }
            }
        }

        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(k);

        MicroHnswResponse::Results(results)
    }

    /// Delete a vector by ID.
    fn delete(&mut self, id: u32) -> MicroHnswResponse {
        if self.remove_node(id) {
            MicroHnswResponse::Ok
        } else {
            MicroHnswResponse::Error(format!("node {id} not found"))
        }
    }

    /// Remove a node by ID, fixing neighbor links.
    fn remove_node(&mut self, id: u32) -> bool {
        let pos = self.nodes.iter().position(|n| n.id == id);
        let Some(removed_idx) = pos else {
            return false;
        };

        self.nodes.remove(removed_idx);

        // Fix all neighbor indices.
        for node in &mut self.nodes {
            node.neighbors.retain(|&idx| idx != removed_idx);
            for idx in &mut node.neighbors {
                if *idx > removed_idx {
                    *idx -= 1;
                }
            }
        }

        true
    }

    /// Find the indices of the k nearest nodes to the given embedding.
    fn find_nearest_indices(&self, embedding: &[f32], k: usize) -> Vec<usize> {
        let mut scored: Vec<(usize, f32)> = self
            .nodes
            .iter()
            .enumerate()
            .map(|(i, node)| (i, cosine_similarity(embedding, &node.embedding)))
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(k);
        scored.into_iter().map(|(i, _)| i).collect()
    }

    /// Brute-force query for small datasets.
    fn brute_force_query(&self, embedding: &[f32], k: usize) -> MicroHnswResponse {
        let mut results: Vec<(u32, f32)> = self
            .nodes
            .iter()
            .map(|node| (node.id, cosine_similarity(embedding, &node.embedding)))
            .collect();

        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(k);

        MicroHnswResponse::Results(results)
    }

    /// Return the number of vectors in the index.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Return `true` if the index is empty.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

impl Default for MicroHnsw {
    fn default() -> Self {
        Self::new(0)
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

    #[test]
    fn empty_index() {
        let index = MicroHnsw::new(3);
        assert!(index.is_empty());
        assert_eq!(index.len(), 0);
    }

    #[test]
    fn insert_and_query() {
        let mut index = MicroHnsw::new(3);
        let resp = index.process(MicroHnswRequest::Insert {
            id: 1,
            embedding: vec![1.0, 0.0, 0.0],
        });
        assert!(matches!(resp, MicroHnswResponse::Ok));

        let resp = index.process(MicroHnswRequest::Insert {
            id: 2,
            embedding: vec![0.0, 1.0, 0.0],
        });
        assert!(matches!(resp, MicroHnswResponse::Ok));

        let resp = index.process(MicroHnswRequest::Query {
            embedding: vec![1.0, 0.0, 0.0],
            k: 1,
        });
        match resp {
            MicroHnswResponse::Results(results) => {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].0, 1);
                assert!((results[0].1 - 1.0).abs() < 0.01);
            }
            _ => panic!("expected Results"),
        }
    }

    #[test]
    fn delete_node() {
        let mut index = MicroHnsw::new(2);
        index.process(MicroHnswRequest::Insert {
            id: 1,
            embedding: vec![1.0, 0.0],
        });
        index.process(MicroHnswRequest::Insert {
            id: 2,
            embedding: vec![0.0, 1.0],
        });

        let resp = index.process(MicroHnswRequest::Delete { id: 1 });
        assert!(matches!(resp, MicroHnswResponse::Ok));

        let resp = index.process(MicroHnswRequest::Count);
        assert!(matches!(resp, MicroHnswResponse::Count(1)));
    }

    #[test]
    fn delete_nonexistent() {
        let mut index = MicroHnsw::new(2);
        let resp = index.process(MicroHnswRequest::Delete { id: 99 });
        assert!(matches!(resp, MicroHnswResponse::Error(_)));
    }

    #[test]
    fn upsert_semantics() {
        let mut index = MicroHnsw::new(2);
        index.process(MicroHnswRequest::Insert {
            id: 1,
            embedding: vec![1.0, 0.0],
        });
        index.process(MicroHnswRequest::Insert {
            id: 1,
            embedding: vec![0.0, 1.0],
        });

        assert_eq!(index.len(), 1);

        let resp = index.process(MicroHnswRequest::Query {
            embedding: vec![0.0, 1.0],
            k: 1,
        });
        match resp {
            MicroHnswResponse::Results(results) => {
                assert_eq!(results[0].0, 1);
                assert!((results[0].1 - 1.0).abs() < 0.01);
            }
            _ => panic!("expected Results"),
        }
    }

    #[test]
    fn dimension_mismatch_rejected() {
        let mut index = MicroHnsw::new(3);
        let resp = index.process(MicroHnswRequest::Insert {
            id: 1,
            embedding: vec![1.0, 0.0],
        });
        assert!(matches!(resp, MicroHnswResponse::Error(_)));
    }

    #[test]
    fn query_empty_index() {
        let mut index = MicroHnsw::new(2);
        let resp = index.process(MicroHnswRequest::Query {
            embedding: vec![1.0, 0.0],
            k: 5,
        });
        match resp {
            MicroHnswResponse::Results(results) => {
                assert!(results.is_empty());
            }
            _ => panic!("expected Results"),
        }
    }

    #[test]
    fn query_k_zero() {
        let mut index = MicroHnsw::new(2);
        index.process(MicroHnswRequest::Insert {
            id: 1,
            embedding: vec![1.0, 0.0],
        });
        let resp = index.process(MicroHnswRequest::Query {
            embedding: vec![1.0, 0.0],
            k: 0,
        });
        match resp {
            MicroHnswResponse::Results(results) => {
                assert!(results.is_empty());
            }
            _ => panic!("expected Results"),
        }
    }

    #[test]
    fn count_message() {
        let mut index = MicroHnsw::new(2);
        index.process(MicroHnswRequest::Insert {
            id: 1,
            embedding: vec![1.0, 0.0],
        });
        let resp = index.process(MicroHnswRequest::Count);
        assert!(matches!(resp, MicroHnswResponse::Count(1)));
    }

    #[test]
    fn query_ordering() {
        let mut index = MicroHnsw::new(2);
        index.process(MicroHnswRequest::Insert {
            id: 1,
            embedding: vec![1.0, 0.0],
        });
        index.process(MicroHnswRequest::Insert {
            id: 2,
            embedding: vec![0.7, 0.7],
        });
        index.process(MicroHnswRequest::Insert {
            id: 3,
            embedding: vec![0.0, 1.0],
        });

        let resp = index.process(MicroHnswRequest::Query {
            embedding: vec![1.0, 0.0],
            k: 3,
        });
        match resp {
            MicroHnswResponse::Results(results) => {
                assert_eq!(results.len(), 3);
                assert_eq!(results[0].0, 1);
                assert_eq!(results[1].0, 2);
                assert_eq!(results[2].0, 3);
            }
            _ => panic!("expected Results"),
        }
    }

    #[test]
    fn default_creates_zero_dim() {
        let index = MicroHnsw::default();
        assert!(index.is_empty());
    }

    #[test]
    fn request_serialization() {
        let req = MicroHnswRequest::Insert {
            id: 1,
            embedding: vec![1.0, 2.0],
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: MicroHnswRequest = serde_json::from_str(&json).unwrap();
        match parsed {
            MicroHnswRequest::Insert { id, embedding } => {
                assert_eq!(id, 1);
                assert_eq!(embedding, vec![1.0, 2.0]);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn response_serialization() {
        let resp = MicroHnswResponse::Results(vec![(1, 0.95), (2, 0.8)]);
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: MicroHnswResponse = serde_json::from_str(&json).unwrap();
        match parsed {
            MicroHnswResponse::Results(results) => {
                assert_eq!(results.len(), 2);
            }
            _ => panic!("wrong variant"),
        }
    }
}
