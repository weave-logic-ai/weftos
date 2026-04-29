//! Integration test: substrate read / publish / subscribe / notify.

use std::sync::Arc;
use std::time::Duration;

use clawft_kernel::boot::Kernel;
use clawft_platform::NativePlatform;
use clawft_types::config::{AgentDefaults, AgentsConfig, Config, KernelConfig};
use ed25519_dalek::{Signer, SigningKey};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::watch;

/// Test helper: a registered node bound to its signing key. All
/// `substrate.publish` calls in these tests go through one of these
/// so the node-identity gate is exercised as the production path
/// would.
struct TestNode {
    sk: SigningKey,
    node_id: String,
}

impl TestNode {
    /// Register a fresh test node with the daemon. Seeds the
    /// signing key from `seed` so each test gets its own
    /// deterministic identity.
    async fn register(socket: &std::path::Path, seed: u8) -> Self {
        let sk = SigningKey::from_bytes(&[seed; 32]);
        let pk = sk.verifying_key().to_bytes();
        let ts: u64 = 1_700_000_000;
        let label = format!("test-node-{seed}");
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
        Self { sk, node_id }
    }

    fn node_id(&self) -> &str {
        &self.node_id
    }

    /// `substrate/<node_id>/<suffix>` — convenience for path-building.
    fn path(&self, suffix: &str) -> String {
        format!("substrate/{}/{suffix}", self.node_id)
    }

