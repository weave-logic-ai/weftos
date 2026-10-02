//! `spawn.json`: the one-shot handshake file the user daemon writes into a
//! child's run dir (ADR-103 A6, Phase 2 packages G and H).
//!
//! The supervisor ([`SpawnFile::write`]) writes it 0600 right before it
//! launches the child; the child ([`SpawnFile::read_and_consume`]) reads it
//! once, refuses anything but a private, unexpired regular file, and deletes
//! it. The nonce in it is also recorded by the user daemon, which accepts
//! each nonce for exactly one `mesh.register`, so a copied file is worth
//! nothing once the real child has registered or 60 s have passed.
//!
//! Honest limit: the file proves "the user daemon wrote this for this
//! project", to a reader that can read a 0600 file in the user's run dir.
//! That is the same uid as everything else Phase 2 trusts; the Phase 3 peer
//! credential binding and the Phase 4 sandbox narrow it.

use std::fmt;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// How long a spawn file stays valid after it is written.
pub const SPAWN_TTL_SECS: u64 = 60;

/// Largest spawn file the reader accepts.
const MAX_SPAWN_BYTES: u64 = 64 * 1024;

/// What the supervisor tells a child it just spawned.
///
/// Unknown keys are ignored on read, so readers that only want a few fields
/// (package F's parent link) keep working when this grows.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpawnFile {
    /// Single-use spawn nonce, 32 lowercase hex.
    pub nonce: String,
    /// The user daemon's socket.
    pub parent_socket: PathBuf,
    /// The user public key, 64 lowercase hex (the trust root the child
    /// verifies its certificate and the parent policy under).
    pub user_pubkey: String,
    /// `key_id` of [`Self::user_pubkey`].
    pub user_key_id: String,
    /// Project ULID.
    pub project_id: String,
    /// The canonical project root, exactly the path the manifest stores (the
    /// string `root_sha256` hashes).
    pub root: PathBuf,
    /// Unix seconds after which the file is void.
    pub expires_unix: u64,
    /// Project-scoped capability token for `shared.*` calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_token: Option<String>,
}

