//! The store an ingested batch lands in, owned by one project's kernel (or
//! by the placing controller when the placement has no project).
//!
//! Vectors are namespaced per instance: the key is `(instance, id)`, so one
//! instance can neither overwrite nor dedup against another instance's ids
//! or values inside a shared project store. Dedup is honoured here, at the
//! owner, never at the bridge: with `dedup: true` a vector is skipped when
//! the same instance already holds that id or a bit-identical value; with
//! `dedup: false` the id is upserted.

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

/// Where a batch came from (kept as metadata, never trusted for routing).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance {
    /// Instance that posted it.
    pub instance_id: String,
    /// Node whose bridge forwarded it.
    pub source_node: String,
}

/// One nearest-neighbour result.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    /// Instance that wrote the vector.
    pub instance_id: String,
    /// The id the instance gave it.
    pub id: u64,
    /// Distance to the query (lower is closer).
    pub distance: f32,
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

    /// The `k` nearest stored vectors to `q`, closest first.
    fn query(&self, q: &[f32; DIMS], k: usize) -> Vec<Hit>;

    /// Vectors held.
    fn len(&self) -> usize;

    /// True when empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn value_key(instance: &str, v: &[f32; DIMS]) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(&(instance.len() as u64).to_le_bytes());
    h.update(instance.as_bytes());
    for f in v {
        h.update(&f.to_le_bytes());
    }
    *h.finalize().as_bytes()
}

fn dist(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum::<f32>().sqrt()
}

/// Per-instance value index with reference counts (an upsert frees the old
/// value's entry only when no other id of that instance holds it).
#[derive(Default)]
struct ValueIndex(HashMap<[u8; 32], usize>);

impl ValueIndex {
    fn has(&self, k: &[u8; 32]) -> bool {
        self.0.contains_key(k)
    }
    fn add(&mut self, k: [u8; 32]) {
        *self.0.entry(k).or_default() += 1;
    }
    fn remove(&mut self, k: &[u8; 32]) {
        if let Some(n) = self.0.get_mut(k) {
            *n -= 1;
            if *n == 0 {
                self.0.remove(k);
            }
        }
    }
}

#[derive(Default)]
struct MemInner {
    by_key: HashMap<(String, u64), ([f32; DIMS], String)>,
    values: ValueIndex,
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

    /// Provenance of the vector `instance` stored under `id`.
    pub fn provenance(&self, instance: &str, id: u64) -> Option<Provenance> {
        let g = self.inner.lock().ok()?;
        let (_, node) = g.by_key.get(&(instance.to_string(), id))?;
        Some(Provenance {
            instance_id: instance.to_string(),
            source_node: node.clone(),
        })
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
        let inst = &from.instance_id;
        let (mut accepted, mut deduped) = (0, 0);
        for v in vectors {
            let key = (inst.clone(), v.id);
            let vk = value_key(inst, &v.values);
            let present = g.by_key.contains_key(&key);
            if dedup && (present || g.values.has(&vk)) {
                deduped += 1;
                continue;
            }
            if !present && g.by_key.len() >= self.max {
                return Err(StoreError::Full(g.by_key.len()));
            }
            if let Some((old, _)) = g.by_key.insert(key, (v.values, from.source_node.clone())) {
                g.values.remove(&value_key(inst, &old));
            }
            g.values.add(vk);
            accepted += 1;
        }
        Ok(IngestOutcome {
            accepted,
            deduped,
            total: g.by_key.len(),
        })
    }

    fn query(&self, q: &[f32; DIMS], k: usize) -> Vec<Hit> {
        let Ok(g) = self.inner.lock() else {
            return vec![];
        };
        let mut hits: Vec<_> = g
            .by_key
            .iter()
            .map(|((inst, id), (v, _))| Hit {
                instance_id: inst.clone(),
                id: *id,
                distance: dist(q, v),
            })
            .collect();
        hits.sort_by(|a, b| {
            a.distance
                .total_cmp(&b.distance)
                .then(a.id.cmp(&b.id))
                .then(a.instance_id.cmp(&b.instance_id))
        });
        hits.truncate(k);
        hits
    }

    fn len(&self) -> usize {
        self.inner.lock().map(|g| g.by_key.len()).unwrap_or(0)
    }
}

#[cfg(feature = "ecc")]
#[derive(Default)]
struct BackendInner {
    /// Backend id -> the (instance, id) it stands for.
    names: HashMap<u64, (String, u64)>,
    values: ValueIndex,
}

