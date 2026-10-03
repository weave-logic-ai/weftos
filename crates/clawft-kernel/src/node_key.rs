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
    /// The key file is a symlink (or otherwise unsafe) and its target is not
    /// private and owned by us; refused rather than used.
    #[error("keyfile {path:?} refused: {reason}")]
    Insecure {
        /// Path that was inspected.
        path: PathBuf,
        /// Why it was refused.
        reason: String,
    },
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
/// Creation is race-free where the filesystem allows: the seed is written
/// to a private temp file in the same directory, then hard-linked to
/// `node.key`. The link fails if the target already exists (create_new
/// semantics), and a reader never sees a partially written key. On
/// filesystems without hard links (exFAT, some network mounts) it falls back
/// to `create_new` + write, and readers retry briefly on a zero-length file.
/// Stale temp files (they hold the seed) are removed on entry.
pub fn load_or_generate_node_key(runtime_dir: &Path) -> Result<SigningKey, NodeKeyError> {
    load_or_generate_key_file(runtime_dir, NODE_KEY_FILE)
}

/// [`load_or_generate_node_key`] for another key file in `dir` (a plain file
/// name, no path separators), with the same creation and permission rules.
/// The daemon's placement control key in service mode uses it (ADR-106).
pub fn load_or_generate_key_file(dir: &Path, file: &str) -> Result<SigningKey, NodeKeyError> {
    if file.is_empty() || file.contains(['/', '\\']) || file.starts_with('.') {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("bad key file name {file:?}")).into());
    }
    fs::create_dir_all(dir)?;
    remove_stale_temps(dir, file);
    let path = dir.join(file);
    if fs::symlink_metadata(&path).is_ok() {
        return load_existing(&path);
    }
    let mut seed = [0u8; 32];
    OsRng.fill_bytes(&mut seed);
    match publish_private(dir, &path, file, &seed) {
        Ok(()) => Ok(SigningKey::from_bytes(&seed)),
        // Lost a race with another process creating the same key.
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => load_existing(&path),
        Err(e) => Err(e.into()),
    }
}

/// How old a `.<file>.*.tmp` must be before it is treated as abandoned.
const STALE_TMP_AGE: std::time::Duration = std::time::Duration::from_secs(30);

fn remove_stale_temps(dir: &Path, file: &str) {
    let prefix = format!(".{file}.");
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.filter_map(Result::ok) {
        let name = e.file_name();
        let name = name.to_string_lossy();
        if !(name.starts_with(&prefix) && name.ends_with(".tmp")) {
            continue;
        }
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age >= STALE_TMP_AGE);
        if old {
            let _ = fs::remove_file(e.path());
        }
    }
}

fn read_seed(path: &Path) -> Result<Vec<u8>, NodeKeyError> {
    // A creator using the non-atomic fallback may not have written yet.
    let mut bytes = fs::read(path)?;
    for _ in 0..20 {
        if !bytes.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        bytes = fs::read(path)?;
    }
    Ok(bytes)
}

