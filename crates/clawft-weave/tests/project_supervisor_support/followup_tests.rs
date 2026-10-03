//! Review follow-ups (ADR-103 Phase 2: S5-S8, cards 7e8d7350 and d302d81c)
//! for `project_supervisor.rs`: stale builds, readiness windows, a child
//! still booting at adoption, a lost heartbeat, a revoked project without
//! its marker, a recycled pid between `SIGTERM` and `SIGKILL`, and the
//! unmanaged-leftover report.

use std::sync::Arc;
use std::time::{Duration, Instant};

use clawft_types::project::ChildState;
use clawft_weave::project_supervisor::adopt::{Found, Skip};
use clawft_weave::project_supervisor::child::pid_alive;
use clawft_weave::project_supervisor::{SupError, Supervisor};
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;

use super::fixture::{Fixture, wait_until};
use super::lifecycle_tests::manual_kernel;
use super::rt;

fn supervisor_on_build(fx: &Fixture, sha: &str) -> Arc<Supervisor> {
    let mut cfg = fx.cfg();
    cfg.build_sha = sha.to_owned();
    Supervisor::new(cfg, fx.deps())
}

/// S6: `weaver update` replaces the daemon and keeps the children running.
/// The new daemon adopts a child that still runs the old build and says so;
/// a restart brings it onto the current one.
pub fn a_child_on_an_older_build_than_the_daemon_is_reported_stale() {
    let fx = Fixture::new();
    fx.behavior("serve");
    std::fs::write(fx.run_dir().join("sha"), "oldbuild00001").unwrap();
    let first = rt();
    let pid = first.block_on(async {
        let sup = supervisor_on_build(&fx, "oldbuild00001");
        let r = sup.ensure_running(&fx.id).await.unwrap();
        let st = sup.status(&fx.id).await;
        assert!(!st.stale_build, "same build as its daemon");
        assert_eq!(st.kernel_sha.as_deref(), Some("oldbuild00001"));
        r.pid
    });
    first.shutdown_background();
    rt().block_on(async {
        let sup = supervisor_on_build(&fx, "newbuild00002");
        assert_eq!(sup.adopt_on_boot().await, vec![Found::Adopted { id: fx.id.clone(), pid }]);
        let st = sup.status(&fx.id).await;
        assert!(st.stale_build, "adopted child runs the old binary");
        assert_eq!(st.kernel_sha.as_deref(), Some("oldbuild00001"));
        let j = st.to_json();
        assert_eq!((j["stale_build"].clone(), j["kernel_sha"].clone()), (true.into(), "oldbuild00001".into()));
        // The restart the finding recommends: the new child is on the current build.
        std::fs::write(fx.run_dir().join("sha"), "newbuild00002").unwrap();
        sup.restart(&fx.id).await.unwrap();
        let st = sup.status(&fx.id).await;
        assert_eq!((st.stale_build, st.kernel_sha.as_deref()), (false, Some("newbuild00002")));
        // A stopped child is not "stale": there is nothing running.
        std::fs::write(fx.run_dir().join("sha"), "oldbuild00001").unwrap();
        sup.stop(&fx.id).await.unwrap();
        assert!(!sup.status(&fx.id).await.stale_build);
    });
}