impl fmt::Debug for SpawnFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpawnFile")
            .field("nonce", &"<redacted>")
            .field("parent_socket", &self.parent_socket)
            .field("user_key_id", &self.user_key_id)
            .field("project_id", &self.project_id)
            .field("root", &self.root)
            .field("expires_unix", &self.expires_unix)
            .field("project_token", &self.project_token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// Why a spawn file was refused.
#[derive(Debug, thiserror::Error)]
pub enum SpawnError {
    /// Missing, unreadable, or not valid JSON for this struct.
    #[error("{path}: {reason}")]
    Unreadable { path: PathBuf, reason: String },
    /// Not a plain file owned by this user, or readable by others.
    #[error("{path}: {reason}")]
    Insecure { path: PathBuf, reason: String },
    /// Past `expires_unix`.
    #[error("spawn.json expired {age_secs} s ago; project kernels are started by the user daemon")]
    Expired { age_secs: u64 },
    /// A field is not what a spawn file must carry.
    #[error("spawn.json: {0}")]
    Invalid(String),
}

impl SpawnFile {
    /// A spawn file valid for [`SPAWN_TTL_SECS`] from `now_unix`.
    pub fn new(
        nonce: String,
        parent_socket: PathBuf,
        user_pubkey: String,
        user_key_id: String,
        project_id: String,
        root: PathBuf,
        project_token: Option<String>,
        now_unix: u64,
    ) -> Self {
        Self {
            nonce,
            parent_socket,
            user_pubkey,
            user_key_id,
            project_id,
            root,
            expires_unix: now_unix.saturating_add(SPAWN_TTL_SECS),
            project_token,
        }
    }

    /// Structural checks: hex shapes, a ULID, absolute paths.
    pub fn validate(&self) -> Result<(), SpawnError> {
        let bad = |m: &str| Err(SpawnError::Invalid(m.to_owned()));
        let hex = |s: &str, n: usize| s.len() == n && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        if !hex(&self.nonce, 32) {
            return bad("`nonce` must be 32 lowercase hex characters");
        }
        if !hex(&self.user_pubkey, 64) {
            return bad("`user_pubkey` must be 64 lowercase hex characters");
        }
        if !hex(&self.user_key_id, 32) {
            return bad("`user_key_id` must be 32 lowercase hex characters");
        }
        if super::validate_id(&self.project_id).is_err() {
            return bad("`project_id` is not a canonical ULID");
        }
        if !self.root.is_absolute() || !self.parent_socket.is_absolute() {
            return bad("`root` and `parent_socket` must be absolute paths");
        }
        Ok(())
    }

    /// Write `path` atomically with mode 0600 (temp file in the same
    /// directory, `O_EXCL`, fsync, rename). Replaces an older spawn file.
    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
        let tmp = dir.join(format!(".spawn.{}.tmp", std::process::id()));
        let _ = std::fs::remove_file(&tmp);
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let body = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        let res = (|| {
            let mut f = opts.open(&tmp)?;
            f.write_all(&body)?;
            f.sync_all()?;
            std::fs::rename(&tmp, path)
        })();
        if res.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        res
    }

    /// Child side: read `path`, refuse an unsafe or expired file, then delete
    /// it. The file is deleted as soon as it has been read, whatever the
    /// outcome of the later checks, so a refused file is not left to be
    /// retried. A symlink, a non-regular file, a file owned by someone else
    /// and any group or world access are refused (before reading it, and
    /// the file is still removed when it is ours).
    pub fn read_and_consume(path: &Path, now_unix: u64) -> Result<Self, SpawnError> {
        let unreadable = |e: &dyn fmt::Display| SpawnError::Unreadable {
            path: path.to_path_buf(),
            reason: e.to_string(),
        };
        let insecure = |r: &str| SpawnError::Insecure { path: path.to_path_buf(), reason: r.to_owned() };
        let mut opts = std::fs::OpenOptions::new();
        opts.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.custom_flags(libc::O_NOFOLLOW);
        }
        let file = opts.open(path).map_err(|e| unreadable(&e))?;
        let meta = file.metadata().map_err(|e| unreadable(&e))?;
        if !meta.is_file() {
            return Err(insecure("not a regular file"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            // SAFETY: geteuid has no preconditions.
            let me = unsafe { libc::geteuid() };
            if meta.uid() != me {
                return Err(insecure("not owned by this user"));
            }
            let mode = meta.permissions().mode();
            if mode & 0o077 != 0 {
                let _ = std::fs::remove_file(path);
                return Err(insecure(&format!(
                    "mode {:04o} lets other users read the spawn nonce and token; it must be 0600",
                    mode & 0o7777
                )));
            }
        }
        if meta.len() > MAX_SPAWN_BYTES {
            return Err(insecure("larger than 64 KiB"));
        }
        let mut text = String::new();
        std::io::Read::read_to_string(&mut std::io::Read::take(&file, MAX_SPAWN_BYTES), &mut text)
            .map_err(|e| unreadable(&e))?;
        // Single use: gone before anything else can fail.
        let _ = std::fs::remove_file(path);
        let spawn: Self = serde_json::from_str(&text).map_err(|e| unreadable(&e))?;
        spawn.validate()?;
        if now_unix >= spawn.expires_unix {
            return Err(SpawnError::Expired { age_secs: now_unix - spawn.expires_unix });
        }
        Ok(spawn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "01JB8Z3Q0V6X9KQ4M2N7T5R1WD";

    fn sample(now: u64) -> SpawnFile {
        SpawnFile::new(
            "ab".repeat(16),
            "/h/.weftos/run/kernel.sock".into(),
            "cd".repeat(32),
            "ef".repeat(16),
            ID.into(),
            "/work/p".into(),
            Some("wft_secret".into()),
            now,
        )
    }

    #[test]
    fn write_then_consume_round_trips_and_deletes() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("spawn.json");
        let s = sample(1000);
        s.write(&p).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert_eq!(SpawnFile::read_and_consume(&p, 1059).unwrap(), s);
        assert!(!p.exists(), "single use");
        assert!(matches!(SpawnFile::read_and_consume(&p, 1059), Err(SpawnError::Unreadable { .. })));
    }

    #[test]
    fn expired_file_is_refused_and_removed() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("spawn.json");
        sample(1000).write(&p).unwrap();
        let e = SpawnFile::read_and_consume(&p, 1000 + SPAWN_TTL_SECS + 5).unwrap_err();
        assert!(matches!(e, SpawnError::Expired { age_secs: 5 }), "{e}");
        assert!(!p.exists());
    }

    #[cfg(unix)]
    #[test]
    fn group_or_world_readable_file_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("spawn.json");
        sample(1000).write(&p).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o640)).unwrap();
        let e = SpawnFile::read_and_consume(&p, 1001).unwrap_err();
        assert!(matches!(e, SpawnError::Insecure { .. }), "{e}");
        assert!(e.to_string().contains("0640"));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_is_refused() {
        let t = tempfile::tempdir().unwrap();
        let real = t.path().join("real.json");
        sample(1000).write(&real).unwrap();
        let link = t.path().join("spawn.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(SpawnFile::read_and_consume(&link, 1001).is_err());
        assert!(real.exists(), "the target is not consumed through a link");
    }

    #[test]
    fn malformed_fields_are_refused() {
        let mut s = sample(1000);
        s.nonce = "XYZ".into();
        assert!(matches!(s.validate(), Err(SpawnError::Invalid(_))));
        let mut s = sample(1000);
        s.project_id = "../etc".into();
        assert!(s.validate().is_err());
        let mut s = sample(1000);
        s.root = "relative".into();
        assert!(s.validate().is_err());
    }

    #[test]
    fn unknown_keys_are_ignored_and_debug_redacts() {
        let mut v = serde_json::to_value(sample(1000)).unwrap();
        v["future_field"] = serde_json::json!(1);
        let s: SpawnFile = serde_json::from_value(v).unwrap();
        let d = format!("{s:?}");
        assert!(!d.contains("wft_secret") && !d.contains(&"ab".repeat(16)));
    }
}
