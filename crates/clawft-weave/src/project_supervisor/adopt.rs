//! Adoption of children that outlived a user-daemon restart (ADR-103 A6).
//!
//! `weaver update` and service restarts keep children running
//! (`AbandonProcessGroup` / `KillMode=process`). When the user daemon boots
//! it scans `<run_root>/*/kernel.pid` and adopts a child only when every
//! check passes:
//!
//! 1. the pid is alive;
//! 2. its executable has the name of the one we start children from (the
//!    path in `state.json`, else the current executable): a recycled pid
//!    that now names an unrelated program fails here;
//! 3. `kernel.lock` is held by someone, and the pid recorded in the lock
//!    file is that pid (a stale lock nobody holds fails here);
//! 4. the child answers `kernel.handshake` with this project's id and the
//!    same pid.
//!
//! Anything that fails a check is reported as unverifiable and is **never
//! signalled**: it is for `weaver doctor` and the operator.

use std::path::{Path, PathBuf};

use super::io::ChildIo;
use super::state;

/// Why a leftover was not adopted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skip {
    /// `kernel.pid` is unreadable.
    BadPidFile(String),
    /// The pid is gone: a stale run dir, nothing to adopt or signal.
    Dead,
    /// The process is not the kernel executable (pid reuse, or another program).
    WrongExe {
        /// What the process is.
        found: String,
        /// What was expected.
        expected: String,
    },
    /// `kernel.lock` is not held by that pid.
    LockNotHeld,
    /// The socket did not answer, or answered for another project or pid.
    HandshakeFailed(String),
    /// Verified as ours but the supervisor refused to manage it (revoked,
    /// no longer registered, ...); see the text for what was done.
    Refused(String),
}

impl std::fmt::Display for Skip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Skip::BadPidFile(e) => write!(f, "kernel.pid unreadable: {e}"),
            Skip::Dead => f.write_str("process is gone"),
            Skip::WrongExe { found, expected } => {
                write!(f, "process is {found:?}, not {expected:?} (pid reuse?)")
            }
            Skip::LockNotHeld => f.write_str("kernel.lock is not held by that pid"),
            Skip::HandshakeFailed(e) => write!(f, "handshake failed: {e}"),
            Skip::Refused(e) => write!(f, "{e}"),
        }
    }
}

/// One scanned run dir.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found {
    /// Verified: adopt this pid.
    Adopted {
        /// Project id.
        id: String,
        /// Child pid.
        pid: u32,
    },
    /// Not adopted, and never signalled.
    Unverifiable {
        /// Project id (the run dir name).
        id: String,
        /// The pid named by `kernel.pid`, when readable.
        pid: Option<u32>,
        /// Which check failed.
        reason: Skip,
    },
}

/// The executable name of `pid`.
pub fn process_exe_name(pid: u32) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let p = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
        let s = p.to_string_lossy().trim_end_matches(" (deleted)").to_owned();
        Path::new(&s).file_name().map(|n| n.to_string_lossy().into_owned())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let out = std::process::Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "comm="])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let s = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        Path::new(&s).file_name().map(|n| n.to_string_lossy().into_owned())
    }
}

