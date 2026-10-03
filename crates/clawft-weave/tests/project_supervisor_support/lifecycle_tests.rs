//! Adoption and spawn-refusal tests for `project_supervisor.rs`.

use std::time::{Duration, Instant};

use clawft_types::project::ChildState;
use clawft_weave::project_supervisor::adopt::{Found, Skip};
use clawft_weave::project_supervisor::child::pid_alive;
use clawft_weave::project_supervisor::{SupError, state};

use super::fixture::{Fixture, wait_until};
use super::rt;

pub fn adoption_after_user_daemon_restart() {
    let fx = Fixture::new();
    fx.behavior("serve");
    // "The user daemon" is a runtime we then drop, cancelling every task it
    // had (monitors included), exactly like the process going away.
    let first = rt();
    let pid = first.block_on(async {
        let sup = fx.supervisor();
        sup.ensure_running(&fx.id).await.unwrap().pid
    });
    first.shutdown_background();
    assert!(pid_alive(pid), "children survive the daemon (AbandonProcessGroup)");
    rt().block_on(async {
        let sup = fx.supervisor();
        let found = sup.adopt_on_boot().await;
        assert_eq!(found, vec![Found::Adopted { id: fx.id.clone(), pid }]);
        assert_eq!(sup.launcher().spawn_count(), 0, "adopted, not respawned");
        let st = sup.status(&fx.id).await;
        assert_eq!((st.state, st.pid), (ChildState::Running, Some(pid)));
        let r = sup.ensure_running(&fx.id).await.unwrap();
        assert!(!r.started && r.pid == pid, "ensure_running finds the adopted child");
        assert_eq!(fx.events("project.kernel.adopted").len(), 1);
        // And it is fully managed again.
        assert!(sup.stop(&fx.id).await.unwrap());
        wait_until("adopted child gone", 5, || !pid_alive(pid)).await;
        assert_eq!(sup.status(&fx.id).await.state, ChildState::Stopped);
    });
}

/// Start a fake kernel by hand in `dir` (not through a supervisor).
pub fn manual_kernel(dir: &std::path::Path, id: &str, mode: &str) -> std::process::Child {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("behavior"), mode).unwrap();
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["kernel", "start", "--foreground", "--profile", "project", "--project", id])
        .env_clear()
        .env("WEFTOS_RUNTIME_DIR", dir)
        .env("WEFTOS_PROJECT_ID", id)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let end = Instant::now() + Duration::from_secs(10);
    while !dir.join("ready").exists() {
        assert!(Instant::now() < end, "manual kernel did not start");
        std::thread::sleep(Duration::from_millis(10));
    }
    child
}

pub fn unverifiable_leftovers() {
    let fx = Fixture::new();
    rt().block_on(async {
        // 1. kernel.pid names an unrelated live process (pid reuse).
        let mut stranger = std::process::Command::new("sleep").arg("60").spawn().unwrap();
        let d1 = fx.run_dir();
        std::fs::create_dir_all(&d1).unwrap();
        std::fs::write(d1.join("kernel.pid"), stranger.id().to_string()).unwrap();
        std::fs::write(d1.join("kernel.lock"), stranger.id().to_string()).unwrap();
        // 2. The right program, but nobody holds kernel.lock.
        let id2 = clawft_types::project::new_id();
        let mut nolock = manual_kernel(&fx.run_root.join(&id2), &id2, "nolock");
        // 3. Lock held, but the handshake names another project.
        let id3 = clawft_types::project::new_id();
        let mut wrong = manual_kernel(&fx.run_root.join(&id3), &id3, "wrong-project");

        let sup = fx.supervisor();
        let mut found = sup.adopt_on_boot().await;
        found.sort_by_key(|f| match f {
            Found::Adopted { id, .. } | Found::Unverifiable { id, .. } => id.clone(),
        });
        let reasons: std::collections::HashMap<String, Skip> = found
            .iter()
            .map(|f| match f {
                Found::Unverifiable { id, reason, .. } => (id.clone(), reason.clone()),
                Found::Adopted { id, .. } => panic!("adopted {id}"),
            })
            .collect();
        assert!(matches!(reasons[&fx.id], Skip::WrongExe { .. }), "{:?}", reasons[&fx.id]);
        assert_eq!(reasons[&id2], Skip::LockNotHeld);
        assert!(matches!(reasons[&id3], Skip::HandshakeFailed(_)), "{:?}", reasons[&id3]);
        // Never signalled: all three are still alive, and nothing is
        // supervised.
        for p in [stranger.id(), nolock.id(), wrong.id()] {
            assert!(pid_alive(p), "pid {p} was signalled");
        }
        assert!(sup.status_all().await.is_empty());
        assert_eq!(sup.unverifiable().len(), 3);
        for c in [&mut stranger, &mut nolock, &mut wrong] {
            let _ = c.kill();
            let _ = c.wait();
        }
    });
}

