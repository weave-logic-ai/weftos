//! What this node has paired with, by project (ADR-108 P2b, ADR-114): the
//! member side of a `pair` action records the primary and the projects the
//! approval named in `mesh-pairings.json`, next to `workload-peers.json`, so a
//! `weftos://<mesh>/projects/<ULID>` name can be resolved to the node that
//! serves it. Names identify things; this file is where the location lives.
//!
//! ```json
//! {"version": 1,
//!  "pairings": [{"peer_node": "<mesh node id>", "role": "primary",
//!                "projects": ["<ULID>"], "advertise": "host:port"}]}
//! ```
//!
//! Entries and keys this build does not know are kept as they are.

use std::path::Path;

use serde_json::{Value, json};

/// The file under the runtime dir.
pub const PAIRINGS_FILE: &str = "mesh-pairings.json";
const MAX_BYTES: u64 = 256 * 1024;
const MAX_ENTRIES: usize = 256;

fn read(dir: &Path) -> Result<Vec<Value>, String> {
    let path = dir.join(PAIRINGS_FILE);
    let Ok(m) = std::fs::metadata(&path) else { return Ok(Vec::new()) };
    if m.len() > MAX_BYTES {
        return Err(format!("{PAIRINGS_FILE} is too large"));
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{PAIRINGS_FILE}: {e}"))?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("{PAIRINGS_FILE}: {e}"))?;
    if v["version"].as_u64().unwrap_or(1) != 1 {
        return Err(format!("{PAIRINGS_FILE}: unsupported version"));
    }
    Ok(v["pairings"].as_array().cloned().unwrap_or_default())
}

fn write(dir: &Path, pairings: &[Value]) -> Result<(), String> {
    let text = serde_json::to_string_pretty(&json!({ "version": 1, "pairings": pairings })).map_err(|e| e.to_string())?;
    crate::dashboard_token::write_atomic(&dir.join(PAIRINGS_FILE), &text).map_err(|e| format!("{PAIRINGS_FILE}: {e}"))
}

/// Record (or replace) the pairing with `peer_node` in `role` for `projects`.
pub fn upsert(dir: &Path, peer_node: &str, role: &str, projects: &[String], advertise: &str) -> Result<(), String> {
    let mut all = read(dir)?;
    let entry = json!({ "peer_node": peer_node, "role": role, "projects": projects, "advertise": advertise });
    let full = all.len() >= MAX_ENTRIES;
    match all.iter_mut().find(|p| p["peer_node"] == peer_node) {
        Some(e) => {
            // Keep keys we do not know.
            if let (Some(o), Some(n)) = (e.as_object_mut(), entry.as_object()) {
                o.extend(n.clone());
            }
        }
        None if full => return Err(format!("{PAIRINGS_FILE}: more than {MAX_ENTRIES} pairings")),
        None => all.push(entry),
    }
    write(dir, &all)
}

/// Forget the pairing with `peer_node`; false when there was none.
pub fn remove(dir: &Path, peer_node: &str) -> Result<bool, String> {
    let mut all = read(dir)?;
    let before = all.len();
    all.retain(|p| p["peer_node"] != peer_node);
    if all.len() == before {
        return Ok(false);
    }
    write(dir, &all)?;
    Ok(true)
}

/// The node id of the primary paired for `ulid`, if any.
pub fn primary_for(dir: &Path, ulid: &str) -> Result<Option<String>, String> {
    Ok(read(dir)?
        .iter()
        .filter(|p| p["role"] == "primary")
        .find(|p| p["projects"].as_array().is_some_and(|a| a.iter().any(|x| x == ulid)))
        .and_then(|p| p["peer_node"].as_str().map(str::to_owned)))
}
