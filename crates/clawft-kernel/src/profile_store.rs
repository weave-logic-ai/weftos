//! Per-user profile namespaces (Cognitum Seed Gap #14).
//!
//! Each profile gets its own isolated vector storage, allowing
//! multi-tenant vector isolation on a shared device. Profiles are
//! persisted to `{storage_path}/{id}/profile.json` and lazily
//! loaded on boot by scanning the profiles directory.
//!
//! This module is compiled only when the `ecc` feature is enabled.

use std::path::{Path, PathBuf};
use std::sync::RwLock;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::vector_backend::{SearchResult, VectorBackend, VectorError, VectorResult};
use crate::vector_hnsw::HnswBackend;
use crate::hnsw_service::HnswServiceConfig;

#[cfg(feature = "exochain")]
use std::sync::Arc;
#[cfg(feature = "exochain")]
use crate::chain::ChainManager;
#[cfg(feature = "exochain")]
use crate::governance::{EffectVector, GovernanceDecision, GovernanceEngine, GovernanceRequest};

// ── Profile metadata ────────────────────────────────────────────────────

/// Persisted metadata for a single profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileMeta {
    /// Unique profile identifier (slug, e.g. "alice").
    pub id: String,
    /// Human-readable display name.
    pub name: String,
    /// When the profile was created.
    pub created_at: DateTime<Utc>,
    /// Number of vectors stored (informational snapshot).
    pub vector_count: usize,
}

/// A loaded profile: metadata plus its own vector backend.
pub struct ProfileEntry {
    pub meta: ProfileMeta,
    pub backend: Box<dyn VectorBackend>,
}

impl std::fmt::Debug for ProfileEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProfileEntry")
            .field("meta", &self.meta)
            .field("backend", &self.meta.id)
            .finish()
    }
}

// ── Profile errors ──────────────────────────────────────────────────────

/// Errors specific to profile operations.
#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    /// Profile already exists.
    #[error("profile already exists: '{0}'")]
    AlreadyExists(String),

    /// Profile not found.
    #[error("profile not found: '{0}'")]
    NotFound(String),

    /// Invalid profile ID (must be alphanumeric + hyphens).
    #[error("invalid profile id: '{0}' -- must be [a-zA-Z0-9_-]+")]
    InvalidId(String),

    /// I/O error reading/writing profile data.
    #[error("profile io error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialization error.
    #[error("profile json error: {0}")]
    Json(#[from] serde_json::Error),

    /// Governance gate denied the operation.
    #[error("governance denied: {0}")]
    GovernanceDenied(String),
}

pub type ProfileResult<T> = Result<T, ProfileError>;

// ── Profile store ───────────────────────────────────────────────────────

/// Manages multiple named profiles, each with isolated vector storage.
///
/// Thread-safe: profiles are stored in a [`DashMap`] and the active
/// profile is tracked via an [`RwLock`].
pub struct ProfileStore {
    /// Base directory for profile storage (e.g., `.weftos/profiles`).
    storage_path: PathBuf,
    /// All loaded profiles.
    profiles: DashMap<String, ProfileEntry>,
    /// Currently active profile ID.
    active_profile: RwLock<String>,
    /// HNSW configuration used when creating new profile backends.
    hnsw_config: HnswServiceConfig,
    /// Chain manager for exochain event logging.
    #[cfg(feature = "exochain")]
    chain_manager: Option<Arc<ChainManager>>,
    /// Governance engine for gating destructive operations.
    #[cfg(feature = "exochain")]
    governance_engine: Option<Arc<GovernanceEngine>>,
}

impl ProfileStore {
    /// Create a new profile store rooted at `storage_path`.
    ///
    /// Does **not** scan the directory; call [`load_existing`] for that.
    pub fn new(storage_path: PathBuf, hnsw_config: HnswServiceConfig) -> Self {
        Self {
            storage_path,
            profiles: DashMap::new(),
            active_profile: RwLock::new("default".to_owned()),
            hnsw_config,
            #[cfg(feature = "exochain")]
            chain_manager: None,
            #[cfg(feature = "exochain")]
            governance_engine: None,
        }
    }

