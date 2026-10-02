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
fn manual_kernel(dir: &std::path::Path, id: &str, mode: &str) -> std::process::Child {
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
        state::mark_revoked(&fx.run_dir(), "test").unwrap();
        assert_eq!(refused(sup.ensure_running(&fx.id).await.unwrap_err()), "project_revoked");
        state::clear_revoked(&fx.run_dir());
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

