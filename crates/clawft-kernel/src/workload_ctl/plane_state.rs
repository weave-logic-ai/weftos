//! Durable controller state (ADR-099 section 7): the known targets (with
//! the key each was learned with) and the placed and unsettled instances
//! survive a controller restart, so instances placed on other nodes stay
//! manageable (`status`, `stop`, `logs`, `unload`).
//!
//! The file is written atomically (temp file, fsync, rename) with mode
//! `0600` after every change. On load, targets come back unreachable and
//! are re-described with their stored key before use; trust tiers are
//! re-derived from operator policy on the next sync. Seed placements are
//! not persisted: the Seed adapter's instance state is in memory.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::plane::{PlacementControlPlane, PlacementRecord, PlaneError, TargetInfo};
use super::plane_seed::SEED_ROUTE;

const STATE_VERSION: u32 = 1;
const MAX_STATE_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlaneState {
    version: u32,
    targets: Vec<TargetInfo>,
    placements: Vec<PlacementRecord>,
    unsettled: Vec<PlacementRecord>,
}

fn read_state(path: &Path) -> Result<Option<PlaneState>, PlaneError> {
    let meta = match std::fs::symlink_metadata(path) {
        Err(_) => return Ok(None),
        Ok(m) => m,
    };
    if !meta.file_type().is_file() || meta.len() > MAX_STATE_BYTES {
        return Err(PlaneError::Invalid(format!(
            "{} is not a regular state file of at most {MAX_STATE_BYTES} bytes",
            path.display()
        )));
    }
    let text = std::fs::read_to_string(path).map_err(|e| PlaneError::Invalid(e.to_string()))?;
    let st: PlaneState = serde_json::from_str(&text)
        .map_err(|e| PlaneError::Invalid(format!("{}: {e}", path.display())))?;
    if st.version != STATE_VERSION {
        return Err(PlaneError::Invalid(format!(
            "{}: unsupported state version {}",
            path.display(),
            st.version
        )));
    }
    Ok(Some(st))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let tmp: PathBuf = path.with_extension("json.tmp");
    {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

impl PlacementControlPlane {
    /// Persist to `path` after every change, restoring what it holds now.
    /// A missing file starts empty; a malformed one is refused (the caller
    /// decides; nothing is overwritten).
    pub fn with_state_file(mut self, path: impl Into<PathBuf>) -> Result<Self, PlaneError> {
        let path = path.into();
        if let Some(st) = read_state(&path)? {
            if let Ok(mut t) = self.targets.write() {
                for mut target in st.targets {
                    target.reachable = false;
                    t.insert(target.node_id.clone(), target);
                }
            }
            if let Ok(mut p) = self.placements.lock() {
                for r in st.placements {
                    p.insert(r.instance_id.clone(), r);
                }
            }
            if let Ok(mut u) = self.unsettled.lock() {
                for r in st.unsettled {
                    u.insert(r.decision_id.clone(), r);
                }
            }
        }
        self.state_file = Some(path);
        Ok(self)
    }

    /// Write the current state (no-op without a state file). Failures are
    /// logged: the in-memory state stays authoritative for this process.
    pub(super) fn persist(&self) {
        let Some(path) = &self.state_file else {
            return;
        };
        let _guard = self.state_lock.lock();
        let st = PlaneState {
            version: STATE_VERSION,
            targets: self.targets(),
            placements: self
                .placements()
                .into_iter()
                .filter(|r| r.variant != SEED_ROUTE)
                .collect(),
            unsettled: self.unsettled(),
        };
        let res = serde_json::to_vec_pretty(&st)
            .map_err(std::io::Error::other)
            .and_then(|b| write_atomic(path, &b));
        if let Err(e) = res {
            tracing::warn!(path = %path.display(), error = %e, "placement state not saved");
        }
    }

    /// Re-describe `node_id` with its stored key when its facts are not
    /// cached (after a restart), so governance sees its real tier.
    pub(super) async fn ensure_described(&self, node_id: &str) {
        if self
            .facts
            .get(node_id, super::plane::now_ms() / 1000)
            .is_some()
        {
            return;
        }
        if let Ok((t, pk)) = self.target(node_id) {
            let _ = self.add_target_expecting(&t.addr, t.tier, Some(pk)).await;
        }
    }
}
