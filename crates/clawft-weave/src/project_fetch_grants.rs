//! Who may fetch which project over the mesh (ADR-108 P3b, contract section 3).
//!
//! `project.fetch` is default deny. The primary serves it to a caller only when
//! both hold at the moment of the call:
//!
//! 1. the caller's node key is listed in `workload-peers.json` **with a key**
//!    at tier `pinned` or `paired` (an entry without a key trusts an address,
//!    not a node, and never earns fetch access);
//! 2. `project-fetch.json` (next to it, written by pairing) has a grant naming
//!    that node id and that project ULID.
//!
//! Both files are re-read per check, so removing the peer or the grant stops
//! the next request. This file is the P3b lane's minimal reader; the pairing
//! lane ships the writer, and the lead reconciles the two at merge.
//!
//! [`FetchPeerPolicy`] is the host's controller policy: controllers may call
//! anything, a peer with a grant may call only `workload.describe` (to be
//! learned by the member's control plane) and `project.fetch`.

use std::path::{Path, PathBuf};

use clawft_kernel::node_id_from_pubkey;
use clawft_kernel::workload_ctl::msg::{ControllerPolicy, method};
use clawft_types::placement::TrustTier;
use serde::Deserialize;

use crate::workload_place_policy::{load_peers, parse};

/// Grants file under the runtime dir.
pub const GRANTS_FILE: &str = "project-fetch.json";
const MAX_GRANTS: usize = 256;
const MAX_PROJECTS_PER_GRANT: usize = 64;

#[derive(Debug, Clone, Deserialize)]
struct GrantsFile {
    #[serde(default = "one")]
    version: u32,
    #[serde(default)]
    grants: Vec<Grant>,
}

fn one() -> u32 {
    1
}

/// One grant: a peer may fetch these projects.
#[derive(Debug, Clone, Deserialize)]
pub struct Grant {
    pub peer_node: String,
    #[serde(default)]
    pub projects: Vec<String>,
    #[serde(default)]
    pub granted_at: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
}

/// The grants of this node, as read from [`GRANTS_FILE`].
#[derive(Debug, Clone, Default)]
pub struct FetchGrants {
    grants: Vec<Grant>,
}

impl FetchGrants {
    /// Read the grants file; absent means no grants. A malformed file is an
    /// error (and so denies everything) rather than a partial read.
    pub fn load(dir: &Path) -> Result<Self, String> {
        let Some(f) = parse::<GrantsFile>(dir, GRANTS_FILE)? else {
            return Ok(Self::default());
        };
        if f.version != 1 {
            return Err(format!("{GRANTS_FILE}: unsupported version {}", f.version));
        }
        if f.grants.len() > MAX_GRANTS {
            return Err(format!("{GRANTS_FILE}: more than {MAX_GRANTS} grants"));
        }
        for g in &f.grants {
            if g.peer_node.is_empty() || g.peer_node.len() > 128 {
                return Err(format!("{GRANTS_FILE}: bad peer_node"));
            }
            if g.projects.len() > MAX_PROJECTS_PER_GRANT {
                return Err(format!("{GRANTS_FILE}: grant for {} lists too many projects", g.peer_node));
            }
            for p in &g.projects {
                clawft_types::project::validate_id(p).map_err(|_| format!("{GRANTS_FILE}: {p:?} is not a ULID"))?;
            }
        }
        Ok(Self { grants: f.grants })
    }

    /// True when a grant names `peer_node` and `project`.
    pub fn allows(&self, peer_node: &str, project: &str) -> bool {
        self.grants.iter().any(|g| g.peer_node == peer_node && g.projects.iter().any(|p| p == project))
    }

    /// True when any grant names `peer_node`.
    pub fn any_for(&self, peer_node: &str) -> bool {
        self.grants.iter().any(|g| g.peer_node == peer_node && !g.projects.is_empty())
    }
}

/// The tier `workload-peers.json` gives the node whose key is `key`: `None`
/// when it is not listed, or listed without a key.
pub fn listed_tier(dir: &Path, key: &[u8; 32]) -> Result<Option<TrustTier>, String> {
    Ok(load_peers(dir)?.into_iter().find(|p| p.key.as_ref() == Some(key)).map(|p| p.tier))
}

/// [`listed_tier`] by node id (the hook sees the verified requester id only).
pub fn listed_tier_of_node(dir: &Path, node_id: &str) -> Result<Option<TrustTier>, String> {
    Ok(load_peers(dir)?
        .into_iter()
        .find(|p| p.key.is_some_and(|k| node_id_from_pubkey(&k) == node_id))
        .map(|p| p.tier))
}

fn tier_ok(t: Option<TrustTier>) -> bool {
    matches!(t, Some(TrustTier::Pinned | TrustTier::Paired))
}

/// Check `requester` (a verified node id) may fetch `project` from this node.
/// `Err` is the reason shown to the caller; it never names other peers or
/// projects.
pub fn authorize(dir: &Path, requester: &str, project: &str) -> Result<TrustTier, String> {
    let tier = listed_tier_of_node(dir, requester).map_err(|e| format!("peer list unreadable: {e}"))?;
    if !tier_ok(tier) {
        return Err("fetch refused: the caller is not a pinned or paired peer of this node".into());
    }
    let grants = FetchGrants::load(dir).map_err(|e| format!("fetch grants unreadable: {e}"))?;
    if !grants.allows(requester, project) {
        return Err("fetch refused: no fetch grant for this peer and project".into());
    }
    Ok(tier.unwrap_or(TrustTier::Paired))
}

/// The host's controller policy with fetch peers admitted for the two
/// methods a fetch needs.
pub struct FetchPeerPolicy {
    controllers: Vec<[u8; 32]>,
    dir: PathBuf,
}

impl FetchPeerPolicy {
    /// `controllers` from `workload-host.json` (plus this node's own key);
    /// `dir` is the runtime dir holding the peer and grant files.
    pub fn new(controllers: Vec<[u8; 32]>, dir: PathBuf) -> Self {
        Self { controllers, dir }
    }

    fn fetch_peer(&self, key: &[u8; 32]) -> bool {
        match listed_tier(&self.dir, key) {
            Ok(t) if tier_ok(t) => {}
            _ => return false,
        }
        FetchGrants::load(&self.dir).is_ok_and(|g| g.any_for(&node_id_from_pubkey(key)))
    }
}

impl ControllerPolicy for FetchPeerPolicy {
    fn allows(&self, public_key: &[u8; 32]) -> bool {
        self.controllers.contains(public_key)
    }

    fn allows_method(&self, public_key: &[u8; 32], m: &str) -> bool {
        self.allows(public_key)
            || (matches!(m, method::DESCRIBE | method::PROJECT_FETCH) && self.fetch_peer(public_key))
    }
}

#[cfg(test)]
#[path = "project_fetch_grants_tests.rs"]
mod tests;