pub fn spawn_refusal_cases() {
    let fx = Fixture::new();
    fx.behavior("serve");
    rt().block_on(async {
        let sup = fx.supervisor();
        let refused = |e: SupError| e.kind();
        // Unknown id, malformed id.
        let unknown = clawft_types::project::new_id();
        assert_eq!(refused(sup.ensure_running(&unknown).await.unwrap_err()), "project_not_found");
        assert_eq!(refused(sup.ensure_running("not-a-ulid").await.unwrap_err()), "invalid_params");
        // The project.toml id differs from the manifest.
        let ptoml = fx.root.join(".weftos/project.toml");
        let original = std::fs::read_to_string(&ptoml).unwrap();
        std::fs::write(&ptoml, original.replace(&fx.id, &unknown)).unwrap();
        assert_eq!(refused(sup.ensure_running(&fx.id).await.unwrap_err()), "project_id_mismatch");
        std::fs::remove_file(&ptoml).unwrap();
        assert_eq!(refused(sup.ensure_running(&fx.id).await.unwrap_err()), "project_id_mismatch");
        std::fs::write(&ptoml, &original).unwrap();
        // A legacy project-rooted daemon holds the project's own kernel.lock.
        let legacy = clawft_types::runtime_paths::RuntimePaths::at(fx.root.join(".weftos/runtime"));
        let _held = clawft_weave::instance_lock::InstanceLock::acquire(&legacy).unwrap();
        assert_eq!(refused(sup.ensure_running(&fx.id).await.unwrap_err()), "legacy_daemon_running");
        drop(_held);
        // The root is $HOME.
        let mut m = fx.manifest();
        let real_root = m.root.clone();
        m.root = fx.home.clone();
        clawft_types::project::write_manifest(&fx.mdir, &m).unwrap();
        assert_eq!(refused(sup.ensure_running(&fx.id).await.unwrap_err()), "root_is_home");
        // The root is gone.
        m.root = fx.home.join("vanished");
        clawft_types::project::write_manifest(&fx.mdir, &m).unwrap();
        assert_eq!(refused(sup.ensure_running(&fx.id).await.unwrap_err()), "root_missing");
        m.root = real_root;
        clawft_types::project::write_manifest(&fx.mdir, &m).unwrap();
        // A run root so deep the child's socket cannot bind.
        let mut deep = fx.cfg();
        deep.run_root = fx.tmp.path().join("d".repeat(90)).join("run");
        let deep = clawft_weave::project_supervisor::Supervisor::new(deep, fx.deps());
        assert_eq!(refused(deep.ensure_running(&fx.id).await.unwrap_err()), "socket_path_too_long");
        // A revoked key with no certificate in force.
        fx.mark_revoked();
        assert_eq!(refused(sup.ensure_running(&fx.id).await.unwrap_err()), "project_revoked");
        std::fs::remove_file(fx.run_dir().join("revoked")).unwrap();
        // No governance engine: no parent policy, no child.
        let mut deps = fx.deps();
        deps.snapshot = std::sync::Arc::new(|| None);
        let bare = clawft_weave::project_supervisor::Supervisor::new(fx.cfg(), deps);
        assert_eq!(refused(bare.ensure_running(&fx.id).await.unwrap_err()), "admission_refused");
        assert_eq!(sup.launcher().spawn_count() + bare.launcher().spawn_count(), 0, "nothing was started");
        // And with everything right it starts.
        assert!(sup.ensure_running(&fx.id).await.unwrap().started);
        sup.stop(&fx.id).await.unwrap();
    });
}


