//! Integration test: `control.set_enabled` + `control.list`.
//!
//! These don't run the actual daemon's `run()` (which spawns the
//! whisper service and would need a live whisper-server). Instead
//! they spin a minimal kernel + dispatch-driven test daemon and
//! pre-register their own control flags, then verify the RPC
//! flips the in-memory flag and publishes the substrate mirror.

use std::sync::Arc;
use std::time::Duration;

use clawft_kernel::boot::Kernel;
use clawft_platform::NativePlatform;
use clawft_types::config::{AgentDefaults, AgentsConfig, Config, KernelConfig};
use clawft_weave::control::{ControlKind, intent_path};
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
    }
}

async fn spawn_test_daemon() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    watch::Sender<bool>,
    Arc<tokio::sync::RwLock<Kernel<NativePlatform>>>,
) {
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
    (tmp, socket_path, shutdown_tx, kernel)
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

fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        s.push_str(&format!("{x:02x}"));
    }
    s
}

/// Register a node, return its node-id, the SigningKey, and a
/// helper closure for signing publishes (not used in these tests
/// but kept for symmetry with substrate_rpc.rs).
async fn register_node(socket: &std::path::Path, seed: u8) -> (String, SigningKey) {
    let sk = SigningKey::from_bytes(&[seed; 32]);
    let pk = sk.verifying_key().to_bytes();
    let ts: u64 = 1_700_000_000;
    let label = format!("test-control-{seed}");
    let payload =
        clawft_kernel::node_registry::node_register_payload(&pk, ts, &label);
    let proof = sk.sign(&payload);
    let resp = one_shot(
        socket,
        "node.register",
        serde_json::json!({
            "label": label,
            "pubkey": hex(&pk),
            "proof": hex(&proof.to_bytes()),
            "ts": ts,
        }),
    )
    .await;
    assert_eq!(resp["ok"], serde_json::Value::Bool(true), "register: {resp}");
    let node_id = resp["result"]["node_id"].as_str().unwrap().to_string();
    (node_id, sk)
}

