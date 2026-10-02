//! The project supervisor against real child processes (ADR-103 A6, Phase 2
//! package G). The test binary is its own fake project kernel (see
//! `project_supervisor_support/fake_kernel.rs`), which is why this target has
//! `harness = false`: `main` either runs as the child (argv `kernel start
//! ...`) or runs the tests below, one after another.
//!
//! Every test uses a tempdir `HOME` and run root; nothing touches the real
//! `~/.weftos`, `~/.clawft` or any running daemon, and no process outside
//! the fake kernels this test starts is ever signalled.

#![cfg(unix)]

#[path = "project_supervisor_support/fake_kernel.rs"]
mod fake_kernel;
#[path = "project_supervisor_support/fixture.rs"]
mod fixture;

use std::time::{Duration, Instant};

use clawft_types::project::ChildState;
use clawft_types::project::spawn::SpawnFile;
use clawft_weave::capability::{CallerCapabilities, Capability};
use clawft_weave::env_probe::{leaked, process_environment};
use clawft_weave::project_supervisor::adopt::{Found, Skip};
use clawft_weave::project_supervisor::child::pid_alive;
use clawft_weave::project_supervisor::idle::Activity;
use clawft_weave::project_supervisor::{SupError, state};
use fixture::{Fixture, wait_state, wait_until};

const SECRETS: &[(&str, &str)] = &[
    ("OPENAI_API_KEY", "sk-fake-openai-1111"),
    ("ANTHROPIC_API_KEY", "sk-ant-fake-2222"),
    ("WEFTOS_TOKEN_SECRET", "wft_fake_3333"),
    ("AWS_SECRET_ACCESS_KEY", "aws-fake-4444"),
];

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}

type TestFn = fn();

fn tests() -> Vec<(&'static str, TestFn)> {
    vec![
        ("env_allowlist_spawn_contract", env_allowlist_spawn_contract),
        ("restart_budget_then_failed", restart_budget_then_failed),
        ("backoff_schedule_is_chained", backoff_schedule_is_chained),
        ("clean_exit_is_not_restarted", clean_exit_is_not_restarted),
        ("idle_stop_then_restart", idle_stop_then_restart),
        ("ignored_shutdown_escalates_to_sigterm", ignored_shutdown_escalates_to_sigterm),
        ("adoption_after_user_daemon_restart", adoption_after_user_daemon_restart),
        ("pid_reuse_and_unverifiable_leftovers_are_never_adopted", unverifiable_leftovers),
        ("spawn_refusal_cases", spawn_refusal_cases),
        ("concurrent_ensure_running_starts_one_child", concurrent_ensure_running),
        ("revoked_marker_stops_the_child_and_blocks_a_restart", revoked_marker),
        ("token_refresh_rotates_and_refuses_strangers", token_refresh),
        ("crash_after_ready_restarts_and_recovers", crash_after_ready_recovers),
    ]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("kernel") {
        fake_kernel::run(&args);
    }
    // Set before any thread exists: the spawn test proves none of these
    // reaches a child.
    for (k, v) in SECRETS {
        // SAFETY: single-threaded here, before the first runtime starts.
        #[allow(unused_unsafe)]
        unsafe {
            std::env::set_var(k, v);
        }
    }
    let filter = args.iter().find(|a| !a.starts_with('-')).cloned();
    let mut failed = Vec::new();
    let mut ran = 0;
    for (name, f) in tests() {
        if filter.as_deref().is_some_and(|x| !name.contains(x)) {
            continue;
        }
        ran += 1;
        let started = Instant::now();
        match std::panic::catch_unwind(f) {
            Ok(()) => println!("test {name} ... ok ({:.1}s)", started.elapsed().as_secs_f32()),
            Err(_) => {
                println!("test {name} ... FAILED");
                failed.push(name);
            }
        }
    }
    println!("\nproject_supervisor: {ran} ran, {} failed", failed.len());
    if !failed.is_empty() {
        std::process::exit(1);
    }
}

