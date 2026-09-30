//! `daemon` checks: running kernel processes, their executable and version.
//!
//! Daemons are found from `ps`, never started or signalled. The executable
//! path comes from `/proc/<pid>/exe` (Linux) or `lsof` (macOS). The version
//! comes from the daemon's own `kernel.status` RPC when its socket can be tied
//! to the pid via `kernel.pid`, otherwise from `<exe> --version` (a plain
//! flag on the CLI binary, side-effect free; it never boots a kernel).

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

use super::env::{project_runtime_dirs, DoctorEnv};
use super::install::BinCopy;
use super::probe::{parse_version, probe_binary, sha256_file};
use super::{Component, Finding, Severity};

/// One `ps` row.
#[derive(Debug, Clone, Default)]
pub struct PsRow {
    /// Process id.
    pub pid: u32,
    /// Full command line.
    pub command: String,
}

/// Snapshot of the process table.
#[derive(Debug, Default)]
pub struct ProcTable {
    /// All rows.
    pub rows: Vec<PsRow>,
    /// `ps` ran and returned rows. False means liveness is UNKNOWN (sandbox,
    /// missing `ps`), which is not the same as "no processes".
    pub ok: bool,
}

/// Whether a pid is running, as far as we could tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    /// Running.
    Alive,
    /// Not running.
    Dead,
    /// Could not determine (`ps` unavailable).
    Unknown,
}

/// Fresh liveness check for one pid (used immediately before any unlink).
/// Honors the canned `ps_override` table in tests.
pub fn pid_liveness(env: &DoctorEnv, pid: u32) -> Liveness {
    if let Some(t) = &env.ps_override {
        return if ProcTable::parse(t).alive(pid) { Liveness::Alive } else { Liveness::Dead };
    }
    match Command::new("ps").args(["-p", &pid.to_string(), "-o", "pid="]).output() {
        Ok(o) if o.status.success() && !o.stdout.is_empty() => Liveness::Alive,
        Ok(o) if o.status.code() == Some(1) && o.stdout.is_empty() => Liveness::Dead,
        _ => Liveness::Unknown,
    }
}

impl ProcTable {
    /// Read via `ps` (or the canned override in `env`).
    pub fn load(env: &DoctorEnv) -> Self {
        let (text, ok) = match &env.ps_override {
            Some(t) => (t.clone(), true),
            None => match Command::new("ps").args(["-axo", "pid=,command="]).output() {
                Ok(o) if o.status.success() && !o.stdout.is_empty() => {
                    (String::from_utf8_lossy(&o.stdout).into_owned(), true)
                }
                _ => (String::new(), false),
            },
        };
        let mut t = Self::parse(&text);
        t.ok = ok;
        t
    }

    /// Parse `pid command...` lines.
    pub fn parse(text: &str) -> Self {
        let rows = text
            .lines()
            .filter_map(|l| {
                let l = l.trim_start();
                let (pid, rest) = l.split_once(char::is_whitespace)?;
                Some(PsRow { pid: pid.parse().ok()?, command: rest.trim().to_string() })
            })
            .collect();
        Self { rows, ok: true }
    }

    /// Whether a pid is in the table.
    pub fn alive(&self, pid: u32) -> bool {
        self.rows.iter().any(|r| r.pid == pid)
    }
}

/// A running kernel daemon.
#[derive(Debug, Clone, Serialize)]
pub struct DaemonProc {
    /// Process id.
    pub pid: u32,
    /// Binary and subcommand only (`weaver kernel start`). Full argv is never
    /// kept: it can carry tokens and API keys.
    pub command: String,
    /// Resolved executable.
    pub exe: Option<PathBuf>,
    /// SHA-256 of the executable.
    pub exe_sha256: Option<String>,
    /// Daemon version.
    pub version: Option<String>,
    /// Build stamp says dirty.
    pub dirty: bool,
    /// How the version was learned: `rpc`, `exe --version`, or none.
    pub version_source: Option<&'static str>,
    /// Socket tied to this pid through `kernel.pid`.
    pub socket: Option<PathBuf>,
}

