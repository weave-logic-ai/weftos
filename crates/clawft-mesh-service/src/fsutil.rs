//! Filesystem trust helpers for the state directory (plan section 7).
//!
//! The state directory must be a real directory owned by the effective uid
//! with no group/other access; every file is opened with `O_NOFOLLOW`,
//! non-blocking (so a FIFO cannot hang us) and then re-checked with `fstat`
//! (regular file, owned by the effective uid). Reads are bounded.

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, Read};
use std::path::Path;

use crate::journal::JournalError;

/// Hard cap on one journal line (and so one record).
pub const MAX_RECORD_BYTES: usize = 1024 * 1024;

#[cfg(unix)]
pub fn euid() -> u32 {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() }
}

/// Create the state dir (0700) if missing, then require a safe directory.
#[cfg(unix)]
pub fn ensure_state_dir(dir: &Path) -> Result<(), JournalError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    if fs::symlink_metadata(dir).is_err() {
        fs::create_dir_all(dir)?;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    let md = fs::symlink_metadata(dir)?;
    let unsafe_ = |why: &str| JournalError::UnsafePath(format!("{}: {why}", dir.display()));
    if !md.file_type().is_dir() {
        return Err(unsafe_("not a real directory (symlink or file)"));
    }
    if md.uid() != euid() {
        return Err(unsafe_("not owned by the effective uid"));
    }
    if md.mode() & 0o077 != 0 {
        return Err(unsafe_("group/other permission bits set (need 0700)"));
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn ensure_state_dir(_dir: &Path) -> Result<(), JournalError> {
    Err(JournalError::Io(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "the machine journal requires a unix host (Windows is design-only in Phase 3)",
    )))
}

/// Open a regular file we own, never following a symlink final component.
#[cfg(unix)]
pub fn open_file(path: &Path, create: bool, write: bool, append: bool) -> Result<File, JournalError> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let mut o = OpenOptions::new();
    o.read(true).write(write).append(append).create(create);
    o.mode(0o600).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    let f = o.open(path)?;
    let md = f.metadata()?;
    if !md.file_type().is_file() {
        return Err(JournalError::UnsafePath(format!("{}: not a regular file", path.display())));
    }
    if md.uid() != euid() {
        return Err(JournalError::UnsafePath(format!("{}: not owned by the effective uid", path.display())));
    }
    Ok(f)
}

#[cfg(not(unix))]
pub fn open_file(_: &Path, _: bool, _: bool, _: bool) -> Result<File, JournalError> {
    Err(JournalError::Io(std::io::Error::new(std::io::ErrorKind::Unsupported, "unix only")))
}

/// Exclusive non-blocking flock on `mesh.lock`, recording our pid.
#[cfg(unix)]
pub fn take_lock(dir: &Path) -> Result<File, JournalError> {
    use std::io::Write;
    use std::os::unix::io::AsRawFd;
    let path = dir.join("mesh.lock");
    let mut f = open_file(&path, true, true, false)?;
    // SAFETY: valid fd owned by `f` for the duration of the call.
    let rc = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        if err.kind() != std::io::ErrorKind::WouldBlock {
            return Err(err.into());
        }
        let mut holder = None;
        for _ in 0..20 {
            let mut s = String::new();
            if let Ok(mut r) = open_file(&path, false, false, false) {
                let _ = Read::take(&mut r, 32).read_to_string(&mut s);
            }
            holder = s.trim().parse::<u32>().ok();
            if holder.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        return Err(JournalError::Locked { holder_pid: holder });
    }
    f.set_len(0)?;
    write!(f, "{}", std::process::id())?;
    f.sync_data()?;
    Ok(f)
}

#[cfg(not(unix))]
pub fn take_lock(_dir: &Path) -> Result<File, JournalError> {
    Err(JournalError::Io(std::io::Error::new(std::io::ErrorKind::Unsupported, "unix only")))
}

pub fn sync_dir(dir: &Path) {
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
}

/// One bounded line read.
pub enum Line {
    Eof,
    /// Newline-terminated line (newline stripped from the buffer).
    Complete,
    /// End of file without a trailing newline.
    Torn,
    /// More than `max` bytes without a newline.
    TooLong,
}

/// Read one line into `buf`, never buffering more than `max + 1` bytes.
/// Returns the kind and the number of bytes consumed.
pub fn read_line<R: BufRead>(r: &mut R, buf: &mut Vec<u8>, max: usize) -> std::io::Result<(Line, usize)> {
    buf.clear();
    let n = r.by_ref().take(max as u64 + 1).read_until(b'\n', buf)?;
    if n == 0 {
        return Ok((Line::Eof, 0));
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
        let kind = if buf.len() > max { Line::TooLong } else { Line::Complete };
        return Ok((kind, n));
    }
    Ok((if n > max { Line::TooLong } else { Line::Torn }, n))
}