/// After a SIGHUP re-exec the daemon is still the parent of its children but
/// has no waiter for them: an adopted child that dies must read as dead, not
/// as a live zombie.
pub fn adopted_zombie_counts_as_dead() {
    use clawft_kernel::workload_runtime::{ChildLauncher, ChildProbe};
    let fx = Fixture::new();
    rt().block_on(async {
        let sup = fx.supervisor();
        // A child of THIS process that nobody waits for.
        let mut zombie = std::process::Command::new("sleep").arg("60").spawn().unwrap();
        let pid = zombie.id();
        std::fs::create_dir_all(fx.run_dir()).unwrap();
        std::fs::write(fx.run_dir().join("kernel.pid"), pid.to_string()).unwrap();
        sup.launcher().adopt(&fx.id, pid);
        assert_eq!(sup.launcher().probe(&fx.id).await, ChildProbe::Running { pid });
        zombie.kill().unwrap(); // dies; stays a zombie until reaped
        wait_until("zombie reads as dead", 5, || sup.launcher().pid_of(&fx.id).is_none()).await;
        assert!(matches!(sup.launcher().probe(&fx.id).await, ChildProbe::Exited { .. }));
        let info = sup.launcher().wait_exit(&fx.id).await;
        assert!(!info.clean(), "kernel.pid was left behind: a crash, not a clean stop");
        // A clean shutdown removes kernel.pid: that reads as clean.
        std::fs::remove_file(fx.run_dir().join("kernel.pid")).unwrap();
        assert!(sup.launcher().wait_exit(&fx.id).await.clean());
        let _ = zombie.wait();
    });
}

/// The user daemon restarts while a project is revoked: the verified child is
/// stopped at adoption (not left silently running) and listed.
pub fn revoked_child_is_stopped_at_adoption() {
    let fx = Fixture::new();
    fx.behavior("serve");
    let first = rt();
    let pid = first.block_on(async { fx.supervisor().ensure_running(&fx.id).await.unwrap().pid });
    first.shutdown_background();
    fx.mark_revoked();
    rt().block_on(async {
        let sup = fx.supervisor();
        let found = sup.adopt_on_boot().await;
        match &found[..] {
            [Found::Unverifiable { pid: Some(p), reason: Skip::Refused(m), .. }] => {
                assert_eq!(*p, pid);
                assert!(m.contains("revoked") && m.contains("was stopped"), "{m}");
            }
            other => panic!("{other:?}"),
        }
        wait_until("revoked child stopped", 5, || !pid_alive(pid)).await;
        assert_eq!(sup.unverifiable().len(), 1, "listed for doctor and status");
        assert!(sup.status_all().await.iter().all(|s| s.pid.is_none()), "nothing is running or supervised");
        assert_eq!(sup.ensure_running(&fx.id).await.unwrap_err().kind(), "project_revoked");
    });
}

/// A verified child nobody supervises is taken over by ensure_running, never
/// duplicated.
pub fn ensure_running_takes_over_a_live_verified_child() {
    let fx = Fixture::new();
    fx.behavior("serve");
    let first = rt();
    let pid = first.block_on(async { fx.supervisor().ensure_running(&fx.id).await.unwrap().pid });
    first.shutdown_background();
    rt().block_on(async {
        let sup = fx.supervisor(); // no adopt_on_boot
        let r = sup.ensure_running(&fx.id).await.unwrap();
        assert!(!r.started && r.pid == pid);
        assert_eq!(sup.launcher().spawn_count(), 0);
        sup.stop(&fx.id).await.unwrap();
    });
    // A live process that holds the lock but does not answer as the project
    // cannot be adopted and a second kernel must not start beside it.
    let fx = Fixture::new();
    let mut squatter = manual_kernel(&fx.run_dir(), &fx.id, "wrong-project");
    rt().block_on(async {
        let sup = fx.supervisor();
        let e = sup.ensure_running(&fx.id).await.unwrap_err();
        assert_eq!(e.kind(), "leftover_kernel", "{e}");
        assert_eq!(sup.launcher().spawn_count(), 0);
    });
    let _ = squatter.kill();
    let _ = squatter.wait();
}