/// Adapter over the kernel's [`VectorBackend`](crate::vector_backend::VectorBackend)
/// (HNSW, DiskANN, hybrid). The backend's dimensionality must be [`DIMS`].
/// The backend id is a 64-bit hash of `(instance, id)`; a hash collision
/// between two different pairs is refused rather than overwriting.
#[cfg(feature = "ecc")]
pub struct VectorBackendStore {
    backend: Arc<dyn crate::vector_backend::VectorBackend>,
    inner: Mutex<BackendInner>,
}

#[cfg(feature = "ecc")]
fn backend_id(instance: &str, id: u64) -> u64 {
    let mut h = blake3::Hasher::new();
    h.update(&(instance.len() as u64).to_le_bytes());
    h.update(instance.as_bytes());
    h.update(&id.to_le_bytes());
    u64::from_le_bytes(h.finalize().as_bytes()[..8].try_into().unwrap_or([0; 8]))
}

#[cfg(feature = "ecc")]
impl VectorBackendStore {
    /// Wrap `backend`.
    pub fn new(backend: Arc<dyn crate::vector_backend::VectorBackend>) -> Self {
        Self {
            backend,
            inner: Mutex::default(),
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
        let mut g = self
            .inner
            .lock()
            .map_err(|_| StoreError::Backend("store lock poisoned".into()))?;
        let inst = &from.instance_id;
        let (mut accepted, mut deduped) = (0, 0);
        for v in vectors {
            let bid = backend_id(inst, v.id);
            let vk = value_key(inst, &v.values);
            let present = match g.names.get(&bid) {
                Some((i, id)) if i == inst && *id == v.id => true,
                Some(_) => {
                    return Err(StoreError::Backend("vector id hash collision".into()));
                }
                None => false,
            };
            if dedup && (present || g.values.has(&vk)) {
                deduped += 1;
                continue;
            }
            let meta = serde_json::json!({
                "instance": from.instance_id,
                "node": from.source_node,
                "id": v.id,
            });
            self.backend
                .insert(bid, &format!("cog-ingest:{inst}:{}", v.id), &v.values, meta)
                .map_err(|e| match e {
                    crate::vector_backend::VectorError::StoreFull { current, .. } => {
                        StoreError::Full(current)
                    }
                    other => StoreError::Backend(other.to_string()),
                })?;
            g.names.insert(bid, (inst.clone(), v.id));
            g.values.add(vk);
            accepted += 1;
        }
        Ok(IngestOutcome {
            accepted,
            deduped,
            total: self.backend.len(),
        })
    }

    fn query(&self, q: &[f32; DIMS], k: usize) -> Vec<Hit> {
        let Ok(g) = self.inner.lock() else {
            return vec![];
        };
        self.backend
            .search(q, k)
            .into_iter()
            .filter_map(|r| {
                let (instance_id, id) = g.names.get(&r.id)?.clone();
                Some(Hit {
                    instance_id,
                    id,
                    distance: r.distance,
                })
            })
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

/// A node's stores for the projects it owns, each an in-memory HNSW index
/// created on first use (persistence follows the project kernel's store
/// work; a restart empties them). The index map is shared between
/// [`views`](Self::view), so two views with different allow-lists (local
/// cogs, remote forwarders) reach the same store for a project they both
/// allow.
#[cfg(feature = "ecc")]
#[derive(Clone)]
pub struct VectorDirectory {
    projects: HashSet<String>,
    fallback: bool,
    stores: Arc<Mutex<HashMap<Option<String>, Arc<VectorBackendStore>>>>,
}

#[cfg(feature = "ecc")]
impl VectorDirectory {
    /// Directory owning `projects`, and the controller fallback if `fallback`.
    pub fn new(projects: impl IntoIterator<Item = String>, fallback: bool) -> Self {
        Self {
            projects: projects.into_iter().collect(),
            fallback,
            stores: Arc::default(),
        }
    }

    /// A view over the same stores allowing only `projects` (and the
    /// fallback if `fallback`).
    pub fn view(&self, projects: impl IntoIterator<Item = String>, fallback: bool) -> Self {
        Self {
            projects: projects.into_iter().collect(),
            fallback,
            stores: self.stores.clone(),
        }
    }
}

#[cfg(feature = "ecc")]
impl StoreDirectory for VectorDirectory {
    fn store_for(&self, project_id: Option<&str>) -> Option<Arc<dyn IngestStore>> {
        match project_id {
            Some(p) if !self.projects.contains(p) => return None,
            None if !self.fallback => return None,
            _ => {}
        }
        let mut g = self.stores.lock().ok()?;
        let s = g.entry(project_id.map(String::from)).or_insert_with(|| {
            Arc::new(VectorBackendStore::new(Arc::new(
                crate::vector_hnsw::HnswBackend::new(
                    crate::hnsw_service::HnswServiceConfig::default(),
                ),
            )))
        });
        Some(s.clone() as Arc<dyn IngestStore>)
    }
}
