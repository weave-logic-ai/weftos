//! Content-addressed artifact store using BLAKE3 hashes (K3-G1).
//!
//! Stores arbitrary binary artifacts (WASM modules, app manifests,
//! config bundles) indexed by their BLAKE3 content hash. Deduplicates
//! automatically: storing the same content twice returns the same hash
//! and increments the reference count.
//!
//! This module requires the `ecc` feature (which pulls `blake3`).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tracing::info;

#[cfg(feature = "exochain")]
use std::sync::Arc;

use crate::error::KernelError;
use crate::health::HealthStatus;
use crate::service::{ServiceType, SystemService};

// ---------------------------------------------------------------------------
// ArtifactType
// ---------------------------------------------------------------------------

/// The kind of content stored in the artifact.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ArtifactType {
    /// WebAssembly module (.wasm).
    WasmModule,
    /// Application manifest (app.json).
    AppManifest,
    /// Configuration bundle.
    ConfigBundle,
    /// Untyped binary blob.
    Generic,
}

impl std::fmt::Display for ArtifactType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WasmModule => write!(f, "wasm-module"),
            Self::AppManifest => write!(f, "app-manifest"),
            Self::ConfigBundle => write!(f, "config-bundle"),
            Self::Generic => write!(f, "generic"),
        }
    }
}

// ---------------------------------------------------------------------------
// StoredArtifact (metadata)
// ---------------------------------------------------------------------------

/// Metadata about a stored artifact (the data lives in the backend).
pub struct StoredArtifact {
    /// BLAKE3 hex hash of the content.
    pub hash: String,
    /// Content size in bytes.
    pub size: u64,
    /// Content type classification.
    pub content_type: ArtifactType,
    /// When this artifact was first stored.
    pub stored_at: DateTime<Utc>,
    /// Number of active references (for GC).
    pub reference_count: AtomicU32,
}

// ---------------------------------------------------------------------------
// ArtifactBackend
// ---------------------------------------------------------------------------

/// Storage backend for artifact data.
#[non_exhaustive]
pub enum ArtifactBackend {
    /// In-memory storage (for tests / embedded use).
    Memory(DashMap<String, Vec<u8>>),
    /// File-system storage with two-level directory sharding.
    File {
        /// Base directory for artifact files.
        base_path: PathBuf,
    },
}

// ---------------------------------------------------------------------------
// ArtifactStore
// ---------------------------------------------------------------------------

/// Content-addressed artifact store using BLAKE3 hashes.
///
/// Thread-safe via `DashMap`. The store guarantees:
/// - **Deduplication**: same content always produces the same hash.
/// - **Integrity**: every load verifies the content hash.
/// - **Reference counting**: tracks how many consumers reference each artifact.
pub struct ArtifactStore {
    /// Hash -> metadata index.
    artifacts: DashMap<String, StoredArtifact>,
    /// Data storage backend.
    backend: ArtifactBackend,
    /// Total stored bytes across all artifacts.
    total_size: AtomicU64,
    /// Optional chain manager for ExoChain event logging.
    #[cfg(feature = "exochain")]
    chain_manager: Option<Arc<crate::chain::ChainManager>>,
}

impl ArtifactStore {
    /// Create a new in-memory artifact store.
    pub fn new_memory() -> Self {
        Self {
            artifacts: DashMap::new(),
            backend: ArtifactBackend::Memory(DashMap::new()),
            total_size: AtomicU64::new(0),
            #[cfg(feature = "exochain")]
            chain_manager: None,
        }
    }

    /// Create a new file-backed artifact store.
    pub fn new_file(base_path: PathBuf) -> Self {
        Self {
            artifacts: DashMap::new(),
            backend: ArtifactBackend::File { base_path },
            total_size: AtomicU64::new(0),
            #[cfg(feature = "exochain")]
            chain_manager: None,
        }
    }

    /// Set the chain manager for ExoChain event logging.
    #[cfg(feature = "exochain")]
    pub fn set_chain_manager(&mut self, cm: Arc<crate::chain::ChainManager>) {
        self.chain_manager = Some(cm);
    }