    /// Set the chain manager for exochain event logging.
    #[cfg(feature = "exochain")]
    pub fn set_chain_manager(&mut self, cm: Arc<ChainManager>) {
        self.chain_manager = Some(cm);
    }

    /// Set the governance engine for gating destructive operations.
    #[cfg(feature = "exochain")]
    pub fn set_governance_engine(&mut self, engine: Arc<GovernanceEngine>) {
        self.governance_engine = Some(engine);
    }

    /// Scan `storage_path` and load existing profiles from disk.
    ///
    /// If no profiles exist, creates a "default" profile automatically.
    pub fn load_existing(&self) -> ProfileResult<usize> {
        let dir = &self.storage_path;
        if !dir.exists() {
            std::fs::create_dir_all(dir)?;
        }

        let mut count = 0;
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let profile_dir = entry.path();
            let meta_path = profile_dir.join("profile.json");
            if !meta_path.exists() {
                continue;
            }
            match Self::load_profile_meta(&meta_path) {
                Ok(meta) => {
                    let backend = Box::new(HnswBackend::new(self.hnsw_config.clone()));
                    debug!(profile_id = %meta.id, "loaded profile from disk");
                    self.profiles.insert(
                        meta.id.clone(),
                        ProfileEntry { meta, backend },
                    );
                    count += 1;
                }
                Err(e) => {
                    warn!(path = %meta_path.display(), error = %e, "failed to load profile");
                }
            }
        }

        // Auto-create default profile if none exist.
        if self.profiles.is_empty() {
            info!("no profiles found, creating default profile");
            self.create_profile_inner("default", "Default Profile")?;
            count = 1;
        }