fn load_existing(path: &Path) -> Result<SigningKey, NodeKeyError> {
    check_and_tighten_perms(path)?;
    let bytes = read_seed(path)?;
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

fn private_open_options() -> fs::OpenOptions {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts
}

/// Write `seed` to a 0600 temp file, sync it, hard-link it to `path`
/// (failing with `AlreadyExists` if present), and remove the temp name.
/// If linking is unsupported, fall back to `create_new` + write.
fn publish_private(dir: &Path, path: &Path, file: &str, seed: &[u8; 32]) -> io::Result<()> {
    use std::io::Write;
    // Unique per call (pid + random), so concurrent creators never share a temp.
    let tmp = dir.join(format!(
        ".{file}.{}.{:016x}.tmp",
        std::process::id(),
        OsRng.next_u64()
    ));
    let linked = (|| {
        let mut f = private_open_options().open(&tmp)?;
        f.write_all(seed)?;
        f.sync_all()?;
        fs::hard_link(&tmp, path)
    })();
    let _ = fs::remove_file(&tmp);
    match linked {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(e),
        Err(e) => {
            tracing::warn!(error = %e, "hard-linking node.key unsupported here; using create_new + write");
            let mut f = private_open_options().open(path)?;
            f.write_all(seed)?;
            f.sync_all()
        }
    }
}

/// Inspect the key file without following symlinks. A regular file that is
/// group/other accessible is warned about and, when we own it, tightened to
/// 0600. A symlink is never chmod'd: it is used only if its target is owned
/// by us with private permissions, otherwise refused (fail closed).
#[cfg(unix)]
fn check_and_tighten_perms(path: &Path) -> Result<(), NodeKeyError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let Ok(link_meta) = fs::symlink_metadata(path) else {
        return Ok(());
    };
    let dir_uid = path
        .parent()
        .and_then(|d| fs::metadata(d).ok())
        .map(|d| d.uid());
    if link_meta.file_type().is_symlink() {
        tracing::warn!(path = %path.display(), "node key is a symlink; not changing its permissions");
        let target = fs::metadata(path).map_err(|e| NodeKeyError::Insecure {
            path: path.to_path_buf(),
            reason: format!("symlink target unreadable: {e}"),
        })?;
        let mode = target.permissions().mode() & 0o777;
        if Some(target.uid()) != dir_uid || mode & 0o077 != 0 {
            return Err(NodeKeyError::Insecure {
                path: path.to_path_buf(),
                reason: format!(
                    "symlink target must be owned by the runtime dir owner with mode 600 (found mode {mode:o})"
                ),
            });
        }
        return Ok(());
    }
    let mode = link_meta.permissions().mode() & 0o777;
    if mode & 0o077 == 0 {
        return Ok(());
    }
    tracing::warn!(
        path = %path.display(),
        mode = format!("{mode:o}"),
        "node key file is readable by group/others"
    );
    if Some(link_meta.uid()) == dir_uid {
        match fs::set_permissions(path, fs::Permissions::from_mode(0o600)) {
            Ok(()) => tracing::warn!(path = %path.display(), "node key mode tightened to 600"),
            Err(e) => tracing::warn!(path = %path.display(), error = %e, "could not tighten node key mode"),
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_and_tighten_perms(_path: &Path) -> Result<(), NodeKeyError> {
    Ok(())
}

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

    #[test]
    fn stale_temp_files_holding_the_seed_are_removed_fresh_ones_kept() {
        let dir = tempfile::tempdir().unwrap();
        let stale = dir.path().join(".node.key.1.00.tmp");
        let fresh = dir.path().join(".node.key.2.11.tmp");
        fs::write(&stale, [7u8; 32]).unwrap();
        fs::write(&fresh, [8u8; 32]).unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        fs::File::options().write(true).open(&stale).unwrap().set_modified(old).unwrap();
        load_or_generate_node_key(dir.path()).unwrap();
        assert!(!stale.exists(), "stale temp must be removed");
        assert!(fresh.exists(), "a possibly live temp must be kept");
    }

    #[test]
    fn empty_key_file_is_retried_then_reported_malformed() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(NODE_KEY_FILE), b"").unwrap();
        assert!(matches!(
            load_or_generate_node_key(dir.path()),
            Err(NodeKeyError::Malformed { got: 0, .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_key_is_not_chmodded_and_loose_target_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let target = store.path().join("real.key");
        fs::write(&target, [5u8; 32]).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
        std::os::unix::fs::symlink(&target, dir.path().join(NODE_KEY_FILE)).unwrap();

        // Loose target: refused, and its mode is left alone.
        assert!(matches!(
            load_or_generate_node_key(dir.path()),
            Err(NodeKeyError::Insecure { .. })
        ));
        assert_eq!(fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o644);

        // Private target owned by us: used, still not chmodded.
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        let k = load_or_generate_node_key(dir.path()).unwrap();
        assert_eq!(k.to_bytes(), [5u8; 32]);
        assert_eq!(fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o600);
    }
}
