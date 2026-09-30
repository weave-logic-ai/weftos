//! `runtime` checks: runtime dir, stale socket and pid files, node keys.
//!
//! `--fix` may remove a `kernel.sock` only after a connect attempt is refused
//! (nothing listens) and its recorded pid is not running, and a `kernel.pid`
//! only when that pid is not running. Both conditions are re-checked
//! immediately before each unlink. `--fix` refuses to act when `ps` failed
//! (liveness unknown) and, by default, only touches the ACTIVE runtime dir;
//! `--all-runtimes` extends it to every candidate dir. Key files are never
//! modified and their contents are never read.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::daemon::{pid_liveness, Liveness, ProcTable};
use super::env::DoctorEnv;
use super::{Component, Finding, Severity};

#[cfg_attr(not(unix), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SocketState {
    Live,
    Stale,
    NotSocket,
    Unknown,
}

#[cfg(unix)]
fn socket_state(p: &Path) -> Option<SocketState> {
    use std::os::unix::fs::FileTypeExt;
    let meta = std::fs::symlink_metadata(p).ok()?;
    if !meta.file_type().is_socket() {
        return Some(SocketState::NotSocket);
    }
    Some(match std::os::unix::net::UnixStream::connect(p) {
        Ok(_) => SocketState::Live,
        Err(e) if matches!(e.kind(), std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound) => {
            SocketState::Stale
        }
        Err(_) => SocketState::Unknown,
    })
}

#[cfg(not(unix))]
fn socket_state(_: &Path) -> Option<SocketState> {
    None
}

#[cfg(unix)]
fn key_mode(p: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).ok().map(|m| m.permissions().mode() & 0o777)
}

#[cfg(not(unix))]
fn key_mode(_: &Path) -> Option<u32> {
    None
}

/// What `--fix` may do for one runtime dir.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FixMode {
    Off,
    NoLiveness,
    OutOfScope,
    On,
}

/// Run all runtime checks. Returns findings and structured data.
///
/// `fix` enables repairs; they apply to the active runtime dir only unless
/// `all_runtimes` is set.
pub fn check(env: &DoctorEnv, procs: &ProcTable, fix: bool, all_runtimes: bool) -> (Vec<Finding>, Value) {
    let c = Component::Runtime;
    let mut out = vec![Finding::new(
        c,
        "dir",
        Severity::Ok,
        format!("runtime dir {} (from {})", env.runtime_dir.display(), env.runtime_source.label()),
    )];
    if !procs.ok {
        out.push(
            Finding::new(c, "liveness", Severity::Warn, "could not list processes (ps failed or sandboxed); pid liveness is unknown")
                .remedy("run doctor outside the sandbox; --fix will not remove anything until liveness is known"),
        );
    }
    let mut dirs_data = Vec::new();
    let mut keys: Vec<PathBuf> = Vec::new();

    for dir in env.runtime_dir_candidates() {
        if !dir.is_dir() {
            continue;
        }
        let mode = if !fix {
            FixMode::Off
        } else if !procs.ok {
            FixMode::NoLiveness
        } else if !(all_runtimes || dir == env.runtime_dir) {
            FixMode::OutOfScope
        } else {
            FixMode::On
        };
        let mut entry = json!({ "dir": dir });
        let sock = dir.join(crate::SOCKET_NAME);
        let pidf = dir.join(crate::PID_FILE_NAME);
        let recorded: Option<u32> = std::fs::read_to_string(&pidf).ok().and_then(|t| t.trim().parse().ok());
        let pid_alive = recorded.is_some_and(|p| procs.alive(p));
        let state = socket_state(&sock);

        if let Some(s) = state {
            entry["socket"] = json!(format!("{s:?}").to_lowercase());
            out.push(socket_finding(env, &dir, &sock, s, recorded, mode));
        } else if dir == env.runtime_dir {
            out.push(Finding::new(c, format!("socket:{}", dir.display()), Severity::Ok, "no daemon socket (kernel not running here)"));
        }
        if pidf.exists() {
            entry["pid_file"] = json!({ "pid": recorded, "alive": pid_alive });
            if let Some(f) = pid_finding(env, &pidf, &sock, recorded, pid_alive, mode) {
                out.push(f);
            }
        }
        let key = dir.join("node.key");
        if key.is_file() {
            let canon = std::fs::canonicalize(&key).unwrap_or(key.clone());
            if !keys.contains(&canon) {
                keys.push(canon);
            }
        }
        dirs_data.push(entry);
    }
    out.extend(key_findings(&keys));
    let key_data: Vec<Value> = keys
        .iter()
        .map(|k| json!({ "path": k, "mode": key_mode(k).map(|m| format!("{m:o}")) }))
        .collect();
    (out, json!({ "dir": env.runtime_dir, "source": env.runtime_source.label(), "dirs": dirs_data, "node_keys": key_data }))
}

