//! cog-runner-style process supervision (COG-001 section 3): a cleared
//! environment, rlimits applied in the child before exec, an unprivileged
//! user, a wall-clock cap, capped output capture, and terminate-then-kill.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;

use super::evidence::{Capture, RunEvidence};
use super::types::RuntimeError;

/// Grace before SIGKILL when a run exceeds its wall-clock cap.
pub const TIMEOUT_GRACE: Duration = Duration::from_secs(2);

/// Resource limits applied with `setrlimit` in the child.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcLimits {
    /// Address-space cap (`RLIMIT_AS`) in bytes. Enforced on Linux; macOS
    /// does not implement it, so there it is best effort.
    pub mem_bytes: u64,
    /// CPU seconds (`RLIMIT_CPU`), for bounded runs.
    pub cpu_secs: Option<u64>,
    /// Open files (`RLIMIT_NOFILE`).
    pub nofile: u64,
    /// Largest file the process may write (`RLIMIT_FSIZE`).
    pub fsize_bytes: u64,
}

impl ProcLimits {
    /// Limits for a cog with `ram_mb` memory.
    pub fn for_ram_mb(ram_mb: u32) -> Self {
        Self {
            mem_bytes: u64::from(ram_mb) * 1024 * 1024,
            cpu_secs: None,
            nofile: 256,
            fsize_bytes: 64 * 1024 * 1024,
        }
    }
}

/// What to launch.
#[derive(Debug, Clone)]
pub struct LaunchSpec {
    /// Executable.
    pub program: PathBuf,
    /// Arguments.
    pub args: Vec<String>,
    /// Full environment (the parent's is cleared).
    pub env: Vec<(String, String)>,
    /// Working directory.
    pub cwd: PathBuf,
    /// rlimits.
    pub limits: ProcLimits,
    /// Bytes captured per stream.
    pub output_limit: usize,
    /// `(uid, gid)` to drop to; required when the supervisor runs as root.
    pub run_as: Option<(u32, u32)>,
}

/// A running supervised process.
pub struct Supervised {
    child: Child,
    out: Arc<Mutex<Capture>>,
    err: Arc<Mutex<Capture>>,
    readers: Vec<JoinHandle<()>>,
    started: Instant,
    args: Vec<String>,
    exited: Option<std::process::ExitStatus>,
}

/// True when this process runs as root.
pub fn running_as_root() -> bool {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() == 0 }
}

fn set_limit(resource: libc::c_int, value: u64) -> std::io::Result<()> {
    let lim = libc::rlimit {
        rlim_cur: value as libc::rlim_t,
        rlim_max: value as libc::rlim_t,
    };
    // SAFETY: plain syscall on a valid, initialized struct.
    if unsafe { libc::setrlimit(resource as _, &lim) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn apply_in_child(limits: ProcLimits, run_as: Option<(u32, u32)>) -> std::io::Result<()> {
    let as_result = set_limit(libc::RLIMIT_AS as libc::c_int, limits.mem_bytes);
    if cfg!(target_os = "linux") {
        as_result?;
    }
    if let Some(cpu) = limits.cpu_secs {
        set_limit(libc::RLIMIT_CPU as libc::c_int, cpu)?;
    }
    set_limit(libc::RLIMIT_NOFILE as libc::c_int, limits.nofile)?;
    set_limit(libc::RLIMIT_FSIZE as libc::c_int, limits.fsize_bytes)?;
    set_limit(libc::RLIMIT_CORE as libc::c_int, 0)?;
    if let Some((uid, gid)) = run_as {
        // SAFETY: async-signal-safe syscalls between fork and exec.
        unsafe {
            if libc::setgroups(0, std::ptr::null()) != 0
                || libc::setgid(gid as libc::gid_t) != 0
                || libc::setuid(uid as libc::uid_t) != 0
            {
                return Err(std::io::Error::last_os_error());
            }
        }
    }
    Ok(())
}

fn pump<R: tokio::io::AsyncRead + Unpin + Send + 'static>(
    mut r: R,
    sink: Arc<Mutex<Capture>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut buf = [0u8; 8192];
        loop {
            match r.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if let Ok(mut c) = sink.lock() {
                        c.push(&buf[..n]);
                    }
                }
            }
        }
    })
}

