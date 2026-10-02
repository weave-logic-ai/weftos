//! Restart the per-user daemon after `weaver update --restart` (ADR-103
//! Phase 1, package H).
//!
//! Safety rules, in order:
//! 1. The only pid ever acted on is the one in `<root>/kernel.pid`
//!    (`~/.weftos/run`). Nothing is found by scanning the process table.
//! 2. That pid must be alive and its executable must be a `weaver`
//!    (a recycled pid belongs to some other program).
//! 3. If the running executable is not the just-installed binary the restart
//!    is reported, not performed: a restart would not pick up the update.
//! 4. launchd / systemd are used only when the managed service's main pid
//!    *is* the pid-file pid; otherwise the pid-file pid gets SIGHUP, as
//!    `weaver kernel restart` does.
//! 5. Before a SIGHUP the user socket must answer `kernel.status` with a
//!    handshake pid equal to the pid-file pid, and the exe is checked once
//!    more immediately before acting.
//!
//! Every effect (liveness, exe lookup, handshake, `launchctl`, `systemctl`,
//! signals, sleeping) goes through [`Host`], so tests inject fakes and never
//! signal a real process. Project-local daemons from manifests
//! `[legacy].runtime_dir` are listed, never touched.

use std::path::{Path, PathBuf};
use std::time::Duration;

use clawft_types::project::list_manifests;
use clawft_types::runtime_paths::{home_dir, user_runtime_root};

use crate::service_units::{LAUNCHD_LABEL, SYSTEMD_UNIT, canonical_exe, strip_deleted};

/// A side effect the restart wants performed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// `launchctl kickstart -k gui/<uid>/ai.weftos.user`.
    LaunchdKickstart { uid: u32 },
    /// `systemctl --user restart weftos`.
    SystemctlRestart,
    /// SIGHUP to the pid-file pid (the daemon re-execs).
    Sighup { pid: u32 },
}

/// What `kernel.status` reports about the daemon behind a socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonStatus {
    /// Build stamp.
    pub sha: String,
    /// The daemon's own pid from its handshake, when reported.
    pub pid: Option<u32>,
}

/// The outside world. The real impl is [`RealHost`]; tests supply fakes.
pub trait Host {
    fn alive(&self, pid: u32) -> bool;
    /// Executable of a process (may carry a Linux ` (deleted)` suffix).
    fn exe_of(&self, pid: u32) -> Option<PathBuf>;
    /// `kernel.status` on `socket`; `None` when nothing answers.
    fn status(&self, socket: &Path) -> Option<DaemonStatus>;
    /// Main pid of the loaded launchd service, `None` if not loaded.
    fn launchd_main_pid(&self, uid: u32) -> Option<u32>;
    /// Main pid of the active systemd user unit, `None` if inactive.
    fn systemd_main_pid(&self) -> Option<u32>;
    fn perform(&self, action: &Action) -> Result<(), String>;
    /// Pause between handshake polls.
    fn settle(&self);
}

/// Everything the restart reads, injectable.
#[derive(Debug, Clone)]
pub struct Inputs {
    pub pid_file: PathBuf,
    pub socket: PathBuf,
    /// The just-installed `weaver` binary.
    pub installed_exe: PathBuf,
    pub uid: u32,
    /// Manifest store to list legacy project daemons from.
    pub manifests_dir: Option<PathBuf>,
    /// Handshake polls after the restart (one [`Host::settle`] between).
    pub polls: u32,
}

/// What happened to the user daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// No live daemon recorded; nothing done.
    NotRunning(String),
    /// The pid file does not name a daemon we may act on; nothing done.
    Refused(String),
    /// Running binary is not the installed one; nothing done.
    ExeMismatch { running: PathBuf, installed: PathBuf },
    /// The action itself failed.
    Failed(String),
    /// Restart issued; `after` is the handshake sha seen afterwards.
    Restarted {
        method: &'static str,
        before: Option<String>,
        after: Option<String>,
        confirmed: bool,
    },
}

/// A project-local daemon found through a manifest, left alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyDaemon {
    pub project: String,
    pub runtime_dir: PathBuf,
    pub pid: Option<u32>,
    pub alive: bool,
}

