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

/// The outside world. The real impl is [`RealHost`]; tests supply fakes.
pub trait Host {
    fn alive(&self, pid: u32) -> bool;
    /// Executable of a process (may carry a Linux ` (deleted)` suffix).
    fn exe_of(&self, pid: u32) -> Option<PathBuf>;
    /// `(version, sha)` from `kernel.status` on `socket`.
    fn status(&self, socket: &Path) -> Option<(String, String)>;
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
    if !same_binary(&exe, &i.installed_exe) {
        return Outcome::ExeMismatch {
            running: strip_deleted(&exe),
            installed: i.installed_exe.clone(),
        };
    }

    let before = host.status(&i.socket).map(|(_, sha)| sha);
    let (method, action) = if host.launchd_main_pid(i.uid) == Some(pid) {
        ("launchd", Action::LaunchdKickstart { uid: i.uid })
    } else if host.systemd_main_pid() == Some(pid) {
        ("systemd", Action::SystemctlRestart)
    } else {
        ("sighup", Action::Sighup { pid })
    };
    if let Err(e) = host.perform(&action) {
        return Outcome::Failed(format!("{method} restart failed: {e}"));
    }

    let mut after = None;
    let mut confirmed = false;
    for n in 0..i.polls {
        if n > 0 {
            host.settle();
        }
        if let Some((_, sha)) = host.status(&i.socket) {
            let new_pid = read_pid(&i.pid_file);
            confirmed = new_pid != Some(pid) || Some(&sha) != before.as_ref();
            after = Some(sha);
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

    fn status(&self, socket: &Path) -> Option<(String, String)> {
        clawft_rpc::doctor::daemon::rpc_status(socket)
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
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Fake {
        alive: Vec<u32>,
        exes: Vec<(u32, &'static str)>,
        launchd: Option<u32>,
        systemd: Option<u32>,
        /// sha answers, consumed per `status` call; the last repeats.
        shas: RefCell<Vec<&'static str>>,
        performed: RefCell<Vec<Action>>,
        /// Pids this fake was asked about (alive/exe lookups).
        asked: RefCell<Vec<u32>>,
        fail: bool,
    }

    impl Host for Fake {
        fn alive(&self, pid: u32) -> bool {
            self.asked.borrow_mut().push(pid);
            self.alive.contains(&pid)
        }
        fn exe_of(&self, pid: u32) -> Option<PathBuf> {
            self.asked.borrow_mut().push(pid);
            self.exes.iter().find(|(p, _)| *p == pid).map(|(_, e)| PathBuf::from(e))
        }
        fn status(&self, _: &Path) -> Option<(String, String)> {
            let mut s = self.shas.borrow_mut();
            let sha = if s.len() > 1 { s.remove(0) } else { *s.first()? };
            Some(("0.8.1".into(), sha.into()))
        }
        fn launchd_main_pid(&self, _: u32) -> Option<u32> {
            self.launchd
        }
        fn systemd_main_pid(&self) -> Option<u32> {
            self.systemd
        }
        fn perform(&self, a: &Action) -> Result<(), String> {
            self.performed.borrow_mut().push(a.clone());
            if self.fail { Err("boom".into()) } else { Ok(()) }
        }
        fn settle(&self) {}
    }

    fn inputs(dir: &Path, pid_text: Option<&str>) -> Inputs {
        if let Some(t) = pid_text {
            std::fs::write(dir.join("kernel.pid"), t).unwrap();
        }
        Inputs {
            pid_file: dir.join("kernel.pid"),
            socket: dir.join("kernel.sock"),
            installed_exe: PathBuf::from("/opt/w/weaver"),
            uid: 501,
            manifests_dir: None,
            polls: 3,
        }
    }

    fn healthy() -> Fake {
        Fake {
            alive: vec![4242],
            exes: vec![(4242, "/opt/w/weaver")],
            shas: RefCell::new(vec!["old", "new"]),
            ..Fake::default()
        }
    }

    #[test]
    fn missing_pid_file_does_nothing() {
        let d = tempfile::tempdir().unwrap();
        let f = healthy();
        let r = restart_with(&inputs(d.path(), None), &f);
        assert!(matches!(r.outcome, Outcome::NotRunning(_)));
        assert!(f.performed.borrow().is_empty());
    }

    #[test]
    fn dead_pid_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let f = Fake { alive: vec![], ..healthy() };
        let r = restart_with(&inputs(d.path(), Some("4242\n")), &f);
        assert!(matches!(r.outcome, Outcome::NotRunning(_)));
        assert!(f.performed.borrow().is_empty());
    }

    #[test]
    fn non_weaver_pid_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let f = Fake { exes: vec![(4242, "/usr/bin/postgres")], ..healthy() };
        let r = restart_with(&inputs(d.path(), Some("4242")), &f);
        assert!(matches!(&r.outcome, Outcome::Refused(m) if m.contains("not a weaver")));
        assert!(f.performed.borrow().is_empty());
    }

    #[test]
    fn unknown_exe_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let f = Fake { exes: vec![], ..healthy() };
        let r = restart_with(&inputs(d.path(), Some("4242")), &f);
        assert!(matches!(r.outcome, Outcome::Refused(_)));
        assert!(f.performed.borrow().is_empty());
    }

    #[test]
    fn garbage_and_reserved_pids_are_refused() {
        for text in ["", "abc", "0", "1", "-5"] {
            let d = tempfile::tempdir().unwrap();
            let f = healthy();
            let r = restart_with(&inputs(d.path(), Some(text)), &f);
            assert!(matches!(r.outcome, Outcome::Refused(_)), "{text:?}");
            assert!(f.performed.borrow().is_empty());
            assert!(f.asked.borrow().is_empty(), "must not even look at {text:?}");
        }
    }

    #[test]
    fn exe_differing_from_installed_reports_without_restarting() {
        let d = tempfile::tempdir().unwrap();
        let f = Fake { exes: vec![(4242, "/other/place/weaver")], ..healthy() };
        let r = restart_with(&inputs(d.path(), Some("4242")), &f);
        assert!(matches!(r.outcome, Outcome::ExeMismatch { .. }));
        assert!(f.performed.borrow().is_empty());
    }

    #[test]
    fn deleted_suffix_still_matches_installed_binary() {
        let d = tempfile::tempdir().unwrap();
        let f = Fake { exes: vec![(4242, "/opt/w/weaver (deleted)")], ..healthy() };
        let r = restart_with(&inputs(d.path(), Some("4242")), &f);
        assert!(matches!(r.outcome, Outcome::Restarted { .. }), "{r:?}");
    }

    #[test]
    fn sighup_targets_only_the_pid_file_pid() {
        let d = tempfile::tempdir().unwrap();
        // 70730 stands in for another project's live daemon: alive, a weaver.
        let f = Fake {
            alive: vec![4242, 70730],
            exes: vec![(4242, "/opt/w/weaver"), (70730, "/opt/w/weaver")],
            ..healthy()
        };
        let r = restart_with(&inputs(d.path(), Some("4242\n")), &f);
        assert_eq!(*f.performed.borrow(), vec![Action::Sighup { pid: 4242 }]);
        assert!(matches!(r.outcome, Outcome::Restarted { method: "sighup", confirmed: true, .. }));
        assert!(!f.asked.borrow().contains(&70730));
    }

    #[test]
    fn launchd_used_only_when_service_pid_is_the_file_pid() {
        let d = tempfile::tempdir().unwrap();
        let f = Fake { launchd: Some(4242), ..healthy() };
        restart_with(&inputs(d.path(), Some("4242")), &f);
        assert_eq!(*f.performed.borrow(), vec![Action::LaunchdKickstart { uid: 501 }]);

        // Loaded, but managing some other pid: fall back to SIGHUP on the file pid.
        let f = Fake { launchd: Some(999), ..healthy() };
        restart_with(&inputs(d.path(), Some("4242")), &f);
        assert_eq!(*f.performed.borrow(), vec![Action::Sighup { pid: 4242 }]);
    }

    #[test]
    fn systemd_used_when_its_main_pid_is_the_file_pid() {
        let d = tempfile::tempdir().unwrap();
        let f = Fake { systemd: Some(4242), ..healthy() };
        restart_with(&inputs(d.path(), Some("4242")), &f);
        assert_eq!(*f.performed.borrow(), vec![Action::SystemctlRestart]);
    }

    #[test]
    fn launchd_wins_over_systemd() {
        let d = tempfile::tempdir().unwrap();
        let f = Fake { launchd: Some(4242), systemd: Some(4242), ..healthy() };
        restart_with(&inputs(d.path(), Some("4242")), &f);
        assert_eq!(*f.performed.borrow(), vec![Action::LaunchdKickstart { uid: 501 }]);
    }

    #[test]
    fn failed_action_is_reported() {
        let d = tempfile::tempdir().unwrap();
        let f = Fake { fail: true, ..healthy() };
        let r = restart_with(&inputs(d.path(), Some("4242")), &f);
        assert!(matches!(r.outcome, Outcome::Failed(_)));
    }

    #[test]
    fn unchanged_sha_and_pid_is_not_confirmed() {
        let d = tempfile::tempdir().unwrap();
        let f = Fake { shas: RefCell::new(vec!["same"]), ..healthy() };
        let r = restart_with(&inputs(d.path(), Some("4242")), &f);
        match r.outcome {
            Outcome::Restarted { confirmed, before, after, .. } => {
                assert!(!confirmed);
                assert_eq!(before, after);
            }
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn legacy_daemons_are_listed_not_touched() {
        let d = tempfile::tempdir().unwrap();
        let mdir = d.path().join("projects");
        std::fs::create_dir_all(&mdir).unwrap();
        let rt = d.path().join("example/.weftos/runtime");
        std::fs::create_dir_all(&rt).unwrap();
        std::fs::write(rt.join("kernel.pid"), "70730\n").unwrap();
        let id = "01JABCDEFGHJKMNPQRSTVWXYZ0";
        std::fs::write(
            mdir.join(format!("{id}.toml")),
            format!(
                "schema = 1\nid = \"{id}\"\nname = \"example\"\nroot = \"{}\"\nstate = \"active\"\n\
created = \"2026-01-01T00:00:00Z\"\nlast_seen = \"2026-01-01T00:00:00Z\"\n\n[legacy]\nruntime_dir = \"{}\"\n",
                d.path().join("example").display(),
                rt.display()
            ),
        )
        .unwrap();
        let run = d.path().join("run");
        std::fs::create_dir_all(&run).unwrap();
        let mut i = inputs(&run, Some("4242"));
        i.manifests_dir = Some(mdir);
        let f = Fake { alive: vec![4242, 70730], ..healthy() };
        let r = restart_with(&i, &f);
        assert_eq!(r.legacy.len(), 1, "{r:?}");
        assert_eq!(r.legacy[0].pid, Some(70730));
        assert!(r.legacy[0].alive);
        assert_eq!(*f.performed.borrow(), vec![Action::Sighup { pid: 4242 }]);
        assert!(r.lines("x").iter().any(|l| l.contains("left alone")));
    }

    #[test]
    fn parsers() {
        assert_eq!(parse_launchctl_pid("x = {\n\tpid = 812\n\tstate = running\n}"), Some(812));
        assert_eq!(parse_launchctl_pid("state = not running"), None);
        assert_eq!(parse_systemctl_show("ActiveState=active\nMainPID=77\n"), Some(77));
        assert_eq!(parse_systemctl_show("ActiveState=inactive\nMainPID=0\n"), None);
        assert_eq!(parse_systemctl_show("ActiveState=active\nMainPID=0\n"), None);
    }
}
