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
        let pid_file = dir.join("kernel.pid");
        if !pid_file.exists() {
            continue;
        }
        let pid = match std::fs::read_to_string(&pid_file)
            .map_err(|e| e.to_string())
            .and_then(|s| s.trim().parse::<u32>().map_err(|e| e.to_string()))
        {
            Ok(p) => p,
            Err(e) => {
                out.push(Found::Unverifiable { id, pid: None, reason: Skip::BadPidFile(e) });
                continue;
            }
        };
        if let Err(reason) = check_identity(&dir, pid, current_exe) {
            out.push(Found::Unverifiable { id, pid: Some(pid), reason });
            continue;
        }
        match io.handshake(&dir.join("kernel.sock")).await {
            Some(h) if h.project_id.as_deref() == Some(id.as_str()) && h.pid == pid => {
                out.push(Found::Adopted { id, pid });
            }
            Some(h) => out.push(Found::Unverifiable {
                id,
                pid: Some(pid),
                reason: Skip::HandshakeFailed(format!(
                    "answered as project {:?} pid {}",
                    h.project_id, h.pid
                )),
            }),
            None => out.push(Found::Unverifiable {
                id,
                pid: Some(pid),
                reason: Skip::HandshakeFailed("no answer on kernel.sock".into()),
            }),
        }
    }
    out
}
