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

/// Load `<runtime_dir>/node.key`, creating it (mode 0600) if absent. The
/// runtime dir is created too.
///
/// Creation is race-free: the seed is written to a private temp file in the
/// same directory, then hard-linked to `node.key`. The link fails if the
/// target already exists (create_new semantics), and a reader never sees a
/// partially written or empty key file, even across a crash.
pub fn load_or_generate_node_key(runtime_dir: &Path) -> Result<SigningKey, NodeKeyError> {
    fs::create_dir_all(runtime_dir)?;
    let path = runtime_dir.join(NODE_KEY_FILE);
    if path.exists() {
        return load_existing(&path);
    }
    let mut seed = [0u8; 32];
    OsRng.fill_bytes(&mut seed);
    match publish_private(runtime_dir, &path, &seed) {
        Ok(()) => Ok(SigningKey::from_bytes(&seed)),
        // Lost a race with another process creating the same key.
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => load_existing(&path),
        Err(e) => Err(e.into()),
    }
}

fn load_existing(path: &Path) -> Result<SigningKey, NodeKeyError> {
    check_and_tighten_perms(path);
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

/// Write `seed` to a 0600 temp file, sync it, hard-link it to `path`
/// (failing with `AlreadyExists` if present), and remove the temp name.
fn publish_private(dir: &Path, path: &Path, seed: &[u8; 32]) -> io::Result<()> {
    use std::io::Write;
    // Unique per call (pid + random), so concurrent creators never share a temp.
    let tmp = dir.join(format!(
        ".node.key.{}.{:016x}.tmp",
        std::process::id(),
        OsRng.next_u64()
    ));
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let result = (|| {
        let mut f = opts.open(&tmp)?;
        f.write_all(seed)?;
        f.sync_all()?;
        fs::hard_link(&tmp, path)
    })();
    let _ = fs::remove_file(&tmp);
    result
}

/// WARN when the key is group/other accessible, and tighten it to 0600 when
/// we own the file.
#[cfg(unix)]
fn check_and_tighten_perms(path: &Path) {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let Ok(meta) = fs::metadata(path) else { return };
    let mode = meta.permissions().mode() & 0o777;
    if mode & 0o077 == 0 {
        return;
    }
    // SAFETY-free ownership check: compare against the owner of the runtime
    // directory we just created or opened for this process.
    let ours = path
        .parent()
        .and_then(|d| fs::metadata(d).ok())
        .is_some_and(|d| d.uid() == meta.uid());
    tracing::warn!(
        path = %path.display(),
        mode = format!("{mode:o}"),
        "node key file is readable by group/others"
    );
    if ours {
        match fs::set_permissions(path, fs::Permissions::from_mode(0o600)) {
            Ok(()) => tracing::warn!(path = %path.display(), "node key mode tightened to 600"),
            Err(e) => tracing::warn!(path = %path.display(), error = %e, "could not tighten node key mode"),
        }
    }
}

#[cfg(not(unix))]
fn check_and_tighten_perms(_path: &Path) {}

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
    #[cfg(unix)]
    fn loose_mode_is_tightened_on_load() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let a = load_or_generate_node_key(dir.path()).unwrap();
        let p = dir.path().join(NODE_KEY_FILE);
        fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap();
        let b = load_or_generate_node_key(dir.path()).unwrap();
        assert_eq!(a.to_bytes(), b.to_bytes());
        assert_eq!(fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn concurrent_creators_agree_and_never_see_an_empty_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let p = path.clone();
                std::thread::spawn(move || load_or_generate_node_key(&p).unwrap().to_bytes())
            })
            .collect();
        let keys: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert!(keys.windows(2).all(|w| w[0] == w[1]), "all creators must agree");
        // No temp files left behind.
        let leftovers = fs::read_dir(&path)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(leftovers, 0);
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