fn skipped(mode: FixMode) -> Option<String> {
    match mode {
        FixMode::Off | FixMode::On => None,
        FixMode::NoLiveness => Some("not fixed: process liveness unknown (ps failed)".into()),
        FixMode::OutOfScope => Some("not fixed: outside the active runtime dir (use --all-runtimes)".into()),
    }
}

fn remove(p: &Path) -> String {
    match std::fs::remove_file(p) {
        Ok(()) => format!("removed {}", p.display()),
        Err(e) => format!("could not remove {}: {e}", p.display()),
    }
}

/// Unlink a stale socket, re-checking staleness right before the unlink.
fn try_remove_socket(env: &DoctorEnv, sock: &Path, recorded: Option<u32>) -> String {
    if let Some(pid) = recorded {
        match pid_liveness(env, pid) {
            Liveness::Alive => return format!("left in place: recorded pid {pid} is still running"),
            Liveness::Unknown => return format!("not fixed: liveness of recorded pid {pid} unknown"),
            Liveness::Dead => {}
        }
    }
    if socket_state(sock) != Some(SocketState::Stale) {
        return "left in place: socket is no longer stale".into();
    }
    remove(sock)
}

/// Unlink a stale pid file, re-checking pid and socket right before.
fn try_remove_pid(env: &DoctorEnv, pidf: &Path, sock: &Path, pid: u32) -> String {
    match pid_liveness(env, pid) {
        Liveness::Alive => return format!("left in place: pid {pid} is running"),
        Liveness::Unknown => return format!("not fixed: liveness of pid {pid} unknown"),
        Liveness::Dead => {}
    }
    if socket_state(sock) == Some(SocketState::Live) {
        return "left in place: the socket answers".into();
    }
    remove(pidf)
}

fn socket_finding(env: &DoctorEnv, dir: &Path, sock: &Path, s: SocketState, recorded: Option<u32>, mode: FixMode) -> Finding {
    let c = Component::Runtime;
    let id = format!("socket:{}", dir.display());
    match s {
        SocketState::Live => Finding::new(c, id, Severity::Ok, format!("{} accepts connections", sock.display())),
        SocketState::Stale => {
            let mut f = Finding::new(c, id, Severity::Warn, format!("stale socket {} (connect refused, nothing listening)", sock.display()))
                .remedy(format!("rm {}   (or: weaver doctor --component runtime --fix)", sock.display()));
            f.fixed = if mode == FixMode::On { Some(try_remove_socket(env, sock, recorded)) } else { skipped(mode) };
            f
        }
        SocketState::NotSocket => Finding::new(c, id, Severity::Warn, format!("{} exists but is not a socket", sock.display()))
            .remedy(format!("inspect and remove by hand: {}", sock.display())),
        SocketState::Unknown => Finding::new(c, id, Severity::Warn, format!("cannot connect to {} (permission or transient error)", sock.display())),
    }
}

fn pid_finding(env: &DoctorEnv, pidf: &Path, sock: &Path, recorded: Option<u32>, alive: bool, mode: FixMode) -> Option<Finding> {
    let c = Component::Runtime;
    let id = format!("pid:{}", pidf.display());
    match recorded {
        None => Some(Finding::new(c, id, Severity::Warn, format!("{} is not a valid pid", pidf.display())).remedy(format!("rm {}", pidf.display()))),
        Some(_) if alive => None,
        Some(p) => {
            let mut f = Finding::new(c, id, Severity::Warn, format!("stale pid file {} (pid {p} not running)", pidf.display()))
                .remedy(format!("rm {}   (or: weaver doctor --component runtime --fix)", pidf.display()));
            f.fixed = if mode == FixMode::On { Some(try_remove_pid(env, pidf, sock, p)) } else { skipped(mode) };
            Some(f)
        }
    }
}