fn env_allowlist_spawn_contract() {
    let fx = Fixture::new();
    fx.behavior("serve");
    rt().block_on(async {
        let sup = fx.supervisor();
        let r = sup.ensure_running(&fx.id).await.unwrap();
        assert!(r.started);
        // The REAL environment of the child, not what it chose to print.
        let real = process_environment(r.pid).expect("read the child's environment");
        let names: Vec<&str> = SECRETS.iter().flat_map(|(k, v)| [*k, *v]).collect();
        // Never print the environment itself: on a failure it would put the
        // developer's real keys into the log.
        let hit = leaked(&real, &names);
        assert!(hit.is_empty(), "these names/values reached the child: {:?}", hit.iter().map(|h| h.len()).collect::<Vec<_>>());
        assert!(real.contains("WEFTOS_PROJECT_ID"), "child environment does not look like one");
        // And what the child itself saw: exactly the allow-list.
        let seen = std::fs::read_to_string(fx.run_dir().join("env.txt")).unwrap();
        let allowed = [
            "HOME", "PATH", "WEFTOS_RUNTIME_DIR", "WEFTOS_PROJECT_ID", "LANG", "LC_ALL", "LC_CTYPE",
            "LC_COLLATE", "LC_MESSAGES", "LC_NUMERIC", "LC_TIME", "LC_MONETARY",
        ];
        for line in seen.lines() {
            let k = line.split('=').next().unwrap();
            // macOS adds this one to every exec itself; it is not ours.
            if k == "__CF_USER_TEXT_ENCODING" {
                continue;
            }
            assert!(allowed.contains(&k), "unexpected variable {k} in the child environment");
        }
        assert!(seen.contains(&format!("WEFTOS_RUNTIME_DIR={}", fx.run_dir().display())));
        assert!(seen.contains(&format!("HOME={}", fx.home.display())));
        // The real argv: always `--profile project`.
        let argv = std::fs::read_to_string(fx.run_dir().join("argv.txt")).unwrap();
        assert_eq!(
            argv.lines().collect::<Vec<_>>(),
            ["kernel", "start", "--foreground", "--profile", "project", "--project", fx.id.as_str()]
        );
        assert_eq!(
            std::fs::read_to_string(fx.run_dir().join("cwd.txt")).unwrap().trim(),
            fx.root.display().to_string()
        );
        // spawn.json as the child saw it.
        let spawn: SpawnFile = SpawnFile::read(&fx.run_dir().join("spawn.seen.json")).unwrap();
        assert_eq!(spawn.project_id, fx.id);
        assert_eq!(spawn.root, fx.root);
        assert_eq!(spawn.nonce.len(), 64);
        let now = state::now_unix();
        assert!(spawn.expires > now && spawn.expires <= now + 60, "60 s expiry");
        assert_eq!(
            spawn.parent_socket.as_deref(),
            Some(fx.run_root.join("kernel.sock").as_path()),
            "the default parent socket is the user daemon's socket"
        );
        // The token: project-scoped, Write only, never Admin.
        let info = fx.tokens.validate(&spawn.project_token).expect("live token");
        assert_eq!(info.project.as_deref(), Some(fx.id.as_str()));
        let caps = CallerCapabilities::from_scopes(info.scope.capability_scopes().iter().copied());
        assert!(!caps.allows(Capability::Admin), "a project token must not be admin");
        assert!(!caps.allows_method("kernel.shutdown"));
        assert!(caps.allows(Capability::Write) && caps.allows(Capability::Read));
        // Files in the run dir: user pin, signed parent policy, modes.
        use std::os::unix::fs::PermissionsExt as _;
        for f in ["user.pub", "parent-policy.json", "spawn.seen.json"] {
            let m = std::fs::metadata(fx.run_dir().join(f)).unwrap().permissions().mode() & 0o777;
            assert_eq!(m & 0o077, 0, "{f} must not be group/world accessible");
        }
        let pin = std::fs::read_to_string(fx.run_dir().join("user.pub")).unwrap();
        assert_eq!(pin.trim(), hex::encode(fx.user_key.verifying_key().to_bytes()));
        let pol = clawft_kernel::parent_policy::load_parent_policy(&fx.run_dir().join("parent-policy.json")).unwrap();
        clawft_kernel::parent_policy::verify_parent_policy(&pol, &fx.user_key.verifying_key().to_bytes()).unwrap();
        // The workload path ran through the gate and the chain.
        let loads = fx.events("workload.load");
        assert!(loads.iter().any(|e| e.payload.as_ref().is_some_and(|p| p["permit_rule"] == "PROJECT-SUPERVISOR-PERMIT")));
        assert!(!fx.events("workload.start").is_empty());
        assert_eq!(fx.events("project.kernel.started").len(), 1);
        assert!(sup.stop(&fx.id).await.unwrap());
    });
}

