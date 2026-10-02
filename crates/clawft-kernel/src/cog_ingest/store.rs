//! The store an ingested batch lands in, owned by one project's kernel (or
//! by the placing controller when the placement has no project).
//!
//! Dedup is honoured here, at the owner, never at the bridge: with
//! `dedup: true` a vector is skipped when the store already holds the same
//! id or a bit-identical value; with `dedup: false` the id is upserted.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use super::types::{DIMS, IngestVector};

/// Result of one batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IngestOutcome {
    /// Vectors written.
    pub accepted: usize,
    /// Vectors skipped as duplicates.
    pub deduped: usize,
    /// Vectors held afterwards.
    pub total: usize,
}

/// Store failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// The store is at its configured size.
    #[error("store full: {0} vectors")]
    Full(usize),
    /// Backend failure.
    #[error("{0}")]
    Backend(String),
}

/// Where did a batch come from (kept as metadata, never trusted for routing).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance {
    /// Instance that posted it.
    pub instance_id: String,
    /// Node whose bridge forwarded it.
    pub source_node: String,
}

/// A store that takes ingested batches.
pub trait IngestStore: Send + Sync {
    /// Write a batch.
    fn ingest(
        &self,
        from: &Provenance,
        vectors: &[IngestVector],
        dedup: bool,
    ) -> Result<IngestOutcome, StoreError>;

    /// The `k` nearest stored vectors to `q` as `(id, distance)`, closest first.
    fn query(&self, q: &[f32; DIMS], k: usize) -> Vec<(u64, f32)>;

    /// Vectors held.
    fn len(&self) -> usize;

    /// True when empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn bits_key(v: &[f32; DIMS]) -> [u8; 32] {
    let mut b = [0u8; 4 * DIMS];
    for (c, f) in b.chunks_mut(4).zip(v) {
        c.copy_from_slice(&f.to_le_bytes());
    }
    *blake3::hash(&b).as_bytes()
}

fn dist(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum::<f32>().sqrt()
}

#[derive(Default)]
struct MemInner {
    by_id: HashMap<u64, ([f32; DIMS], Provenance)>,
    hashes: HashSet<[u8; 32]>,
}

/// In-memory store (tests, and a node's scratch store).
pub struct MemoryIngestStore {
    max: usize,
    inner: Mutex<MemInner>,
}

impl MemoryIngestStore {
    /// Store holding at most `max` vectors.
    pub fn new(max: usize) -> Self {
        Self {
            max,
            inner: Mutex::default(),
        }
    }

    /// Provenance of the vector stored under `id`.
    pub fn provenance(&self, id: u64) -> Option<Provenance> {
        self.inner.lock().ok()?.by_id.get(&id).map(|(_, p)| p.clone())
    }
}

impl IngestStore for MemoryIngestStore {
    fn ingest(
        &self,
        from: &Provenance,
        vectors: &[IngestVector],
        dedup: bool,
    ) -> Result<IngestOutcome, StoreError> {
        let mut g = self
            .inner
            .lock()
            .map_err(|_| StoreError::Backend("store lock poisoned".into()))?;
        let (mut accepted, mut deduped) = (0, 0);
        for v in vectors {
            let key = bits_key(&v.values);
            let present = g.by_id.contains_key(&v.id);
            if dedup && (present || g.hashes.contains(&key)) {
                deduped += 1;
                continue;
            }
            if !present && g.by_id.len() >= self.max {
                return Err(StoreError::Full(g.by_id.len()));
            }
            if let Some((old, _)) = g.by_id.insert(v.id, (v.values, from.clone())) {
                // The upserted id no longer holds its old value.
                let old = bits_key(&old);
                if !g.by_id.values().any(|(x, _)| bits_key(x) == old) {
                    g.hashes.remove(&old);
                }
            }
            g.hashes.insert(key);
            accepted += 1;
        }
        Ok(IngestOutcome {
            accepted,
            deduped,
            total: g.by_id.len(),
        })
    }

