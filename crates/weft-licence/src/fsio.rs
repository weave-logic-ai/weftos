//! Private-file helpers: 0700 directories, 0600 files, atomic durable writes.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;

/// Create `dir` (and parents) with mode 0700. An existing directory is left alone.
pub fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    if dir.exists() {
        return Ok(());
    }
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b.create(dir)
}

/// Open a new file for writing with mode 0600, failing if it exists.
pub fn create_new_private(path: &Path) -> io::Result<File> {
    let mut o = OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path)
}

/// Write `bytes` to `path` durably: a 0600 temp file in the same directory,
/// `fsync`, rename over the target, then `fsync` of the directory so the
/// rename itself survives power loss.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    ensure_private_dir(dir)?;
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let result = (|| {
        let mut f = create_new_private(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)?;
        File::open(dir)?.sync_all()
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Read a file of at most `max` bytes. `Ok(None)` when it does not exist.
pub fn read_capped(path: &Path, max: u64) -> io::Result<Option<Vec<u8>>> {
    let f = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    f.take(max.saturating_add(1)).read_to_end(&mut out)?;
    if out.len() as u64 > max {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "file over the size cap"));
    }
    Ok(Some(out))
}

/// The permission bits (`& 0o777`) of `path`; 0 off Unix.
pub fn mode_of(path: &Path) -> io::Result<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Ok(std::fs::metadata(path)?.permissions().mode() & 0o777)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(0)
    }
}

/// `n` random bytes from the OS (`/dev/urandom`).
pub fn random_bytes<const N: usize>() -> io::Result<[u8; N]> {
    let mut b = [0u8; N];
    File::open("/dev/urandom")?.read_exact(&mut b)?;
    Ok(b)
}

/// The effective uid of this process (0 off Unix).
pub fn euid() -> u32 {
    #[cfg(unix)]
    {
        // SAFETY: geteuid has no preconditions and cannot fail.
        unsafe { libc::geteuid() }
    }
    #[cfg(not(unix))]
    {
        0
    }
}

/// The owning uid of `path`, without following a final symlink.
pub fn owner_of(path: &Path) -> io::Result<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(std::fs::symlink_metadata(path)?.uid())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(0)
    }
}

/// True when `path` is a symlink.
pub fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

/// Refuse to act unless this process owns `dir`: files written by another
/// user (root, say) could not be read back by the service user. A directory
/// that does not exist yet is fine, except for root, who would create it
/// root-owned.
pub fn require_owner(dir: &Path) -> Result<(), String> {
    if is_symlink(dir) {
        return Err(format!("{} is a symlink; name the real state directory", dir.display()));
    }
    match owner_of(dir) {
        Ok(uid) if uid == euid() => Ok(()),
        Ok(uid) => Err(format!(
            "{} is owned by uid {uid} but this process runs as uid {}; run as the service user, e.g. `sudo -u weft-licence weft-licence ...`",
            dir.display(),
            euid()
        )),
        Err(e) if e.kind() == io::ErrorKind::NotFound && euid() != 0 => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Err(format!(
            "{} does not exist; as root, create it owned by the service user first (install -d -m 0700 -o weft-licence ...), or run as that user",
            dir.display()
        )),
        Err(e) => Err(e.to_string()),
    }
}