/// A gate that allows everything the supervisor does except stopping.
struct DenyStop(clawft_kernel::workload_governance::WorkloadGate);

impl clawft_kernel::gate::GateBackend for DenyStop {
    fn check(&self, agent: &str, action: &str, ctx: &serde_json::Value) -> clawft_kernel::gate::GateDecision {
        if action == "workload.stop" {
            return clawft_kernel::gate::GateDecision::Deny { reason: "stop denied by test".into(), receipt: None };
        }
        self.0.check(agent, action, ctx)
    }
}

/// Revoke with a governance gate that refuses the stop: credentials die
/// first, the signal path still stops the child, the project is `failed`.
pub fn revoke_stops_the_child_even_when_the_gated_stop_fails() {
    use clawft_kernel::workload_governance::{WorkloadGate, project_supervisor_permit};
    let fx = Fixture::new();
    fx.behavior("serve");
    rt().block_on(async {
        let mut deps = fx.deps();
        let gate = WorkloadGate::exempt(0.95, false, "test").with_permit(project_supervisor_permit()).unwrap();
        deps.gate = Some(std::sync::Arc::new(DenyStop(gate)));
        let sup = clawft_weave::project_supervisor::Supervisor::new(fx.cfg(), deps);
        let r = sup.ensure_running(&fx.id).await.unwrap();
        let token = super::fixture::seen_spawn(&fx.run_dir()).project_token.unwrap();
        assert!(fx.tokens.validate(&token).is_some());
        fx.mark_revoked();
        sup.revoked(&fx.id, "project.revoke").await;
        assert!(fx.tokens.validate(&token).is_none(), "credentials are dead");
        wait_until("child stopped by the fallback", 5, || !pid_alive(r.pid)).await;
        let st = sup.status(&fx.id).await;
        assert_eq!(st.state, ChildState::Failed);
        assert!(st.failed_reason.unwrap().contains("revoked"));
        assert!(!fx.run_dir().join("spawn.json").exists());
        assert_eq!(sup.ensure_running(&fx.id).await.unwrap_err().kind(), "project_revoked");
    });
}

/// A failed spawn leaves no token, no spawn file and no expectation.
pub fn a_failed_spawn_leaves_nothing_behind() {
    let fx = Fixture::new();
    rt().block_on(async {
        let mut cfg = fx.cfg();
        cfg.exe = fx.tmp.path().join("no-such-kernel");
        let sup = clawft_weave::project_supervisor::Supervisor::new(cfg, fx.deps());
        let e = sup.ensure_running(&fx.id).await.unwrap_err();
        assert_eq!(e.kind(), "project_start_failed", "{e}");
        assert!(fx.tokens.list().is_empty(), "the issued token was revoked");
        assert!(!fx.run_dir().join("spawn.json").exists());
        assert!(!clawft_weave::mesh_local_registry::spawn_expected(&fx.id, state::now_unix()));
    });
}

/// stop deletes spawn.json (the real child consumes it at boot; the fake
/// leaves it), and a child answering with another pid is not "ready".
pub fn stop_removes_spawn_json_and_a_foreign_pid_is_not_ready() {
    let fx = Fixture::new();
    fx.behavior("serve");
    rt().block_on(async {
        let sup = fx.supervisor();
        sup.ensure_running(&fx.id).await.unwrap();
        assert!(fx.run_dir().join("spawn.json").exists());
        sup.stop(&fx.id).await.unwrap();
        assert!(!fx.run_dir().join("spawn.json").exists());
    });
    let fx = Fixture::new();
    fx.behavior("wrong-pid");
    rt().block_on(async {
        let mut cfg = fx.cfg();
        cfg.ready_timeout = Duration::from_millis(400);
        let sup = clawft_weave::project_supervisor::Supervisor::new(cfg, fx.deps());
        let e = sup.ensure_running(&fx.id).await.unwrap_err();
        assert_eq!(e.kind(), "project_not_ready", "{e}");
        let _ = sup.stop(&fx.id).await;
    });
}
