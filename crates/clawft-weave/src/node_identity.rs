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

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;
use rand::RngCore;

/// Filename of the node keypair under the runtime directory.
const KEYFILE_NAME: &str = "node.key";

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
/// `.weftos/runtime/`. Created if absent (the keyfile parent must
/// exist for the write to succeed).
pub fn load_or_generate(runtime_dir: &Path) -> Result<DaemonIdentity, IdentityError> {
    fs::create_dir_all(runtime_dir)?;
    let path = runtime_dir.join(KEYFILE_NAME);
    let signing_key = if path.exists() {
        load_existing(&path)?
    } else {
        let key = generate()?;
        write_keyfile(&path, &key)?;
        key
    };
    let pubkey_bytes: [u8; 32] = signing_key.verifying_key().to_bytes();
    let node_id = clawft_kernel::node_id_from_pubkey(&pubkey_bytes);
    Ok(DaemonIdentity { signing_key, node_id })
}

fn generate() -> Result<SigningKey, IdentityError> {
    let mut seed = [0u8; 32];
    // OsRng.fill_bytes is infallible — taps the OS CSPRNG.
    OsRng.fill_bytes(&mut seed);
    Ok(SigningKey::from_bytes(&seed))
}

fn load_existing(path: &Path) -> Result<SigningKey, IdentityError> {
    let bytes = fs::read(path)?;
    if bytes.len() != 32 {
        return Err(IdentityError::Malformed {
            path: path.to_path_buf(),
            got: bytes.len(),
        });
    }
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&bytes);
    Ok(SigningKey::from_bytes(&seed))
}

fn write_keyfile(path: &Path, key: &SigningKey) -> Result<(), IdentityError> {
    let seed = key.to_bytes();
    fs::write(path, seed)?;
    set_keyfile_perms(path)?;
    Ok(())
}

#[cfg(unix)]
fn set_keyfile_perms(path: &Path) -> Result<(), IdentityError> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o600);
    fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_keyfile_perms(_path: &Path) -> Result<(), IdentityError> {
    // Non-unix platforms (the daemon doesn't support these today,
    // but stubbed for portability). Future Windows-style ACL is a
    // separate workstream.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn first_run_generates_and_persists_keyfile() {
        let dir = TempDir::new().unwrap();
        let id = load_or_generate(dir.path()).expect("first-run identity");
        let keyfile = dir.path().join(KEYFILE_NAME);
        assert!(keyfile.exists());
        assert_eq!(fs::read(&keyfile).unwrap().len(), 32);
        assert!(id.node_id.starts_with("n-"));
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