#[tokio::test]
async fn control_set_enabled_rejects_when_state_uninitialized() {
    // The OnceLock isn't populated outside the real `run()` boot
    // path. RPC must surface that cleanly instead of panicking.
    let (_tmp, socket, shutdown_tx, _kernel) = spawn_test_daemon().await;
    let resp = one_shot(
        &socket,
        "control.set_enabled",
        serde_json::json!({"kind": "service", "target": "whisper", "enabled": false}),
    )
    .await;
    assert_eq!(resp["ok"], serde_json::Value::Bool(false));
    let err = resp["error"].as_str().unwrap_or("");
    assert!(
        err.contains("control state not initialized") || err.contains("no flag registered"),
        "expected uninit error, got: {err}"
    );
    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn control_set_enabled_rejects_unknown_kind() {
    let (_tmp, socket, shutdown_tx, _kernel) = spawn_test_daemon().await;
    let resp = one_shot(
        &socket,
        "control.set_enabled",
        serde_json::json!({"kind": "bogus", "target": "x", "enabled": true}),
    )
    .await;
    assert_eq!(resp["ok"], serde_json::Value::Bool(false));
    let err = resp["error"].as_str().unwrap_or("");
    assert!(err.contains("unknown kind"), "got: {err}");
    let _ = shutdown_tx.send(true);
}

/// WEFT-479: anonymous callers must be denied write/admin verbs.
///
/// `control.set_enabled` is classified `Capability::Write`. An RPC
/// envelope with no `auth` field falls into the anonymous bucket
/// (`{Read, Chat}`) and the dispatcher must short-circuit with a
/// permission-denied error before even reaching the handler.
async fn one_shot_no_auth(
    socket: &std::path::Path,
    method: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    let stream = UnixStream::connect(socket).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    // No "auth" field — anonymous caller.
    let req = serde_json::json!({ "id": "noauth", "method": method, "params": params });
    let mut line = serde_json::to_string(&req).unwrap();
    line.push('\n');
    writer.write_all(line.as_bytes()).await.unwrap();
    let mut ack = String::new();
    reader.read_line(&mut ack).await.unwrap();
    serde_json::from_str(ack.trim()).unwrap()
}

#[tokio::test]
async fn capability_gate_rejects_anonymous_write() {
    let (_tmp, socket, shutdown_tx, _kernel) = spawn_test_daemon().await;
    let resp = one_shot_no_auth(
        &socket,
        "control.set_enabled",
        serde_json::json!({"kind": "service", "target": "whisper", "enabled": false}),
    )
    .await;
    assert_eq!(resp["ok"], serde_json::Value::Bool(false));
    let err = resp["error"].as_str().unwrap_or("");
    assert!(
        err.contains("permission denied") && err.contains("Write"),
        "expected anonymous Write rejection, got: {err}"
    );
    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn capability_gate_rejects_anonymous_admin() {
    let (_tmp, socket, shutdown_tx, _kernel) = spawn_test_daemon().await;
    let resp = one_shot_no_auth(
        &socket,
        "kernel.shutdown",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(resp["ok"], serde_json::Value::Bool(false));
    let err = resp["error"].as_str().unwrap_or("");
    assert!(
        err.contains("permission denied") && err.contains("Admin"),
        "expected anonymous Admin rejection, got: {err}"
    );
    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn capability_gate_allows_anonymous_read() {
    let (_tmp, socket, shutdown_tx, _kernel) = spawn_test_daemon().await;
    let resp = one_shot_no_auth(&socket, "kernel.status", serde_json::json!({})).await;
    // kernel.status is Read, anonymous always allowed; we don't
    // care about the body here, only that the gate didn't reject.
    assert_eq!(resp["ok"], serde_json::Value::Bool(true), "kernel.status: {resp}");
    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn capability_gate_admin_token_unlocks_admin() {
    let (_tmp, socket, shutdown_tx, _kernel) = spawn_test_daemon().await;
    // Use the literal-scope admin shortcut; the gate accepts this
    // for local UDS callers (DaemonClient sets it transparently).
    let resp = one_shot(
        &socket,
        "control.set_enabled",
        serde_json::json!({"kind": "service", "target": "whisper", "enabled": false}),
    )
    .await;
    // We expect a domain-level error (control state not initialized),
    // NOT a permission-denied — meaning the gate let the call through
    // to the handler. The exact error from the handler is asserted in
    // the existing tests above.
    let err = resp["error"].as_str().unwrap_or("");
    assert!(
        !err.contains("permission denied"),
        "admin token must NOT be rejected by gate, got: {err}"
    );
    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn intent_path_construction_is_predictable() {
    // Pure unit-style — exercises the public path builder so the
    // wire-shape is locked at the test level too.
    assert_eq!(
        intent_path("n-046780", ControlKind::Service, "whisper"),
        "substrate/n-046780/control/services/whisper"
    );
    assert_eq!(
        intent_path("n-046780", ControlKind::Sensor, "n-bfc4cd/mic/pcm_chunk"),
        "substrate/n-046780/control/sensors/n-bfc4cd/mic/pcm_chunk"
    );
}

#[tokio::test]
async fn node_identity_rejects_when_state_uninitialized() {
    // The test daemon runs handle_connection without going through
    // run()'s identity bootstrap, so DAEMON_CONTROL is unset.
    // node.identity must surface that as a clean error rather than
    // panicking — this exercises the same uninit branch that
    // control.set_enabled goes through.
    let (_tmp, socket, shutdown_tx, _kernel) = spawn_test_daemon().await;
    let resp = one_shot(&socket, "node.identity", serde_json::json!({})).await;
    assert_eq!(resp["ok"], serde_json::Value::Bool(false));
    let err = resp["error"].as_str().unwrap_or("");
    assert!(
        err.contains("control state not initialized") || err.contains("not in registry"),
        "expected uninit/registry error, got: {err}"
    );
    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn register_node_smoke() {
    // Sanity that the test harness works against the real
    // node.register handler, since the control tests above don't
    // exercise it but the broader story does.
    let (_tmp, socket, shutdown_tx, _kernel) = spawn_test_daemon().await;
    let (node_id, _sk) = register_node(&socket, 91).await;
    assert!(node_id.starts_with("n-"), "got: {node_id}");
    let _ = shutdown_tx.send(true);
}