/// Is this command line a kernel daemon? Returns the binary name.
///
/// The subcommand sequence must be exactly `kernel start` (weaver) or `boot`
/// (weftos) as the first positional arguments, after any `-flags`, so
/// `weaver ask how to start the kernel` is not a daemon.
pub fn daemon_kind(command: &str) -> Option<&'static str> {
    let mut it = command.split_whitespace();
    let base = Path::new(it.next()?).file_name()?.to_str()?.to_string();
    // Skip flags, and the separate value of the value-taking ones. The only
    // value flag on the weaver CLI before the subcommand is `-c/--config`
    // (`kernel --config x start`); `-v` and `--foreground` take none.
    let mut positionals: Vec<&str> = Vec::new();
    while let Some(a) = it.next() {
        if a == "-c" || a == "--config" {
            it.next();
        } else if !a.starts_with('-') {
            positionals.push(a);
        }
    }
    match base.as_str() {
        "weaver" if positionals.first() == Some(&"kernel") && positionals.get(1) == Some(&"start") => Some("weaver"),
        "weftos" if positionals.first() == Some(&"boot") => Some("weftos"),
        _ => None,
    }
}

/// `(exe, cwd)` for a pid, best effort.
pub fn resolve_exe_cwd(pid: u32) -> (Option<PathBuf>, Option<PathBuf>) {
    #[cfg(target_os = "linux")]
    {
        let d = PathBuf::from(format!("/proc/{pid}"));
        (std::fs::read_link(d.join("exe")).ok(), std::fs::read_link(d.join("cwd")).ok())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let Ok(out) = Command::new("lsof")
            .args(["-a", "-p", &pid.to_string(), "-d", "cwd,txt", "-Fn"])
            .output()
        else {
            return (None, None);
        };
        parse_lsof(&String::from_utf8_lossy(&out.stdout))
    }
}

/// Parse `lsof -Fn` output: `fcwd` then `n<path>`, `ftxt` then `n<path>` (first txt is the exe).
#[cfg_attr(target_os = "linux", allow(dead_code))]
fn parse_lsof(text: &str) -> (Option<PathBuf>, Option<PathBuf>) {
    let (mut exe, mut cwd, mut cur) = (None, None, "");
    for line in text.lines() {
        if let Some(f) = line.strip_prefix('f') {
            cur = if f == "cwd" { "cwd" } else if f == "txt" { "txt" } else { "" };
        } else if let Some(n) = line.strip_prefix('n') {
            match cur {
                "cwd" if cwd.is_none() => cwd = Some(PathBuf::from(n)),
                "txt" if exe.is_none() => exe = Some(PathBuf::from(n)),
                _ => {}
            }
        }
    }
    (exe, cwd)
}

/// Query `kernel.status` over a unix socket (blocking, short timeouts).
#[cfg(unix)]
pub fn rpc_status(socket: &Path) -> Option<(String, String)> {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::time::Duration;
    let mut s = UnixStream::connect(socket).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    s.set_write_timeout(Some(Duration::from_secs(2))).ok()?;
    s.write_all(b"{\"method\":\"kernel.status\",\"params\":null,\"auth\":\"read\"}\n").ok()?;
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line).ok()?;
    let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    let b = v.pointer("/result/build")?;
    Some((b.get("version")?.as_str()?.to_string(), b.get("sha").and_then(|s| s.as_str()).unwrap_or("").to_string()))
}

#[cfg(not(unix))]
pub fn rpc_status(_: &Path) -> Option<(String, String)> {
    None
}

/// Find the socket that belongs to `pid`: a runtime dir whose `kernel.pid` names it.
fn socket_for_pid(env: &DoctorEnv, pid: u32, cwd: Option<&Path>) -> Option<PathBuf> {
    let mut dirs = env.runtime_dir_candidates();
    if let Some(c) = cwd {
        dirs.extend(project_runtime_dirs(c));
    }
    dirs.into_iter().find_map(|d| {
        let recorded: u32 = std::fs::read_to_string(d.join(crate::PID_FILE_NAME)).ok()?.trim().parse().ok()?;
        (recorded == pid).then(|| d.join(crate::SOCKET_NAME))
    })
}

/// Discover daemons in the process table.
pub fn discover(env: &DoctorEnv, procs: &ProcTable) -> Vec<DaemonProc> {
    discover_with(env, procs, &resolve_exe_cwd)
}

/// [`discover`] with an injectable exe/cwd resolver (tests).
pub fn discover_with(
    env: &DoctorEnv,
    procs: &ProcTable,
    resolver: &dyn Fn(u32) -> (Option<PathBuf>, Option<PathBuf>),
) -> Vec<DaemonProc> {
    let me = std::process::id();
    procs
        .rows
        .iter()
        .filter(|r| r.pid != me)
        .filter_map(|r| daemon_kind(&r.command).map(|k| (r, k)))
        .map(|(r, kind)| {
            let (exe, cwd) = resolver(r.pid);
            let socket = socket_for_pid(env, r.pid, cwd.as_deref());
            let mut version = None;
            let mut source = None;
            if let Some((v, sha)) = socket.as_deref().and_then(rpc_status) {
                let dirty = sha.ends_with("-dirty");
                version = Some((v, dirty));
                source = Some("rpc");
            } else if let Some(i) = exe
                .as_deref()
                .filter(|_| kind == "weaver")
                .and_then(|e| probe_binary(e, env.probe_timeout, env.probe_scripts).0)
            {
                version = Some((i.version, i.dirty));
                source = Some("exe --version");
            }
            DaemonProc {
                pid: r.pid,
                command: if kind == "weaver" { "weaver kernel start".into() } else { "weftos boot".into() },
                exe_sha256: exe.as_deref().and_then(sha256_file),
                dirty: version.as_ref().is_some_and(|v| v.1),
                version: version.map(|v| v.0),
                version_source: source,
                exe,
                socket,
            }
        })
        .collect()
}

