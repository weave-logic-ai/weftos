//! Principals revoked while the journal could not record it.
//!
//! Cutting someone off must not wait for a broken journal, so `bind.revoke`
//! falls back to this set: register and renew refuse these principals, the
//! signed facts list their certificate serials as revoked, and the set is
//! persisted (`force-revoked.json`, 0600, atomic write) so the revocation
//! survives a restart. Approve and rebind (which are journalled) clear it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use clawft_mesh_local::Principal;

/// File name inside the state directory.
pub const FORCE_REVOKED_FILE: &str = "force-revoked.json";

/// Ordering for the set (principals have no `Ord`): their JSON form.
fn sort_key(p: &Principal) -> String {
    serde_json::to_string(p).unwrap_or_default()
}

pub struct ForceRevoked {
    path: PathBuf,
    set: Mutex<BTreeSet<String>>,
}

impl ForceRevoked {
    /// Load `<state_dir>/force-revoked.json`. A missing file is empty; an
    /// unreadable or corrupt one is an error (fail closed: the service cannot
    /// know whom it was told to keep out).
    pub fn load(state_dir: &Path) -> Result<Self, String> {
        let path = state_dir.join(FORCE_REVOKED_FILE);
        let set = match std::fs::read(&path) {
            Ok(bytes) => {
                let list: Vec<Principal> = serde_json::from_slice(&bytes)
                    .map_err(|e| format!("{} is corrupt: {e}", path.display()))?;
                list.iter().map(sort_key).collect()
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeSet::new(),
            Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
        };
        Ok(Self { path, set: Mutex::new(set) })
    }

    pub fn contains(&self, p: &Principal) -> bool {
        self.set.lock().expect("force lock").contains(&sort_key(p))
    }

    pub fn list(&self) -> Vec<Principal> {
        self.set
            .lock()
            .expect("force lock")
            .iter()
            .filter_map(|s| serde_json::from_str(s).ok())
            .collect()
    }

    fn persist(&self, set: &BTreeSet<String>) -> std::io::Result<()> {
        let list: Vec<Principal> = set.iter().filter_map(|s| serde_json::from_str(s).ok()).collect();
        let bytes = serde_json::to_vec_pretty(&list).map_err(std::io::Error::other)?;
        crate::facts::write_private(&self.path, &bytes)
    }

    /// Add `p`. In memory first (enforcement never waits for the disk); the
    /// error says the file could not be written.
    pub fn insert(&self, p: &Principal) -> std::io::Result<()> {
        let mut g = self.set.lock().expect("force lock");
        if g.insert(sort_key(p)) {
            return self.persist(&g);
        }
        Ok(())
    }

    /// Remove `p` (approve / rebind).
    pub fn remove(&self, p: &Principal) {
        let mut g = self.set.lock().expect("force lock");
        if g.remove(&sort_key(p))
            && let Err(e) = self.persist(&g)
        {
            tracing::error!(error = %e, "could not update force-revoked.json");
        }
    }
}