    /// Store content and return the BLAKE3 hash.
    ///
    /// If the content is already stored, increments the reference count
    /// and returns the existing hash (deduplication).
    pub fn store(&self, content: &[u8], content_type: ArtifactType) -> Result<String, KernelError> {
        let hash = blake3::hash(content).to_hex().to_string();

        // Dedup: if hash exists, just bump reference count.
        if let Some(existing) = self.artifacts.get(&hash) {
            existing.reference_count.fetch_add(1, Ordering::Relaxed);
            return Ok(hash);
        }

        // Write to backend.
        match &self.backend {
            ArtifactBackend::Memory(map) => {
                map.insert(hash.clone(), content.to_vec());
            }
            ArtifactBackend::File { base_path } => {
                let prefix = &hash[..2.min(hash.len())];
                let dir = base_path.join(prefix);
                std::fs::create_dir_all(&dir)
                    .map_err(|e| KernelError::Service(format!("artifact dir create: {e}")))?;
                std::fs::write(dir.join(&hash), content)
                    .map_err(|e| KernelError::Service(format!("artifact write: {e}")))?;
            }
        }

        // Record metadata.
        let size = content.len() as u64;
        #[cfg(feature = "exochain")]
        let content_type_str = content_type.to_string();
        self.artifacts.insert(
            hash.clone(),
            StoredArtifact {
                hash: hash.clone(),
                size,
                content_type,
                stored_at: Utc::now(),
                reference_count: AtomicU32::new(1),
            },
        );
        self.total_size.fetch_add(size, Ordering::Relaxed);

        #[cfg(feature = "exochain")]
        if let Some(ref cm) = self.chain_manager {
            cm.append(
                "artifact_store",
                crate::chain::EVENT_KIND_ARTIFACT_STORE,
                Some(serde_json::json!({
                    "hash": hash,
                    "size": size,
                    "content_type": content_type_str,
                })),
            );
        }

        info!(hash = %hash, size, "artifact stored");
        Ok(hash)
    }

    /// Load content by hash, verifying integrity on read.
    pub fn load(&self, hash: &str) -> Result<Vec<u8>, KernelError> {
        if !self.artifacts.contains_key(hash) {
            return Err(KernelError::Service(format!("artifact not found: {hash}")));
        }

        let content = match &self.backend {
            ArtifactBackend::Memory(map) => map.get(hash).map(|v| v.value().clone()),
            ArtifactBackend::File { base_path } => {
                let prefix = &hash[..2.min(hash.len())];
                let path = base_path.join(prefix).join(hash);
                std::fs::read(&path).ok()
            }
        };

        let content = content
            .ok_or_else(|| KernelError::Service(format!("artifact data missing: {hash}")))?;

        // Verify integrity.
        let actual_hash = blake3::hash(&content).to_hex().to_string();
        if actual_hash != hash {
            return Err(KernelError::Service(format!(
                "artifact integrity error: expected {hash}, got {actual_hash}"
            )));
        }

        Ok(content)
    }

    /// Check whether an artifact exists by hash.
    pub fn contains(&self, hash: &str) -> bool {
        self.artifacts.contains_key(hash)
    }

    /// Decrement the reference count for an artifact.
    ///
    /// Returns `true` if the reference count reached zero (eligible for GC).
    pub fn release(&self, hash: &str) -> bool {
        if let Some(entry) = self.artifacts.get(hash) {
            let prev = entry.reference_count.fetch_sub(1, Ordering::Relaxed);
            return prev <= 1;
        }
        false
    }

    /// Remove an artifact and its data from storage.
    pub fn remove(&self, hash: &str) -> Result<(), KernelError> {
        if let Some((_, meta)) = self.artifacts.remove(hash) {
            self.total_size.fetch_sub(meta.size, Ordering::Relaxed);
            match &self.backend {
                ArtifactBackend::Memory(map) => {
                    map.remove(hash);
                }
                ArtifactBackend::File { base_path } => {
                    let prefix = &hash[..2.min(hash.len())];
                    let path = base_path.join(prefix).join(hash);
                    let _ = std::fs::remove_file(path);
                }
            }

            #[cfg(feature = "exochain")]
            if let Some(ref cm) = self.chain_manager {
                cm.append(
                    "artifact_store",
                    crate::chain::EVENT_KIND_ARTIFACT_REMOVE,
                    Some(serde_json::json!({
                        "hash": hash,
                    })),
                );
            }
        }
        Ok(())
    }

    /// Total number of stored artifacts.
    pub fn count(&self) -> usize {
        self.artifacts.len()
    }

    /// Total stored bytes.
    pub fn total_bytes(&self) -> u64 {
        self.total_size.load(Ordering::Relaxed)
    }

    /// Get metadata for an artifact (hash, size, type, stored_at, refcount).
    pub fn metadata(&self, hash: &str) -> Option<(String, u64, ArtifactType, DateTime<Utc>, u32)> {
        self.artifacts.get(hash).map(|a| {
            (
                a.hash.clone(),
                a.size,
                a.content_type.clone(),
                a.stored_at,
                a.reference_count.load(Ordering::Relaxed),
            )
        })
    }
}

