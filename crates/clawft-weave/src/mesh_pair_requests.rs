//! Pending pair requests (ADR-108 P2b, contract section 2): what this node
//! has asked to pair with, kept in `pair-requests.json` (mode 0600, next to
//! `workload-peers.json`) and carried in every heartbeat as
//! `report.pair_requests` (at most [`MAX_REPORTED`], oldest first) until the
//! dashboard's approval arrives as a `pair` action or the request is
//! cancelled.
//!
//! `weaver mesh pair request|list|cancel` and the install handler (an install
//! whose primary is not a paired peer) write through [`record`]; the `pair`
//! action settles the requests of the peer it adds or removes.
//!
//! ```json
//! {"version": 1,
//!  "requests": [{"request_id": "<uuid v4>", "with_node": "<mesh node id>",
//!                "projects": ["<ULID>"], "requested_at": "<rfc3339>"}]}
//! ```

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// The requests file under the daemon's runtime dir.
pub const REQUESTS_FILE: &str = "pair-requests.json";
/// Most requests one heartbeat carries.
pub const MAX_REPORTED: usize = 8;
/// Most requests the file holds; a new one past this is refused.
pub const MAX_STORED: usize = 32;
/// Most projects one request names.
pub const MAX_PROJECTS: usize = 32;
const VERSION: u64 = 1;
const MAX_BYTES: u64 = 64 * 1024;

/// One pending request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairRequest {
    /// UUID v4, made here.
    pub request_id: String,
    /// Mesh node id of the other side (32 hex).
    pub with_node: String,
    /// Project ULIDs the pairing is for (may be empty: "any project").
    #[serde(default)]
    pub projects: Vec<String>,
    /// RFC 3339.
    pub requested_at: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    version: u64,
    #[serde(default)]
    requests: Vec<PairRequest>,
    /// Keys this build does not know, kept as they are.
    #[serde(flatten)]
    extra: Map<String, Value>,
}

/// A Crockford base32 ULID (26 characters).
pub fn is_ulid(s: &str) -> bool {
    s.len() == 26 && s.chars().all(|c| matches!(c, '0'..='9' | 'A'..='H' | 'J' | 'K' | 'M' | 'N' | 'P'..='T' | 'V'..='Z'))
}

/// Validate a project list from a caller: ULIDs only, deduplicated, capped.
pub fn projects_ok(projects: &[String]) -> Result<Vec<String>, String> {
    if projects.len() > MAX_PROJECTS {
        return Err(format!("more than {MAX_PROJECTS} projects"));
    }
    let mut out: Vec<String> = Vec::with_capacity(projects.len());
    for p in projects {
        if !is_ulid(p) {
            return Err(format!("{p:?} is not a project ULID"));
        }
        if !out.contains(p) {
            out.push(p.clone());
        }
    }
    Ok(out)
}

fn read(dir: &Path) -> Result<File, String> {
    let path = dir.join(REQUESTS_FILE);
    let Ok(m) = std::fs::metadata(&path) else {
        return Ok(File { version: VERSION, ..File::default() });
    };
    crate::project_fetch_grants::private_file_ok(&path, &m)?;
    if m.len() > MAX_BYTES {
        return Err(format!("{REQUESTS_FILE} is too large"));
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{REQUESTS_FILE}: {e}"))?;
    let f: File = serde_json::from_str(&text).map_err(|e| format!("{REQUESTS_FILE}: {e}"))?;
    if f.version != VERSION {
        return Err(format!("{REQUESTS_FILE}: version must be {VERSION}"));
    }
    Ok(f)
}

fn write(dir: &Path, f: &File) -> Result<(), String> {
    let text = serde_json::to_string_pretty(f).map_err(|e| e.to_string())?;
    crate::dashboard_token::write_atomic(&dir.join(REQUESTS_FILE), &text).map_err(|e| format!("{REQUESTS_FILE}: {e}"))
}

/// Record a request to pair with `with_node` for `projects`. A pending
/// request for the same node and the same projects is returned as it is
/// (no duplicate); a different project set is a new request.
pub fn record(dir: &Path, with_node: &str, projects: &[String]) -> Result<PairRequest, String> {
    if !clawft_kernel::is_node_id(with_node) {
        return Err(format!("{with_node:?} is not a mesh node id (32 lower-case hex)"));
    }
    let projects = projects_ok(projects)?;
    let mut f = read(dir)?;
    if let Some(r) = f.requests.iter().find(|r| r.with_node == with_node && same_set(&r.projects, &projects)) {
        return Ok(r.clone());
    }
    if f.requests.len() >= MAX_STORED {
        return Err(format!("{REQUESTS_FILE}: already {MAX_STORED} pending requests; cancel some first"));
    }
    let r = PairRequest {
        request_id: uuid::Uuid::new_v4().to_string(),
        with_node: with_node.to_owned(),
        projects,
        requested_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    };
    f.requests.push(r.clone());
    write(dir, &f)?;
    Ok(r)
}

fn same_set(a: &[String], b: &[String]) -> bool {
    a.len() == b.len() && a.iter().all(|x| b.contains(x))
}

/// Every pending request, oldest first.
pub fn list(dir: &Path) -> Result<Vec<PairRequest>, String> {
    Ok(read(dir)?.requests)
}

/// Drop one request by id. False when there was none.
pub fn cancel(dir: &Path, request_id: &str) -> Result<bool, String> {
    let mut f = read(dir)?;
    let before = f.requests.len();
    f.requests.retain(|r| r.request_id != request_id);
    if f.requests.len() == before {
        return Ok(false);
    }
    write(dir, &f)?;
    Ok(true)
}

/// The `pair` action for `peer_node` ran: drop its requests (and the one the
/// action names, whichever node it was for). Returns how many were dropped.
pub fn settle(dir: &Path, peer_node: &str, request_id: Option<&str>) -> Result<usize, String> {
    let mut f = read(dir)?;
    let before = f.requests.len();
    f.requests.retain(|r| r.with_node != peer_node && Some(r.request_id.as_str()) != request_id);
    let dropped = before - f.requests.len();
    if dropped > 0 {
        write(dir, &f)?;
    }
    Ok(dropped)
}

/// Put `pair_requests` (at most [`MAX_REPORTED`], oldest first) on a report.
pub fn attach(report: &mut Value, requests: &[PairRequest]) {
    let shown: Vec<&PairRequest> = requests.iter().take(MAX_REPORTED).collect();
    report["pair_requests"] = serde_json::to_value(shown).unwrap_or_else(|_| json!([]));
}

#[cfg(test)]
#[path = "mesh_pair_requests_tests.rs"]
mod tests;
