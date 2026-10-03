//! `runtime` checks for per-project child kernels (ADR-103 A6, Phase 2
//! package G): failed children, children running without a user daemon,
//! run dirs whose `kernel.pid` names something that is not a project kernel,
//! and project-rooted (legacy) daemons that run beside the user daemon.
//!
//! Read-only: nothing here signals a process or changes a file. A child that
//! cannot be verified is reported and left alone; adopting or stopping it is
//! the user daemon's job (it re-verifies pid, exe, lock and handshake first).

use std::path::Path;

use clawft_types::project::{ChildState, ServeVia, list_manifests, validate_id};
use clawft_types::runtime_paths::{PID_FILE_NAME, STATE_JSON_FILE, user_runtime_root};

use super::daemon::{ProcTable, daemon_kind};
use super::env::DoctorEnv;
use super::{Component, Finding, Severity};

/// Is `command` a project kernel started by the supervisor for `id`?
pub fn is_child_command(command: &str, id: &str) -> bool {
    daemon_kind(command).is_some()
        && command.split_whitespace().any(|a| a == "project")
        && command.contains("--profile")
        && command.contains(id)
}

fn read_pid(p: &Path) -> Option<u32> {
    std::fs::read_to_string(p).ok()?.trim().parse().ok()
}

fn failed_reason(run_dir: &Path) -> Option<String> {
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(run_dir.join(STATE_JSON_FILE)).ok()?).ok()?;
    let state: ChildState = serde_json::from_value(v.get("state")?.clone()).ok()?;
    (state == ChildState::Failed).then(|| v["failed_reason"].as_str().unwrap_or("restart budget spent").to_owned())
}

/// What the supervisor says about one child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildView {
    /// State machine position (`running`, `starting`, `idle-stopping`, ...).
    pub state: String,
    /// The pid it supervises.
    pub pid: Option<u32>,
    /// The kernel runs another build than the user daemon.
    pub stale_build: bool,
    /// Build stamp the kernel reported, when it did.
    pub kernel_sha: Option<String>,
    /// Seconds an adopted child has not registered with the daemon.
    pub unregistered_secs: Option<u64>,
}

/// An adopted child silent this long (no registered session) is reported:
/// the supervisor never restarts it for a lost heartbeat.
const UNREGISTERED_WARN_SECS: u64 = 120;

/// The raw probe request. It bypasses `stamp_request`, so it carries `proto`
/// itself: the daemon refuses a no-proto request for anything not read-only.
fn status_request() -> crate::Request {
    let mut req = crate::Request::with_params("project.status", serde_json::json!({})).with_auth("admin");
    req.proto = Some(crate::PROTO_VERSION);
    req
}

/// What the user daemon's supervisor says about its children
/// (`project.status`): project id -> [`ChildView`]. `None` when the daemon
/// cannot be reached or does not answer within two seconds, so the process
/// table alone never vouches for a child.
pub fn supervisor_view(run_root: &Path) -> Option<std::collections::HashMap<String, ChildView>> {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::time::Duration;
    let mut s = std::os::unix::net::UnixStream::connect(run_root.join(crate::SOCKET_NAME)).ok()?;
    let t = Some(Duration::from_secs(2));
    let _ = s.set_write_timeout(t);
    let _ = s.set_read_timeout(t);
    writeln!(s, "{}", serde_json::to_string(&status_request()).ok()?).ok()?;
    let mut line = String::new();
    BufReader::new(s.take(1 << 20)).read_line(&mut line).ok()?;
    let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    let kids = v.get("result")?.get("children")?.as_array()?;
    Some(
        kids.iter()
            .filter_map(|k| {
                Some((
                    k.get("project_id")?.as_str()?.to_owned(),
                    ChildView {
                        state: k.get("state")?.as_str()?.to_owned(),
                        pid: k.get("pid").and_then(|p| p.as_u64()).map(|p| p as u32),
                        stale_build: k.get("stale_build").and_then(|b| b.as_bool()).unwrap_or(false),
                        kernel_sha: k.get("kernel_sha").and_then(|b| b.as_str()).map(str::to_owned),
                        unregistered_secs: k.get("unregistered_secs").and_then(|b| b.as_u64()),
                    },
                ))
            })
            .collect(),
    )
}

