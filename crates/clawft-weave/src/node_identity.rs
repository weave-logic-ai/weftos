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

/// Loaded daemon identity: the node-id and whatever key material backs it.
///
/// Cheap to clone. Collapsed daemons hold the node key; a daemon in service
/// mode (ADR-103 P3-U) holds only the machine's *public* key, because the
/// box key belongs to the mesh service and never reaches the user daemon.
#[derive(Clone)]
pub struct DaemonIdentity {
    /// Stable node-id derived from the pubkey
    /// (see [`clawft_kernel::node_id_from_pubkey`]).
    pub node_id: String,
    binding: IdentityBinding,
}

#[derive(Clone)]
enum IdentityBinding {
    /// This daemon's own `node.key`.
    Local(SigningKey),
    /// The machine mesh service's public key.
    Service { machine_pubkey: [u8; 32] },
}

impl DaemonIdentity {
    /// Identity backed by a local node key.
    pub fn local(signing_key: SigningKey) -> Self {
        let node_id = clawft_kernel::node_id_from_pubkey(&signing_key.verifying_key().to_bytes());
        Self { node_id, binding: IdentityBinding::Local(signing_key) }
    }

    /// Identity of the machine mesh service. `node_id` must be the id of
    /// `machine_pubkey`; anything else is refused (never papered over).
    pub fn for_service(node_id: String, machine_pubkey: [u8; 32]) -> Result<Self, IdentityError> {
        let derived = clawft_kernel::node_id_from_pubkey(&machine_pubkey);
        if derived != node_id {
            return Err(IdentityError::ServiceMismatch { claimed: node_id, derived });
        }
        Ok(Self { node_id, binding: IdentityBinding::Service { machine_pubkey } })
    }

    /// True when the box key lives in the mesh service.
    pub fn is_service(&self) -> bool {
        matches!(self.binding, IdentityBinding::Service { .. })
    }

    /// Public key of this node (the machine key in service mode).
    pub fn public_key(&self) -> [u8; 32] {
        match &self.binding {
            IdentityBinding::Local(k) => k.verifying_key().to_bytes(),
            IdentityBinding::Service { machine_pubkey } => *machine_pubkey,
        }
    }

    /// The local node key, or an error in service mode. There is no fallback
    /// to a generated key: a caller that needs to sign as the node must say
    /// what it does instead when the service holds the key.
    pub fn signing_key(&self) -> Result<&SigningKey, IdentityError> {
        match &self.binding {
            IdentityBinding::Local(k) => Ok(k),
            IdentityBinding::Service { .. } => Err(IdentityError::KeyHeldByService),
        }
    }
}

impl std::fmt::Debug for DaemonIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the signing key — only the public node-id.
        f.debug_struct("DaemonIdentity")
            .field("node_id", &self.node_id)
            .field("service", &self.is_service())
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
    /// In service mode the node key is held by the mesh service.
    #[error("the node key is held by the machine mesh service; this daemon cannot sign as the node")]
    KeyHeldByService,
    /// The service's node id does not belong to its public key.
    #[error("service node id {claimed} does not match its public key (derives {derived})")]
    ServiceMismatch {
        /// Id the service claimed.
        claimed: String,
        /// Id derived from the key it presented.
        derived: String,
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
    Ok(DaemonIdentity::local(signing_key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// The on-disk name is a stable contract (`RuntimePaths::node_key`).
    const KEYFILE_NAME: &str = "node.key";

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
            first.public_key(),
            second.public_key(),
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
            .signing_key()
            .unwrap()
            .to_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert!(!s.contains(&hex_seed));
    }

    #[test]
    fn service_identity_holds_no_signing_key() {
        let k = SigningKey::from_bytes(&[5u8; 32]);
        let pk = k.verifying_key().to_bytes();
        let id = DaemonIdentity::for_service(clawft_kernel::node_id_from_pubkey(&pk), pk).unwrap();
        assert!(id.is_service());
        assert_eq!(id.public_key(), pk);
        assert!(matches!(id.signing_key(), Err(IdentityError::KeyHeldByService)));
    }

    #[test]
    fn service_identity_with_a_foreign_node_id_is_refused() {
        let pk = SigningKey::from_bytes(&[5u8; 32]).verifying_key().to_bytes();
        let other = clawft_kernel::node_id_from_pubkey(&[9u8; 32]);
        assert!(matches!(
            DaemonIdentity::for_service(other, pk),
            Err(IdentityError::ServiceMismatch { .. })
        ));
    }
}