fn key_findings(keys: &[PathBuf]) -> Vec<Finding> {
    let c = Component::Runtime;
    let mut out = Vec::new();
    match keys.len() {
        0 => out.push(Finding::new(c, "node_key", Severity::Ok, "no node.key found (created on first boot)")),
        1 => out.push(Finding::new(c, "node_key", Severity::Ok, format!("one node.key: {}", keys[0].display()))),
        n => {
            let list = keys.iter().map(|k| k.display().to_string()).collect::<Vec<_>>().join(", ");
            out.push(
                Finding::new(c, "node_key", Severity::Warn, format!("{n} node.key files, one per runtime, so this machine has {n} node identities: {list}"))
                    .remedy("check which runtime each daemon uses (weaver doctor --component daemon); doctor never deletes or prints keys"),
            );
        }
    }
    for k in keys {
        if let Some(m) = key_mode(k).filter(|m| m & 0o077 != 0) {
            out.push(
                Finding::new(c, format!("node_key_perms:{}", k.display()), Severity::Warn, format!("{} is mode {m:o}, readable beyond its owner", k.display()))
                    .remedy(format!("chmod 600 {}", k.display())),
            );
        }
    }
    out
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::doctor::env::test_env;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;

    fn setup(d: &Path) -> (DoctorEnv, PathBuf) {
        let mut env = test_env(d);
        let rt = d.join("proj/.weftos/runtime");
        std::fs::create_dir_all(&rt).unwrap();
        std::fs::create_dir_all(&env.cwd).unwrap();
        env.runtime_dir = rt.clone();
        (env, rt)
    }

    fn stale_socket(rt: &Path) {
        let l = UnixListener::bind(rt.join("kernel.sock")).unwrap();
        drop(l); // file stays, nobody listens
    }

    #[test]
    fn stale_socket_and_pid_reported_and_left_alone_without_fix() {
        let _serial = crate::doctor::env::serial();
        let d = tempfile::tempdir().unwrap();
        let (env, rt) = setup(d.path());
        stale_socket(&rt);
        std::fs::write(rt.join("kernel.pid"), "999999\n").unwrap();
        let (f, _) = check(&env, &ProcTable::parse(""), false, false);
        assert!(f.iter().any(|x| x.id.starts_with("socket:") && x.severity == Severity::Warn && x.remedy.is_some()));
        assert!(f.iter().any(|x| x.id.starts_with("pid:") && x.severity == Severity::Warn));
        assert!(f.iter().all(|x| x.fixed.is_none()));
        assert!(rt.join("kernel.sock").exists() && rt.join("kernel.pid").exists());
    }

    #[test]
    fn fix_removes_only_provably_stale_files_and_reports_it() {
        let _serial = crate::doctor::env::serial();
        let d = tempfile::tempdir().unwrap();
        let (env, rt) = setup(d.path());
        stale_socket(&rt);
        std::fs::write(rt.join("kernel.pid"), "999999\n").unwrap();
        std::fs::write(rt.join("node.key"), b"secret").unwrap();
        let (f, _) = check(&env, &ProcTable::parse(""), true, false);
        assert!(f.iter().filter(|x| x.fixed.as_deref().is_some_and(|s| s.starts_with("removed"))).count() == 2);
        assert!(!rt.join("kernel.sock").exists() && !rt.join("kernel.pid").exists());
        assert_eq!(std::fs::read(rt.join("node.key")).unwrap(), b"secret");
    }

    #[test]
    fn live_socket_is_never_touched_even_with_fix() {
        let _serial = crate::doctor::env::serial();
        let d = tempfile::tempdir().unwrap();
        let (env, rt) = setup(d.path());
        let _l = UnixListener::bind(rt.join("kernel.sock")).unwrap();
        std::fs::write(rt.join("kernel.pid"), "999999\n").unwrap();
        let (f, _) = check(&env, &ProcTable::parse(""), true, false);
        assert!(rt.join("kernel.sock").exists());
        assert!(f.iter().any(|x| x.id.starts_with("socket:") && x.severity == Severity::Ok));
        // Pid file is stale but the socket answers: leave both alone.
        assert!(rt.join("kernel.pid").exists());
    }

    #[test]
    fn stale_socket_kept_when_recorded_pid_is_running() {
        let _serial = crate::doctor::env::serial();
        let d = tempfile::tempdir().unwrap();
        let (env, rt) = setup(d.path());
        stale_socket(&rt);
        std::fs::write(rt.join("kernel.pid"), "777\n").unwrap();
        let mut env = env;
        env.ps_override = Some("777 weaver kernel start\n".into());
        let procs = ProcTable::load(&env);
        let (f, _) = check(&env, &procs, true, false);
        assert!(rt.join("kernel.sock").exists());
        assert!(f.iter().any(|x| x.fixed.as_deref().is_some_and(|s| s.starts_with("left in place"))));
    }

    #[test]
    fn multiple_node_keys_warn_without_reading_or_deleting() {
        let _serial = crate::doctor::env::serial();
        let d = tempfile::tempdir().unwrap();
        let (env, rt) = setup(d.path());
        let home_rt = env.home.join(".clawft");
        std::fs::create_dir_all(&home_rt).unwrap();
        for p in [rt.join("node.key"), home_rt.join("node.key")] {
            std::fs::write(&p, b"TOPSECRET").unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let (f, data) = check(&env, &ProcTable::parse(""), true, false);
        let k = f.iter().find(|x| x.id == "node_key").unwrap();
        assert_eq!(k.severity, Severity::Warn);
        assert!(!format!("{f:?}{data}").contains("TOPSECRET"));
        assert!(rt.join("node.key").exists() && home_rt.join("node.key").exists());
    }

    #[test]
    fn loose_key_permissions_warn() {
        let _serial = crate::doctor::env::serial();
        let d = tempfile::tempdir().unwrap();
        let (env, rt) = setup(d.path());
        let p = rt.join("node.key");
        std::fs::write(&p, b"k").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        let (f, _) = check(&env, &ProcTable::parse(""), false, false);
        assert!(f.iter().any(|x| x.id.starts_with("node_key_perms:") && x.remedy.as_deref().unwrap().starts_with("chmod 600")));
    }

    #[test]
    fn fix_defaults_to_the_active_runtime_dir_only() {
        let _serial = crate::doctor::env::serial();
        let d = tempfile::tempdir().unwrap();
        let (env, rt) = setup(d.path());
        let other = env.home.join(".clawft");
        std::fs::create_dir_all(&other).unwrap();
        stale_socket(&rt);
        stale_socket(&other);
        let (f, _) = check(&env, &ProcTable::parse(""), true, false);
        assert!(!rt.join("kernel.sock").exists());
        assert!(other.join("kernel.sock").exists(), "outside the active dir");
        assert!(f.iter().any(|x| x.fixed.as_deref().is_some_and(|s| s.contains("--all-runtimes"))));
        let (_, _) = check(&env, &ProcTable::parse(""), true, true);
        assert!(!other.join("kernel.sock").exists(), "--all-runtimes extends scope");
    }

    #[test]
    fn env_override_is_the_only_candidate_even_with_project_dirs_above() {
        let _serial = crate::doctor::env::serial();
        let d = tempfile::tempdir().unwrap();
        let (mut env, rt) = setup(d.path());
        let sandbox = d.path().join("sandbox-rt");
        std::fs::create_dir_all(&sandbox).unwrap();
        stale_socket(&rt); // a "real" project runtime that must stay untouched
        env.runtime_dir = sandbox.clone();
        env.runtime_source = crate::doctor::env::RuntimeSource::EnvOverride;
        assert_eq!(env.runtime_dir_candidates(), vec![sandbox]);
        let (_, _) = check(&env, &ProcTable::parse(""), true, true);
        assert!(rt.join("kernel.sock").exists());
    }

    #[test]
    fn fix_refuses_when_ps_failed() {
        let _serial = crate::doctor::env::serial();
        let d = tempfile::tempdir().unwrap();
        let (env, rt) = setup(d.path());
        stale_socket(&rt);
        std::fs::write(rt.join("kernel.pid"), "999999\n").unwrap();
        let unknown = ProcTable { rows: vec![], ok: false };
        let (f, _) = check(&env, &unknown, true, true);
        assert!(rt.join("kernel.sock").exists() && rt.join("kernel.pid").exists());
        assert!(f.iter().any(|x| x.id == "liveness" && x.severity == Severity::Warn));
        assert!(f.iter().any(|x| x.fixed.as_deref().is_some_and(|s| s.contains("liveness unknown"))));
    }

    #[test]
    fn unlink_rechecks_immediately_before_removal() {
        let _serial = crate::doctor::env::serial();
        let d = tempfile::tempdir().unwrap();
        let (env, rt) = setup(d.path());
        // The socket went live after the scan: the re-check must keep it.
        let _l = UnixListener::bind(rt.join("kernel.sock")).unwrap();
        let msg = try_remove_socket(&env, &rt.join("kernel.sock"), None);
        assert!(msg.starts_with("left in place"), "{msg}");
        assert!(rt.join("kernel.sock").exists());
        // The recorded pid came alive after the scan: keep the pid file.
        let mut env2 = env.clone();
        env2.ps_override = Some("555 weaver kernel start\n".into());
        std::fs::write(rt.join("kernel.pid"), "555\n").unwrap();
        let msg = try_remove_pid(&env2, &rt.join("kernel.pid"), &rt.join("kernel.sock"), 555);
        assert!(msg.starts_with("left in place"), "{msg}");
        assert!(rt.join("kernel.pid").exists());
    }
}
