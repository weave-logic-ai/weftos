//! `<run>/<id>/state.json` and the `revoked` marker.

use std::path::Path;

use clawft_kernel::overlay_trust::REVOKED_FILE;
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

/// Drop the `revoked` marker the child checks (a revoked or rekeyed
/// project's running kernel stops taking policy). Creates the run dir.
pub fn mark_revoked(run_dir: &Path, reason: &str) -> std::io::Result<()> {
    write_atomic_0600(&run_dir.join(REVOKED_FILE), format!("{reason}\n").as_bytes())
}

/// True when the marker exists.
pub fn is_marked_revoked(run_dir: &Path) -> bool {
    run_dir.join(REVOKED_FILE).exists()
}

/// Remove the marker (a valid certificate is in force again after a rekey).
pub fn clear_revoked(run_dir: &Path) {
    let _ = std::fs::remove_file(run_dir.join(REVOKED_FILE));
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
