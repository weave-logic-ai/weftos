//! Vector-index seam for the v2 [`EmbeddingRouter`].
//!
//! Production wiring uses a `ruvector-diskann@2.1` impl
//! ([`DiskAnnEmbeddingIndex`], gated behind the `embedding-router`
//! feature). Tests and the offline build use [`BruteForceIndex`].
//!
//! Both backends speak L2-squared on **unit-normalised** vectors so the
//! `cosine = 1 - dist / 2` derivation holds across the whole router.

use super::{normalise, EmbeddingRouterError};

/// One nearest-neighbour hit returned by an [`Index`].
///
/// `distance` is L2-squared on unit vectors, so it sits in `[0.0, 4.0]`
/// where 0 = identical and 4 = opposite. The cosine similarity is
/// recovered as `1.0 - dist / 2.0`.
#[derive(Debug, Clone)]
pub(crate) struct IndexHit {
    pub key: String,
    pub distance: f32,
}

/// Vector-index seam for [`super::EmbeddingRouter`].
pub(crate) trait Index: Send + Sync {
    /// Return the top-`k` nearest hits to `query` (must be unit-length
    /// and dimension-matched). Empty results are valid.
    fn search(&self, query: &[f32], k: usize) -> Vec<IndexHit>;
    /// Number of vectors in the index.
    fn len(&self) -> usize;
}

/// Brute-force cosine-similarity index used by tests and as the offline
/// floor when the `embedding-router` feature is not compiled.
///
/// Linear in `len()`, but the skill catalog is ~35 entries so the cost
/// stays under a microsecond. Stores **unit-normalised** vectors; the
/// reported distance is L2-squared so [`Index`] callers see the same
/// metric ordering as the diskann backend.
//
// `allow(dead_code)`: this type is only referenced from the
// `not(feature = "embedding-router")` build of `build_index` and from
// `#[cfg(test)]`. Without the allow the default-features build (which
// has `embedding-router` on) flags `new` / `insert` / the struct as
// unused. The test build still exercises every method.
#[allow(dead_code)]
pub(crate) struct BruteForceIndex {
    entries: Vec<(String, Vec<f32>)>,
}

#[allow(dead_code)]
impl BruteForceIndex {
    pub(crate) fn new() -> Self {
        Self { entries: Vec::new() }
    }

    pub(crate) fn insert(&mut self, key: String, vector: Vec<f32>) {
        let unit = normalise(vector);
        self.entries.push((key, unit));
    }
}

impl Index for BruteForceIndex {
    fn search(&self, query: &[f32], k: usize) -> Vec<IndexHit> {
        if self.entries.is_empty() || k == 0 {
            return Vec::new();
        }
        let mut scored: Vec<(String, f32)> = self
            .entries
            .iter()
            .map(|(key, v)| (key.clone(), l2_squared(query, v)))
            .collect();
        scored.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(k);
        scored
            .into_iter()
            .map(|(key, distance)| IndexHit { key, distance })
            .collect()
    }

    fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Production-grade index over `ruvector-diskann@2.1`.
///
/// Same Vamana + PQ + mmap layout the kernel already uses
/// (`crates/clawft-kernel/src/vector_diskann.rs`). Built once at
/// [`super::EmbeddingRouter::new`] time; not rebuilt mid-process —
/// the v2 contract is "full process restart on skill hot-reload is
/// fine" (registry-change rebuild ships in v2.5).
#[cfg(feature = "embedding-router")]
pub(crate) struct DiskAnnEmbeddingIndex {
    inner: ruvector_diskann::DiskAnnIndex,
    len: usize,
}

#[cfg(feature = "embedding-router")]
impl DiskAnnEmbeddingIndex {
    /// Build a diskann index over `(key, vector)` pairs. The vectors
    /// are L2-normalised in place so search-time L2² ↔ cosine.
    pub(crate) fn build(
        dim: usize,
        entries: Vec<(String, Vec<f32>)>,
    ) -> Result<Self, EmbeddingRouterError> {
        let config = ruvector_diskann::DiskAnnConfig {
            dim,
            // ~35 skills today; tiny graph parameters keep build cost
            // negligible (~ms-scale) while still giving recall close to
            // 1.0 on small N. Production tuning is fine to revisit later.
            max_degree: 32,
            build_beam: 64,
            search_beam: 64,
            alpha: 1.2,
            // PQ disabled: dataset is too small (≤ ~64 vectors) for PQ
            // training to be meaningful and an untrained PQ raises noise.
            pq_subspaces: 0,
            pq_iterations: 0,
            // No persistence: the router rebuilds in-memory at boot.
            storage_path: None,
        };
        let mut index = ruvector_diskann::DiskAnnIndex::new(config);
        let len = entries.len();
        for (key, vector) in entries {
            let unit = normalise(vector);
            index
                .insert(key, unit)
                .map_err(|e| EmbeddingRouterError::IndexError(e.to_string()))?;
        }
        index
            .build()
            .map_err(|e| EmbeddingRouterError::IndexError(e.to_string()))?;
        Ok(Self { inner: index, len })
    }
}

#[cfg(feature = "embedding-router")]
impl Index for DiskAnnEmbeddingIndex {
    fn search(&self, query: &[f32], k: usize) -> Vec<IndexHit> {
        let unit = normalise(query.to_vec());
        match self.inner.search(&unit, k) {
            Ok(hits) => hits
                .into_iter()
                .map(|h| IndexHit {
                    key: h.id,
                    distance: h.distance,
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    fn len(&self) -> usize {
        self.len
    }
}

/// L2-squared distance between two equal-length vectors.
//
// `allow(dead_code)`: only the brute-force backend calls this; the
// diskann backend does its own scoring internally. The default-features
// `cargo check` (with `embedding-router` on) therefore flags it without
// the allow. The test build always exercises it via `BruteForceIndex`.
#[allow(dead_code)]
fn l2_squared(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| {
            let d = x - y;
            d * d
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brute_force_index_basic() {
        let mut idx = BruteForceIndex::new();
        idx.insert("a".into(), vec![1.0, 0.0]);
        idx.insert("b".into(), vec![0.0, 1.0]);
        let unit = normalise(vec![1.0, 0.0]);
        let hits = idx.search(&unit, 1);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].key, "a");
        assert!(hits[0].distance < 1e-5);
    }

    #[test]
    fn brute_force_index_empty_returns_empty() {
        let idx = BruteForceIndex::new();
        let hits = idx.search(&[1.0, 0.0], 5);
        assert!(hits.is_empty());
    }

    #[test]
    fn l2_squared_basic() {
        assert!((l2_squared(&[1.0, 0.0], &[0.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!((l2_squared(&[1.0, 1.0], &[2.0, 2.0]) - 2.0).abs() < 1e-6);
    }
}