/// Findings for the discovered daemons.
pub fn findings(env: &DoctorEnv, copies: &[BinCopy], daemons: &[DaemonProc], self_version: &str) -> Vec<Finding> {
    let c = Component::Daemon;
    if daemons.is_empty() {
        return vec![Finding::new(c, "daemon:none", Severity::Ok, "no kernel daemon process running")];
    }
    let me = parse_version(self_version);
    let mut out = Vec::new();
    for d in daemons {
        let ver = match (&d.version, d.dirty) {
            (Some(v), true) => format!("{v}-dirty"),
            (Some(v), false) => v.clone(),
            (None, _) => "unknown version".into(),
        };
        let exe = d.exe.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "unknown exe".into());
        out.push(Finding::new(c, format!("process:{}", d.pid), Severity::Ok, format!("pid {} {ver} from {exe}", d.pid)));
        if let Some(exe_path) = &d.exe
            && let Some(f) = binary_finding(env, copies, d, exe_path)
        {
            out.push(f);
        }
        if let (Some(dv), Some(m)) = (&d.version, &me)
            && *dv != m.version
        {
            out.push(
                Finding::new(c, format!("skew:{}", d.pid), Severity::Warn, format!("CLI {} but daemon pid {} runs {dv}", m.version, d.pid))
                    .remedy("update the older side, then restart the daemon: weaver kernel stop && weaver kernel start"),
            );
        }
    }
    out
}

