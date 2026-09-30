//! Daemon node identity bootstrap.
//!
//! Every WeftOS daemon is a **node** in the mesh. It needs a stable
//! ed25519 keypair so it can sign its substrate publishes — the
//! kernel's [`crate::node_registry::NodeRegistry`] gates writes by
//! verifying those signatures and enforcing the
//! `substrate/<node-id>/...` prefix rule.
//!
//! This module owns the file-backed keypair lifecycle:
//!
//! - On first run, generate an ed25519 keypair and persist it to
//!   `<runtime-dir>/node.key` with `0600` perms.
//! - On subsequent runs, load it back from the same file.
//! - Derive the daemon's node-id from the pubkey via
//!   [`clawft_kernel::node_id_from_pubkey`].
//!
//! The keyfile is an opaque 32-byte raw seed. Plain-on-disk for MVP
//! is the right tradeoff against ergonomics; the journal flags
//! encrypted-NVS / eFuse-style key custody as the upgrade path
//! (see `.planning/sensors/JOURNALED-NODE-ESP32.md` §2.4).

use std::io;
use std::path::{Path, PathBuf};

use ed25519_dalek::SigningKey;

#[cfg(test)]
use clawft_kernel::NODE_KEY_FILE as KEYFILE_NAME;

/// Loaded daemon identity: signing key + derived node-id.
///
/// Cheap to clone — the signing key is 32 bytes.
#[derive(Clone)]
pub struct DaemonIdentity {
    /// Ed25519 keypair the daemon signs with.
    pub signing_key: SigningKey,
    /// Stable node-id derived from the pubkey
    /// (see [`clawft_kernel::node_id_from_pubkey`]).
    pub node_id: String,
}

impl std::fmt::Debug for DaemonIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the signing key — only the public node-id.
        f.debug_struct("DaemonIdentity")
            .field("node_id", &self.node_id)
            .finish_non_exhaustive()
    }
}

/// Errors from loading or generating the daemon identity.
#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    /// I/O failure reading or writing the keyfile.
    #[error("keyfile io: {0}")]
    Io(#[from] io::Error),
    /// Keyfile exists but is not the expected 32-byte length.
    #[error("keyfile {path:?} is malformed: expected 32 bytes, got {got}")]
    Malformed {
        /// Path that was read.
        path: PathBuf,
        /// Bytes actually present.
        got: usize,
    },
}

/// Load the daemon identity, generating + persisting a fresh
/// keypair if `<runtime_dir>/node.key` does not exist yet.
///
/// `runtime_dir` is the daemon's runtime directory — typically
/// `.weftos/runtime/`. Created if absent. The file is created with
/// mode 0600 atomically (shared implementation:
/// [`clawft_kernel::load_or_generate_node_key`]).
pub fn load_or_generate(runtime_dir: &Path) -> Result<DaemonIdentity, IdentityError> {
    let signing_key =
        clawft_kernel::load_or_generate_node_key(runtime_dir).map_err(|e| match e {
            clawft_kernel::NodeKeyError::Io(e) => IdentityError::Io(e),
            clawft_kernel::NodeKeyError::Malformed { path, got } => {
                IdentityError::Malformed { path, got }
            }
            clawft_kernel::NodeKeyError::Insecure { path, reason } => {
                IdentityError::Io(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("{path:?}: {reason}"),
                ))
            }
        })?;
    let pubkey_bytes: [u8; 32] = signing_key.verifying_key().to_bytes();
    let node_id = clawft_kernel::node_id_from_pubkey(&pubkey_bytes);
    Ok(DaemonIdentity {
        signing_key,
        node_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn first_run_generates_and_persists_keyfile() {
        let dir = TempDir::new().unwrap();
        let id = load_or_generate(dir.path()).expect("first-run identity");
        let keyfile = dir.path().join(KEYFILE_NAME);
        assert!(keyfile.exists());
        assert_eq!(fs::read(&keyfile).unwrap().len(), 32);
        assert!(clawft_kernel::is_node_id(&id.node_id));
    }

    #[test]
    fn second_run_reloads_same_identity() {
        let dir = TempDir::new().unwrap();
        let first = load_or_generate(dir.path()).unwrap();
        let second = load_or_generate(dir.path()).unwrap();
        assert_eq!(first.node_id, second.node_id);
        // Pubkey round-trips too.
        assert_eq!(
            first.signing_key.verifying_key().to_bytes(),
            second.signing_key.verifying_key().to_bytes(),
        );
    }

    #[test]
    fn keyfile_is_owner_readable_only_on_unix() {
        // Smoke test that perms are applied. On non-unix platforms
        // this test is a no-op assertion via the cfg gate.
        let dir = TempDir::new().unwrap();
        load_or_generate(dir.path()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dir.path().join(KEYFILE_NAME))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn malformed_keyfile_is_reported() {
        let dir = TempDir::new().unwrap();
        // Plant a too-short keyfile.
        fs::write(dir.path().join(KEYFILE_NAME), b"too-short").unwrap();
        let err = load_or_generate(dir.path()).unwrap_err();
        match err {
            IdentityError::Malformed { got, .. } => assert_eq!(got, 9),
            other => panic!("expected Malformed, got {other:?}"),
        }
    }

    #[test]
    fn nonexistent_runtime_dir_is_created() {
        let parent = TempDir::new().unwrap();
        let nested = parent.path().join("nonexistent").join("runtime");
        assert!(!nested.exists());
        load_or_generate(&nested).unwrap();
        assert!(nested.exists());
        assert!(nested.join(KEYFILE_NAME).exists());
    }

    #[test]
    fn debug_does_not_leak_signing_key() {
        let dir = TempDir::new().unwrap();
        let id = load_or_generate(dir.path()).unwrap();
        let s = format!("{id:?}");
        assert!(s.contains(&id.node_id));
        // The signing-key bytes must not appear in the Debug output.
        // Hex-encode them and check.
        let hex_seed: String = id
            .signing_key
            .to_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert!(!s.contains(&hex_seed));
    }
}