/// Result of [`restart_user_daemon`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub outcome: Outcome,
    pub legacy: Vec<LegacyDaemon>,
}

fn read_pid(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.split_whitespace().next()?.parse().ok()
}

fn is_weaver(exe: &Path) -> bool {
    matches!(
        strip_deleted(exe).file_name().and_then(|n| n.to_str()),
        Some("weaver" | "weaver.exe")
    )
}

fn same_binary(a: &Path, b: &Path) -> bool {
    canonical_exe(a) == canonical_exe(b)
}

/// Restart the user daemon described by `i`, using `host` for every effect.
pub fn restart_with(i: &Inputs, host: &dyn Host) -> Report {
    Report {
        outcome: decide_and_restart(i, host),
        legacy: list_legacy(i, host),
    }
}

fn decide_and_restart(i: &Inputs, host: &dyn Host) -> Outcome {
    let Ok(text) = std::fs::read_to_string(&i.pid_file) else {
        return Outcome::NotRunning(format!("no pid file at {}", i.pid_file.display()));
    };
    let pid = match text.split_whitespace().next().map(str::parse::<u32>) {
        Some(Ok(p)) if p > 1 => p,
        _ => return Outcome::Refused(format!("{} does not hold a usable pid", i.pid_file.display())),
    };
    if !host.alive(pid) {
        return Outcome::NotRunning(format!("pid {pid} in {} is not running (stale)", i.pid_file.display()));
    }
    let Some(exe) = host.exe_of(pid) else {
        return Outcome::Refused(format!("cannot determine the executable of pid {pid}; not restarting"));
    };
    if !is_weaver(&exe) {
        return Outcome::Refused(format!(
            "pid {pid} is {}, not a weaver; the pid file is stale or the pid was reused",
            exe.display()
        ));
    }
    // The machine mesh service is never ours to signal or restart: it runs the
    // root-owned copy and is restarted by an administrator (printed lines only).
    if canonical_exe(&exe) == canonical_exe(Path::new(crate::service_units_system::SERVICE_EXE)) {
        return Outcome::Refused(format!(
            "pid {pid} is the machine mesh service ({}); it is restarted by an administrator, never by weaver update",
            crate::service_units_system::SERVICE_EXE
        ));
    }
    if !same_binary(&exe, &i.installed_exe) {
        return Outcome::ExeMismatch {
            running: strip_deleted(&exe),
            installed: i.installed_exe.clone(),
        };
    }

    let before_status = host.status(&i.socket);
    let before = before_status.as_ref().map(|st| st.sha.clone());
    let (method, action) = if host.launchd_main_pid(i.uid) == Some(pid) {
        ("launchd", Action::LaunchdKickstart { uid: i.uid })
    } else if host.systemd_main_pid() == Some(pid) {
        ("systemd", Action::SystemctlRestart)
    } else {
        // A signal needs proof the socket and the pid file name one daemon.
        match before_status.as_ref().map(|st| st.pid) {
            None => {
                return Outcome::Refused(format!(
                    "the user socket {} does not answer kernel.status; not signalling pid {pid}",
                    i.socket.display()
                ));
            }
            Some(p) if p != Some(pid) => {
                return Outcome::Refused(format!(
                    "the daemon on {} reports pid {}, not pid {pid} from the pid file; not signalling",
                    i.socket.display(),
                    p.map_or_else(|| "none".into(), |p| p.to_string())
                ));
            }
            Some(_) => {}
        }
        ("sighup", Action::Sighup { pid })
    };
    // Re-check the exe right before acting: the pid may have been recycled
    // since the first look.
    match host.exe_of(pid) {
        Some(e) if is_weaver(&e) && same_binary(&e, &i.installed_exe) => {}
        _ => return Outcome::Refused(format!("pid {pid} changed identity just before the restart; not acting")),
    }
    if let Err(e) = host.perform(&action) {
        return Outcome::Failed(format!("{method} restart failed: {e}"));
    }

    let mut after = None;
    let mut confirmed = false;
    for n in 0..i.polls {
        if n > 0 {
            host.settle();
        }
        if let Some(st) = host.status(&i.socket) {
            let new_pid = read_pid(&i.pid_file);
            confirmed = new_pid != Some(pid) || Some(&st.sha) != before.as_ref();
            after = Some(st.sha);
            if confirmed {
                break;
            }
        }
    }
    Outcome::Restarted { method, before, after, confirmed }
}

