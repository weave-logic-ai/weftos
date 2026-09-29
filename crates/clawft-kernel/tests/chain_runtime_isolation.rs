//! A kernel booted with `WEFTOS_RUNTIME_DIR` set must chain to its OWN
//! chain file and signing key under that runtime dir, and must never load,
//! rewrite or append to the operator chain under `$HOME/.clawft/`.
//!
//! Regression for the mesh-placement-06 incident, where a demo daemon run
//! with `WEFTOS_RUNTIME_DIR` appended ~70 events to the operator's chain.
//!
//! This file holds exactly one test because it sets process environment
//! (`HOME`, `WEFTOS_RUNTIME_DIR`); each integration-test file runs in its own
//! process, so no other test observes the change. `HOME` points at a fake
//! temp home: the real operator home is never read or written.

#![cfg(all(feature = "native", feature = "exochain"))]

use std::path::Path;
use std::sync::Arc;

use clawft_kernel::boot::Kernel;
use clawft_kernel::chain::ChainManager;
use clawft_platform::NativePlatform;
use clawft_types::config::{Config, KernelConfig};

const OPERATOR_CHAIN_BYTES: &[u8] = b"operator-chain-rvf-must-not-change";
const OPERATOR_KEY_BYTES: &[u8] = b"operator-chain-key-must-not-change";

fn snapshot(path: &Path) -> (Vec<u8>, std::time::SystemTime) {
    let bytes = std::fs::read(path).unwrap();
    let mtime = std::fs::metadata(path).unwrap().modified().unwrap();
    (bytes, mtime)
}

#[tokio::test]
async fn runtime_dir_daemon_chains_to_isolated_files_not_operator_chain() {
    let fake_home = tempfile::tempdir().unwrap();
    let runtime = tempfile::tempdir().unwrap();

    // Seed a fake operator chain + key under the fake HOME.
    let op_dir = fake_home.path().join(".clawft");
    std::fs::create_dir_all(&op_dir).unwrap();
    let op_chain = op_dir.join("chain.rvf");
    let op_key = op_dir.join("chain.key");
    let op_json = op_dir.join("chain.json");
    std::fs::write(&op_chain, OPERATOR_CHAIN_BYTES).unwrap();
    std::fs::write(&op_key, OPERATOR_KEY_BYTES).unwrap();
    let before_chain = snapshot(&op_chain);
    let before_key = snapshot(&op_key);

    // SAFETY: this is the only test in this binary (own process), and the
    // variables are set before any kernel/tokio worker reads them.
    unsafe {
        std::env::set_var("HOME", fake_home.path());
        std::env::set_var("WEFTOS_RUNTIME_DIR", runtime.path());
    }

    // Default kernel config: no explicit chain paths, exactly as a probe or
    // demo daemon boots.
    let platform = Arc::new(NativePlatform::new());
    let mut kernel = Kernel::boot(Config::default(), KernelConfig::default(), platform)
        .await
        .expect("kernel boots");

    let pinned = kernel
        .kernel_config()
        .chain
        .as_ref()
        .and_then(|c| c.checkpoint_path.clone())
        .expect("chain path pinned at boot");
    assert!(
        Path::new(&pinned).starts_with(runtime.path()),
        "chain checkpoint must live under WEFTOS_RUNTIME_DIR, got {pinned}"
    );

    // The isolated daemon still chains its actions (demo-style events).
    let cm = Arc::clone(kernel.chain_manager().expect("chain enabled"));
    cm.append("demo-app", "app.install", Some(serde_json::json!({"app": "probe"})));
    cm.append("demo-app", "app.start", Some(serde_json::json!({"app": "probe"})));
    let seq_before_shutdown = cm.sequence();
    kernel.shutdown().await.expect("clean shutdown persists chain");

    // Operator chain and key: byte-identical and not rewritten.
    assert_eq!(snapshot(&op_chain), before_chain, "operator chain.rvf changed");
    assert_eq!(snapshot(&op_key), before_key, "operator chain.key changed");
    assert!(!op_json.exists(), "no chain.json written under operator home");

    // Isolated chain + key exist under the runtime dir and hold the events.
    let iso_rvf = runtime.path().join("chain.rvf");
    let iso_key = runtime.path().join("chain.key");
    assert!(iso_key.exists(), "isolated signing key created under runtime dir");
    assert!(iso_rvf.exists(), "isolated chain persisted under runtime dir");
    let restored = ChainManager::load_from_rvf(&iso_rvf, 1000).expect("isolated chain loads");
    assert!(restored.sequence() >= seq_before_shutdown);
    let kinds: Vec<String> = restored
        .tail(usize::MAX)
        .into_iter()
        .filter(|e| e.source == "demo-app")
        .map(|e| e.kind)
        .collect();
    assert_eq!(kinds, vec!["app.install".to_string(), "app.start".to_string()]);
}
