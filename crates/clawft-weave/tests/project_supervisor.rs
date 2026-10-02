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
#[path = "project_supervisor_support/followup_tests.rs"]
mod followup_tests;
#[path = "project_supervisor_support/lifecycle_tests.rs"]
mod lifecycle_tests;
#[path = "project_supervisor_support/rpc_test.rs"]
mod rpc_test;

use std::time::{Duration, Instant};

use clawft_types::project::ChildState;
use clawft_weave::capability::{CallerCapabilities, Capability};
use clawft_weave::env_probe::{leaked, process_environment};
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

pub fn rt() -> tokio::runtime::Runtime {
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
        ("adoption_after_user_daemon_restart", lifecycle_tests::adoption_after_user_daemon_restart),
        ("revoked_child_is_stopped_at_adoption", lifecycle_tests::revoked_child_is_stopped_at_adoption),
        ("ensure_running_takes_over_a_live_verified_child", lifecycle_tests::ensure_running_takes_over_a_live_verified_child),
        ("revoke_stops_the_child_even_when_the_gated_stop_fails", lifecycle_tests::revoke_stops_the_child_even_when_the_gated_stop_fails),
        ("a_failed_spawn_leaves_nothing_behind", lifecycle_tests::a_failed_spawn_leaves_nothing_behind),
        ("stop_removes_spawn_json_and_a_foreign_pid_is_not_ready", lifecycle_tests::stop_removes_spawn_json_and_a_foreign_pid_is_not_ready),
        ("adopted_zombie_counts_as_dead", lifecycle_tests::adopted_zombie_counts_as_dead),
        ("pid_reuse_and_unverifiable_leftovers_are_never_adopted", lifecycle_tests::unverifiable_leftovers),
        ("spawn_refusal_cases", lifecycle_tests::spawn_refusal_cases),
        ("concurrent_ensure_running_starts_one_child", concurrent_ensure_running),
        ("revoked_marker_stops_the_child_and_blocks_a_restart", revoked_marker),
        ("token_refresh_rotates_and_refuses_strangers", token_refresh),
        ("crash_after_ready_restarts_and_recovers", crash_after_ready_recovers),
        ("a_child_on_an_older_build_than_the_daemon_is_reported_stale", followup_tests::a_child_on_an_older_build_than_the_daemon_is_reported_stale),
        ("ensure_running_during_an_automatic_restart_waits_for_the_socket", followup_tests::ensure_running_during_an_automatic_restart_waits_for_the_socket),
        ("a_child_still_booting_at_adoption_is_adopted_or_found_by_the_stop_cascade", followup_tests::a_child_still_booting_at_adoption_is_adopted_or_found_by_the_stop_cascade),
        ("a_child_that_lost_its_heartbeat_is_restarted_and_then_failed_when_the_budget_is_spent", followup_tests::a_child_that_lost_its_heartbeat_is_restarted_and_then_failed_when_the_budget_is_spent),
        ("a_revoked_project_without_a_marker_is_refused_before_spawning", followup_tests::a_revoked_project_without_a_marker_is_refused_before_spawning),
        ("an_adopted_pid_recycled_between_sigterm_and_sigkill_is_never_killed", followup_tests::an_adopted_pid_recycled_between_sigterm_and_sigkill_is_never_killed),
        ("an_adopted_but_refused_leftover_is_reported_as_unmanaged", followup_tests::an_adopted_but_refused_leftover_is_reported_as_unmanaged),
        ("a_wedged_leftover_delays_boot_and_the_stop_cascade_only_briefly", followup_tests::a_wedged_leftover_delays_boot_and_the_stop_cascade_only_briefly),
        // Last: installs the process-wide supervisor.
        ("lifecycle_rpc_end_to_end", rpc_test::lifecycle_rpc_end_to_end),
    ]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("kernel") {
        fake_kernel::run(&args);
    }
    // The libtest protocol, so `cargo nextest` (and `cargo test -- --list`)
    // can drive this binary: `--list --format terse` prints `<name>: test`
    // per test (nothing for `--ignored`), `<name> --exact` runs just that
    // one; unknown flags are ignored.
    if args.iter().any(|a| a == "--list") {
        if !args.iter().any(|a| a == "--ignored") {
            for (name, _) in tests() {
                println!("{name}: test");
            }
        }
        return;
    }
    let exact = args.iter().any(|a| a == "--exact");
    let mut filters: Vec<&str> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if matches!(a.as_str(), "--test-threads" | "--skip" | "--format" | "--color" | "--logfile") {
            it.next();
        } else if !a.starts_with('-') {
            filters.push(a);
        }
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
    let mut failed = Vec::new();
    let mut ran = 0;
    for (name, f) in tests() {
        let wanted = filters.is_empty()
            || filters.iter().any(|f| if exact { name == *f } else { name.contains(f) });
        if !wanted {
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
        let spawn = fixture::seen_spawn(&fx.run_dir());
        assert_eq!(spawn.project_id, fx.id);
        assert_eq!(spawn.root, fx.root);
        assert_eq!(spawn.nonce.len(), 64);
        let now = state::now_unix();
        assert!(spawn.expires_unix > now && spawn.expires_unix <= now + 60, "60 s expiry");
        assert_eq!(
            spawn.parent_socket.as_path(),
            fx.run_root.join("kernel.sock").as_path(),
            "the default parent socket is the user daemon's socket"
        );
        // The token: project-scoped, Write only, never Admin.
        let info = fx.tokens.validate(spawn.project_token.as_ref().unwrap()).expect("live token");
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
        // The supervisor records which kernel it started in the manifest.
        let serve = fx.manifest().serve.unwrap();
        assert_eq!(serve.kernel_version.as_deref(), Some(env!("CARGO_PKG_VERSION")));
        assert_eq!(serve.kernel_sha.as_deref(), Some(env!("BUILD_GIT_HASH")));
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
        fx.mark_revoked(); // what the project.revoke RPC does first
        sup.revoked(&fx.id, "project.revoke").await;
        assert!(state::is_marked_revoked(&fx.run_root, &fx.id), "the marker the child checks");
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
        let t1 = fixture::seen_spawn(&fx.run_dir()).project_token.unwrap();
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
        assert!(!fx.events("project.kernel.restarted").is_empty());
        sup.stop(&fx.id).await.unwrap();
    });
}