fn restart_budget_then_failed() {
    let fx = Fixture::new();
    fx.set_serve(|s| {
        s.restart_max = Some(2);
        s.restart_window_secs = Some(60);
    });
    fx.behavior("crash");
    rt().block_on(async {
        let sup = fx.supervisor();
        assert!(matches!(sup.ensure_running(&fx.id).await, Err(SupError::NotReady(_))));
        wait_state(&sup, &fx.id, ChildState::Failed, 10).await;
        // The start plus exactly two restarts: the third crash is the end.
        assert_eq!(sup.launcher().spawn_count(), 3);
        assert_eq!(fx.events("project.kernel.failed").len(), 1);
        // No automatic start until `restart`, and ensure_running does not
        // sneak one in either.
        let err = sup.ensure_running(&fx.id).await.unwrap_err();
        assert!(matches!(err, SupError::Failed(_)), "{err}");
        assert_eq!(err.kind(), "project_failed");
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(sup.launcher().spawn_count(), 3);
        // An explicit restart clears the failure.
        fx.behavior("serve");
        let r = sup.restart(&fx.id).await.unwrap();
        assert!(r.started);
        assert_eq!(sup.launcher().spawn_count(), 4);
        let st = sup.status(&fx.id).await;
        assert_eq!((st.state, st.restarts), (ChildState::Running, 0));
        sup.stop(&fx.id).await.unwrap();
    });
}

fn backoff_schedule_is_chained() {
    let fx = Fixture::new();
    fx.set_serve(|s| s.restart_max = Some(4));
    fx.behavior("crash");
    rt().block_on(async {
        let sup = fx.supervisor();
        let _ = sup.ensure_running(&fx.id).await;
        wait_state(&sup, &fx.id, ChildState::Failed, 10).await;
        let delays: Vec<u64> = fx
            .events("project.kernel.exited")
            .iter()
            .filter_map(|e| e.payload.as_ref()?["restart_in_ms"].as_u64())
            .collect();
        // base 20 ms doubling to the 80 ms cap (production: 1 s to 30 s).
        assert_eq!(delays, [20, 40, 80, 80]);
        assert_eq!(sup.launcher().spawn_count(), 5);
    });
}

fn clean_exit_is_not_restarted() {
    let fx = Fixture::new();
    fx.behavior("exit0");
    rt().block_on(async {
        let sup = fx.supervisor();
        assert!(matches!(sup.ensure_running(&fx.id).await, Err(SupError::NotReady(_))));
        wait_state(&sup, &fx.id, ChildState::Stopped, 5).await;
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(sup.launcher().spawn_count(), 1, "exit 0 is Transient: never restarted");
        let exited = fx.events("project.kernel.exited");
        assert_eq!(exited.len(), 1);
        assert_eq!(exited[0].payload.as_ref().unwrap()["clean"], true);
    });
}

fn idle_stop_then_restart() {
    let fx = Fixture::new();
    fx.set_serve(|s| s.idle_stop_secs = Some(10));
    fx.behavior("serve");
    rt().block_on(async {
        let sup = fx.supervisor();
        let r = sup.ensure_running(&fx.id).await.unwrap();
        let act = |busy: u32, last: u64| {
            *fx.activity.0.lock().unwrap() =
                Some(Activity { last_activity_unix: last, busy_agents: busy, ..Activity::default() });
        };
        // No data: busy. Quiet for 5 s of 10: not yet. Busy agent: never.
        assert!(sup.idle_pass(5000).await.is_empty());
        act(0, 1000);
        assert!(sup.idle_pass(1005).await.is_empty());
        act(1, 1000);
        assert!(sup.idle_pass(9000).await.is_empty());
        assert!(pid_alive(r.pid));
        // Quiet long enough: graceful stop through kernel.shutdown.
        act(0, 1000);
        assert_eq!(sup.idle_pass(1010).await, vec![fx.id.clone()]);
        assert!(fx.run_dir().join("shutdown.seen").exists(), "kernel.shutdown (final anchor) came first");
        wait_until("child gone", 5, || !pid_alive(r.pid)).await;
        assert_eq!(sup.status(&fx.id).await.state, ChildState::Stopped);
        assert_eq!(fx.events("project.kernel.idle_stop").len(), 1);
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(sup.launcher().spawn_count(), 1, "an idle stop is not restarted");
        // On demand it comes back.
        let again = sup.ensure_running(&fx.id).await.unwrap();
        assert!(again.started && again.pid != r.pid);
        assert_eq!(sup.launcher().spawn_count(), 2);
        sup.stop(&fx.id).await.unwrap();
    });
}

