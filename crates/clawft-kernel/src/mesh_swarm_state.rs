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

/// Where a package's licence stands, from its signed manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrantOrigin {
    /// Cognitum provenance (a `cognitum.*` attestation or a `cognitum`
    /// release URL): licence-gated, whatever else the manifest says.
    Cognitum {
        /// Cog id from the manifest.
        cog_id: String,
        /// Cog version from the manifest.
        version: String,
    },
    /// The signer wrote `redistributable = true`.
    OptIn,
    /// The signer did not opt in (the field is absent or false).
    NotFlagged,
}

/// What allows one content hash to be seeded: the verified package that
/// lists it and that package's signers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantInfo {
    /// Package id of the verified manifest.
    pub package_id: String,
    /// Lower-case hex public keys of the manifest's accepted signers.
    pub signers: Vec<String>,
    /// Licence standing of the package. Anything but `OptIn` must not be
    /// handed to other nodes by default (see [`ManifestPolicy`]).
    pub origin: GrantOrigin,
}

impl GrantInfo {
    /// True when the signer opted in to redistribution.
    pub fn is_opt_in(&self) -> bool {
        self.origin == GrantOrigin::OptIn
    }
}

/// An authenticated (or not) peer asking to be served.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServePeer {
    /// The peer's node id.
    pub node_id: String,
    /// True only when the id was verified by admission (a signed hello bound
    /// to the connection); false for a claimed id.
    pub verified: bool,
}

impl ServePeer {
    /// A peer whose id is only claimed (the default for [`crate::mesh_artifact::ArtifactExchange::serve`]).
    pub fn unverified(node_id: impl Into<String>) -> Self {
        Self {
            node_id: node_id.into(),
            verified: false,
        }
    }

    /// A peer whose id admission verified.
    pub fn verified(node_id: impl Into<String>) -> Self {
        Self {
            node_id: node_id.into(),
            verified: true,
        }
    }
}

/// Who a redistribution decision is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Audience<'a> {
    /// Serving pieces (or a descriptor) to this requesting peer.
    Serve(&'a ServePeer),
    /// Listing the content in facts broadcast to every peer.
    Advertise,
    /// Starting to seed it (`artifact.seed`).
    Seed,
}

/// Decides whether the grants for a content hash allow handing it to an
/// audience. The one policy point: seeding, advertisement and serving all go
/// through [`crate::mesh_artifact::ArtifactExchange`]'s grant check, which
/// asks this. It is fixed when the exchange is built
/// ([`crate::mesh_artifact_types::ExchangeConfig::redistribution`]); there is
/// no way to swap it afterwards.
///
/// Whatever the policy says, content that is not `OptIn` is never listed in
/// broadcast facts: it can be discovered only by asking a peer
/// ([`crate::mesh_artifact::ArtifactExchange::who_has`]), where the serving
/// side sees who is asking.
///
/// `grants` is every grant held for the hash, revoked or not (a revoked one
/// still counts toward a veto until the sweep removes it).
pub trait RedistributionPolicy: std::fmt::Debug + Send + Sync + 'static {
    /// May `content_hash` be handed to `audience`, given its `grants`? Fail
    /// closed: an empty list is "no".
    fn allows(&self, content_hash: &[u8; 32], grants: &[GrantInfo], audience: &Audience<'_>)
    -> bool;
}

/// The default: every package that lists the hash must have been signed
/// `redistributable = true` and have no Cognitum provenance (`OptIn`); any
/// other grant vetoes the hash for every package, for every audience.
#[derive(Debug, Default, Clone, Copy)]
pub struct ManifestPolicy;

impl RedistributionPolicy for ManifestPolicy {
    fn allows(&self, _: &[u8; 32], grants: &[GrantInfo], _: &Audience<'_>) -> bool {
        !grants.is_empty() && grants.iter().all(GrantInfo::is_opt_in)
    }
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
    /// Redistribution policy, fixed at construction.
    pub(crate) policy: Arc<dyn RedistributionPolicy>,
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
            policy: cfg.redistribution.clone(),
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