#[async_trait]
impl SystemService for ArtifactStore {
    fn name(&self) -> &str {
        "artifact-store"
    }

    fn service_type(&self) -> ServiceType {
        ServiceType::Core
    }

    async fn start(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("artifact store started");
        Ok(())
    }

    async fn stop(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!(
            count = self.count(),
            total_bytes = self.total_bytes(),
            "artifact store stopped"
        );
        Ok(())
    }

    async fn health_check(&self) -> HealthStatus {
        HealthStatus::Healthy
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_and_load_roundtrip() {
        let store = ArtifactStore::new_memory();
        let content = b"hello weftos artifact";
        let hash = store.store(content, ArtifactType::Generic).unwrap();
        let loaded = store.load(&hash).unwrap();
        assert_eq!(loaded, content);
    }

    #[test]
    fn hash_verification_on_load() {
        let store = ArtifactStore::new_memory();
        let content = b"original content";
        let hash = store.store(content, ArtifactType::WasmModule).unwrap();

        // Tamper with stored data.
        if let ArtifactBackend::Memory(map) = &store.backend {
            map.insert(hash.clone(), b"tampered content".to_vec());
        }

        let result = store.load(&hash);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("integrity error"), "got: {err}");
    }

    #[test]
    fn duplicate_store_returns_same_hash() {
        let store = ArtifactStore::new_memory();
        let content = b"dedup test";
        let h1 = store.store(content, ArtifactType::Generic).unwrap();
        let h2 = store.store(content, ArtifactType::Generic).unwrap();
        assert_eq!(h1, h2);
        assert_eq!(store.count(), 1);
    }

    #[test]
    fn duplicate_store_increments_refcount() {
        let store = ArtifactStore::new_memory();
        let content = b"refcount test";
        let hash = store.store(content, ArtifactType::Generic).unwrap();
        let _ = store.store(content, ArtifactType::Generic).unwrap();
        let (_, _, _, _, refcount) = store.metadata(&hash).unwrap();
        assert_eq!(refcount, 2);
    }

    #[test]
    fn reference_counting_release() {
        let store = ArtifactStore::new_memory();
        let content = b"rc data";
        let hash = store.store(content, ArtifactType::Generic).unwrap();
        let _ = store.store(content, ArtifactType::Generic).unwrap();

        assert!(!store.release(&hash)); // 2 -> 1, not zero yet
        assert!(store.release(&hash)); // 1 -> 0, eligible for GC
    }

    #[test]
    fn load_nonexistent_fails() {
        let store = ArtifactStore::new_memory();
        let result = store.load("0000000000000000000000000000000000000000000000000000000000000000");
        assert!(result.is_err());
    }

    #[test]
    fn total_bytes_tracked() {
        let store = ArtifactStore::new_memory();
        store.store(b"aaaa", ArtifactType::Generic).unwrap();
        store.store(b"bbb", ArtifactType::Generic).unwrap();
        assert_eq!(store.total_bytes(), 7);
    }

    #[test]
    fn remove_artifact() {
        let store = ArtifactStore::new_memory();
        let hash = store.store(b"remove me", ArtifactType::Generic).unwrap();
        assert!(store.contains(&hash));
        store.remove(&hash).unwrap();
        assert!(!store.contains(&hash));
        assert_eq!(store.count(), 0);
    }

    #[test]
    fn file_backend_roundtrip() {
        let dir = std::env::temp_dir().join(format!("artifact_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        let store = ArtifactStore::new_file(dir.clone());
        let content = b"file backend content";
        let hash = store.store(content, ArtifactType::AppManifest).unwrap();

        // Verify two-level directory structure.
        let prefix = &hash[..2];
        let file_path = dir.join(prefix).join(&hash);
        assert!(file_path.exists(), "expected file at {file_path:?}");

        let loaded = store.load(&hash).unwrap();
        assert_eq!(loaded, content);

        // Cleanup.
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn different_content_different_hash() {
        let store = ArtifactStore::new_memory();
        let h1 = store.store(b"alpha", ArtifactType::Generic).unwrap();
        let h2 = store.store(b"beta", ArtifactType::Generic).unwrap();
        assert_ne!(h1, h2);
        assert_eq!(store.count(), 2);
    }

    #[tokio::test]
    async fn system_service_impl() {
        let store = ArtifactStore::new_memory();
        assert_eq!(store.name(), "artifact-store");
        assert_eq!(store.service_type(), ServiceType::Core);
        store.start().await.unwrap();
        let health = store.health_check().await;
        assert_eq!(health, HealthStatus::Healthy);
        store.stop().await.unwrap();
    }
}