/// S7a: during an automatic restart the child has a pid but has not bound its
/// socket. `ensure_running` must wait for it, never hand out the unbound one.
pub fn ensure_running_during_an_automatic_restart_waits_for_the_socket() {
    let fx = Fixture::new();
    fx.behavior("serve");
    rt().block_on(async {
        let sup = fx.supervisor();
        let first = sup.ensure_running(&fx.id).await.unwrap();
        // The replacement binds its socket 1.2 s after it starts; the dead
        // child's stale socket file stays behind (nothing listens on it).
        std::fs::write(fx.run_dir().join("start_delay_ms"), "1200").unwrap();
        kill(Pid::from_raw(first.pid as i32), Signal::SIGKILL).unwrap();
        let end = Instant::now() + Duration::from_secs(10);
        let relaunched = loop {
            let st = sup.status(&fx.id).await;
            if st.state == ChildState::Starting && st.pid.is_some_and(|p| p != first.pid) {
                break st.pid.unwrap();
            }
            assert!(Instant::now() < end, "the supervisor never relaunched the child");
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        assert!(
            std::os::unix::net::UnixStream::connect(&first.socket).is_err(),
            "precondition: the relaunched child has not bound its socket yet"
        );
        let r = sup.ensure_running(&fx.id).await.unwrap();
        assert_eq!((r.started, r.pid), (false, relaunched));
        assert!(
            std::os::unix::net::UnixStream::connect(&r.socket).is_ok(),
            "ensure_running returned a socket nobody listens on"
        );
        assert_eq!(sup.status(&fx.id).await.state, ChildState::Running);
        sup.stop(&fx.id).await.unwrap();
    });
}

/// Start a fake kernel by hand and return once it holds the lock and has
/// written `kernel.pid`, before it binds its socket (`start_delay_ms`).
fn booting_kernel(fx: &Fixture, delay_ms: u64) -> std::process::Child {
    let dir = fx.run_dir();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("start_delay_ms"), delay_ms.to_string()).unwrap();
    std::fs::write(dir.join("behavior"), "serve").unwrap();
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["kernel", "start", "--foreground", "--profile", "project", "--project", &fx.id])
        .env_clear()
        .env("WEFTOS_RUNTIME_DIR", &dir)
        .env("WEFTOS_PROJECT_ID", &fx.id)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let end = Instant::now() + Duration::from_secs(10);
    while !dir.join("kernel.pid").exists() {
        assert!(Instant::now() < end, "booting kernel did not start");
        std::thread::sleep(Duration::from_millis(5));
    }
    child
}

/// S7b: a child that was still booting when the daemon restarted (pid and
/// lock, no socket yet) is adopted when it starts answering; and when the
/// scan gave up on it, the stop cascade looks again instead of leaving it.
pub fn a_child_still_booting_at_adoption_is_adopted_or_found_by_the_stop_cascade() {
    // (a) the adoption scan waits for it.
    let fx = Fixture::new();
    let mut kid = booting_kernel(&fx, 600);
    let pid = kid.id();
    rt().block_on(async {
        let sup = fx.supervisor();
        let found = sup.adopt_on_boot().await;
        assert_eq!(found, vec![Found::Adopted { id: fx.id.clone(), pid }]);
        assert_eq!(sup.status(&fx.id).await.state, ChildState::Running);
        assert_eq!(sup.stop_all().await, vec![fx.id.clone()]);
        wait_until("adopted child gone", 5, || !pid_alive(pid)).await;
    });
    let _ = kid.wait();

    // (b) the scan gave up (short ready timeout); the cascade finds it.
    let fx = Fixture::new();
    let mut kid = booting_kernel(&fx, 700);
    let pid = kid.id();
    rt().block_on(async {
        let mut cfg = fx.cfg();
        cfg.ready_timeout = Duration::from_millis(100);
        let sup = Supervisor::new(cfg, fx.deps());
        let found = sup.adopt_on_boot().await;
        assert!(
            matches!(&found[..], [Found::Unverifiable { reason: Skip::HandshakeFailed(_), pid: Some(p), .. }] if *p == pid),
            "{found:?}"
        );
        assert!(sup.status_all().await.is_empty(), "not supervised yet");
        wait_until("the booting child binds", 5, || fx.run_dir().join("ready").exists()).await;
        let stopped = sup.stop_all().await;
        assert_eq!(stopped, vec![fx.id.clone()], "the cascade did not skip the mid-boot child");
        wait_until("child gone", 5, || !pid_alive(pid)).await;
    });
    let _ = kid.wait();
}