fn exe_basename(p: &Path) -> String {
    p.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

/// The pid recorded in a lock file by its holder.
pub fn lock_holder_pid(lock: &Path) -> Option<u32> {
    std::fs::read_to_string(lock).ok()?.trim().parse().ok()
}

/// True when another open file description holds `lock` (an exclusive
/// `flock`); false when nobody does or the file is missing.
pub fn lock_held(lock: &Path) -> bool {
    use nix::errno::Errno;
    use nix::fcntl::{Flock, FlockArg};
    let Ok(file) = std::fs::File::open(lock) else { return false };
    match Flock::lock(file, FlockArg::LockSharedNonblock) {
        Ok(_released_on_drop) => false,
        Err((_, Errno::EWOULDBLOCK)) => true,
        Err(_) => false,
    }
}

/// The executable name a child of this run dir is expected to have: the one
/// recorded at spawn, else `fallback`'s.
fn expected_exe(run_dir: &Path, fallback: &Path) -> String {
    state::read(run_dir)
        .and_then(|s| s.exe)
        .map_or_else(|| exe_basename(fallback), |e| exe_basename(Path::new(&e)))
}

/// Checks 2 and 3 for `pid` in `run_dir`: cheap, synchronous, used both by
/// adoption and right before an adopted pid is signalled.
pub fn identity_ok(run_dir: &Path, pid: u32, current_exe: &Path) -> bool {
    check_identity(run_dir, pid, current_exe).is_ok()
}

fn check_identity(run_dir: &Path, pid: u32, current_exe: &Path) -> Result<(), Skip> {
    if !super::child::pid_alive(pid) {
        return Err(Skip::Dead);
    }
    let expected = expected_exe(run_dir, current_exe);
    let found = process_exe_name(pid).unwrap_or_default();
    if found != expected {
        return Err(Skip::WrongExe { found, expected });
    }
    let lock = run_dir.join("kernel.lock");
    if !lock_held(&lock) || lock_holder_pid(&lock) != Some(pid) {
        return Err(Skip::LockNotHeld);
    }
    Ok(())
}

/// How long an empty `kernel.pid` (or lock file) is read again before it is
/// called bad: the kernel creates the file a moment before the pid is in it.
const PID_FILE_GRACE: std::time::Duration = std::time::Duration::from_secs(2);
const PID_FILE_POLL: std::time::Duration = std::time::Duration::from_millis(20);

fn lock_is_booting(lock: &Path) -> bool {
    lock_held(lock) && std::fs::read_to_string(lock).is_ok_and(|s| s.trim().is_empty())
}

/// Read a pid file. Empty content is read again for up to `grace`, but only
/// while `lock` is held (a kernel still booting); with nobody holding the lock
/// an empty file is stale and fails at once. Any other content that is not a
/// pid is bad at once.
pub async fn read_pid_file(path: &Path, lock: &Path, grace: std::time::Duration) -> Result<u32, String> {
    let deadline = tokio::time::Instant::now() + grace;
    loop {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let t = text.trim();
        if !t.is_empty() {
            return t.parse::<u32>().map_err(|e| e.to_string());
        }
        if tokio::time::Instant::now() >= deadline || !lock_held(lock) {
            return t.parse::<u32>().map_err(|e| e.to_string());
        }
        tokio::time::sleep(PID_FILE_POLL).await;
    }
}

/// Verify one run dir. `None` when it has no `kernel.pid`.
pub async fn scan_one(dir: &Path, id: &str, current_exe: &Path, io: &dyn ChildIo) -> Option<Found> {
    let id = id.to_owned();
    let pid_file = dir.join("kernel.pid");
    if !pid_file.exists() {
        return None;
    }
    let pid = match read_pid_file(&pid_file, &dir.join("kernel.lock"), PID_FILE_GRACE).await {
        Ok(p) => p,
        Err(e) => return Some(Found::Unverifiable { id, pid: None, reason: Skip::BadPidFile(e) }),
    };
    let deadline = tokio::time::Instant::now() + PID_FILE_GRACE;
    loop {
        match check_identity(dir, pid, current_exe) {
            Ok(()) => break,
            // The holder truncates and rewrites the lock file's pid; an empty
            // one under a held lock is a kernel mid-write, not a stale lock.
            Err(Skip::LockNotHeld) if lock_is_booting(&dir.join("kernel.lock")) && tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(PID_FILE_POLL).await;
            }
            Err(reason) => return Some(Found::Unverifiable { id, pid: Some(pid), reason }),
        }
    }
    Some(match io.handshake(&dir.join("kernel.sock")).await {
        Some(h) if h.project_id.as_deref() == Some(id.as_str()) && h.pid == pid => Found::Adopted { id, pid },
        Some(h) => Found::Unverifiable {
            id,
            pid: Some(pid),
            reason: Skip::HandshakeFailed(format!("answered as project {:?} pid {}", h.project_id, h.pid)),
        },
        None => Found::Unverifiable {
            id,
            pid: Some(pid),
            reason: Skip::HandshakeFailed("no answer on kernel.sock".into()),
        },
    })
}