        Ok(count)
    }

    /// Create a new profile with the given ID and display name.
    pub fn create_profile(&self, id: &str, name: &str) -> ProfileResult<ProfileMeta> {
        Self::validate_id(id)?;

        if self.profiles.contains_key(id) {
            return Err(ProfileError::AlreadyExists(id.to_owned()));
        }

        // Governance gate: profile creation.
        #[cfg(feature = "exochain")]
        if let Some(ref engine) = self.governance_engine {
            let req = GovernanceRequest::new("system", "profile.create")
                .with_effect(EffectVector { risk: 0.3, privacy: 0.2, ..Default::default() });
            let result = engine.evaluate(&req);
            match &result.decision {
                GovernanceDecision::Deny(reason) | GovernanceDecision::EscalateToHuman(reason) => {
                    return Err(ProfileError::GovernanceDenied(reason.clone()));
                }
                _ => {}
            }
        }

        let meta = self.create_profile_inner(id, name)?;

        // Chain logging: profile.create
        #[cfg(feature = "exochain")]
        if let Some(ref cm) = self.chain_manager {
            cm.append(
                "profile_store",
                crate::chain::EVENT_KIND_PROFILE_CREATE,
                Some(serde_json::json!({
                    "profile_id": id,
                    "name": name,
                })),
            );
        }

        Ok(meta)
    }

    /// Delete a profile by ID. Cannot delete the currently active profile.
    pub fn delete_profile(&self, id: &str) -> ProfileResult<()> {
        if !self.profiles.contains_key(id) {
            return Err(ProfileError::NotFound(id.to_owned()));
        }

        // Prevent deleting the active profile.
        {
            let active = self.active_profile.read().expect("active_profile lock poisoned");
            if *active == id {
                return Err(ProfileError::InvalidId(
                    format!("cannot delete active profile '{id}'; switch first"),
                ));
            }
        }

        // Governance gate: bulk destruction (profile delete).
        #[cfg(feature = "exochain")]
        if let Some(ref engine) = self.governance_engine {
            let req = GovernanceRequest::new("system", "profile.delete")
                .with_effect(EffectVector { risk: 0.7, privacy: 0.4, ..Default::default() });
            let result = engine.evaluate(&req);
            match &result.decision {
                GovernanceDecision::Deny(reason) | GovernanceDecision::EscalateToHuman(reason) => {
                    return Err(ProfileError::GovernanceDenied(reason.clone()));
                }
                _ => {}
            }
        }

        self.profiles.remove(id);

        // Remove the directory on disk.
        let dir = self.storage_path.join(id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }

        // Chain logging: profile.delete
        #[cfg(feature = "exochain")]
        if let Some(ref cm) = self.chain_manager {
            cm.append(
                "profile_store",
                crate::chain::EVENT_KIND_PROFILE_DELETE,
                Some(serde_json::json!({
                    "profile_id": id,
                })),
            );
        }

        info!(profile_id = %id, "deleted profile");
        Ok(())
    }

    /// List all profiles (metadata only).
    pub fn list_profiles(&self) -> Vec<ProfileMeta> {
        self.profiles
            .iter()
            .map(|entry| {
                let mut meta = entry.value().meta.clone();
                meta.vector_count = entry.value().backend.len();
                meta
            })
            .collect()
    }

    /// Switch the active profile. Subsequent vector operations target
    /// the new profile.
    pub fn switch_profile(&self, id: &str) -> ProfileResult<()> {
        if !self.profiles.contains_key(id) {
            return Err(ProfileError::NotFound(id.to_owned()));
        }

        let previous = self.active_profile_id();
        let mut active = self.active_profile.write().expect("active_profile lock poisoned");
        *active = id.to_owned();

        // Chain logging: profile.switch
        #[cfg(feature = "exochain")]
        if let Some(ref cm) = self.chain_manager {
            cm.append(
                "profile_store",
                crate::chain::EVENT_KIND_PROFILE_SWITCH,
                Some(serde_json::json!({
                    "profile_id": id,
                    "previous": previous,
                })),
            );
        }

        info!(profile_id = %id, "switched active profile");
        Ok(())
    }

    /// Get the currently active profile ID.
    pub fn active_profile_id(&self) -> String {
        self.active_profile
            .read()
            .expect("active_profile lock poisoned")
            .clone()
    }

    /// Get a reference to a specific profile's vector backend.
    ///
    /// Returns `None` if the profile does not exist.
    pub fn get_profile_backend(&self, id: &str) -> Option<dashmap::mapref::one::Ref<'_, String, ProfileEntry>> {
        self.profiles.get(id)
    }

    /// Get a reference to the active profile's vector backend.
    pub fn active_backend(&self) -> Option<dashmap::mapref::one::Ref<'_, String, ProfileEntry>> {
        let id = self.active_profile_id();
        self.profiles.get(&id)
    }

    /// Insert a vector into the active profile's backend.
    pub fn insert(
        &self,
        id: u64,
        key: &str,
        vector: &[f32],
        metadata: serde_json::Value,
    ) -> VectorResult<()> {
        let profile_id = self.active_profile_id();
        let entry = self
            .profiles
            .get(&profile_id)
            .ok_or_else(|| VectorError::Other(format!("active profile '{profile_id}' not found")))?;
        let result = entry.backend.insert(id, key, vector, metadata);

        // Chain logging: profile.vector.insert
        #[cfg(feature = "exochain")]
        if result.is_ok()
            && let Some(ref cm) = self.chain_manager {
                cm.append(
                    "profile_store",
                    crate::chain::EVENT_KIND_PROFILE_VECTOR_INSERT,
                    Some(serde_json::json!({
                        "profile_id": profile_id,
                        "vector_id": id,
                        "key": key,
                    })),
                );
            }

        result
    }

    /// Search the active profile's backend.
    pub fn search(&self, query: &[f32], k: usize) -> Vec<SearchResult> {
        let profile_id = self.active_profile_id();
        match self.profiles.get(&profile_id) {
            Some(entry) => entry.backend.search(query, k),
            None => Vec::new(),
        }
    }

    /// Return the total number of profiles.
    pub fn len(&self) -> usize {
        self.profiles.len()
    }

    /// Return `true` if no profiles are loaded.
    pub fn is_empty(&self) -> bool {
        self.profiles.is_empty()
    }

    /// Persist the metadata for a given profile to disk.
    pub fn persist_meta(&self, id: &str) -> ProfileResult<()> {
        let entry = self
            .profiles
            .get(id)
            .ok_or_else(|| ProfileError::NotFound(id.to_owned()))?;

        let mut meta = entry.meta.clone();
        meta.vector_count = entry.backend.len();

        let dir = self.storage_path.join(id);
        std::fs::create_dir_all(&dir)?;
        let meta_path = dir.join("profile.json");
        let json = serde_json::to_string_pretty(&meta)?;
        std::fs::write(&meta_path, json)?;
        Ok(())
    }

    // ── Internal helpers ────────────────────────────────────────────

    fn create_profile_inner(&self, id: &str, name: &str) -> ProfileResult<ProfileMeta> {
        let meta = ProfileMeta {
            id: id.to_owned(),
            name: name.to_owned(),
            created_at: Utc::now(),
            vector_count: 0,
        };

        let backend = Box::new(HnswBackend::new(self.hnsw_config.clone()));

        // Persist metadata to disk.
        let dir = self.storage_path.join(id);
        std::fs::create_dir_all(dir.join("vectors"))?;
        let meta_path = dir.join("profile.json");
        let json = serde_json::to_string_pretty(&meta)?;
        std::fs::write(&meta_path, json)?;

        self.profiles.insert(
            id.to_owned(),
            ProfileEntry {
                meta: meta.clone(),
                backend,
            },
        );

        info!(profile_id = %id, name = %name, "created profile");
        Ok(meta)
    }

    fn load_profile_meta(path: &Path) -> ProfileResult<ProfileMeta> {
        let data = std::fs::read_to_string(path)?;
        let meta: ProfileMeta = serde_json::from_str(&data)?;
        Ok(meta)
    }

    fn validate_id(id: &str) -> ProfileResult<()> {
        if id.is_empty() {
            return Err(ProfileError::InvalidId("empty id".to_owned()));
        }
        if !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(ProfileError::InvalidId(id.to_owned()));
        }
        Ok(())
    }
}