/// S8b: a `running` child whose registry session stays expired (three missed
/// beats, no re-registration) is a crash: restarted inside the budget, then
/// `failed` when the budget is spent. A session that comes back resets the
/// clock.
pub fn a_child_that_lost_its_heartbeat_is_restarted_and_then_failed_when_the_budget_is_spent() {
    let fx = Fixture::new();
    fx.behavior("serve");
    fx.set_serve(|s| s.restart_max = Some(2));
    rt().block_on(async {
        let mut cfg = fx.cfg();
        cfg.lost_heartbeat_grace = Duration::from_millis(100);
        let sup = Supervisor::new(cfg, fx.deps());
        let first = sup.ensure_running(&fx.id).await.unwrap();
        let t0 = Instant::now();
        assert!(sup.liveness_pass(t0).await.is_empty(), "a healthy child is left alone");

        fx.activity.set_lost(true);
        assert!(sup.liveness_pass(t0).await.is_empty(), "the first sighting only starts the clock");
        fx.activity.set_lost(false);
        assert!(sup.liveness_pass(t0 + Duration::from_millis(200)).await.is_empty());
        fx.activity.set_lost(true);
        assert!(
            sup.liveness_pass(t0 + Duration::from_millis(210)).await.is_empty(),
            "a session that came back reset the clock"
        );

        let mut now = t0 + Duration::from_millis(500);
        assert_eq!(sup.liveness_pass(now).await, vec![fx.id.clone()]);
        let st = sup.status(&fx.id).await;
        assert_eq!((st.state, st.restarts), (ChildState::Running, 1));
        assert!(st.pid.is_some_and(|p| p != first.pid) && !pid_alive(first.pid), "the wedged child was replaced");
        assert!(
            fx.events("project.kernel.exited").iter().any(|e| e.payload.as_ref().unwrap()["reason"] == "heartbeat lost")
        );
        assert_eq!(fx.events("project.kernel.restarted").len(), 1);

        // Still lost for ever: the budget (two restarts) runs out, then `failed`.
        for _ in 0..4 {
            if sup.status(&fx.id).await.state != ChildState::Running {
                break;
            }
            now += Duration::from_millis(500);
            sup.liveness_pass(now).await; // starts the clock for the new child
            now += Duration::from_millis(500);
            sup.liveness_pass(now).await;
        }
        let st = sup.status(&fx.id).await;
        assert_eq!(st.state, ChildState::Failed, "{st:?}");
        assert!(st.failed_reason.unwrap().contains("heartbeat lost"));
        assert!(st.pid.is_none());
        assert_eq!(fx.events("project.kernel.failed").len(), 1);
        assert!(
            sup.liveness_pass(now + Duration::from_secs(1)).await.is_empty(),
            "failed children are not touched again"
        );
    });
}

/// A child whose last beat said it was busy is spared the short grace but not
/// for ever: it is restarted after `lost_heartbeat_busy_ceiling`.
pub fn a_child_wedged_while_busy_is_restarted_after_the_ceiling() {
    let fx = Fixture::new();
    fx.behavior("serve");
    fx.set_serve(|s| s.lost_heartbeat_busy_ceiling_secs = Some(10));
    rt().block_on(async {
        let mut cfg = fx.cfg();
        cfg.lost_heartbeat_grace = Duration::from_millis(100);
        // The daemon default is far away; the project's own knob wins.
        cfg.lost_heartbeat_busy_ceiling = Duration::from_secs(3600);
        let sup = Supervisor::new(cfg, fx.deps());
        let first = sup.ensure_running(&fx.id).await.unwrap();
        let t0 = Instant::now();
        fx.activity.set_lost_busy(true);
        assert!(sup.liveness_pass(t0).await.is_empty(), "the first sighting starts the clock");
        assert!(
            sup.liveness_pass(t0 + Duration::from_secs(5)).await.is_empty(),
            "far beyond the plain grace, inside the busy ceiling: spared"
        );
        assert_eq!(sup.status(&fx.id).await.pid, Some(first.pid));
        assert_eq!(sup.liveness_pass(t0 + Duration::from_secs(11)).await, vec![fx.id.clone()]);
        let st = sup.status(&fx.id).await;
        assert!(st.pid.is_some_and(|p| p != first.pid) && !pid_alive(first.pid), "the wedged child was replaced");
        assert!(fx.events("project.kernel.exited").iter().any(|e| e.payload.as_ref().unwrap()["reason"] == "heartbeat lost"));
        sup.stop_all().await;
    });
}

/// The plain (non-busy) lost path still restarts at the short grace, with the
/// busy ceiling far away.
pub fn a_plain_lost_child_is_restarted_at_the_short_grace_not_the_busy_ceiling() {
    let fx = Fixture::new();
    fx.behavior("serve");
    rt().block_on(async {
        let mut cfg = fx.cfg();
        cfg.lost_heartbeat_grace = Duration::from_millis(100);
        cfg.lost_heartbeat_busy_ceiling = Duration::from_secs(3600);
        let sup = Supervisor::new(cfg, fx.deps());
        let first = sup.ensure_running(&fx.id).await.unwrap();
        let t0 = Instant::now();
        fx.activity.set_lost(true);
        assert!(sup.liveness_pass(t0).await.is_empty(), "first sighting");
        assert_eq!(sup.liveness_pass(t0 + Duration::from_millis(200)).await, vec![fx.id.clone()]);
        assert!(sup.status(&fx.id).await.pid.is_some_and(|p| p != first.pid));
        sup.stop_all().await;
    });
}

