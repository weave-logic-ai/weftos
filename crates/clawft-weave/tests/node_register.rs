//! Integration test: `node.register` proof-of-possession + lookup.
//!
//! Spins up a test daemon, calls `node.register` with a valid
//! Ed25519 proof-of-possession, asserts the returned node-id is
//! the deterministic `n-<6-hex>` BLAKE3 derivation, and that
//! re-registering the same key yields the same id (idempotent).
//!
//! Mirrors `agent_register_and_sign.rs` but for nodes: a node is a
//! physical thing in the mesh that signs *emissions*, not Actions.

use std::sync::Arc;
use std::time::Duration;

use clawft_kernel::boot::Kernel;
use clawft_platform::NativePlatform;
use clawft_types::config::{AgentDefaults, AgentsConfig, Config, KernelConfig};
use ed25519_dalek::{Signer, SigningKey};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::watch;

fn base_config() -> Config {
    Config {
        agents: AgentsConfig {
            defaults: AgentDefaults {
                workspace: "~/.clawft/workspace".into(),
                model: "test/model".into(),
                max_tokens: 1024,
                temperature: 0.5,
                max_tool_iterations: 5,
                memory_window: 10,
            },
            ..AgentsConfig::default()
        },
        ..Config::default()
    }
}

fn minimal_kernel_config() -> KernelConfig {
    KernelConfig {
        enabled: true,
        max_processes: 64,
        health_check_interval_secs: 5,
        cluster: None,
        chain: None,
        resource_tree: None,
        vector: None,
        profiles: None,
        pairing: None,
        mesh: None,
        anchor: None,
        ipc_tcp: None,
        llm: None,
        agent: None,
    }
}