/// All child-kernel findings for `env.home`.
pub fn check(env: &DoctorEnv, procs: &ProcTable) -> Vec<Finding> {
    let c = Component::Runtime;
    // A `WEFTOS_RUNTIME_DIR` override is the user daemon's root (sandboxes
    // and tests); only without one is it `~/.weftos/run`. Never dial the real
    // home's daemon on behalf of an isolated run.
    let run_root = match env.runtime_source {
        crate::doctor::env::RuntimeSource::EnvOverride => env.runtime_dir.clone(),
        _ => user_runtime_root(&env.home),
    };
    let user_daemon_alive = read_pid(&run_root.join(PID_FILE_NAME)).is_some_and(|p| procs.alive(p));
    let supervisor = if user_daemon_alive { supervisor_view(&run_root) } else { None };
    let mut out = Vec::new();
    let mut children = 0usize;
    let mut unconfirmed = 0usize;

    if let Ok(rd) = std::fs::read_dir(&run_root) {
        let mut dirs: Vec<_> = rd
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|e| {
                let id = e.file_name().to_string_lossy().into_owned();
                validate_id(&id).ok().map(|()| (id, e.path()))
            })
            .collect();
        dirs.sort();
        for (id, dir) in dirs {
            if let Some(why) = failed_reason(&dir) {
                out.push(
                    Finding::new(
                        c,
                        format!("child:{id}:failed"),
                        Severity::Warn,
                        format!("project {id}: kernel failed and is not restarted ({why})"),
                    )
                    .remedy(format!(
                        "read {} then run `weaver kernel restart --project {id}`",
                        dir.join("kernel.log").display()
                    )),
                );
            }
            let Some(pid) = read_pid(&dir.join(PID_FILE_NAME)) else { continue };
            if !procs.alive(pid) {
                continue;
            }
            let command = procs
                .rows
                .iter()
                .find(|r| r.pid == pid)
                .map(|r| r.command.as_str())
                .unwrap_or_default();
            if is_child_command(command, &id) {
                if user_daemon_alive {
                    // `starting` (an automatic restart) and `idle-stopping`
                    // are states of a supervised child, not signs of a stray.
                    let supervised = supervisor.as_ref().and_then(|v| v.get(&id)).filter(|c| {
                        c.pid == Some(pid) && matches!(c.state.as_str(), "running" | "starting" | "idle-stopping")
                    });
                    match (&supervisor, supervised) {
                        (Some(_), Some(cv)) => {
                            if cv.state == "running" {
                                children += 1;
                            }
                            if let Some(secs) = cv.unregistered_secs.filter(|s| *s >= UNREGISTERED_WARN_SECS) {
                                out.push(
                                    Finding::new(
                                        c,
                                        format!("child:{id}:unregistered"),
                                        Severity::Warn,
                                        format!(
                                            "project {id}: kernel pid {pid} was adopted but has not registered with the \
                                             user daemon for {secs}s; it shows as running but is never restarted \
                                             for a lost heartbeat (an older build may not re-register)"
                                        ),
                                    )
                                    .remedy(format!("`weaver kernel restart --project {id}` starts it on the current build")),
                                );
                            }
                            if cv.stale_build {
                                out.push(
                                    Finding::new(
                                        c,
                                        format!("child:{id}:stale-build"),
                                        Severity::Warn,
                                        format!(
                                            "project {id}: kernel pid {pid} runs build {} but the user daemon is a \
                                             different build (`weaver update` keeps project kernels running)",
                                            cv.kernel_sha.as_deref().map_or("?", |s| &s[..s.len().min(12)])
                                        ),
                                    )
                                    .remedy(format!("`weaver kernel restart --project {id}` starts it on the current build")),
                                );
                            }
                        }
                        (Some(_), None) => out.push(
                            Finding::new(
                                c,
                                format!("child:{id}:unsupervised"),
                                Severity::Warn,
                                format!(
                                    "project {id}: kernel pid {pid} runs but the user daemon's supervisor does not \
                                     list it as running (a process in the table is not a supervised child)"
                                ),
                            )
                            .remedy(format!("`weaver kernel start --project {id}` adopts a verified child; or restart the user daemon")),
                        ),
                        (None, _) => unconfirmed += 1,
                    }
                }
                if !user_daemon_alive {
                    out.push(
                        Finding::new(
                            c,
                            format!("child:{id}:orphan"),
                            Severity::Warn,
                            format!("project {id}: kernel pid {pid} runs without a user daemon (orphaned child)"),
                        )
                        .remedy("run `weaver kernel start --profile user`: it verifies and adopts the child"),
                    );
                }
            } else {
                out.push(
                    Finding::new(
                        c,
                        format!("child:{id}:unverified"),
                        Severity::Warn,
                        format!(
                            "project {id}: {} names live pid {pid}, which is not a project kernel \
                             (recycled pid?); it is not adopted and will never be signalled",
                            dir.join(PID_FILE_NAME).display()
                        ),
                    )
                    .remedy(format!("inspect with `ps -p {pid}`; if stale, remove the pid file by hand")),
                );
            }
        }
    }
    if children > 0 && user_daemon_alive {
        out.push(Finding::new(
            c,
            "children",
            Severity::Ok,
            format!("{children} project kernel(s) running under the user daemon (confirmed by its supervisor)"),
        ));
    }
    if unconfirmed > 0 {
        out.push(
            Finding::new(
                c,
                "children:unconfirmed",
                Severity::Warn,
                format!(
                    "{unconfirmed} project kernel(s) are in the process table but the user daemon \
                     did not answer `project.status`, so they are not confirmed as supervised"
                ),
            )
            .remedy("`weaver kernel status --profile user`"),
        );
    }

    // Project-rooted (legacy) daemons that run beside the user daemon, or
    // that a project marked `child-kernel` still has.
    if let Ok(listing) = list_manifests(&crate::resolve::manifests_dir(&env.home)) {
        for m in listing.manifests {
            let legacy_dir = m.root.join(".weftos").join("runtime");
            let Some(pid) = read_pid(&legacy_dir.join(PID_FILE_NAME)) else { continue };
            if !procs.alive(pid) {
                continue;
            }
            let child_kernel = m.serve.as_ref().is_some_and(|s| s.via == ServeVia::ChildKernel);
            if child_kernel {
                out.push(
                    Finding::new(
                        c,
                        format!("legacy:{}:child-kernel", m.id),
                        Severity::Warn,
                        format!(
                            "project {} ({}) is set to child-kernel but a project-rooted daemon (pid {pid}) \
                             still runs in {}",
                            m.id,
                            m.name,
                            legacy_dir.display()
                        ),
                    )
                    .remedy(format!(
                        "stop it from {} with its own binary, then `weaver kernel start --project {}`",
                        m.root.display(),
                        m.id
                    )),
                );
            } else if user_daemon_alive {
                out.push(
                    Finding::new(
                        c,
                        format!("legacy:{}", m.id),
                        Severity::Warn,
                        format!(
                            "project {} ({}) runs a project-rooted daemon (pid {pid}) beside the user daemon",
                            m.id, m.name
                        ),
                    )
                    .remedy(format!("`weaver project migrate-kernel {} --dry-run`", m.id)),
                );
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn supervisor_probe_carries_proto_and_is_not_read_only_listed() {
        // project.status is not on the read-only allowlist, so without proto
        // the daemon would refuse it and the child listing would go dark.
        let req = super::status_request();
        assert_eq!(req.proto, Some(crate::PROTO_VERSION));
        assert!(!crate::handshake::is_read_only_method(&req.method));
        let line = serde_json::to_string(&req).unwrap();
        assert!(line.contains("\"proto\":1"), "{line}");
    }

    use super::*;
    use crate::doctor::env::test_env;

    const ID: &str = "01JB8Z3Q0V6X9KQ4M2N7T5R1WD";

    fn setup() -> (tempfile::TempDir, DoctorEnv) {
        // Short path: the supervisor-status tests bind a unix socket here.
        let t = tempfile::Builder::new().prefix("dch").tempdir_in("/tmp").unwrap();
        let env = test_env(t.path());
        std::fs::create_dir_all(user_runtime_root(&env.home).join(ID)).unwrap();
        (t, env)
    }

    fn with_ps(mut env: DoctorEnv, ps: &str) -> (DoctorEnv, ProcTable) {
        env.ps_override = Some(ps.to_owned());
        let procs = ProcTable::load(&env);
        (env, procs)
    }

    fn ids(f: &[Finding]) -> Vec<&str> {
        f.iter().map(|f| f.id.as_str()).collect()
    }

    #[test]
    fn command_matching_is_exact_about_the_child_profile() {
        let cmd = format!("/u/weaver kernel start --foreground --profile project --project {ID}");
        assert!(is_child_command(&cmd, ID));
        assert!(!is_child_command(&cmd, "01JB8Z3Q0V6X9KQ4M2N7T5R1WE"));
        assert!(!is_child_command("/u/weaver kernel start --foreground --profile user", ID));
        assert!(!is_child_command("sleep 60", ID));
    }

    #[test]
    fn failed_child_is_reported_with_its_reason() {
        let (_t, env) = setup();
        let dir = user_runtime_root(&env.home).join(ID);
        std::fs::write(dir.join("state.json"), r#"{"state":"failed","failed_reason":"6 restarts"}"#).unwrap();
        let (env, procs) = with_ps(env, "");
        let f = check(&env, &procs);
        assert_eq!(ids(&f), [format!("child:{ID}:failed")]);
        assert!(f[0].message.contains("6 restarts"));
    }

    #[test]
    fn a_child_without_a_user_daemon_is_an_orphan() {
        let (_t, env) = setup();
        let dir = user_runtime_root(&env.home).join(ID);
        std::fs::write(dir.join("kernel.pid"), "4242").unwrap();
        let ps = format!("4242 /u/weaver kernel start --foreground --profile project --project {ID}\n");
        let (env, procs) = with_ps(env, &ps);
        assert_eq!(ids(&check(&env, &procs)), [format!("child:{ID}:orphan")]);
        // A live user daemon that confirms the child: healthy.
        let root = user_runtime_root(&env.home);
        std::fs::write(root.join("kernel.pid"), "100").unwrap();
        let ps2 = format!("100 /u/weaver kernel start --foreground --profile user\n{ps}");
        let (env, procs) = with_ps(env, &ps2);
        let status = serde_json::json!({"ok": true, "result": {"children": [
            {"project_id": ID, "state": "running", "pid": 4242}], "unverifiable": []}});
        serve_status(&root, status);
        assert_eq!(ids(&check(&env, &procs)), ["children"]);
    }

    /// A one-connection-at-a-time fake user daemon answering `project.status`.
    fn serve_status(run_root: &std::path::Path, reply: serde_json::Value) {
        use std::io::{BufRead, BufReader, Write};
        let l = std::os::unix::net::UnixListener::bind(run_root.join("kernel.sock")).unwrap();
        std::thread::spawn(move || {
            for s in l.incoming().flatten() {
                let mut line = String::new();
                BufReader::new(&s).read_line(&mut line).unwrap_or(0);
                let mut s = s;
                let _ = writeln!(s, "{reply}");
            }
        });
    }

    #[test]
    fn the_process_table_alone_never_vouches_for_a_child() {
        let (_t, env) = setup();
        let root = user_runtime_root(&env.home);
        std::fs::write(root.join(ID).join("kernel.pid"), "4242").unwrap();
        std::fs::write(root.join("kernel.pid"), "100").unwrap();
        let ps = format!(
            "100 /u/weaver kernel start --foreground --profile user\n\
             4242 /u/weaver kernel start --foreground --profile project --project {ID}\n"
        );
        // The user daemon is in the table but does not answer: unconfirmed.
        let (env, procs) = with_ps(env, &ps);
        assert_eq!(ids(&check(&env, &procs)), ["children:unconfirmed"]);
        // It answers and does not list the child (or lists another pid).
        serve_status(&root, serde_json::json!({"ok": true, "result": {"children": [
            {"project_id": ID, "state": "running", "pid": 999}], "unverifiable": []}}));
        assert_eq!(ids(&check(&env, &procs)), [format!("child:{ID}:unsupervised")]);
    }

    #[test]
    fn a_pid_naming_something_else_is_unverified_not_orphaned() {
        let (_t, env) = setup();
        let dir = user_runtime_root(&env.home).join(ID);
        std::fs::write(dir.join("kernel.pid"), "4242").unwrap();
        let (env, procs) = with_ps(env, "4242 sleep 60\n");
        let f = check(&env, &procs);
        assert_eq!(ids(&f), [format!("child:{ID}:unverified")]);
        assert!(f[0].message.contains("never be signalled"));
    }

    #[test]
    fn a_dead_pid_and_a_clean_home_say_nothing() {
        let (_t, env) = setup();
        std::fs::write(user_runtime_root(&env.home).join(ID).join("kernel.pid"), "4242").unwrap();
        let (env, procs) = with_ps(env, "1 init\n");
        assert!(check(&env, &procs).is_empty());
    }

    fn live_user_daemon(env: &DoctorEnv, root: &std::path::Path) -> (DoctorEnv, ProcTable) {
        std::fs::write(root.join(ID).join("kernel.pid"), "4242").unwrap();
        std::fs::write(root.join("kernel.pid"), "100").unwrap();
        let ps = format!(
            "100 /u/weaver kernel start --foreground --profile user\n\
             4242 /u/weaver kernel start --foreground --profile project --project {ID}\n"
        );
        with_ps(env.clone(), &ps)
    }

    #[test]
    fn a_starting_or_idle_stopping_child_is_not_unsupervised() {
        let (_t, env) = setup();
        let root = user_runtime_root(&env.home);
        let (env, procs) = live_user_daemon(&env, &root);
        serve_status(&root, serde_json::json!({"ok": true, "result": {"children": [
            {"project_id": ID, "state": "starting", "pid": 4242}], "unverifiable": []}}));
        // An automatic restart is in flight: no false WARN, and not counted as running.
        assert!(check(&env, &procs).is_empty(), "{:?}", ids(&check(&env, &procs)));
    }

    #[test]
    fn an_adopted_child_that_never_registered_is_reported() {
        let (_t, env) = setup();
        let root = user_runtime_root(&env.home);
        let (env, procs) = live_user_daemon(&env, &root);
        serve_status(&root, serde_json::json!({"ok": true, "result": {"children": [
            {"project_id": ID, "state": "running", "pid": 4242, "unregistered_secs": 600}], "unverifiable": []}}));
        let f = check(&env, &procs);
        assert_eq!(ids(&f), [format!("child:{ID}:unregistered"), "children".to_owned()]);
        assert!(f[0].message.contains("600s") && f[0].message.contains("never restarted"));
        // A short silence is the normal re-register window: no finding.
        let root2 = user_runtime_root(&env.home);
        std::fs::remove_file(root2.join("kernel.sock")).unwrap();
        serve_status(&root2, serde_json::json!({"ok": true, "result": {"children": [
            {"project_id": ID, "state": "running", "pid": 4242, "unregistered_secs": 10}], "unverifiable": []}}));
        assert_eq!(ids(&check(&env, &procs)), ["children"]);
    }

    #[test]
    fn a_child_on_an_older_build_than_the_daemon_is_reported() {
        let (_t, env) = setup();
        let root = user_runtime_root(&env.home);
        let (env, procs) = live_user_daemon(&env, &root);
        serve_status(&root, serde_json::json!({"ok": true, "result": {"children": [
            {"project_id": ID, "state": "running", "pid": 4242,
             "kernel_sha": "0123456789abcdef0123", "stale_build": true}], "unverifiable": []}}));
        let f = check(&env, &procs);
        assert_eq!(ids(&f), [format!("child:{ID}:stale-build"), "children".to_owned()]);
        assert!(f[0].message.contains("0123456789ab") && !f[0].message.contains("0123456789abc"));
        assert!(f[0].remedy.as_deref().is_some_and(|r| r.contains(&format!("restart --project {ID}"))));
    }

    #[test]
    fn a_runtime_dir_override_is_the_run_root_and_the_real_home_is_not_dialled() {
        let t = tempfile::Builder::new().prefix("dch").tempdir_in("/tmp").unwrap();
        let mut env = test_env(t.path());
        // The home's own run root holds a decoy user daemon that would
        // report a healthy child; an isolated run must never ask it.
        let home_root = user_runtime_root(&env.home);
        std::fs::create_dir_all(home_root.join(ID)).unwrap();
        serve_status(&home_root, serde_json::json!({"ok": true, "result": {"children": [
            {"project_id": ID, "state": "running", "pid": 4242}], "unverifiable": []}}));
        let rt = t.path().join("rt");
        std::fs::create_dir_all(rt.join(ID)).unwrap();
        std::fs::write(rt.join(ID).join("kernel.pid"), "4242").unwrap();
        env.runtime_source = crate::doctor::env::RuntimeSource::EnvOverride;
        env.runtime_dir = rt;
        let ps = format!("4242 /u/weaver kernel start --foreground --profile project --project {ID}\n");
        let (env, procs) = with_ps(env, &ps);
        assert_eq!(ids(&check(&env, &procs)), [format!("child:{ID}:orphan")]);
    }
}