impl std::fmt::Debug for ProfileStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProfileStore")
            .field("storage_path", &self.storage_path)
            .field("profiles", &self.profiles.len())
            .field("active", &self.active_profile_id())
            .finish()
    }
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "profile_store_test_{name}_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn make_store(name: &str) -> (ProfileStore, PathBuf) {
        let dir = tmp_dir(name);
        let store = ProfileStore::new(dir.clone(), HnswServiceConfig::default());
        (store, dir)
    }

    fn cleanup(dir: &Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn create_and_list_profiles() {
        let (store, dir) = make_store("create_list");
        store.create_profile("alice", "Alice").unwrap();
        store.create_profile("bob", "Bob").unwrap();

        let profiles = store.list_profiles();
        assert_eq!(profiles.len(), 2);

        let names: Vec<&str> = profiles.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"Alice"));
        assert!(names.contains(&"Bob"));

        cleanup(&dir);
    }

    #[test]
    fn duplicate_profile_rejected() {
        let (store, dir) = make_store("duplicate");
        store.create_profile("alice", "Alice").unwrap();
        assert!(matches!(
            store.create_profile("alice", "Alice Again"),
            Err(ProfileError::AlreadyExists(_))
        ));
        cleanup(&dir);
    }

    #[test]
    fn invalid_id_rejected() {
        let (store, dir) = make_store("invalid_id");
        assert!(matches!(
            store.create_profile("a b c", "Bad"),
            Err(ProfileError::InvalidId(_))
        ));
        assert!(matches!(
            store.create_profile("", "Empty"),
            Err(ProfileError::InvalidId(_))
        ));
        assert!(matches!(
            store.create_profile("a/b", "Slash"),
            Err(ProfileError::InvalidId(_))
        ));
        cleanup(&dir);
    }

    #[test]
    fn switch_and_active_profile() {
        let (store, dir) = make_store("switch");
        store.create_profile("alice", "Alice").unwrap();
        store.create_profile("bob", "Bob").unwrap();

        assert_eq!(store.active_profile_id(), "default");

        store.switch_profile("alice").unwrap();
        assert_eq!(store.active_profile_id(), "alice");

        store.switch_profile("bob").unwrap();
        assert_eq!(store.active_profile_id(), "bob");

        // Switch to nonexistent fails.
        assert!(matches!(
            store.switch_profile("nobody"),
            Err(ProfileError::NotFound(_))
        ));

        cleanup(&dir);
    }

    #[test]
    fn delete_profile() {
        let (store, dir) = make_store("delete");
        store.create_profile("alice", "Alice").unwrap();
        store.create_profile("bob", "Bob").unwrap();

        // Cannot delete nonexistent.
        assert!(matches!(
            store.delete_profile("nobody"),
            Err(ProfileError::NotFound(_))
        ));

        // Switch to alice, cannot delete active.
        store.switch_profile("alice").unwrap();
        assert!(store.delete_profile("alice").is_err());

        // Can delete bob (not active).
        store.delete_profile("bob").unwrap();
        assert_eq!(store.len(), 1);

        cleanup(&dir);
    }

    #[test]
    fn insert_and_search_active_profile() {
        let (store, dir) = make_store("insert_search");
        store.create_profile("alice", "Alice").unwrap();
        store.switch_profile("alice").unwrap();

        store
            .insert(1, "v1", &[1.0, 0.0, 0.0], serde_json::json!({}))
            .unwrap();
        store
            .insert(2, "v2", &[0.0, 1.0, 0.0], serde_json::json!({}))
            .unwrap();

        let results = store.search(&[1.0, 0.0, 0.0], 1);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "v1");

        cleanup(&dir);
    }

    #[test]
    fn profiles_are_isolated() {
        let (store, dir) = make_store("isolated");
        store.create_profile("alice", "Alice").unwrap();
        store.create_profile("bob", "Bob").unwrap();

        // Insert into alice
        store.switch_profile("alice").unwrap();
        store
            .insert(1, "alice-vec", &[1.0, 0.0], serde_json::json!({}))
            .unwrap();

        // Insert into bob
        store.switch_profile("bob").unwrap();
        store
            .insert(1, "bob-vec", &[0.0, 1.0], serde_json::json!({}))
            .unwrap();

        // Alice's search returns alice-vec
        store.switch_profile("alice").unwrap();
        let results = store.search(&[1.0, 0.0], 1);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "alice-vec");

        // Bob's search returns bob-vec
        store.switch_profile("bob").unwrap();
        let results = store.search(&[0.0, 1.0], 1);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "bob-vec");

        cleanup(&dir);
    }

    #[test]
    fn load_existing_creates_default() {
        let (store, dir) = make_store("load_default");
        let count = store.load_existing().unwrap();
        assert_eq!(count, 1);
        assert!(store.profiles.contains_key("default"));
        cleanup(&dir);
    }

    #[test]
    fn load_existing_reads_persisted() {
        let dir = tmp_dir("load_persisted");

        // First store: create and persist
        {
            let store = ProfileStore::new(dir.clone(), HnswServiceConfig::default());
            store.create_profile("alice", "Alice").unwrap();
            store.create_profile("bob", "Bob").unwrap();
            store.persist_meta("alice").unwrap();
            store.persist_meta("bob").unwrap();
        }

        // Second store: load from disk
        {
            let store = ProfileStore::new(dir.clone(), HnswServiceConfig::default());
            let count = store.load_existing().unwrap();
            assert_eq!(count, 2);
            assert!(store.profiles.contains_key("alice"));
            assert!(store.profiles.contains_key("bob"));
        }

        cleanup(&dir);
    }

    #[test]
    fn persist_meta_updates_vector_count() {
        let (store, dir) = make_store("persist_count");
        store.create_profile("alice", "Alice").unwrap();
        store.switch_profile("alice").unwrap();
        store
            .insert(1, "v1", &[1.0, 0.0], serde_json::json!({}))
            .unwrap();
        store.persist_meta("alice").unwrap();

        // Read back the JSON
        let meta_path = dir.join("alice").join("profile.json");
        let data = std::fs::read_to_string(&meta_path).unwrap();
        let meta: ProfileMeta = serde_json::from_str(&data).unwrap();
        assert_eq!(meta.vector_count, 1);

        cleanup(&dir);
    }
}
