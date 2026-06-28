//! Integration test: external-socket streaming subscribers.
//!
//! Spawns a daemon listener on a temp Unix socket, then runs two
//! clients: one calls `ipc.subscribe_stream` on a topic; the other
//! calls `ipc.publish` on the same topic. Asserts the streaming
//! client receives the published message.
//!
//! This is the Commit 1 acceptance test — external processes
//! (Python bridge, another Claude Code session) can now subscribe.

use std::sync::Arc;
use std::time::Duration;

use clawft_kernel::boot::Kernel;
use clawft_platform::NativePlatform;
use clawft_types::config::{AgentDefaults, AgentsConfig, Config, KernelConfig};
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
                    if *shutdown_rx.borrow() {
                        break;
                    }
                }
            }
        }
    });

    // Tiny settle so the listener is ready.
    tokio::time::sleep(Duration::from_millis(50)).await;

    (tmp, socket_path, shutdown_tx)
}

async fn send_request(
    socket: &std::path::Path,
    method: &str,
    params: serde_json::Value,
) -> (
    BufReader<tokio::net::unix::OwnedReadHalf>,
    tokio::net::unix::OwnedWriteHalf,
    serde_json::Value,
) {
    let stream = UnixStream::connect(socket).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let req = serde_json::json!({
        "id": "test-1",
        "method": method,
        "params": params,
    });
    let mut line = serde_json::to_string(&req).unwrap();
    line.push('\n');
    writer.write_all(line.as_bytes()).await.unwrap();

    let mut ack = String::new();
    reader.read_line(&mut ack).await.unwrap();
    let ack_json: serde_json::Value = serde_json::from_str(ack.trim()).unwrap();
    (reader, writer, ack_json)
}

#[tokio::test]
async fn external_subscriber_receives_publish() {
    let (_tmp, socket, shutdown_tx) = spawn_test_daemon().await;

    // Client A: subscribe_stream on topic "t1"
    let (mut reader_a, _writer_a, ack_a) = send_request(
        &socket,
        "ipc.subscribe_stream",
        serde_json::json!({ "topic": "t1" }),
    )
    .await;
    assert_eq!(ack_a["ok"], serde_json::Value::Bool(true), "ack: {ack_a}");
    assert_eq!(ack_a["result"]["streaming"], serde_json::Value::Bool(true));

    // Client B: publish on "t1"
    let (_reader_b, _writer_b, ack_b) = send_request(
        &socket,
        "ipc.publish",
        serde_json::json!({ "topic": "t1", "message": "hello world" }),
    )
    .await;
    assert_eq!(
        ack_b["ok"],
        serde_json::Value::Bool(true),
        "publish ack: {ack_b}"
    );

    // Client A should now receive the published message as a JSON line.
    let mut line = String::new();
    let read = tokio::time::timeout(Duration::from_secs(2), reader_a.read_line(&mut line))
        .await
        .expect("stream subscriber should receive within 2s");
    let n = read.unwrap();
    assert!(n > 0, "received empty line from stream");

    let received: serde_json::Value = serde_json::from_str(line.trim()).expect("valid JSON");
    // The kernel forwards the full KernelMessage envelope; payload is Text("hello world")
    let payload = &received["payload"];
    assert!(
        payload["Text"] == "hello world" || payload["Json"] == "hello world",
        "expected text payload, got: {received}"
    );

    // Tear down
    let _ = shutdown_tx.send(true);
}
