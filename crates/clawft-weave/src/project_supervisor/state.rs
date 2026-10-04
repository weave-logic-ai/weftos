//! `<run>/<id>/state.json` and the `revoked` marker.

use std::path::Path;

use clawft_kernel::parent_policy::write_atomic_0600;
use clawft_types::project::ChildState;
use clawft_types::runtime_paths::STATE_JSON_FILE;
use serde::{Deserialize, Serialize};

/// What the supervisor records about one child (survives a user-daemon
/// restart; adoption reads `exe` and `pid`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateFile {
    /// Supervisor state machine.
    pub state: ChildState,
    /// Child pid when one was started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// Inspected immutable identity for a Linux container child.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<ContainerState>,
    /// Executable the child was started from (adoption compares its name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exe: Option<String>,
    /// Unix seconds of the last start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_unix: Option<u64>,
    /// Unix seconds of this write.
    #[serde(default)]
    pub updated_unix: u64,
    /// Automatic restarts so far.
    #[serde(default)]
    pub restarts: u32,
    /// Exit code of the last exit, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_exit_code: Option<i32>,
    /// Why the project is `failed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_reason: Option<String>,
    /// Build stamp the running kernel reported in its handshake, recorded
    /// when it became ready or was adopted (the stale-build check reads it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kernel_sha: Option<String>,
    /// Crate version the running kernel reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kernel_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerState {
    pub engine: String,
    pub id: String,
    pub host_socket: std::path::PathBuf,
}

/// Write `state.json` atomically (0600). Best effort: a failure is logged,
/// never fatal to supervision.
pub fn write(run_dir: &Path, st: &StateFile) {
    let text = match serde_json::to_vec_pretty(st) {
        Ok(t) => t,
        Err(_) => return,
    };
    if let Err(e) = write_atomic_0600(&run_dir.join(STATE_JSON_FILE), &text) {
        tracing::warn!(dir = %run_dir.display(), error = %e, "could not write state.json");
    }
}

/// Read `state.json`; `None` when missing or unparseable.
pub fn read(run_dir: &Path) -> Option<StateFile> {
    let text = std::fs::read_to_string(run_dir.join(STATE_JSON_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

// The terminal `revoked` marker is written by the user daemon's
// `project.revoke` (`project_cert_rpc::on_identity_change`); the supervisor
// only reads it, at the same path (`runtime_paths::revoked_marker`).

/// True when the marker of `id` under the daemon's `run_root` exists.
pub fn is_marked_revoked(run_root: &Path, id: &str) -> bool {
    clawft_types::runtime_paths::revoked_marker(run_root, id).is_some_and(|p| p.exists())
}

/// Unix seconds now.
pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Read-modify-write `state.json` (missing file starts from the default).
pub fn update(run_dir: &Path, f: impl FnOnce(&mut StateFile)) {
    let mut st = read(run_dir).unwrap_or_default();
    f(&mut st);
    st.updated_unix = now_unix();
    write(run_dir, &st);
}