fn binary_finding(env: &DoctorEnv, copies: &[BinCopy], d: &DaemonProc, exe: &Path) -> Option<Finding> {
    let canon = std::fs::canonicalize(exe).unwrap_or_else(|_| exe.to_path_buf());
    let name = exe.file_name()?.to_str()?;
    let known = copies.iter().any(|x| x.canonical == canon)
        || env.scan_dirs().iter().any(|dir| canon.parent() == Some(dir.as_path()) || exe.parent() == Some(dir.as_path()));
    let winner = copies.iter().find(|x| x.name == name && x.winner);
    let mut reasons = Vec::new();
    if !known {
        reasons.push("outside any known install location".to_string());
    }
    if let Some(w) = winner.filter(|w| w.canonical != canon) {
        let same = w.sha256.is_some() && w.sha256 == d.exe_sha256;
        reasons.push(format!("differs from PATH winner {}{}", w.path.display(), if same { " (identical bytes)" } else { "" }));
    }
    if reasons.is_empty() {
        return None;
    }
    Some(
        Finding::new(Component::Daemon, format!("binary:{}", d.pid), Severity::Warn, format!("daemon pid {} exe {}", d.pid, reasons.join("; ")))
            .remedy("restart it from the installed binary (from the project that owns it): weaver kernel stop && weaver kernel start"),
    )
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::doctor::env::test_env;

    #[test]
    fn detects_only_kernel_start_processes() {
        assert_eq!(daemon_kind("./bin/weaver -v kernel start --foreground"), Some("weaver"));
        assert_eq!(daemon_kind("/usr/local/bin/weaver kernel status"), None);
        assert_eq!(daemon_kind("grep weaver kernel start"), None);
        assert_eq!(daemon_kind("/x/weftos boot"), Some("weftos"));
        assert_eq!(daemon_kind("/x/weaver kernel --config /x.toml start --foreground"), Some("weaver"));
        assert_eq!(daemon_kind("weaver -v kernel -c /x.toml start"), Some("weaver"));
        assert_eq!(daemon_kind("weaver kernel --config=/x.toml start"), Some("weaver"));
        assert_eq!(daemon_kind("weaver kernel --config /x.toml status"), None);
        // `kernel` and `start` merely appearing in argv is not a daemon.
        assert_eq!(daemon_kind("weaver ask how to start the kernel"), None);
        assert_eq!(daemon_kind("weaver kernel status start"), None);
        assert_eq!(daemon_kind("weaver chat kernel start"), None);
        assert_eq!(daemon_kind("weftos status boot-log"), None);
    }

    #[test]
    fn json_never_carries_argv_secrets() {
        let d = tempfile::tempdir().unwrap();
        let mut env = test_env(d.path());
        env.ps_override = Some("4242 weaver kernel start --token SECRET123 --api-key sk-abc\n".into());
        let procs = ProcTable::load(&env);
        let daemons = discover_with(&env, &procs, &|_| (None, None));
        assert_eq!(daemons.len(), 1);
        let j = serde_json::to_string(&daemons).unwrap();
        assert!(!j.contains("SECRET123") && !j.contains("sk-abc"), "{j}");
        assert_eq!(daemons[0].command, "weaver kernel start");
    }

    #[test]
    fn liveness_uses_the_table_and_ps_failure_is_flagged() {
        let d = tempfile::tempdir().unwrap();
        let mut env = test_env(d.path());
        env.ps_override = Some("777 weaver kernel start\n".into());
        assert_eq!(pid_liveness(&env, 777), Liveness::Alive);
        assert_eq!(pid_liveness(&env, 778), Liveness::Dead);
        // A real `ps` result of nothing is not "no processes": ok=false.
        let t = ProcTable { rows: vec![], ok: false };
        assert!(!t.ok);
        assert!(ProcTable::parse("1 init").ok);
    }

    #[test]
    fn parses_ps_and_lsof() {
        let t = ProcTable::parse("  70730 ./bin/weaver -v kernel start --foreground\n    1 /sbin/launchd\n");
        assert_eq!(t.rows.len(), 2);
        assert!(t.alive(70730) && !t.alive(2));
        let (exe, cwd) = parse_lsof("p1\nfcwd\nn/proj\nftxt\nn/proj/bin/weaver\nftxt\nn/lib/dyld\n");
        assert_eq!(exe.unwrap(), Path::new("/proj/bin/weaver"));
        assert_eq!(cwd.unwrap(), Path::new("/proj"));
    }

    #[test]
    fn daemon_outside_install_locations_warns_and_version_skews() {
        let d = tempfile::tempdir().unwrap();
        let mut env = test_env(d.path());
        let other = d.path().join("other/bin");
        std::fs::create_dir_all(&other).unwrap();
        let exe = other.join("weaver");
        std::fs::write(&exe, "#!/bin/sh\necho 'weaver 0.8.1 (2cd752e1-dirty 2026-09-28T14:25Z)'\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        env.ps_override = Some("4242 ./bin/weaver -v kernel start --foreground\n".into());
        let procs = ProcTable::load(&env);
        let e2 = exe.clone();
        let daemons = discover_with(&env, &procs, &move |_| (Some(e2.clone()), None));
        assert_eq!(daemons.len(), 1);
        assert_eq!(daemons[0].version.as_deref(), Some("0.8.1"));
        assert!(daemons[0].dirty);
        let f = findings(&env, &[], &daemons, "0.8.0 (dec3b28f-dirty)");
        assert!(f.iter().any(|x| x.id == "binary:4242" && x.message.contains("outside any known install location")));
        assert!(f.iter().any(|x| x.id == "skew:4242" && x.severity == Severity::Warn));
    }

    #[test]
    fn rpc_status_over_a_real_socket_ties_pid_to_version() {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixListener;
        let d = tempfile::tempdir().unwrap();
        let mut env = test_env(d.path());
        let rt = d.path().join("rt");
        std::fs::create_dir_all(&rt).unwrap();
        std::fs::write(rt.join(crate::PID_FILE_NAME), "4243\n").unwrap();
        env.runtime_dir = rt.clone();
        let l = UnixListener::bind(rt.join(crate::SOCKET_NAME)).unwrap();
        std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut line = String::new();
            BufReader::new(s.try_clone().unwrap()).read_line(&mut line).unwrap();
            s.write_all(b"{\"ok\":true,\"result\":{\"build\":{\"version\":\"0.8.1\",\"sha\":\"abc\"}}}\n").unwrap();
        });
        env.ps_override = Some("4243 weaver kernel start\n".into());
        let procs = ProcTable::load(&env);
        let daemons = discover_with(&env, &procs, &|_| (None, None));
        assert_eq!(daemons[0].version.as_deref(), Some("0.8.1"));
        assert_eq!(daemons[0].version_source, Some("rpc"));
    }

    #[test]
    fn no_daemon_is_a_pass() {
        let d = tempfile::tempdir().unwrap();
        let env = test_env(d.path());
        let f = findings(&env, &[], &[], "0.8.1");
        assert_eq!(f[0].severity, Severity::Ok);
    }
}