/// An adopted child that never re-registers is not restarted, but it is
/// reported in `status` (and so by doctor), and the report clears when it
/// registers.
pub fn an_adopted_child_that_never_registers_is_reported_in_status() {
    let fx = Fixture::new();
    fx.behavior("serve");
    rt().block_on(async {
        let sup = fx.supervisor();
        sup.ensure_running(&fx.id).await.unwrap();
        assert_eq!(sup.status(&fx.id).await.unregistered_secs, None);
        let t0 = Instant::now();
        fx.activity.set_unregistered(true);
        assert!(sup.liveness_pass(t0).await.is_empty(), "never restarted for being unregistered");
        let st = sup.status(&fx.id).await;
        assert!(st.unregistered_secs.is_some(), "{st:?}");
        assert!(st.to_json()["unregistered_secs"].is_u64());
        assert_eq!(st.state, ChildState::Running);
        fx.activity.set_unregistered(false);
        sup.liveness_pass(t0 + Duration::from_secs(1)).await;
        assert_eq!(sup.status(&fx.id).await.unregistered_secs, None, "it registered");
        sup.stop_all().await;
    });
}

/// Note (b) of card 7e8d7350: a revoked project whose marker is missing (a
/// full disk, a hand-removed file) is refused from the journal before
/// anything is spawned, instead of spawning to die on `project_revoked`.
pub fn a_revoked_project_without_a_marker_is_refused_before_spawning() {
    use clawft_kernel::project_identity as ident;
    use clawft_types::project::cert::{PopOp, key_id};
    use clawft_weave::project_cert_rpc::{
        RegisterRequest, SpawnInfo, claim_nonce, issue_challenge, register, revoke, root_sha256,
    };
    let fx = Fixture::new();
    fx.behavior("serve");
    let env = fx.deps().cert_env;
    let ukid = key_id(&env.user_key.verifying_key().to_bytes());
    let k = ed25519_dalek::SigningKey::from_bytes(&[21u8; 32]);
    let n = issue_challenge(&fx.id).unwrap();
    register(
        &env,
        RegisterRequest {
            project_id: fx.id.clone(),
            project_pubkey: k.verifying_key().to_bytes(),
            root_sha256: root_sha256(&fx.root),
            spawn: SpawnInfo { pid: 1, exe_sha: "ab".repeat(32) },
            pop_sig: ident::pop_sign(&k, PopOp::Register, &ukid, &n, &fx.id).unwrap(),
            nonce: claim_nonce(&n, &fx.id).unwrap(),
        },
        chrono::Utc::now(),
    )
    .unwrap();
    revoke(&env, &serde_json::json!({"id": fx.id, "reason": "test"})).unwrap();
    assert!(!fx.run_dir().join("revoked").exists(), "precondition: no marker");
    rt().block_on(async {
        let sup = fx.supervisor();
        let e = sup.ensure_running(&fx.id).await.unwrap_err();
        assert!(matches!(e, SupError::Revoked(_)), "{e}");
        assert_eq!(sup.launcher().spawn_count(), 0, "nothing was spawned");
        assert!(!fx.run_dir().join("spawn.json").exists());
    });
}

/// d302d81c item 5: an adopted pid that stops being ours between `SIGTERM`
/// and `SIGKILL` (recycled by the OS) is never sent `SIGKILL`.
pub fn an_adopted_pid_recycled_between_sigterm_and_sigkill_is_never_killed() {
    use clawft_kernel::workload_runtime::{ChildLauncher, ChildRef};
    let fx = Fixture::new();
    let mut kid = manual_kernel(&fx.run_dir(), &fx.id, "ignore-term");
    let pid = kid.id();
    rt().block_on(async {
        let sup = fx.supervisor();
        sup.launcher().adopt(&fx.id, pid);
        let l = Arc::clone(sup.launcher());
        let (id, p) = (fx.id.clone(), pid);
        let term = tokio::spawn(async move {
            l.terminate(&ChildRef { project_id: id, pid: p }, Duration::from_millis(100)).await
        });
        // SIGTERM went out (the fake survives it and says so)...
        let dir = fx.run_dir();
        wait_until("SIGTERM delivered", 5, || dir.join("term.seen").exists()).await;
        // ...and now "the pid is recycled": the lock no longer names it.
        std::fs::write(dir.join("kernel.lock"), "1\n").unwrap();
        let r = term.await.unwrap();
        assert!(r.is_err(), "still running after the escalation: {r:?}");
        assert!(pid_alive(pid), "SIGKILL went to a pid that no longer verified as the child");
    });
    let _ = kid.kill();
    let _ = kid.wait();
}