    /// Sign and send a `substrate.publish` for this node.
    async fn publish(
        &self,
        socket: &std::path::Path,
        path: &str,
        value: serde_json::Value,
    ) -> serde_json::Value {
        let ts: u64 = 1_700_000_100;
        let value_bytes = serde_json::to_vec(&value).unwrap();
        let value_str = String::from_utf8_lossy(&value_bytes);
        let payload = clawft_kernel::node_publish_payload(path, &value_str, ts, &self.node_id);
        let sig = self.sk.sign(&payload);
        one_shot(
            socket,
            "substrate.publish",
            serde_json::json!({
                "path": path,
                "value": value,
                "node_id": self.node_id,
                "node_signature": hex(&sig.to_bytes()),
                "node_ts": ts,
            }),
        )
        .await
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

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

async fn one_shot(
    socket: &std::path::Path,
    method: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    let stream = UnixStream::connect(socket).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let req = serde_json::json!({ "id": "t", "method": method, "params": params });
    let mut line = serde_json::to_string(&req).unwrap();
    line.push('\n');
    writer.write_all(line.as_bytes()).await.unwrap();
    let mut ack = String::new();
    reader.read_line(&mut ack).await.unwrap();
    serde_json::from_str(ack.trim()).unwrap()
}

#[tokio::test]
async fn substrate_read_write_notify_roundtrip() {
    let (_tmp, socket, shutdown_tx) = spawn_test_daemon().await;
    let node = TestNode::register(&socket, 7).await;
    let path = node.path("test/ping");

    // Empty read returns null value + tick=0.
    let r1 = one_shot(
        &socket,
        "substrate.read",
        serde_json::json!({ "path": path }),
    )
    .await;
    assert_eq!(r1["ok"], serde_json::Value::Bool(true));
    assert!(r1["result"]["value"].is_null());
    assert_eq!(r1["result"]["tick"], 0);

    // Publish a value through the gate.
    let r2 = node.publish(&socket, &path, serde_json::json!({ "x": 7 })).await;
    assert_eq!(r2["ok"], serde_json::Value::Bool(true), "publish: {r2}");
    let tick_after_publish = r2["result"]["tick"].as_u64().unwrap();
    assert!(tick_after_publish > 0);

    // Read back the value.
    let r3 = one_shot(
        &socket,
        "substrate.read",
        serde_json::json!({ "path": path }),
    )
    .await;
    assert_eq!(r3["result"]["value"]["x"], 7);
    assert_eq!(r3["result"]["tick"], tick_after_publish);

    // Notify bumps the tick but not the value. Notify is unsigned
    // for now — tick-only signals don't carry data and the gate
    // covers value writes; if we tighten notify in a follow-up the
    // shape mirrors publish.
    let r4 = one_shot(
        &socket,
        "substrate.notify",
        serde_json::json!({ "path": path }),
    )
    .await;
    assert_eq!(r4["ok"], serde_json::Value::Bool(true));
    let tick_after_notify = r4["result"]["tick"].as_u64().unwrap();
    assert!(tick_after_notify > tick_after_publish);

    let r5 = one_shot(
        &socket,
        "substrate.read",
        serde_json::json!({ "path": path }),
    )
    .await;
    assert_eq!(r5["result"]["value"]["x"], 7);
    assert_eq!(r5["result"]["tick"], tick_after_notify);

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn substrate_list_returns_prefix_children() {
    let (_tmp, socket, shutdown_tx) = spawn_test_daemon().await;

    let node = TestNode::register(&socket, 11).await;
    let prefix = format!("substrate/{}/list-test", node.node_id());

    // Empty list before any publish.
    let empty = one_shot(
        &socket,
        "substrate.list",
        serde_json::json!({ "prefix": prefix, "depth": 1 }),
    )
    .await;
    assert_eq!(empty["ok"], serde_json::Value::Bool(true));
    assert!(empty["result"]["children"].as_array().unwrap().is_empty());

    // Seed two children and one grandchild — all under the test
    // node's prefix so the gate accepts them.
    let paths_to_publish = [
        (node.path("list-test/mic"), serde_json::json!({ "rms_db": -20 })),
        (node.path("list-test/tof"), serde_json::json!({ "frame": 1 })),
        (
            node.path("list-test/mic/history"),
            serde_json::json!([1, 2, 3]),
        ),
    ];
    for (path, value) in &paths_to_publish {
        let r = node.publish(&socket, path, value.clone()).await;
        assert_eq!(r["ok"], serde_json::Value::Bool(true), "publish {path}: {r}");
    }

    let mic_path = node.path("list-test/mic");
    let tof_path = node.path("list-test/tof");
    let history_path = node.path("list-test/mic/history");

    // depth = 1 (default) — expect two direct children, mic having one grandchild.
    let r = one_shot(
        &socket,
        "substrate.list",
        serde_json::json!({ "prefix": prefix, "depth": 1 }),
    )
    .await;
    assert_eq!(r["ok"], serde_json::Value::Bool(true));
    let children = r["result"]["children"].as_array().unwrap();
    assert_eq!(children.len(), 2, "{children:?}");
    let mic = children.iter().find(|c| c["path"] == mic_path).unwrap();
    assert_eq!(mic["has_value"], serde_json::Value::Bool(true));
    assert_eq!(mic["child_count"], 1);
    let tof = children.iter().find(|c| c["path"] == tof_path).unwrap();
    assert_eq!(tof["has_value"], serde_json::Value::Bool(true));
    assert_eq!(tof["child_count"], 0);
    assert!(r["result"]["tick"].as_u64().unwrap() > 0);

    // Default depth: omit field → treat as 1 (per protocol default).
    let r2 = one_shot(
        &socket,
        "substrate.list",
        serde_json::json!({ "prefix": prefix }),
    )
    .await;
    assert_eq!(r2["result"]["children"].as_array().unwrap().len(), 2);

    // depth = 2 — flat list including the grandchild.
    let r3 = one_shot(
        &socket,
        "substrate.list",
        serde_json::json!({ "prefix": prefix, "depth": 2 }),
    )
    .await;
    let paths: Vec<String> = r3["result"]["children"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["path"].as_str().unwrap().to_string())
        .collect();
    assert!(paths.contains(&history_path));

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn substrate_subscribe_streams_updates() {
    let (_tmp, socket, shutdown_tx) = spawn_test_daemon().await;
    let node = TestNode::register(&socket, 13).await;
    let path = node.path("test/stream");

    // Open a streaming subscribe.
    let stream = UnixStream::connect(&socket).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let sub = serde_json::json!({
        "id": "sub",
        "method": "substrate.subscribe",
        "params": { "path": path },
    });
    let mut line = serde_json::to_string(&sub).unwrap();
    line.push('\n');
    writer.write_all(line.as_bytes()).await.unwrap();
    let mut ack = String::new();
    reader.read_line(&mut ack).await.unwrap();
    let ack_v: serde_json::Value = serde_json::from_str(ack.trim()).unwrap();
    assert_eq!(ack_v["ok"], serde_json::Value::Bool(true));

    // Publish twice; subscriber should see both in order.
    let pub_resp = node.publish(&socket, &path, serde_json::json!(1)).await;
    assert_eq!(pub_resp["ok"], serde_json::Value::Bool(true), "publish: {pub_resp}");
    one_shot(
        &socket,
        "substrate.notify",
        serde_json::json!({ "path": path }),
    )
    .await;

    let mut buf1 = String::new();
    tokio::time::timeout(Duration::from_secs(2), reader.read_line(&mut buf1))
        .await
        .unwrap()
        .unwrap();
    let first: serde_json::Value = serde_json::from_str(buf1.trim()).unwrap();
    assert_eq!(first["kind"], "publish");
    assert_eq!(first["value"], 1);

    let mut buf2 = String::new();
    tokio::time::timeout(Duration::from_secs(2), reader.read_line(&mut buf2))
        .await
        .unwrap()
        .unwrap();
    let second: serde_json::Value = serde_json::from_str(buf2.trim()).unwrap();
    assert_eq!(second["kind"], "notify");

    let _ = shutdown_tx.send(true);
}

// ── node-identity write gate ───────────────────────────────────

#[tokio::test]
async fn substrate_publish_rejects_unsigned() {
    let (_tmp, socket, shutdown_tx) = spawn_test_daemon().await;

    // Bare publish with no node_id — must be rejected outright.
    let r = one_shot(
        &socket,
        "substrate.publish",
        serde_json::json!({ "path": "substrate/anywhere", "value": 1 }),
    )
    .await;
    assert_eq!(r["ok"], serde_json::Value::Bool(false));
    let err = r["error"].as_str().unwrap_or("");
    assert!(err.contains("node_id"), "expected node_id error, got: {err}");

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn substrate_publish_rejects_cross_node_write() {
    let (_tmp, socket, shutdown_tx) = spawn_test_daemon().await;
    let alice = TestNode::register(&socket, 21).await;
    let bob = TestNode::register(&socket, 22).await;

    // Alice tries to write under Bob's prefix — must be rejected.
    let bob_path = format!("substrate/{}/sneaky", bob.node_id());
    let r = alice.publish(&socket, &bob_path, serde_json::json!("hi")).await;
    assert_eq!(r["ok"], serde_json::Value::Bool(false));
    let err = r["error"].as_str().unwrap_or("");
    assert!(
        err.contains("gate denied") || err.contains("must sit under"),
        "expected gate-denied error, got: {err}"
    );

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn substrate_publish_rejects_top_level_write() {
    let (_tmp, socket, shutdown_tx) = spawn_test_daemon().await;
    let node = TestNode::register(&socket, 23).await;

    // A path that doesn't sit under `substrate/<node-id>/` is
    // rejected even with valid signature, because the gate enforces
    // the prefix rule.
    let pk = node.sk.verifying_key().to_bytes();
    let ts: u64 = 1_700_000_500;
    let path = "substrate/legacy-flat/value"; // wrong shape
    let value = serde_json::json!(0);
    let value_bytes = serde_json::to_vec(&value).unwrap();
    let value_str = String::from_utf8_lossy(&value_bytes);
    let payload =
        clawft_kernel::node_publish_payload(path, &value_str, ts, &node.node_id);
    let sig = node.sk.sign(&payload);
    // sanity: avoid unused warning on pk
    let _ = pk;
    let r = one_shot(
        &socket,
        "substrate.publish",
        serde_json::json!({
            "path": path,
            "value": value,
            "node_id": node.node_id,
            "node_signature": hex(&sig.to_bytes()),
            "node_ts": ts,
        }),
    )
    .await;
    assert_eq!(r["ok"], serde_json::Value::Bool(false));
    let err = r["error"].as_str().unwrap_or("");
    assert!(
        err.contains("gate denied"),
        "expected gate-denied, got: {err}"
    );

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn substrate_canonical_publish_payload_echoes_verifier_input() {
    let (_tmp, socket, shutdown_tx) = spawn_test_daemon().await;

    // The exact firmware-side worked example from the dialog file:
    // path / value / ts / node_id known; expected canonical value
    // JSON is alphabetically ordered.
    let path = "substrate/n-bfc4cd/sensor/mic/rms";
    let value = serde_json::json!({
        "rms_db":            -26.4,
        "peak_db":           -12.1,
        "sample_rate":       16000,
        "available":         true,
        "samples_in_window": 16000,
        "characterization":  "Rate",
    });
    let node_ts: u64 = 12345;
    let node_id = "n-bfc4cd";

    let r = one_shot(
        &socket,
        "substrate.canonical_publish_payload",
        serde_json::json!({
            "path": path,
            "value": value,
            "node_id": node_id,
            "node_ts": node_ts,
        }),
    )
    .await;
    assert_eq!(r["ok"], serde_json::Value::Bool(true), "{r}");

    let got_canonical = r["result"]["canonical_value_json"].as_str().unwrap();
    let expected_canonical = r#"{"available":true,"characterization":"Rate","peak_db":-12.1,"rms_db":-26.4,"sample_rate":16000,"samples_in_window":16000}"#;
    assert_eq!(got_canonical, expected_canonical);

    // Length matches the worked example: 23 + path(33) + 1 + value(121)
    // + 1 + 8 + 1 + node_id(8) = 196.
    let got_len = r["result"]["payload_len"].as_u64().unwrap();
    assert_eq!(got_len, 196);

    // Hex prefix matches "substrate.publish.node\0" → 73 75 62 …
    let hex = r["result"]["payload_hex"].as_str().unwrap();
    assert!(hex.starts_with("7375627374726174652e7075626c6973682e6e6f646500"));
    // Trailing 8 bytes are the node_id "n-bfc4cd".
    assert!(hex.ends_with(&hex_str(node_id.as_bytes())));

    let _ = shutdown_tx.send(true);
}

fn hex_str(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        s.push_str(&format!("{x:02x}"));
    }
    s
}

#[tokio::test]
async fn substrate_canonical_publish_payload_matches_real_publish_payload() {
    let (_tmp, socket, shutdown_tx) = spawn_test_daemon().await;
    let node = TestNode::register(&socket, 33).await;
    let path = node.path("test/round-trip");
    let value = serde_json::json!({"z": 1, "a": 2, "m": 3});
    let ts = 1_700_000_000u64;

    // Echo RPC: returns the bytes the daemon would verify.
    let echo = one_shot(
        &socket,
        "substrate.canonical_publish_payload",
        serde_json::json!({
            "path": path,
            "value": value,
            "node_id": node.node_id,
            "node_ts": ts,
        }),
    )
    .await;
    assert_eq!(echo["ok"], serde_json::Value::Bool(true));
    let echoed_hex = echo["result"]["payload_hex"].as_str().unwrap().to_string();

    // Compute the same bytes locally via the public kernel helper,
    // mirroring what the daemon does on a real publish.
    let value_bytes = serde_json::to_vec(&value).unwrap();
    let value_str = String::from_utf8_lossy(&value_bytes);
    let local_payload =
        clawft_kernel::node_publish_payload(&path, &value_str, ts, &node.node_id);
    let local_hex = hex_str(&local_payload);

    assert_eq!(
        echoed_hex, local_hex,
        "canonical_publish_payload must match what publish_gated would verify"
    );

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn substrate_publish_unknown_node_rejected() {
    let (_tmp, socket, shutdown_tx) = spawn_test_daemon().await;

    // Forged node_id that was never registered.
    let r = one_shot(
        &socket,
        "substrate.publish",
        serde_json::json!({
            "path": "substrate/n-deadbe/x",
            "value": 1,
            "node_id": "n-deadbe",
            "node_signature": hex(&[0u8; 64]),
            "node_ts": 1_700_000_000_u64,
        }),
    )
    .await;
    assert_eq!(r["ok"], serde_json::Value::Bool(false));
    let err = r["error"].as_str().unwrap_or("");
    assert!(
        err.contains("unknown node_id") || err.contains("unauthorized"),
        "expected unknown-node error, got: {err}"
    );

    let _ = shutdown_tx.send(true);
}
