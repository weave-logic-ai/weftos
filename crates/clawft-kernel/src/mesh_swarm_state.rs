//! Per-node swarm state held by [`crate::mesh_artifact::ArtifactExchange`]
//! (ADR-099 section 6, card mesh-placement-25): bandwidth caps, banned
//! peers, measured link speeds, the revocation list, the seeding record and
//! the cache hook.

use std::sync::{Arc, Mutex, OnceLock, Weak};

use dashmap::DashMap;

use crate::mesh_artifact_types::ExchangeConfig;
use crate::mesh_swarm_cache::ArtifactCache;
use crate::mesh_swarm_picker::LinkStats;
use crate::mesh_swarm_rate::Bandwidth;
use crate::revocation::RevocationList;

/// What allows one content hash to be seeded: the verified package that
/// lists it and that package's signers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantInfo {
    /// Package id of the verified manifest.
    pub package_id: String,
    /// Lower-case hex public keys of the manifest's accepted signers.
    pub signers: Vec<String>,
    /// False for a package that must not be handed to other nodes (Cognitum
    /// provenance; no licence check exists here): it is never seeded,
    /// advertised or served, though this node may hold and run it.
    pub redistributable: bool,
}

/// Mutable swarm state of one node.
pub(crate) struct SwarmState {
    pub(crate) bandwidth: Bandwidth,
    pub(crate) links: LinkStats,
    /// Banned peer -> reason.
    pub(crate) bans: DashMap<String, String>,
    /// Corrupt pieces seen per peer.
    pub(crate) corrupt: DashMap<String, u32>,
    pub(crate) revocations: OnceLock<Arc<RevocationList>>,
    /// Blobs this exchange created in the store (the only ones it may evict).
    pub(crate) owned: DashMap<[u8; 32], ()>,
    /// Content hashes already chained as `artifact.seed`.
    pub(crate) seeded: DashMap<[u8; 32], ()>,
    cache: Mutex<Weak<ArtifactCache>>,
}

impl SwarmState {
    pub(crate) fn new(cfg: &ExchangeConfig) -> Self {
        Self {
            bandwidth: Bandwidth::new(cfg.upload_bytes_per_sec, cfg.download_bytes_per_sec),
            links: LinkStats::default(),
            bans: DashMap::new(),
            corrupt: DashMap::new(),
            revocations: OnceLock::new(),
            seeded: DashMap::new(),
            owned: DashMap::new(),
            cache: Mutex::new(Weak::new()),
        }
    }

    pub(crate) fn attach_cache(&self, cache: &Arc<ArtifactCache>) {
        *self.cache.lock().unwrap_or_else(|p| p.into_inner()) = Arc::downgrade(cache);
    }

    pub(crate) fn cache(&self) -> Option<Arc<ArtifactCache>> {
        self.cache.lock().unwrap_or_else(|p| p.into_inner()).upgrade()
    }
}
