//! Single-instance guard for a runtime dir (ADR-103 P0b).
//!
//! The daemon holds an exclusive advisory lock on `<root>/kernel.lock` for
//! its whole lifetime, so two kernels can never share one runtime dir, and
//! the socket can be safely reclaimed once the lock is held: nobody else is
//! serving it.
//!
//! - **Unix**: `flock(LOCK_EX | LOCK_NB)`; the kernel drops it on any exit,
//!   including a crash, so a stale lock file never blocks a restart.
//! - **Windows**: the lock file is opened with no sharing, which fails while
//!   another process holds it.
//!
//! The holder's PID is written into the lock file for the error message.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use clawft_types::runtime_paths::RuntimePaths;

/// Why the lock could not be taken.
#[derive(Debug, thiserror::Error)]
pub enum LockError {
    /// Another live process holds the lock.
    #[error("another kernel owns {root} (pid {pid})")]
    Held {
        /// Runtime root the lock guards.
        root: String,
        /// Holder PID, or `?` when it could not be read.
        pid: String,
    },
    /// Filesystem failure creating or opening the lock file.
    #[error("cannot take kernel lock {path}: {source}")]
    Io {
        /// Lock file path.
        path: PathBuf,
        /// Underlying error.
        source: std::io::Error,
    },
}

/// Held for the daemon's lifetime; dropping it releases the lock.
#[derive(Debug)]
pub struct InstanceLock {
    #[cfg(unix)]
    _guard: nix::fcntl::Flock<File>,
    #[cfg(not(unix))]
    _guard: File,
    path: PathBuf,
}

impl InstanceLock {
    /// Take the exclusive lock for `paths`, creating the runtime dir.
    pub fn acquire(paths: &RuntimePaths) -> Result<Self, LockError> {
        let path = paths.lock();
        let io_err = |source| LockError::Io {
            path: path.clone(),
            source,
        };
        std::fs::create_dir_all(paths.root()).map_err(io_err)?;
        let file = open_lock_file(&path);
        #[cfg(unix)]
        {
            use nix::errno::Errno;
            use nix::fcntl::{Flock, FlockArg};
            let file = file.map_err(io_err)?;
            match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
                Ok(mut guard) => {
                    record_pid(&mut guard);
                    Ok(Self {
                        _guard: guard,
                        path,
                    })
                }
                Err((_, Errno::EWOULDBLOCK)) => Err(held(paths, &path)),
                Err((_, errno)) => Err(io_err(std::io::Error::from(errno))),
            }
        }
        #[cfg(not(unix))]
        {
            match file {
                Ok(mut f) => {
                    record_pid(&mut f);
                    Ok(Self { _guard: f, path })
                }
                // ERROR_SHARING_VIOLATION (32) / ERROR_LOCK_VIOLATION (33).
                Err(e) if matches!(e.raw_os_error(), Some(32 | 33)) => Err(held(paths, &path)),
                Err(e) => Err(io_err(e)),
            }
        }
    }

    /// Lock file path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(unix)]
fn open_lock_file(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
}

#[cfg(windows)]
fn open_lock_file(path: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(path)
}

#[cfg(not(any(unix, windows)))]
fn open_lock_file(path: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
}

fn record_pid(file: &mut File) {
    use std::io::Seek;
    // Best effort: the lock, not the PID text, is the guarantee.
    let _ = file.set_len(0);
    let _ = file.rewind();
    let _ = write!(file, "{}", std::process::id());
    let _ = file.flush();
}

fn held(paths: &RuntimePaths, lock: &Path) -> LockError {
    // The lock file is truncated only by its holder, so it carries the
    // holder's PID; the pid file is the fallback (and the only source on
    // Windows, where the locked file cannot be read).
    let read_pid = |p: &Path| {
        let mut s = String::new();
        File::open(p).ok()?.read_to_string(&mut s).ok()?;
        s.trim().parse::<u32>().ok()
    };
    let pid = read_pid(lock)
        .or_else(|| read_pid(&paths.pid()))
        .map_or_else(|| "?".to_string(), |p| p.to_string());
    LockError::Held {
        root: paths.root().display().to_string(),
        pid,
    }
}

/// With the instance lock held, make the socket path bindable.
///
/// A socket file that refuses connections (or vanished) is stale: nobody
/// serves it, so it is unlinked. One that accepts connections belongs to a
/// server that does not hold our lock; it is never taken over.
#[cfg(unix)]
pub async fn reclaim_stale_socket(paths: &RuntimePaths) -> anyhow::Result<()> {
    use clawft_rpc::probe::{SocketState, probe_socket};
    let socket = paths.socket();
    if !socket.exists() {
        return Ok(());
    }
    match probe_socket(&socket).await {
        SocketState::Reachable => anyhow::bail!(
            "daemon already running (socket accepts connections: {}); \
             it does not hold {} - refusing to take over its socket",
            socket.display(),
            paths.lock().display()
        ),
        // ECONNREFUSED / ENOENT / anything else unusable: nobody serves it.
        _ => {
            std::fs::remove_file(&socket)?;
            tracing::warn!(socket = %socket.display(), "removed stale socket file");
            Ok(())
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_fails_with_holder_pid_then_succeeds_after_drop() {
        let dir = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::at(dir.path());
        let first = InstanceLock::acquire(&paths).expect("first lock");
        assert_eq!(first.path(), dir.path().join("kernel.lock"));

        let err = InstanceLock::acquire(&paths).expect_err("second must fail");
        let msg = err.to_string();
        assert!(msg.contains("another kernel owns"), "{msg}");
        assert!(msg.contains(&dir.path().display().to_string()), "{msg}");
        assert!(
            msg.contains(&format!("pid {}", std::process::id())),
            "{msg}"
        );

        drop(first);
        InstanceLock::acquire(&paths).expect("lock free after drop");
    }

    #[tokio::test]
    async fn stale_socket_is_unlinked_and_live_socket_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::at(dir.path());
        // No socket file: nothing to do.
        reclaim_stale_socket(&paths).await.unwrap();

        // Dead listener left its file behind: reclaimed.
        let dead = std::os::unix::net::UnixListener::bind(paths.socket()).unwrap();
        drop(dead);
        assert!(paths.socket().exists());
        reclaim_stale_socket(&paths).await.unwrap();
        assert!(!paths.socket().exists());

        // Live listener: refused, and its socket file is left alone.
        let _live = tokio::net::UnixListener::bind(paths.socket()).unwrap();
        let err = reclaim_stale_socket(&paths).await.unwrap_err();
        assert!(err.to_string().contains("already running"), "{err}");
        assert!(paths.socket().exists());
    }

    #[test]
    fn stale_lock_file_without_holder_does_not_block() {
        let dir = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::at(dir.path());
        std::fs::write(paths.lock(), "99999999").unwrap();
        InstanceLock::acquire(&paths).expect("a leftover file is not a lock");
    }
}
