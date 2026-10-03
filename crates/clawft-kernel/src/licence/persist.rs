//! Atomic, 0600, size-capped file persistence for the licence stores.

use std::io::Write;
use std::path::Path;

use super::{LicenceError, MAX_STORE_BYTES};

/// Read `path` if it exists. A missing file is `Ok(None)`; a file over the
/// cap is an error (the caller poisons itself and never overwrites it).
pub(super) fn read_capped(path: &Path) -> Result<Option<Vec<u8>>, LicenceError> {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(LicenceError::Persist(e.to_string())),
    };
    if meta.len() > MAX_STORE_BYTES {
        return Err(LicenceError::TooLarge);
    }
    std::fs::read(path)
        .map(Some)
        .map_err(|e| LicenceError::Persist(e.to_string()))
}

/// Write `bytes` to `path`: a 0600 temp file in the same directory (0700 if
/// created), synced, then renamed over the target.
pub(super) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), LicenceError> {
    let err = |e: std::io::Error| LicenceError::Persist(e.to_string());
    if bytes.len() as u64 > MAX_STORE_BYTES {
        return Err(LicenceError::TooLarge);
    }
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    create_private_dir(dir).map_err(err)?;
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let result = (|| {
        let mut f = opts.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map_err(err)
}

fn create_private_dir(dir: &Path) -> std::io::Result<()> {
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
