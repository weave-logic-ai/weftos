//! File-backed node key (`<runtime-dir>/node.key`).
//!
//! The 32-byte Ed25519 seed the node id is derived from (ADR-025, ADR-103
//! D11). Shared by the daemon and `weft kernel boot --foreground` so both
//! get the same stable id.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use ed25519_dalek::SigningKey;
use rand::RngCore;
use rand::rngs::OsRng;

/// Filename of the node keypair under the runtime directory.
pub const NODE_KEY_FILE: &str = "node.key";

/// Errors loading or creating the node key.
#[derive(Debug, thiserror::Error)]
pub enum NodeKeyError {
    /// I/O failure reading or writing the key file.
    #[error("keyfile io: {0}")]
    Io(#[from] io::Error),
    /// Key file exists but is not 32 bytes.
    #[error("keyfile {path:?} is malformed: expected 32 bytes, got {got}")]
    Malformed {
        /// Path that was read.
        path: PathBuf,
        /// Bytes actually present.
        got: usize,
    },
}

/// Load `<runtime_dir>/node.key`, creating it (mode 0600, created
/// atomically with that mode) if absent. The runtime dir is created too.
pub fn load_or_generate_node_key(runtime_dir: &Path) -> Result<SigningKey, NodeKeyError> {
    fs::create_dir_all(runtime_dir)?;
    let path = runtime_dir.join(NODE_KEY_FILE);
    if path.exists() {
        return load_existing(&path);
    }
    let mut seed = [0u8; 32];
    OsRng.fill_bytes(&mut seed);
    match create_private(&path, &seed) {
        Ok(()) => Ok(SigningKey::from_bytes(&seed)),
        // Lost a race with another process creating the same key.
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => load_existing(&path),
        Err(e) => Err(e.into()),
    }
}

fn load_existing(path: &Path) -> Result<SigningKey, NodeKeyError> {
    warn_if_loose(path);
    let bytes = fs::read(path)?;
    if bytes.len() != 32 {
        return Err(NodeKeyError::Malformed {
            path: path.to_path_buf(),
            got: bytes.len(),
        });
    }
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&bytes);
    Ok(SigningKey::from_bytes(&seed))
}

/// `create_new` with mode 0600 from the first byte: no write-then-chmod window.
fn create_private(path: &Path, seed: &[u8; 32]) -> io::Result<()> {
    use std::io::Write;
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(seed)?;
    f.sync_all()
}

#[cfg(unix)]
fn warn_if_loose(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = fs::metadata(path) {
        let mode = meta.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            tracing::warn!(
                path = %path.display(),
                mode = format!("{mode:o}"),
                "node key file is readable by group/others; run chmod 600"
            );
        }
    }
}

#[cfg(not(unix))]
fn warn_if_loose(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_then_reloads_same_key_with_private_mode() {
        let dir = tempfile::tempdir().unwrap();
        let a = load_or_generate_node_key(dir.path()).unwrap();
        let b = load_or_generate_node_key(dir.path()).unwrap();
        assert_eq!(a.to_bytes(), b.to_bytes());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dir.path().join(NODE_KEY_FILE))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn malformed_key_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(NODE_KEY_FILE), b"short").unwrap();
        assert!(matches!(
            load_or_generate_node_key(dir.path()),
            Err(NodeKeyError::Malformed { got: 5, .. })
        ));
    }
}