impl Supervised {
    /// Launch under limits. Refuses to run as root without `run_as`.
    pub fn spawn(spec: LaunchSpec) -> Result<Self, RuntimeError> {
        if running_as_root() && spec.run_as.is_none() {
            return Err(RuntimeError::AdmissionRefused(
                "native workloads run unprivileged: configure run_as when the host runs as root"
                    .into(),
            ));
        }
        if let Some((0, _)) = spec.run_as {
            return Err(RuntimeError::InvalidConfig(
                "run_as uid 0 is not unprivileged".into(),
            ));
        }
        let mut cmd = Command::new(&spec.program);
        cmd.args(&spec.args)
            .env_clear()
            .envs(spec.env.iter().cloned())
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .current_dir(&spec.cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);
        let (limits, run_as) = (spec.limits, spec.run_as);
        // SAFETY: the closure only makes async-signal-safe syscalls.
        unsafe {
            cmd.pre_exec(move || apply_in_child(limits, run_as));
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| RuntimeError::Backend(format!("spawn {}: {e}", spec.program.display())))?;
        let out = Arc::new(Mutex::new(Capture::new(spec.output_limit)));
        let err = Arc::new(Mutex::new(Capture::new(spec.output_limit)));
        let mut readers = Vec::new();
        if let Some(o) = child.stdout.take() {
            readers.push(pump(o, out.clone()));
        }
        if let Some(e) = child.stderr.take() {
            readers.push(pump(e, err.clone()));
        }
        Ok(Self {
            child,
            out,
            err,
            readers,
            started: Instant::now(),
            args: spec.args,
            exited: None,
        })
    }

    /// OS process id, while running.
    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    /// Non-blocking exit check.
    pub fn try_exit(&mut self) -> Option<std::process::ExitStatus> {
        if self.exited.is_none() {
            self.exited = self.child.try_wait().ok().flatten();
        }
        self.exited
    }

    fn signal_group(&self, sig: libc::c_int) {
        if let Some(pid) = self.child.id() {
            // SAFETY: signalling our own child's process group.
            unsafe {
                libc::kill(-(pid as libc::pid_t), sig);
            }
        }
    }

    /// Wait up to `max`; on timeout terminate, then kill after
    /// [`TIMEOUT_GRACE`], and mark the evidence `killed_for_timeout`.
    pub async fn wait_for(mut self, max: Duration) -> RunEvidence {
        let timed_out = tokio::time::timeout(max, self.child.wait()).await.is_err();
        if timed_out {
            self.terminate_inner(TIMEOUT_GRACE).await;
        }
        let mut ev = self.finish().await;
        ev.killed_for_timeout = timed_out;
        ev
    }

    /// SIGTERM the group, SIGKILL after `grace`, and collect evidence.
    pub async fn terminate(mut self, grace: Duration) -> RunEvidence {
        if self.try_exit().is_none() {
            self.terminate_inner(grace).await;
        }
        self.finish().await
    }

    async fn terminate_inner(&mut self, grace: Duration) {
        self.signal_group(libc::SIGTERM);
        if tokio::time::timeout(grace, self.child.wait())
            .await
            .is_err()
        {
            self.signal_group(libc::SIGKILL);
            let _ = self.child.kill().await;
        }
    }

    async fn finish(mut self) -> RunEvidence {
        let status = match self.exited {
            Some(s) => Some(s),
            None => self.child.wait().await.ok(),
        };
        // Readers finish at EOF; a grandchild holding the pipe open is
        // bounded by this wait.
        for r in self.readers.drain(..) {
            let _ = tokio::time::timeout(Duration::from_secs(2), r).await;
        }
        use std::os::unix::process::ExitStatusExt;
        let ev = RunEvidence {
            args: self.args.clone(),
            exit_code: status.and_then(|s| s.code()),
            signal: status.and_then(|s| s.signal()),
            elapsed_ms: self.started.elapsed().as_millis() as u64,
            ..RunEvidence::default()
        };
        let out = self
            .out
            .lock()
            .map(|c| c.clone())
            .unwrap_or_else(|_| Capture::new(0));
        let err = self
            .err
            .lock()
            .map(|c| c.clone())
            .unwrap_or_else(|_| Capture::new(0));
        ev.with_output(&out, &err)
    }
}