async fn spawn_test_daemon() -> (tempfile::TempDir, std::path::PathBuf, watch::Sender<bool>) {
    let tmp = tempfile::tempdir().unwrap();
    let socket_path = tmp.path().join("kernel.sock");

    let platform = NativePlatform::new();
    let kernel = Kernel::boot(base_config(), minimal_kernel_config(), Arc::new(platform))
        .await
        .expect("kernel boot");
    let kernel = Arc::new(tokio::sync::RwLock::new(kernel));

    let listener = UnixListener::bind(&socket_path).unwrap();
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let accept_kernel = Arc::clone(&kernel);
    let accept_shutdown_tx = shutdown_tx.clone();

    tokio::spawn(async move {
        loop {
            tokio::select! {
                result = listener.accept() => {
                    match result {
                        Ok((stream, _)) => {
                            let k = Arc::clone(&accept_kernel);
                            let tx = accept_shutdown_tx.clone();
                            tokio::spawn(clawft_weave::daemon::handle_connection(stream, k, tx));
                        }
                        Err(_) => break,
                    }
                }
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() { break; }
                }
            }
        }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    (tmp, socket_path, shutdown_tx)
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

async fn one_shot(
    socket: &std::path::Path,
    method: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    let stream = UnixStream::connect(socket).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let req = serde_json::json!({ "id": "t", "method": method, "params": params, "auth": "admin" });
    let mut line = serde_json::to_string(&req).unwrap();
    line.push('\n');
    writer.write_all(line.as_bytes()).await.unwrap();

    let mut ack = String::new();
    reader.read_line(&mut ack).await.unwrap();
    serde_json::from_str(ack.trim()).unwrap()
}

#[tokio::test]
async fn register_returns_deterministic_node_id() {
    let (_tmp, socket, shutdown_tx) = spawn_test_daemon().await;

    let sk = SigningKey::from_bytes(&[7u8; 32]);
    let pk_bytes = sk.verifying_key().to_bytes();

    // Pre-compute the expected id locally; the daemon must agree.
    let expected_id = clawft_kernel::node_id_from_pubkey(&pk_bytes);

    let ts: u64 = 1_700_000_000;
    let label = "esp32-workbench";
    let payload = clawft_kernel::node_registry::node_register_payload(&pk_bytes, ts, label);
    let proof = sk.sign(&payload);

    let resp = one_shot(
        &socket,
        "node.register",
        serde_json::json!({
            "label": label,
            "pubkey": hex(&pk_bytes),
            "proof": hex(&proof.to_bytes()),
            "ts": ts,
        }),
    )
    .await;
    assert_eq!(
        resp["ok"],
        serde_json::Value::Bool(true),
        "register failed: {resp}"
    );
    assert_eq!(resp["result"]["node_id"], expected_id);
    assert_eq!(resp["result"]["label"], label);
    assert!(
        expected_id.starts_with("n-"),
        "expected n-<hex>, got {expected_id}"
    );

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn register_with_bad_proof_rejected() {
    let (_tmp, socket, shutdown_tx) = spawn_test_daemon().await;

    let sk = SigningKey::from_bytes(&[5u8; 32]);
    let pk_bytes = sk.verifying_key().to_bytes();

    // Sign the wrong payload (different label).
    let wrong = clawft_kernel::node_registry::node_register_payload(&pk_bytes, 100, "wrong");
    let bad_sig = sk.sign(&wrong);

    let resp = one_shot(
        &socket,
        "node.register",
        serde_json::json!({
            "label": "right",
            "pubkey": hex(&pk_bytes),
            "proof": hex(&bad_sig.to_bytes()),
            "ts": 100,
        }),
    )
    .await;
    assert_eq!(resp["ok"], serde_json::Value::Bool(false));
    let err = resp["error"].as_str().unwrap_or("");
    assert!(
        err.contains("verify failed") || err.contains("proof"),
        "expected verify-failed error, got: {err}"
    );

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn reregister_same_key_returns_same_id() {
    let (_tmp, socket, shutdown_tx) = spawn_test_daemon().await;

    let sk = SigningKey::from_bytes(&[42u8; 32]);
    let pk_bytes = sk.verifying_key().to_bytes();

    let mk_resp = |label: &str, ts: u64| {
        let payload = clawft_kernel::node_registry::node_register_payload(&pk_bytes, ts, label);
        let proof = sk.sign(&payload);
        (
            label.to_string(),
            ts,
            hex(&pk_bytes),
            hex(&proof.to_bytes()),
        )
    };

    let (l1, t1, pk1, sig1) = mk_resp("first", 1);
    let r1 = one_shot(
        &socket,
        "node.register",
        serde_json::json!({"label": l1, "pubkey": pk1, "proof": sig1, "ts": t1}),
    )
    .await;
    assert_eq!(r1["ok"], serde_json::Value::Bool(true));
    let id1 = r1["result"]["node_id"].as_str().unwrap().to_string();

    let (l2, t2, pk2, sig2) = mk_resp("second", 2);
    let r2 = one_shot(
        &socket,
        "node.register",
        serde_json::json!({"label": l2, "pubkey": pk2, "proof": sig2, "ts": t2}),
    )
    .await;
    assert_eq!(r2["ok"], serde_json::Value::Bool(true));
    let id2 = r2["result"]["node_id"].as_str().unwrap().to_string();

    assert_eq!(id1, id2, "node-id must be stable per-pubkey");
    // Latest label wins.
    assert_eq!(r2["result"]["label"], "second");

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn register_with_empty_label_succeeds() {
    let (_tmp, socket, shutdown_tx) = spawn_test_daemon().await;

    let sk = SigningKey::from_bytes(&[11u8; 32]);
    let pk_bytes = sk.verifying_key().to_bytes();

    let ts: u64 = 50;
    let payload = clawft_kernel::node_registry::node_register_payload(&pk_bytes, ts, "");
    let proof = sk.sign(&payload);

    let resp = one_shot(
        &socket,
        "node.register",
        serde_json::json!({
            "label": "",
            "pubkey": hex(&pk_bytes),
            "proof": hex(&proof.to_bytes()),
            "ts": ts,
        }),
    )
    .await;
    assert_eq!(resp["ok"], serde_json::Value::Bool(true), "{resp}");
    assert_eq!(resp["result"]["label"], "");

    let _ = shutdown_tx.send(true);
}
