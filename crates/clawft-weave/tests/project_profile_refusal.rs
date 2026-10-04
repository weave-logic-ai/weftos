//! `--profile project` without a spawn handshake must not boot as a plain
//! kernel (P2 H review). Own process: it sets process-wide state.
//!
//! Harness constraint: this binary holds exactly ONE test. It sets and removes
//! process-wide environment variables and flips the process-wide project
//! profile, and libtest runs a binary's tests on parallel threads, so a second
//! test here would race the environment. Any test added to this file must
//! start with `claim_process_env()`, which fails loudly when a second one
//! tries.

static ENV_CLAIMED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn claim_process_env() {
    assert!(
        !ENV_CLAIMED.swap(true, std::sync::atomic::Ordering::SeqCst),
        "project_profile_refusal.rs is a one-test binary: it mutates process-wide environment and state"
    );
}

use clawft_types::config::{Config, KernelConfig, KernelProfile};

#[tokio::test(flavor = "multi_thread")]
async fn project_profile_without_spawn_json_refuses_to_boot() {
    claim_process_env();
    let run = tempfile::tempdir().unwrap();
    // SAFETY: the only test in this binary (see `claim_process_env`); nothing
    // reads the environment concurrently.
    unsafe {
        std::env::set_var("WEFTOS_RUNTIME_DIR", run.path());
        std::env::set_var("WEFTOS_PROJECT_ID", "01JB8Z3Q0V6X9KQ4M2N7T5R1WD");
    }
    // 1. The config path: profile = Project in the config.
    let mut config = Config::default();
    let kc = KernelConfig {
        profile: Some(KernelProfile::Project),
        ..KernelConfig::default()
    };
    config.kernel = kc.clone();
    let e = clawft_weave::project_hooks::pre_boot(&mut config, &kc)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("started by the user daemon"), "{e}");

    // 2. The CLI path: `--profile project` sets a flag and a default config
    // gets the project profile forced inside `daemon::run`, which refuses at
    // its first step, before a lock, key or chain is touched.
    clawft_weave::user_daemon::set_project_profile(true);
    let e = clawft_weave::daemon::run(
        Config::default(),
        KernelConfig::default(),
        Default::default(),
        None,
    )
    .await
    .unwrap_err();
    assert!(e.to_string().contains("started by the user daemon"), "{e}");
    assert!(
        !run.path().join("kernel.lock").exists(),
        "nothing was opened"
    );
    assert!(!run.path().join("node.key").exists());

    // 3. No run dir variable at all.
    unsafe { std::env::remove_var("WEFTOS_PROJECT_ID") };
    let e = clawft_weave::daemon::run(
        Config::default(),
        KernelConfig::default(),
        Default::default(),
        None,
    )
    .await
    .unwrap_err();
    assert!(e.to_string().contains("started by the user daemon"), "{e}");
}
