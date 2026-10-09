//! `project-fetch.json` (ADR-108 P2b/P3b contract, section 3): which paired
//! peers may fetch which projects over `project.fetch`. The `pair` action
//! writes it (P2b, [`crate::mesh_pair`]); the fetch gate reads it (P3b).
//!
//! Default deny. A missing file grants nothing. A file that is group- or
//! world-writable, owned by another user, too large, not version 1 or not
//! JSON is an error, and the caller must refuse. Entries that are not well
//! formed grant nothing but are kept as they are by the writer, as is every
//! key this build does not know.
//!
//! ```json
//! {"version": 1,
//!  "grants": [{"peer_node": "<mesh node id>", "projects": ["<ULID>"],
//!              "granted_at": "<rfc3339>", "source": "dashboard-pair:<action id>",
//!              "peer_ed25519": "<64 hex>"}]}
//! ```
//!
//! `peer_ed25519` is this build's addition to the contract: the peer's
//! `workload-host` signing key, so a gate can match the signer of a request
//! and not only the node id it derives (`node_id_from_pubkey`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The grants file under the daemon's runtime dir (next to `workload-peers.json`).
pub const FETCH_FILE: &str = "project-fetch.json";
/// The only version this build reads or writes.
pub const VERSION: u64 = 1;
/// Most grants one file may hold.
pub const MAX_GRANTS: usize = 256;
const MAX_BYTES: u64 = 256 * 1024;

/// One grant: a peer and the projects it may fetch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    /// Mesh node id of the peer (32 hex).
    pub peer_node: String,
    /// Project ULIDs it may fetch.
    #[serde(default)]
    pub projects: Vec<String>,
    /// RFC 3339 time the grant was written.
    #[serde(default)]
    pub granted_at: Option<String>,
    /// `dashboard-pair:<action id>`.
    #[serde(default)]
    pub source: Option<String>,
    /// The peer's `workload-host` signing key (64 hex), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_ed25519: Option<String>,
}

/// The grants a node holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Grants {
    pub grants: Vec<Grant>,
}

impl Grants {
    /// Read the grants under `dir`; an absent file is no grants.
    pub fn load(dir: &Path) -> Result<Self, String> {
        let raw = read_raw(&dir.join(FETCH_FILE))?;
        let grants = raw["grants"]
            .as_array()
            .map(|a| a.iter().filter_map(|g| serde_json::from_value::<Grant>(g.clone()).ok()).collect())
            .unwrap_or_default();
        Ok(Self { grants })
    }

    /// True when a grant names `peer_node` and `ulid` (exact matches).
    pub fn is_granted(&self, peer_node: &str, ulid: &str) -> bool {
        self.grants
            .iter()
            .any(|g| g.peer_node == peer_node && g.projects.iter().any(|p| p == ulid))
    }

    /// The grants of one peer.
    pub fn for_peer(&self, peer_node: &str) -> Vec<&Grant> {
        self.grants.iter().filter(|g| g.peer_node == peer_node).collect()
    }
}

/// Non-unix hosts have no mode bits to check (the reporter refuses to run there anyway).
#[cfg(not(unix))]
pub(crate) fn private_file_ok(_path: &Path, _m: &std::fs::Metadata) -> Result<(), String> {
    Ok(())
}

/// Refuse a file someone other than its owner (or root) could have written.
#[cfg(unix)]
pub(crate) fn private_file_ok(path: &Path, m: &std::fs::Metadata) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    if m.mode() & 0o022 != 0 {
        return Err(format!("{} is group- or world-writable (mode {:o}); chmod 600 it", path.display(), m.mode() & 0o7777));
    }
    let me = nix::unistd::geteuid().as_raw();
    if m.uid() != me && m.uid() != 0 {
        return Err(format!("{} is owned by another user (uid {}); it must be owned by the daemon's user", path.display(), m.uid()));
    }
    Ok(())
}

/// The file as JSON, `{"version": 1, "grants": []}` when absent.
fn read_raw(path: &Path) -> Result<Value, String> {
    let Ok(m) = std::fs::metadata(path) else {
        return Ok(json!({ "version": VERSION, "grants": [] }));
    };
    private_file_ok(path, &m)?;
    if m.len() > MAX_BYTES {
        return Err(format!("{} is too large", path.display()));
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("{FETCH_FILE}: {e}"))?;
    if !v.is_object() {
        return Err(format!("{FETCH_FILE}: not an object"));
    }
    if v["version"].as_u64() != Some(VERSION) {
        return Err(format!("{FETCH_FILE}: version must be {VERSION}"));
    }
    if !v["grants"].is_array() {
        return Err(format!("{FETCH_FILE}: grants must be an array"));
    }
    Ok(v)
}

fn write_raw(path: &Path, v: &Value) -> Result<(), String> {
    let text = serde_json::to_string_pretty(v).map_err(|e| e.to_string())?;
    crate::dashboard_token::write_atomic(path, &text).map_err(|e| format!("{}: {e}", path.display()))
}

fn names(entry: &Value, peer_node: &str) -> bool {
    entry.get("peer_node").and_then(Value::as_str) == Some(peer_node)
}

/// Write `g`, replacing every grant of the same peer (so a re-add is one
/// grant). Other entries, and other keys of the file, are kept verbatim.
pub fn grant(dir: &Path, g: &Grant) -> Result<PathBuf, String> {
    let path = dir.join(FETCH_FILE);
    let mut raw = read_raw(&path)?;
    let list = raw["grants"].as_array_mut().ok_or("grants must be an array")?;
    list.retain(|e| !names(e, &g.peer_node));
    if list.len() >= MAX_GRANTS {
        return Err(format!("{FETCH_FILE}: more than {MAX_GRANTS} grants"));
    }
    list.push(serde_json::to_value(g).map_err(|e| e.to_string())?);
    write_raw(&path, &raw)?;
    Ok(path)
}

/// Remove every grant of `peer_node`. False when there was none (the file
/// is then left untouched).
pub fn revoke(dir: &Path, peer_node: &str) -> Result<bool, String> {
    let path = dir.join(FETCH_FILE);
    let mut raw = read_raw(&path)?;
    let list = raw["grants"].as_array_mut().ok_or("grants must be an array")?;
    let before = list.len();
    list.retain(|e| !names(e, peer_node));
    if list.len() == before {
        return Ok(false);
    }
    write_raw(&path, &raw)?;
    Ok(true)
}

#[cfg(test)]
#[path = "project_fetch_grants_tests.rs"]
mod tests;
