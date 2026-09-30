//! `runtime` checks: runtime dir, stale socket and pid files, node keys.
//!
//! `--fix` may remove a `kernel.sock` only after a connect attempt is refused
//! (nothing listens) and its recorded pid is not running, and a `kernel.pid`
//! only when that pid is not running. Key files are never modified and their
//! contents are never read.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::daemon::ProcTable;
use super::env::DoctorEnv;
use super::{Component, Finding, Severity};

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

/// Run all runtime checks. Returns findings and structured data.
pub fn check(env: &DoctorEnv, procs: &ProcTable, fix: bool) -> (Vec<Finding>, Value) {
    let c = Component::Runtime;
    let mut out = vec![Finding::new(
        c,
        "dir",
        Severity::Ok,
        format!("runtime dir {} (from {})", env.runtime_dir.display(), env.runtime_source.label()),
    )];
    let mut dirs_data = Vec::new();
    let mut keys: Vec<PathBuf> = Vec::new();

    for dir in env.runtime_dir_candidates() {
        if !dir.is_dir() {
            continue;
        }
        let mut entry = json!({ "dir": dir });
        let sock = dir.join(crate::SOCKET_NAME);
        let pidf = dir.join(crate::PID_FILE_NAME);
        let recorded: Option<u32> = std::fs::read_to_string(&pidf).ok().and_then(|t| t.trim().parse().ok());
        let pid_alive = recorded.is_some_and(|p| procs.alive(p));
        let state = socket_state(&sock);

        if let Some(s) = state {
            entry["socket"] = json!(format!("{s:?}").to_lowercase());
            out.push(socket_finding(&dir, &sock, s, recorded, pid_alive, fix));
        } else if dir == env.runtime_dir {
            out.push(Finding::new(c, format!("socket:{}", dir.display()), Severity::Ok, "no daemon socket (kernel not running here)"));
        }
        if pidf.exists() {
            entry["pid_file"] = json!({ "pid": recorded, "alive": pid_alive });
            if let Some(f) = pid_finding(&pidf, recorded, pid_alive, state, fix) {
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

fn socket_finding(dir: &Path, sock: &Path, s: SocketState, recorded: Option<u32>, pid_alive: bool, fix: bool) -> Finding {
    let c = Component::Runtime;
    let id = format!("socket:{}", dir.display());
    match s {
        SocketState::Live => Finding::new(c, id, Severity::Ok, format!("{} accepts connections", sock.display())),
        SocketState::Stale => {
            let mut f = Finding::new(c, id, Severity::Warn, format!("stale socket {} (connect refused, nothing listening)", sock.display()))
                .remedy(format!("rm {}   (or: weaver doctor --component runtime --fix)", sock.display()));
            if fix && !pid_alive {
                f.fixed = Some(match std::fs::remove_file(sock) {
                    Ok(()) => format!("removed {}", sock.display()),
                    Err(e) => format!("could not remove {}: {e}", sock.display()),
                });
            } else if fix {
                f.fixed = Some(format!("left in place: recorded pid {} is still running", recorded.unwrap_or(0)));
            }
            f
        }
        SocketState::NotSocket => Finding::new(c, id, Severity::Warn, format!("{} exists but is not a socket", sock.display()))
            .remedy(format!("inspect and remove by hand: {}", sock.display())),
        SocketState::Unknown => Finding::new(c, id, Severity::Warn, format!("cannot connect to {} (permission or transient error)", sock.display())),
    }
}

fn pid_finding(pidf: &Path, recorded: Option<u32>, alive: bool, sock: Option<SocketState>, fix: bool) -> Option<Finding> {
    let c = Component::Runtime;
    let id = format!("pid:{}", pidf.display());
    match recorded {
        None => Some(Finding::new(c, id, Severity::Warn, format!("{} is not a valid pid", pidf.display())).remedy(format!("rm {}", pidf.display()))),
        Some(_) if alive => None,
        Some(p) => {
            let mut f = Finding::new(c, id, Severity::Warn, format!("stale pid file {} (pid {p} not running)", pidf.display()))
                .remedy(format!("rm {}   (or: weaver doctor --component runtime --fix)", pidf.display()));
            if fix && sock != Some(SocketState::Live) {
                f.fixed = Some(match std::fs::remove_file(pidf) {
                    Ok(()) => format!("removed {}", pidf.display()),
                    Err(e) => format!("could not remove {}: {e}", pidf.display()),
                });
            }
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
        let d = tempfile::tempdir().unwrap();
        let (env, rt) = setup(d.path());
        stale_socket(&rt);
        std::fs::write(rt.join("kernel.pid"), "999999\n").unwrap();
        let (f, _) = check(&env, &ProcTable::default(), false);
        assert!(f.iter().any(|x| x.id.starts_with("socket:") && x.severity == Severity::Warn && x.remedy.is_some()));
        assert!(f.iter().any(|x| x.id.starts_with("pid:") && x.severity == Severity::Warn));
        assert!(f.iter().all(|x| x.fixed.is_none()));
        assert!(rt.join("kernel.sock").exists() && rt.join("kernel.pid").exists());
    }

    #[test]
    fn fix_removes_only_provably_stale_files_and_reports_it() {
        let d = tempfile::tempdir().unwrap();
        let (env, rt) = setup(d.path());
        stale_socket(&rt);
        std::fs::write(rt.join("kernel.pid"), "999999\n").unwrap();
        std::fs::write(rt.join("node.key"), b"secret").unwrap();
        let (f, _) = check(&env, &ProcTable::default(), true);
        assert!(f.iter().filter(|x| x.fixed.as_deref().is_some_and(|s| s.starts_with("removed"))).count() == 2);
        assert!(!rt.join("kernel.sock").exists() && !rt.join("kernel.pid").exists());
        assert_eq!(std::fs::read(rt.join("node.key")).unwrap(), b"secret");
    }

    #[test]
    fn live_socket_is_never_touched_even_with_fix() {
        let d = tempfile::tempdir().unwrap();
        let (env, rt) = setup(d.path());
        let _l = UnixListener::bind(rt.join("kernel.sock")).unwrap();
        std::fs::write(rt.join("kernel.pid"), "999999\n").unwrap();
        let (f, _) = check(&env, &ProcTable::default(), true);
        assert!(rt.join("kernel.sock").exists());
        assert!(f.iter().any(|x| x.id.starts_with("socket:") && x.severity == Severity::Ok));
        // Pid file is stale but the socket answers: leave both alone.
        assert!(rt.join("kernel.pid").exists());
    }

    #[test]
    fn stale_socket_kept_when_recorded_pid_is_running() {
        let d = tempfile::tempdir().unwrap();
        let (env, rt) = setup(d.path());
        stale_socket(&rt);
        std::fs::write(rt.join("kernel.pid"), "777\n").unwrap();
        let procs = ProcTable::parse("777 weaver kernel start\n");
        let (f, _) = check(&env, &procs, true);
        assert!(rt.join("kernel.sock").exists());
        assert!(f.iter().any(|x| x.fixed.as_deref().is_some_and(|s| s.starts_with("left in place"))));
    }

    #[test]
    fn multiple_node_keys_warn_without_reading_or_deleting() {
        let d = tempfile::tempdir().unwrap();
        let (env, rt) = setup(d.path());
        let home_rt = env.home.join(".clawft");
        std::fs::create_dir_all(&home_rt).unwrap();
        for p in [rt.join("node.key"), home_rt.join("node.key")] {
            std::fs::write(&p, b"TOPSECRET").unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let (f, data) = check(&env, &ProcTable::default(), true);
        let k = f.iter().find(|x| x.id == "node_key").unwrap();
        assert_eq!(k.severity, Severity::Warn);
        assert!(!format!("{f:?}{data}").contains("TOPSECRET"));
        assert!(rt.join("node.key").exists() && home_rt.join("node.key").exists());
    }

    #[test]
    fn loose_key_permissions_warn() {
        let d = tempfile::tempdir().unwrap();
        let (env, rt) = setup(d.path());
        let p = rt.join("node.key");
        std::fs::write(&p, b"k").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        let (f, _) = check(&env, &ProcTable::default(), false);
        assert!(f.iter().any(|x| x.id.starts_with("node_key_perms:") && x.remedy.as_deref().unwrap().starts_with("chmod 600")));
    }
}
