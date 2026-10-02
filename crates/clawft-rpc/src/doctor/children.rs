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

/// All child-kernel findings for `env.home`.
pub fn check(env: &DoctorEnv, procs: &ProcTable) -> Vec<Finding> {
    let c = Component::Runtime;
    let run_root = user_runtime_root(&env.home);
    let user_daemon_alive = read_pid(&run_root.join(PID_FILE_NAME)).is_some_and(|p| procs.alive(p));
    let mut out = Vec::new();
    let mut children = 0usize;

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
                children += 1;
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
            format!("{children} project kernel(s) running under the user daemon"),
        ));
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
    use super::*;
    use crate::doctor::env::test_env;

    const ID: &str = "01JB8Z3Q0V6X9KQ4M2N7T5R1WD";

    fn setup() -> (tempfile::TempDir, DoctorEnv) {
        let t = tempfile::tempdir().unwrap();
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
        // With a live user daemon the same child is healthy.
        std::fs::write(user_runtime_root(&env.home).join("kernel.pid"), "100").unwrap();
        let (env, procs) = with_ps(env, &format!("100 /u/weaver kernel start --foreground --profile user\n{ps}"));
        assert_eq!(ids(&check(&env, &procs)), ["children"]);
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
}
