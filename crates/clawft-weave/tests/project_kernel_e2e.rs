//! Phase 2 end to end (ADR-103 A6/A7, plan section 3): a user daemon
//! supervises a REAL project kernel.
//!
//! The user daemon runs in this process (an isolated user chain, the real RPC
//! dispatch on `$WEFTOS_RUNTIME_DIR/kernel.sock`, the real `post_boot` seam).
//! Its supervisor starts children from `current_exe()`, which is this test
//! binary: when started as `kernel start --foreground --profile project
//! --project <id>`, `main` hands the argv to the real `weaver kernel` command
//! (`commands::kernel_cmd::run`), so the child is the production code path:
//! `spawn.json`, project key, `mesh.challenge` / `mesh.register` with proof of
//! possession, certificate, signed parent policy, overlay, project chain with
//! genesis, heartbeats and the final anchor. That is also why this target has
//! `harness = false` and speaks the libtest `--list` / `--exact` protocol.
//!
//! Covered, in order: the revoked marker has one path for writer, supervisor
//! and child under a `$WEFTOS_RUNTIME_DIR` and a manifest-store override;
//! `post_boot` installs the supervisor and its adoption scan lists (never
//! signals) an unverifiable leftover; start; registration and certificate; an
//! overlay deny refuses `app.install` (which the parent permits) with the rule
//! hash on the child chain, and clearing it plus `governance.reload` permits
//! it; idle stop with
//! the final anchor on the user chain; restart on demand with the same node id
//! and no second certificate; three kills (two restarts with backoff, then
//! `failed`, no automatic start); `project.restart`; revoke (child stopped,
//! marker present, `ensure_running` refused).
//!
//! `HOME` and `$WEFTOS_RUNTIME_DIR` are a fresh tempdir set in `main` before
//! any thread exists (one test per binary run, so one tempdir per test).
//! Nothing touches the real `~/.weftos`, `~/.clawft` or a running daemon, and
//! the only processes ever signalled are kernels this test started (their
//! executable is re-checked first).

#![cfg(unix)]

#[path = "project_kernel_e2e_support/scenario.rs"]
mod scenario;
#[path = "project_kernel_e2e_support/world.rs"]
mod world;

use std::time::Instant;

type TestFn = fn(world::Dirs);

fn tests() -> Vec<(&'static str, TestFn)> {
    vec![("project_kernel_end_to_end", scenario::run)]
}

/// The child: run the real `weaver kernel ...` with this argv.
fn real_kernel(args: &[String]) -> ! {
    use clap::Parser as _;
    use tracing_subscriber::layer::SubscriberExt as _;
    use tracing_subscriber::util::SubscriberInitExt as _;
    // As `weaver`'s main does: chain bridge plus logs on stderr (kernel.log).
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new("info"))
        .with(clawft_weave::chain_bridge::ChainEventLayer::new())
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .init();
    let argv = std::iter::once("weaver".to_owned()).chain(args[1..].iter().cloned());
    let parsed = clawft_weave::commands::kernel_cmd::KernelArgs::parse_from(argv);
    let rt = tokio::runtime::Runtime::new().expect("child runtime");
    let code = match rt.block_on(clawft_weave::commands::kernel_cmd::run(parsed)) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("Error: {e:#}");
            1
        }
    };
    std::process::exit(code)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("kernel") {
        real_kernel(&args);
    }
    // The libtest protocol, so `cargo nextest` (and `cargo test -- --list`)
    // can drive this binary.
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
    let wanted: Vec<(&str, TestFn)> = tests()
        .into_iter()
        .filter(|(name, _)| {
            filters.is_empty() || filters.iter().any(|f| if exact { name == f } else { name.contains(f) })
        })
        .collect();
    if wanted.is_empty() {
        println!("\nproject_kernel_e2e: 0 ran, 0 failed");
        return;
    }
    // One process-wide HOME and run root, set while single-threaded. The
    // child inherits neither from here: the supervisor passes an allow-list.
    let dirs = world::Dirs::create();
    for (k, v) in [("HOME", &dirs.home), ("WEFTOS_RUNTIME_DIR", &dirs.run_root)] {
        // SAFETY: no other thread exists yet.
        #[allow(unused_unsafe)]
        unsafe {
            std::env::set_var(k, v);
        }
    }
    let mut failed = Vec::new();
    let ran = wanted.len();
    for (name, f) in wanted {
        let started = Instant::now();
        let d = world::Dirs {
            base: dirs.base.clone(),
            home: dirs.home.clone(),
            run_root: dirs.run_root.clone(),
            manifests: dirs.manifests.clone(),
        };
        match std::panic::catch_unwind(move || f(d)) {
            Ok(()) => println!("test {name} ... ok ({:.1}s)", started.elapsed().as_secs_f32()),
            Err(_) => {
                println!("test {name} ... FAILED");
                failed.push(name);
            }
        }
    }
    println!("\nproject_kernel_e2e: {ran} ran, {} failed", failed.len());
    if failed.is_empty() {
        let _ = std::fs::remove_dir_all(&dirs.base);
    } else {
        println!("kept {} for inspection", dirs.base.display());
        std::process::exit(1);
    }
}
