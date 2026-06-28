//! Integration test: `agent.register` + signed `ipc.publish`.
//!
//! Spins up a test daemon, calls `agent.register` with a valid
//! Ed25519 proof-of-possession, then publishes with a signature —
//! asserting delivery. Also asserts that publishing with a wrong
//! signature is rejected.

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
async fn register_then_publish_with_valid_signature_delivered() {
    let (_tmp, socket, shutdown_tx) = spawn_test_daemon().await;

    // Generate a fresh Ed25519 keypair.
    let sk = SigningKey::from_bytes(&[7u8; 32]);
    let pk = sk.verifying_key();
    let pk_bytes = pk.to_bytes();

    // Build proof-of-possession.
    let ts: u64 = 1_700_000_000;
    let reg_payload = clawft_kernel::register_payload("python-bridge", &pk_bytes, ts);
    let proof = sk.sign(&reg_payload);

    let reg_resp = one_shot(
        &socket,
        "agent.register",
        serde_json::json!({
            "name": "python-bridge",
            "pubkey": hex(&pk_bytes),
            "proof": hex(&proof.to_bytes()),
            "ts": ts,
        }),
    )
    .await;
    assert_eq!(
        reg_resp["ok"],
        serde_json::Value::Bool(true),
        "register failed: {reg_resp}"
    );
    let agent_id = reg_resp["result"]["agent_id"].as_str().unwrap().to_string();

    // Subscribe on the published topic, streaming, no auth (bring-up).
    let stream = UnixStream::connect(&socket).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let sub = serde_json::json!({
        "id": "sub",
        "method": "ipc.subscribe_stream",
        "params": { "topic": "hello" },
    });
    let mut line = serde_json::to_string(&sub).unwrap();
    line.push('\n');
    writer.write_all(line.as_bytes()).await.unwrap();
    let mut ack = String::new();
    reader.read_line(&mut ack).await.unwrap();
    let ack_v: serde_json::Value = serde_json::from_str(ack.trim()).unwrap();
    assert_eq!(ack_v["ok"], serde_json::Value::Bool(true));

    // Publish with a valid signature.
    let pub_ts: u64 = 1_700_000_100;
    let pub_payload = clawft_kernel::publish_payload("hello", "world", pub_ts, &agent_id);
    let pub_sig = sk.sign(&pub_payload);
    let pub_resp = one_shot(
        &socket,
        "ipc.publish",
        serde_json::json!({
            "topic": "hello",
            "message": "world",
            "actor_id": agent_id,
            "signature": hex(&pub_sig.to_bytes()),
            "ts": pub_ts,
        }),
    )
    .await;
    assert_eq!(
        pub_resp["ok"],
        serde_json::Value::Bool(true),
        "valid-sig publish rejected: {pub_resp}"
    );

    // Subscriber should have received the message.
    let mut out = String::new();
    tokio::time::timeout(Duration::from_secs(2), reader.read_line(&mut out))
        .await
        .expect("subscriber received")
        .unwrap();
    assert!(out.contains("\"world\""), "subscriber got: {out}");

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn publish_with_wrong_signature_is_rejected() {
    let (_tmp, socket, shutdown_tx) = spawn_test_daemon().await;

    let sk = SigningKey::from_bytes(&[9u8; 32]);
    let pk_bytes = sk.verifying_key().to_bytes();

    let ts: u64 = 100;
    let reg_payload = clawft_kernel::register_payload("bad-actor", &pk_bytes, ts);
    let proof = sk.sign(&reg_payload);
    let reg_resp = one_shot(
        &socket,
        "agent.register",
        serde_json::json!({
            "name": "bad-actor",
            "pubkey": hex(&pk_bytes),
            "proof": hex(&proof.to_bytes()),
            "ts": ts,
        }),
    )
    .await;
    assert_eq!(reg_resp["ok"], serde_json::Value::Bool(true));
    let agent_id = reg_resp["result"]["agent_id"].as_str().unwrap().to_string();

    // Sign the WRONG payload (different topic).
    let wrong_payload = clawft_kernel::publish_payload("not-the-topic", "x", 200, &agent_id);
    let wrong_sig = sk.sign(&wrong_payload);

    let resp = one_shot(
        &socket,
        "ipc.publish",
        serde_json::json!({
            "topic": "real-topic",
            "message": "x",
            "actor_id": agent_id,
            "signature": hex(&wrong_sig.to_bytes()),
            "ts": 200,
        }),
    )
    .await;
    assert_eq!(resp["ok"], serde_json::Value::Bool(false));
    let err = resp["error"].as_str().unwrap_or("");
    assert!(
        err.contains("unauthorized"),
        "expected unauthorized, got: {err}"
    );

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn register_with_bad_proof_of_possession_rejected() {
    let (_tmp, socket, shutdown_tx) = spawn_test_daemon().await;

    let sk = SigningKey::from_bytes(&[11u8; 32]);
    let pk_bytes = sk.verifying_key().to_bytes();

    // Sign the wrong payload (tamper the name).
    let bad_payload = clawft_kernel::register_payload("wrong-name", &pk_bytes, 1);
    let bad_proof = sk.sign(&bad_payload);

    let resp = one_shot(
        &socket,
        "agent.register",
        serde_json::json!({
            "name": "correct-name",
            "pubkey": hex(&pk_bytes),
            "proof": hex(&bad_proof.to_bytes()),
            "ts": 1,
        }),
    )
    .await;
    assert_eq!(resp["ok"], serde_json::Value::Bool(false));
    let err = resp["error"].as_str().unwrap_or("");
    assert!(
        err.contains("proof-of-possession"),
        "expected proof-of-possession failure, got: {err}"
    );

    let _ = shutdown_tx.send(true);
}
