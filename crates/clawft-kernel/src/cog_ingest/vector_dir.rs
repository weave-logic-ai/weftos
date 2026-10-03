//! A node's per-project vector stores (ADR-100 decision 5): in-memory HNSW
//! indexes on the owner daemon, optionally backed by a durable log.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use super::store::{IngestStore, StoreDirectory, VectorBackendStore};
use super::store_log::{OpenError, run_blocking};

/// A node's stores for the projects it owns, each an HNSW index created on
/// first use. Without [`with_persistence`](Self::with_persistence) the
/// indexes are memory-only and a restart empties them; with it, each store
/// keeps a capped, append-only log under the given directory (one file per
/// project) and is rebuilt from it on first use after a restart. The index
/// map is shared between [`views`](Self::view), so two views with different
/// allow-lists (local cogs, remote forwarders) reach the same store for a
/// project they both allow.
#[derive(Clone)]
pub struct VectorDirectory {
    projects: HashSet<String>,
    fallback: bool,
    stores: Arc<Mutex<HashMap<Option<String>, Slot>>>,
    persist: Option<(std::path::PathBuf, u64)>,
}

/// One project's store, opened on first use. The slot has its own lock, so
/// replaying a big log blocks only callers of that project, never the
/// directory; a failed open leaves it empty and the next call retries.
type Slot = Arc<Mutex<Option<Arc<VectorBackendStore>>>>;

/// File of the controller-fallback store inside the persistence dir. Not a
/// valid project id, so it cannot collide with a project's file.
const FALLBACK_LOG: &str = "_controller.vec";

impl VectorDirectory {
    /// Directory owning `projects`, and the controller fallback if `fallback`.
    pub fn new(projects: impl IntoIterator<Item = String>, fallback: bool) -> Self {
        Self {
            projects: projects.into_iter().collect(),
            fallback,
            stores: Arc::default(),
            persist: None,
        }
    }

    /// Keep each store's vectors in `<dir>/<project id>.vec` (the controller
    /// fallback in `<dir>/_controller.vec`), each capped at
    /// [`DEFAULT_MAX_LOG_BYTES`](super::store_log::DEFAULT_MAX_LOG_BYTES).
    /// Call before taking [`view`](Self::view)s.
    pub fn with_persistence(self, dir: std::path::PathBuf) -> Self {
        self.with_persistence_capped(dir, super::store_log::DEFAULT_MAX_LOG_BYTES)
    }

    /// [`with_persistence`](Self::with_persistence) with an explicit cap per file.
    pub fn with_persistence_capped(mut self, dir: std::path::PathBuf, max_log_bytes: u64) -> Self {
        self.persist = Some((dir, max_log_bytes));
        self
    }

    /// A view over the same stores allowing only `projects` (and the
    /// fallback if `fallback`).
    pub fn view(&self, projects: impl IntoIterator<Item = String>, fallback: bool) -> Self {
        Self {
            projects: projects.into_iter().collect(),
            fallback,
            stores: self.stores.clone(),
            persist: self.persist.clone(),
        }
    }

    fn open_store(&self, project_id: Option<&str>) -> Option<Arc<VectorBackendStore>> {
        let backend = || {
            Arc::new(crate::vector_hnsw::HnswBackend::new(
                crate::hnsw_service::HnswServiceConfig::default(),
            ))
        };
        let Some((dir, cap)) = &self.persist else {
            return Some(Arc::new(VectorBackendStore::new(backend())));
        };
        let file = match project_id {
            Some(p) => format!("{p}.vec"),
            None => FALLBACK_LOG.to_string(),
        };
        let path = dir.join(&file);
        match VectorBackendStore::persistent(backend(), &path, *cap) {
            Ok(s) => Some(Arc::new(s)),
            Err(OpenError::Io(e)) => {
                // Possibly transient and the file is not implicated: refuse for now, retry next call.
                tracing::warn!(error = %e, path = %path.display(), "cog ingest vector log could not be opened");
                None
            }
            Err(OpenError::Corrupt(e)) => {
                // Never overwrite what could not be read: keep it aside, under a name that
                // cannot clobber earlier evidence, and start this project's store empty.
                let ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or_default();
                let aside = dir.join(format!("{file}.unreadable.{ts}"));
                tracing::warn!(error = %e, path = %path.display(), aside = %aside.display(), "cog ingest vector log unusable; moving it aside");
                std::fs::rename(&path, &aside).ok()?;
                VectorBackendStore::persistent(backend(), &path, *cap).ok().map(Arc::new)
            }
        }
    }
}

impl StoreDirectory for VectorDirectory {
    fn store_for(&self, project_id: Option<&str>) -> Option<Arc<dyn IngestStore>> {
        match project_id {
            Some(p) if !self.projects.contains(p) => return None,
            None if !self.fallback => return None,
            _ => {}
        }
        let slot = {
            let mut g = self.stores.lock().ok()?;
            g.entry(project_id.map(String::from)).or_default().clone()
        };
        // The directory lock is released; only this project's slot is held while the log replays.
        let mut cell = slot.lock().ok()?;
        if cell.is_none() {
            *cell = run_blocking(|| self.open_store(project_id));
        }
        cell.clone().map(|s| s as Arc<dyn IngestStore>)
    }
}