/// Scan `run_root` and verify every child dir. Pure observation: nothing is
/// signalled or modified.
pub async fn scan(run_root: &Path, current_exe: &Path, io: &dyn ChildIo) -> Vec<Found> {
    let Ok(rd) = std::fs::read_dir(run_root) else { return Vec::new() };
    let mut dirs: Vec<(String, PathBuf)> = rd
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| {
            let id = e.file_name().to_string_lossy().into_owned();
            clawft_types::project::validate_id(&id).ok().map(|()| (id, e.path()))
        })
        .collect();
    dirs.sort();
    let mut out = Vec::new();
    for (id, dir) in dirs {
        if let Some(f) = scan_one(&dir, &id, current_exe, io).await {
            out.push(f);
        }
    }
    out
}

#[cfg(test)]
mod pid_file_tests {
    use super::*;
    use std::time::Duration;

    /// Hold `kernel.lock` in `dir` for as long as the guard lives.
    fn held_lock(dir: &Path) -> (nix::fcntl::Flock<std::fs::File>, std::path::PathBuf) {
        let lock = dir.join("kernel.lock");
        let f = std::fs::OpenOptions::new().create(true).write(true).truncate(false).open(&lock).unwrap();
        let g = nix::fcntl::Flock::lock(f, nix::fcntl::FlockArg::LockExclusiveNonblock).map_err(|(_, e)| e).unwrap();
        (g, lock)
    }

    #[tokio::test]
    async fn an_empty_pid_file_with_no_lock_held_fails_at_once() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("kernel.pid");
        std::fs::write(&p, "").unwrap();
        let start = std::time::Instant::now();
        assert!(read_pid_file(&p, &t.path().join("kernel.lock"), Duration::from_secs(2)).await.is_err());
        assert!(start.elapsed() < Duration::from_millis(100), "stale empty file cost {:?}", start.elapsed());
    }

    #[tokio::test]
    async fn an_empty_pid_file_that_fills_in_shortly_is_read() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("kernel.pid");
        std::fs::write(&p, "").unwrap();
        let (_held, lock) = held_lock(t.path());
        let p2 = p.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            std::fs::write(&p2, "4242").unwrap();
        });
        assert_eq!(read_pid_file(&p, &lock, Duration::from_secs(2)).await, Ok(4242));
    }

    #[tokio::test]
    async fn bad_content_is_bad_at_once_and_a_forever_empty_file_after_the_grace() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("kernel.pid");
        std::fs::write(&p, "12x").unwrap();
        let (_held, lock) = held_lock(t.path());
        let start = std::time::Instant::now();
        assert!(read_pid_file(&p, &lock, Duration::from_secs(2)).await.is_err());
        assert!(start.elapsed() < Duration::from_millis(500), "garbage is not retried");
        std::fs::write(&p, "").unwrap();
        assert!(read_pid_file(&p, &lock, Duration::from_millis(100)).await.is_err());
    }

    #[tokio::test]
    async fn scan_one_adopts_nothing_from_garbage_and_reports_bad_pid_file() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("kernel.pid"), "nope").unwrap();
        let io = crate::project_supervisor::io::RpcChildIo::new(
            ed25519_dalek::SigningKey::from_bytes(&[1; 32]),
            t.path().to_path_buf(),
        );
        let f = scan_one(t.path(), "01JB8Z3Q0V6X9KQ4M2N7T5R1WD", Path::new("/x/weaver"), &io).await;
        assert!(matches!(f, Some(Found::Unverifiable { reason: Skip::BadPidFile(_), .. })), "{f:?}");
    }
}
