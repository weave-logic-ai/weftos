//! `spawn.json`: the handshake the user daemon writes for a child kernel
//! (ADR-103 A6, Phase 2 packages G and H).
//!
//! The user-daemon supervisor ([`SpawnFile::write`]) creates it (0600,
//! atomic) in the child's run dir `<run>/<id>/spawn.json` immediately before
//! it launches the child. The child ([`SpawnFile::take`]) reads it once,
//! checks the expiry and deletes it so the nonce and token do not stay on
//! disk. The file carries a secret (`project_token`), so [`Debug`] redacts
//! it.
//!
//! ```json
//! {"nonce":"<64 hex>","parent_socket":"/Users/x/.weftos/run/kernel.sock",
//!  "user_pubkey":"<hex32>","user_key_id":"<hex32>",
//!  "project_id":"01JB8Z3Q0V6X9KQ4M2N7T5R1WD","root":"/work/p",
//!  "expires":1760000060,"project_token":"wft_..."}
//! ```
//!
//! `expires` is unix seconds, [`SPAWN_TTL_SECS`] after issue. The token is a
//! project-scoped `wft_` token with Write capability only (never Admin) and
//! a lifetime of [`PROJECT_TOKEN_TTL_SECS`]; the child's parent link renews
//! it with `project.token.refresh` before it expires (see
//! [`PROJECT_TOKEN_REFRESH_SECS`]).

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

/// How long a freshly written `spawn.json` stays acceptable.
pub const SPAWN_TTL_SECS: u64 = 60;
/// Lifetime of the project token the supervisor issues (and every refresh).
pub const PROJECT_TOKEN_TTL_SECS: u64 = 3_600;
/// A child should refresh its token when this little lifetime is left.
pub const PROJECT_TOKEN_REFRESH_SECS: u64 = 900;
/// RPC the child calls on the user daemon to renew its project token.
pub const TOKEN_REFRESH_METHOD: &str = "project.token.refresh";

/// What the supervisor tells a child about its parent and itself.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpawnFile {
    /// Random single-use value the child echoes in `mesh.register`.
    pub nonce: String,
    /// The user daemon's socket. Defaults to `<run>/../kernel.sock` when
    /// absent (hand-written files).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_socket: Option<PathBuf>,
    /// The user key's public key (hex), the trust anchor for the parent
    /// policy and the forward header.
    pub user_pubkey: String,
    /// `hex(SHA-256(user_pubkey)[..16])`.
    pub user_key_id: String,
    /// The project id (ULID).
    pub project_id: String,
    /// The canonical project root, as stored in the manifest.
    pub root: PathBuf,
    /// Unix seconds after which the file must be refused.
    pub expires: u64,
    /// Project-scoped, Write-only `wft_` token for the parent link. Empty
    /// means the link fails closed.
    #[serde(default)]
    pub project_token: String,
}

impl std::fmt::Debug for SpawnFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpawnFile")
            .field("nonce", &"<redacted>")
            .field("parent_socket", &self.parent_socket)
            .field("user_key_id", &self.user_key_id)
            .field("project_id", &self.project_id)
            .field("root", &self.root)
            .field("expires", &self.expires)
            .field("project_token", &"<redacted>")
            .finish()
    }
}

/// Why a `spawn.json` was refused.
#[derive(Debug, thiserror::Error)]
pub enum SpawnFileError {
    #[error("{0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("{0}: not a valid spawn.json: {1}")]
    Parse(PathBuf, String),
    #[error("spawn.json expired {0} s ago")]
    Expired(u64),
}

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

impl SpawnFile {
    /// True when `now_unix` is past `expires`.
    pub fn expired_at(&self, now_unix: u64) -> bool {
        now_unix > self.expires
    }

    /// Write to `path` (0600, temp file plus rename).
    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        let dir = path
            .parent()
            .ok_or_else(|| std::io::Error::other("spawn.json path has no parent"))?;
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!(
            ".spawn.json.tmp.{}.{}",
            std::process::id(),
            TMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let text = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        let result = (|| {
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut f = opts.open(&tmp)?;
            f.write_all(&text)?;
            f.sync_all()?;
            std::fs::rename(&tmp, path)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    }

    /// Read without deleting (tests, `ParentLink` fallback).
    pub fn read(path: &Path) -> Result<Self, SpawnFileError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| SpawnFileError::Io(path.to_path_buf(), e))?;
        serde_json::from_str(&text)
            .map_err(|e| SpawnFileError::Parse(path.to_path_buf(), e.to_string()))
    }

    /// Child side: read the file, delete it, then check the expiry. The
    /// file is deleted even when it is expired or malformed so a stale
    /// token never lingers.
    pub fn take(path: &Path, now_unix: u64) -> Result<Self, SpawnFileError> {
        let read = Self::read(path);
        let _ = std::fs::remove_file(path);
        let spawn = read?;
        if spawn.expired_at(now_unix) {
            return Err(SpawnFileError::Expired(now_unix - spawn.expires));
        }
        Ok(spawn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(expires: u64) -> SpawnFile {
        SpawnFile {
            nonce: "ab".repeat(32),
            parent_socket: Some("/h/.weftos/run/kernel.sock".into()),
            user_pubkey: "cd".repeat(32),
            user_key_id: "ef".repeat(16),
            project_id: "01JB8Z3Q0V6X9KQ4M2N7T5R1WD".into(),
            root: "/work/p".into(),
            expires,
            project_token: "wft_secret".into(),
        }
    }

    #[test]
    fn round_trip_is_0600_and_take_deletes() {
        let dir = std::env::temp_dir().join(format!("weft-spawn-{}", std::process::id()));
        let path = dir.join("spawn.json");
        sample(100).write(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert_eq!(SpawnFile::read(&path).unwrap(), sample(100));
        assert_eq!(SpawnFile::take(&path, 90).unwrap(), sample(100));
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn expired_file_is_refused_and_still_deleted() {
        let dir = std::env::temp_dir().join(format!("weft-spawn-exp-{}", std::process::id()));
        let path = dir.join("spawn.json");
        sample(100).write(&path).unwrap();
        assert!(matches!(SpawnFile::take(&path, 161), Err(SpawnFileError::Expired(61))));
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn debug_redacts_secrets() {
        let s = format!("{:?}", sample(1));
        assert!(!s.contains("wft_secret") && !s.contains(&"ab".repeat(32)));
    }
}