/// d302d81c item 2: an adopted-but-refused leftover is reported as
/// unmanaged by pid, and `stop` does not pretend it was not there.
pub fn an_adopted_but_refused_leftover_is_reported_as_unmanaged() {
    let fx = Fixture::new();
    fx.behavior("serve");
    let first = rt();
    let pid = first.block_on(async { fx.supervisor().ensure_running(&fx.id).await.unwrap().pid });
    first.shutdown_background();
    // The tree's project.toml now names another project: verified as ours
    // but refused (left running, never signalled).
    let ptoml = fx.root.join(".weftos/project.toml");
    let text = std::fs::read_to_string(&ptoml).unwrap();
    std::fs::write(&ptoml, text.replace(&fx.id, &clawft_types::project::new_id())).unwrap();
    rt().block_on(async {
        let sup = fx.supervisor();
        let found = sup.adopt_on_boot().await;
        assert!(matches!(&found[..], [Found::Unverifiable { reason: Skip::Refused(_), .. }]), "{found:?}");
        let (p, why) = sup.unmanaged(&fx.id).expect("the leftover is reported");
        assert_eq!(p, pid);
        assert!(why.contains("left running"), "{why}");
        assert!(!sup.stop(&fx.id).await.unwrap(), "nothing supervised to stop");
        assert!(pid_alive(pid), "never signalled");
        assert!(sup.unmanaged(&clawft_types::project::new_id()).is_none());
    });
    std::fs::write(&ptoml, text).unwrap();
}

/// A leftover that holds its lock but never answers must not hold up boot or
/// a stop cascade for the whole `ready_timeout` (10 s here, 30 s in production).
pub fn a_wedged_leftover_delays_boot_and_the_stop_cascade_only_briefly() {
    let fx = Fixture::new();
    let mut kid = booting_kernel(&fx, 120_000);
    rt().block_on(async {
        let sup = fx.supervisor(); // ready_timeout is 10 s
        let t = Instant::now();
        let found = sup.adopt_on_boot().await;
        assert!(t.elapsed() < Duration::from_secs(6), "boot waited {:?}", t.elapsed());
        assert!(matches!(&found[..], [Found::Unverifiable { reason: Skip::HandshakeFailed(_), .. }]), "{found:?}");
        let t = Instant::now();
        assert!(sup.stop_all().await.is_empty());
        assert!(t.elapsed() < Duration::from_secs(5), "the cascade waited {:?}", t.elapsed());
        assert!(pid_alive(kid.id()), "an unverifiable leftover is never signalled");
    });
    let _ = kid.kill();
    let _ = kid.wait();
}

/// An empty `kernel.pid` (the kernel created the file, the pid is not in it
/// yet) is "still booting": the scan reads it again and adopts the child once
/// the pid is there, instead of filing `BadPidFile` at once.
pub fn an_empty_pid_file_that_fills_in_is_adopted_not_bad() {
    let fx = Fixture::new();
    let mut kid = manual_kernel(&fx.run_dir(), &fx.id, "serve");
    let pid = kid.id();
    let pid_file = fx.run_dir().join("kernel.pid");
    std::fs::write(&pid_file, "").unwrap();
    let writer = {
        let p = pid_file.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(250));
            std::fs::write(&p, pid.to_string()).unwrap();
        })
    };
    rt().block_on(async {
        let sup = fx.supervisor();
        let found = sup.adopt_on_boot().await;
        assert_eq!(found, vec![Found::Adopted { id: fx.id.clone(), pid }]);
        assert!(sup.stop(&fx.id).await.unwrap());
    });
    writer.join().unwrap();
    let _ = kid.wait();
}