    fn query(&self, q: &[f32; DIMS], k: usize) -> Vec<(u64, f32)> {
        let Ok(g) = self.inner.lock() else {
            return vec![];
        };
        let mut hits: Vec<_> = g
            .by_id
            .iter()
            .map(|(id, (v, _))| (*id, dist(q, v)))
            .collect();
        hits.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
        hits.truncate(k);
        hits
    }

    fn len(&self) -> usize {
        self.inner.lock().map(|g| g.by_id.len()).unwrap_or(0)
    }
}

/// Adapter over the kernel's [`VectorBackend`](crate::vector_backend::VectorBackend)
/// (HNSW, DiskANN, hybrid). The backend's dimensionality must be [`DIMS`].
#[cfg(feature = "ecc")]
pub struct VectorBackendStore {
    backend: Arc<dyn crate::vector_backend::VectorBackend>,
    hashes: Mutex<HashSet<[u8; 32]>>,
}

#[cfg(feature = "ecc")]
impl VectorBackendStore {
    /// Wrap `backend`.
    pub fn new(backend: Arc<dyn crate::vector_backend::VectorBackend>) -> Self {
        Self {
            backend,
            hashes: Mutex::default(),
        }
    }
}

#[cfg(feature = "ecc")]
impl IngestStore for VectorBackendStore {
    fn ingest(
        &self,
        from: &Provenance,
        vectors: &[IngestVector],
        dedup: bool,
    ) -> Result<IngestOutcome, StoreError> {
        let mut hashes = self
            .hashes
            .lock()
            .map_err(|_| StoreError::Backend("store lock poisoned".into()))?;
        let (mut accepted, mut deduped) = (0, 0);
        for v in vectors {
            let key = bits_key(&v.values);
            if dedup && (self.backend.contains(v.id) || hashes.contains(&key)) {
                deduped += 1;
                continue;
            }
            let meta = serde_json::json!({
                "instance": from.instance_id,
                "node": from.source_node,
            });
            self.backend
                .insert(v.id, &format!("cog-ingest:{}", v.id), &v.values, meta)
                .map_err(|e| match e {
                    crate::vector_backend::VectorError::StoreFull { current, .. } => {
                        StoreError::Full(current)
                    }
                    other => StoreError::Backend(other.to_string()),
                })?;
            hashes.insert(key);
            accepted += 1;
        }
        Ok(IngestOutcome {
            accepted,
            deduped,
            total: self.backend.len(),
        })
    }

    fn query(&self, q: &[f32; DIMS], k: usize) -> Vec<(u64, f32)> {
        self.backend
            .search(q, k)
            .into_iter()
            .map(|r| (r.id, r.distance))
            .collect()
    }

    fn len(&self) -> usize {
        self.backend.len()
    }
}

/// Which store a project's batches land in, on the node that owns stores.
pub trait StoreDirectory: Send + Sync {
    /// The store of `project_id`, or the node's fallback (the placing
    /// controller's store) when `project_id` is `None`. `None` result: no
    /// such store here.
    fn store_for(&self, project_id: Option<&str>) -> Option<Arc<dyn IngestStore>>;
}

/// Directory over fixed maps.
#[derive(Default)]
pub struct StaticDirectory {
    projects: Mutex<HashMap<String, Arc<dyn IngestStore>>>,
    fallback: Mutex<Option<Arc<dyn IngestStore>>>,
}

impl StaticDirectory {
    /// Empty directory.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a project's store.
    pub fn with_project(self, project_id: &str, store: Arc<dyn IngestStore>) -> Self {
        if let Ok(mut m) = self.projects.lock() {
            m.insert(project_id.to_string(), store);
        }
        self
    }

    /// Register the store used when a placement has no project.
    pub fn with_fallback(self, store: Arc<dyn IngestStore>) -> Self {
        if let Ok(mut f) = self.fallback.lock() {
            *f = Some(store);
        }
        self
    }
}

impl StoreDirectory for StaticDirectory {
    fn store_for(&self, project_id: Option<&str>) -> Option<Arc<dyn IngestStore>> {
        match project_id {
            Some(p) => self.projects.lock().ok()?.get(p).cloned(),
            None => self.fallback.lock().ok()?.clone(),
        }
    }
}
