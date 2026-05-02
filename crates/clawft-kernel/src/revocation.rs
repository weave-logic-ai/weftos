//! Persistent host revocation (ban list).
//!
//! Stores a JSON file at `.weftos/runtime/revoked_hosts.json` containing
//! hosts that have been banned from joining the mesh. The ban list is
//! loaded at kernel boot and checked during mesh peer handshake.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

/// A single revocation entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevokedHost {
    /// The host/node ID that was revoked.
    pub host_id: String,
    /// Unix timestamp (seconds) when the revocation was recorded.
    pub revoked_at: u64,
    /// Human-readable reason for revocation.
    pub reason: String,
}

/// Persistent host revocation manager.
///
/// Thread-safe via internal `Mutex`. Reads and writes a JSON file
/// on disk so that bans survive kernel restarts.
pub struct RevocationList {
    inner: Mutex<RevocationInner>,
}

struct RevocationInner {
    hosts: Vec<RevokedHost>,
    path: PathBuf,
}

impl RevocationList {
    /// Create a new empty revocation list that persists to `path`.
    pub fn new(path: PathBuf) -> Self {
        Self {
            inner: Mutex::new(RevocationInner {
                hosts: Vec::new(),
                path,
            }),
        }
    }

    /// Load the ban list from disk. If the file does not exist or is
    /// malformed, starts with an empty list.
    pub fn load(path: PathBuf) -> Self {
        let hosts = if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(data) => match serde_json::from_str::<Vec<RevokedHost>>(&data) {
                    Ok(list) => {
                        info!(count = list.len(), path = %path.display(), "loaded host revocation list");
                        list
                    }
                    Err(e) => {
                        warn!(error = %e, path = %path.display(), "malformed revocation file, starting empty");
                        Vec::new()
                    }
                },
                Err(e) => {
                    warn!(error = %e, path = %path.display(), "failed to read revocation file, starting empty");
                    Vec::new()
                }
            }
        } else {
            debug!(path = %path.display(), "no revocation file found, starting empty");
            Vec::new()
        };

        Self {
            inner: Mutex::new(RevocationInner { hosts, path }),
        }
    }

    /// Revoke a host. Adds to the ban list and persists to disk.
    ///
    /// Returns `true` if newly added, `false` if already revoked.
    pub fn revoke_host(&self, host_id: &str, reason: &str) -> bool {
        let mut inner = self.inner.lock().unwrap();
        if inner.hosts.iter().any(|h| h.host_id == host_id) {
            return false;
        }

        let entry = RevokedHost {
            host_id: host_id.to_owned(),
            revoked_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            reason: reason.to_owned(),
        };
        inner.hosts.push(entry);
        Self::save_inner(&inner);
        info!(host_id, reason, "host revoked");
        true
    }

    /// Check whether a host is currently revoked.
    pub fn is_revoked(&self, host_id: &str) -> bool {
        let inner = self.inner.lock().unwrap();
        inner.hosts.iter().any(|h| h.host_id == host_id)
    }

    /// List all revoked hosts.
    pub fn list_revoked(&self) -> Vec<RevokedHost> {
        let inner = self.inner.lock().unwrap();
        inner.hosts.clone()
    }

    /// Remove a host from the ban list. Returns `true` if it was present.
    pub fn unrevoke_host(&self, host_id: &str) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let before = inner.hosts.len();
        inner.hosts.retain(|h| h.host_id != host_id);
        let removed = inner.hosts.len() < before;
        if removed {
            Self::save_inner(&inner);
            info!(host_id, "host unrevoked");
        }
        removed
    }

    /// Return the number of revoked hosts.
    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().hosts.len()
    }

    /// Check if the revocation list is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.lock().unwrap().hosts.is_empty()
    }

    /// Get the file path used for persistence.
    pub fn path(&self) -> PathBuf {
        self.inner.lock().unwrap().path.clone()
    }

    /// Persist the current state to disk.
    fn save_inner(inner: &RevocationInner) {
        if let Some(parent) = inner.path.parent()
            && let Err(e) = std::fs::create_dir_all(parent) {
                warn!(error = %e, "failed to create revocation dir");
                return;
            }
        match serde_json::to_string_pretty(&inner.hosts) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&inner.path, json) {
                    warn!(error = %e, path = %inner.path.display(), "failed to write revocation file");
                }
            }
            Err(e) => {
                warn!(error = %e, "failed to serialize revocation list");
            }
        }
    }

    /// Default path for the revocation file relative to a base directory.
    pub fn default_path(base: &Path) -> PathBuf {
        base.join(".weftos").join("runtime").join("revoked_hosts.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn revoke_and_check() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("revoked.json");
        let list = RevocationList::new(path);

        assert!(!list.is_revoked("host-1"));
        assert!(list.revoke_host("host-1", "misbehaving"));
        assert!(list.is_revoked("host-1"));
        assert_eq!(list.len(), 1);

        // Duplicate revoke returns false
        assert!(!list.revoke_host("host-1", "second attempt"));
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn unrevoke() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("revoked.json");
        let list = RevocationList::new(path);

        list.revoke_host("host-1", "test");
        assert!(list.unrevoke_host("host-1"));
        assert!(!list.is_revoked("host-1"));
        assert!(list.is_empty());

        // Unrevoke nonexistent returns false
        assert!(!list.unrevoke_host("host-1"));
    }

    #[test]
    fn persistence_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("revoked.json");

        // Write with first instance
        {
            let list = RevocationList::new(path.clone());
            list.revoke_host("host-1", "reason-a");
            list.revoke_host("host-2", "reason-b");
        }

        // Load with second instance
        let list = RevocationList::load(path);
        assert_eq!(list.len(), 2);
        assert!(list.is_revoked("host-1"));
        assert!(list.is_revoked("host-2"));
        assert!(!list.is_revoked("host-3"));
    }

    #[test]
    fn load_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.json");
        let list = RevocationList::load(path);
        assert!(list.is_empty());
    }

    #[test]
    fn load_malformed_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        {
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(b"not valid json{{{").unwrap();
        }
        let list = RevocationList::load(path);
        assert!(list.is_empty());
    }

    #[test]
    fn list_revoked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("revoked.json");
        let list = RevocationList::new(path);

        list.revoke_host("a", "reason-a");
        list.revoke_host("b", "reason-b");

        let entries = list.list_revoked();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].host_id, "a");
        assert_eq!(entries[1].host_id, "b");
    }

    #[test]
    fn default_path() {
        let base = Path::new("/tmp/test");
        let p = RevocationList::default_path(base);
        assert!(p.to_string_lossy().contains("revoked_hosts.json"));
        assert!(p.to_string_lossy().contains(".weftos/runtime"));
    }
}