/// Project-local daemons named by manifests; read-only.
fn list_legacy(i: &Inputs, host: &dyn Host) -> Vec<LegacyDaemon> {
    let Some(dir) = &i.manifests_dir else { return Vec::new() };
    let Ok(listing) = list_manifests(dir) else { return Vec::new() };
    let own_root = i.pid_file.parent();
    listing
        .manifests
        .iter()
        .filter_map(|m| {
            let rt = m.legacy.as_ref()?.runtime_dir.clone()?;
            if Some(rt.as_path()) == own_root {
                return None;
            }
            let pid = read_pid(&rt.join("kernel.pid"));
            Some(LegacyDaemon {
                project: m.name.clone(),
                alive: pid.is_some_and(|p| host.alive(p)),
                runtime_dir: rt,
                pid,
            })
        })
        .collect()
}

impl Report {
    /// Human-readable lines for the CLI.
    pub fn lines(&self, manual_cmd: &str) -> Vec<String> {
        let mut out = Vec::new();
        match &self.outcome {
            Outcome::NotRunning(why) => out.push(format!("User daemon not running ({why}); nothing to restart.")),
            Outcome::Refused(why) => out.push(format!("Not restarting: {why}.")),
            Outcome::ExeMismatch { running, installed } => {
                out.push(format!(
                    "Not restarting: the daemon runs {} but the installed binary is {}.",
                    running.display(),
                    installed.display()
                ));
                out.push(format!("Restart it yourself once you know which build you want: {manual_cmd}"));
            }
            Outcome::Failed(why) => {
                out.push(format!("Restart failed: {why}"));
                out.push(format!("Restart it yourself: {manual_cmd}"));
            }
            Outcome::Restarted { method, before, after, confirmed } => {
                let sha = |s: &Option<String>| s.clone().unwrap_or_else(|| "unknown".into());
                out.push(format!("Restart requested via {method}."));
                match (confirmed, after) {
                    (true, _) => out.push(format!("Daemon is back: build {} -> {}.", sha(before), sha(after))),
                    (false, Some(_)) => out.push(format!(
                        "Daemon answers but looks unchanged (build {}); the binary may not have changed.",
                        sha(after)
                    )),
                    (false, None) => out.push("Daemon did not answer after the restart; check `weaver doctor`.".into()),
                }
            }
        }
        for d in &self.legacy {
            let state = match (d.pid, d.alive) {
                (Some(p), true) => format!("running, pid {p}"),
                (Some(p), false) => format!("pid {p} not running"),
                (None, _) => "no pid file".to_owned(),
            };
            out.push(format!(
                "Project-local daemon for {} ({state}) at {} was left alone.",
                d.project,
                d.runtime_dir.display()
            ));
        }
        out
    }
}

/// Restart the user daemon with the real host. Never panics, never errors:
/// the result says what was and was not done.
pub fn restart_user_daemon() -> Report {
    let Some(home) = home_dir() else {
        return Report {
            outcome: Outcome::NotRunning("cannot determine the home directory".into()),
            legacy: Vec::new(),
        };
    };
    let root = user_runtime_root(&home);
    let installed = std::env::current_exe().unwrap_or_default();
    let inputs = Inputs {
        pid_file: root.join("kernel.pid"),
        socket: root.join("kernel.sock"),
        installed_exe: installed,
        uid: local_uid(),
        manifests_dir: Some(clawft_rpc::resolve::manifests_dir(&home)),
        polls: 30,
    };
    restart_with(&inputs, &RealHost)
}

fn local_uid() -> u32 {
    #[cfg(unix)]
    {
        nix::unistd::getuid().as_raw()
    }
    #[cfg(not(unix))]
    {
        0
    }
}

/// Parse `pid = N` from `launchctl print` output.
pub fn parse_launchctl_pid(text: &str) -> Option<u32> {
    text.lines()
        .map(str::trim)
        .find_map(|l| l.strip_prefix("pid = "))
        .and_then(|p| p.trim().parse().ok())
}