fn ignored_shutdown_escalates_to_sigterm() {
    let fx = Fixture::new();
    fx.behavior("ignore-shutdown");
    rt().block_on(async {
        let sup = fx.supervisor();
        let r = sup.ensure_running(&fx.id).await.unwrap();
        let t = Instant::now();
        assert!(sup.stop(&fx.id).await.unwrap());
        assert!(t.elapsed() >= Duration::from_millis(400), "waited the grace before signalling");
        assert!(!pid_alive(r.pid));
        assert_eq!(sup.status(&fx.id).await.state, ChildState::Stopped);
    });
}

fn adoption_after_user_daemon_restart() {
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

fn unverifiable_leftovers() {
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

fn spawn_refusal_cases() {
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

fn concurrent_ensure_running() {
    let fx = Fixture::new();
    fx.behavior("serve");
    rt().block_on(async {
        let sup = fx.supervisor();
        for i in 0..100u64 {
            let calls: Vec<_> = (0..8)
                .map(|_| {
                    let (s, id) = (sup.clone(), fx.id.clone());
                    tokio::spawn(async move { s.ensure_running(&id).await })
                })
                .collect();
            let mut pids = std::collections::HashSet::new();
            let mut started = 0;
            for c in calls {
                let r = c.await.unwrap().unwrap();
                pids.insert(r.pid);
                started += u32::from(r.started);
            }
            assert_eq!(pids.len(), 1, "iteration {i}: two children for one project");
            assert_eq!(started, 1, "iteration {i}: exactly one call starts it");
            assert_eq!(sup.launcher().spawn_count(), i + 1);
            assert!(sup.stop(&fx.id).await.unwrap());
        }
    });
}

fn revoked_marker() {
    let fx = Fixture::new();
    fx.behavior("serve");
    rt().block_on(async {
        let sup = fx.supervisor();
        let r = sup.ensure_running(&fx.id).await.unwrap();
        sup.revoked(&fx.id, "project.revoke").await;
        assert!(state::is_marked_revoked(&fx.run_dir()), "the marker the child checks");
        wait_until("child stopped", 5, || !pid_alive(r.pid)).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(sup.launcher().spawn_count(), 1, "a revoked project is not restarted");
        assert_eq!(sup.ensure_running(&fx.id).await.unwrap_err().kind(), "project_revoked");
    });
}

fn token_refresh() {
    use clawft_kernel::token_authority::Issuer;
    let fx = Fixture::new();
    fx.behavior("serve");
    rt().block_on(async {
        let sup = fx.supervisor();
        sup.ensure_running(&fx.id).await.unwrap();
        let t1 = SpawnFile::read(&fx.run_dir().join("spawn.seen.json")).unwrap().project_token;
        let (t2, _) = sup.launcher().refresh_token(&fx.id, &t1).unwrap();
        let (t3, _) = sup.launcher().refresh_token(&fx.id, &t2).unwrap();
        assert!(fx.tokens.validate(&t1).is_none(), "the token before the previous one is revoked");
        assert!(fx.tokens.validate(&t2).is_some() && fx.tokens.validate(&t3).is_some());
        // An owner token, an unknown secret and another project's token
        // cannot refresh.
        let (owner, _) = fx.tokens.issue("owner", None, None, &Issuer::default()).unwrap();
        assert!(sup.launcher().refresh_token(&fx.id, &owner).is_err());
        assert!(sup.launcher().refresh_token(&fx.id, "wft_nope").is_err());
        let other = clawft_types::project::new_id();
        let (foreign, _) = fx
            .tokens
            .issue_project(&other, chrono::Duration::minutes(5), &Issuer::default())
            .unwrap();
        assert!(sup.launcher().refresh_token(&fx.id, &foreign).is_err());
        // Stopping the child revokes its tokens.
        sup.stop(&fx.id).await.unwrap();
        assert!(fx.tokens.validate(&t2).is_none() && fx.tokens.validate(&t3).is_none());
    });
}

fn crash_after_ready_recovers() {
    let fx = Fixture::new();
    fx.behavior("crash-after-ready");
    rt().block_on(async {
        let sup = fx.supervisor();
        let r = sup.ensure_running(&fx.id).await.unwrap();
        // It dies 300 ms after answering; the supervisor restarts it. Make
        // the restarted child healthy so the loop ends.
        wait_until("child crashed", 5, || !pid_alive(r.pid)).await;
        fx.behavior("serve");
        wait_until("restarted", 10, || sup.launcher().spawn_count() >= 2).await;
        wait_state(&sup, &fx.id, ChildState::Running, 10).await;
        let st = sup.status(&fx.id).await;
        assert!(st.restarts >= 1 && st.pid.is_some_and(|p| p != r.pid));
        assert!(fx.events("project.kernel.restarted").len() >= 1);
        sup.stop(&fx.id).await.unwrap();
    });
}