/// Parse `MainPID=` / `ActiveState=` from `systemctl show`; `Some` only when active.
pub fn parse_systemctl_show(text: &str) -> Option<u32> {
    let get = |k: &str| text.lines().find_map(|l| l.strip_prefix(k));
    if get("ActiveState=")? != "active" {
        return None;
    }
    get("MainPID=")?.trim().parse().ok().filter(|p| *p > 1)
}

/// Query `kernel.status` over the unix socket (blocking, short timeouts).
#[cfg(unix)]
fn rpc_daemon_status(socket: &Path) -> Option<DaemonStatus> {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    let mut s = UnixStream::connect(socket).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    s.set_write_timeout(Some(Duration::from_secs(2))).ok()?;
    s.write_all(b"{\"method\":\"kernel.status\",\"params\":null,\"auth\":\"read\",\"proto\":1}\n").ok()?;
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line).ok()?;
    parse_status(&line)
}

#[cfg(not(unix))]
fn rpc_daemon_status(_: &Path) -> Option<DaemonStatus> {
    None
}

/// Parse a `kernel.status` response line.
pub fn parse_status(line: &str) -> Option<DaemonStatus> {
    let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    let sha = v.pointer("/result/build/sha")?.as_str()?.to_owned();
    let pid = v
        .pointer("/result/handshake/pid")
        .and_then(serde_json::Value::as_u64)
        .and_then(|p| u32::try_from(p).ok());
    Some(DaemonStatus { sha, pid })
}

/// The real process / service manager.
pub struct RealHost;

impl Host for RealHost {
    fn alive(&self, pid: u32) -> bool {
        #[cfg(unix)]
        {
            nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), None).is_ok()
        }
        #[cfg(not(unix))]
        {
            let _ = pid;
            false
        }
    }

    fn exe_of(&self, pid: u32) -> Option<PathBuf> {
        clawft_rpc::doctor::daemon::resolve_exe_cwd(pid).0
    }

    fn status(&self, socket: &Path) -> Option<DaemonStatus> {
        rpc_daemon_status(socket)
    }

    fn launchd_main_pid(&self, uid: u32) -> Option<u32> {
        if !cfg!(target_os = "macos") {
            return None;
        }
        let out = std::process::Command::new("launchctl")
            .args(["print", &format!("gui/{uid}/{LAUNCHD_LABEL}")])
            .output()
            .ok()?;
        out.status.success().then(|| parse_launchctl_pid(&String::from_utf8_lossy(&out.stdout)))?
    }

    fn systemd_main_pid(&self) -> Option<u32> {
        if !cfg!(target_os = "linux") {
            return None;
        }
        let out = std::process::Command::new("systemctl")
            .args(["--user", "show", SYSTEMD_UNIT, "-p", "ActiveState", "-p", "MainPID"])
            .output()
            .ok()?;
        parse_systemctl_show(&String::from_utf8_lossy(&out.stdout))
    }

    fn perform(&self, action: &Action) -> Result<(), String> {
        let run = |cmd: &str, args: &[&str]| {
            let st = std::process::Command::new(cmd).args(args).status().map_err(|e| e.to_string())?;
            st.success().then_some(()).ok_or_else(|| format!("{cmd} exited with {st}"))
        };
        match action {
            Action::LaunchdKickstart { uid } => {
                run("launchctl", &["kickstart", "-k", &format!("gui/{uid}/{LAUNCHD_LABEL}")])
            }
            Action::SystemctlRestart => run("systemctl", &["--user", "restart", SYSTEMD_UNIT]),
            #[cfg(unix)]
            Action::Sighup { pid } => nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(*pid as i32),
                nix::sys::signal::Signal::SIGHUP,
            )
            .map_err(|e| e.to_string()),
            #[cfg(not(unix))]
            Action::Sighup { .. } => Err("SIGHUP restart is unix-only".into()),
        }
    }

    fn settle(&self) {
        std::thread::sleep(Duration::from_millis(500));
    }
}

#[cfg(test)]
#[path = "daemon_restart_tests.rs"]
mod tests;
