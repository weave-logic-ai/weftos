//! Integration test: `kernel.logs_stream` (WEFT-434).
//!
//! Spawns a daemon on a temp Unix socket, opens a streaming log
//! subscription, pushes new event-log entries, and asserts the
//! subscriber receives them as line-delimited JSON with monotonic
//! `seq` fields.

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
        governance: Default::default(),
        profile: None,
        shared_services: Default::default(),
        enabled: true,
        max_processes: 64,
        health_check_interval_secs: 5,
        brand: "WeftOS".to_string(),
        cluster: None,
        // Never the operator chain under ~/.clawft (isolated per boot).
        chain: Some(clawft_types::config::ChainConfig::isolated_in(
            &tempfile::tempdir().unwrap().keep(),
        )),
        resource_tree: None,
        vector: None,
        spatial: None,
        profiles: None,
        pairing: None,
        mesh: None,
        anchor: None,
        ipc_tcp: None,
        llm: None,
        agent: None,
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
                    if *shutdown_rx.borrow() {
                        break;
                    }
                }
            }
        }
    });

    tokio::time::sleep(Duration::from_millis(50)).await;
    (tmp, socket_path, shutdown_tx, kernel)
}

#[tokio::test]
async fn logs_stream_delivers_live_entries_with_seq() {
    let (_tmp, socket, shutdown_tx, kernel) = spawn_test_daemon().await;

    // Seed a couple of entries before subscribe so the initial tail
    // path is exercised (count > 0).
    {
        let k = kernel.read().await;
        k.event_log().info("test", "seed-one");
        k.event_log().info("test", "seed-two");
    }

    let stream = UnixStream::connect(&socket).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let req = serde_json::json!({
        "id": "logs-1",
        "method": "kernel.logs_stream",
        "params": { "count": 10 },
    });
    let mut line = serde_json::to_string(&req).unwrap();
    line.push('\n');
    writer.write_all(line.as_bytes()).await.unwrap();

    let mut ack = String::new();
    reader.read_line(&mut ack).await.unwrap();
    let ack_v: serde_json::Value = serde_json::from_str(ack.trim()).unwrap();
    assert_eq!(
        ack_v["ok"],
        serde_json::Value::Bool(true),
        "stream ack: {ack_v}"
    );
    assert_eq!(ack_v["result"]["streaming"], serde_json::Value::Bool(true));
    let initial = ack_v["result"]["initial_count"].as_u64().unwrap_or(0);
    assert!(
        initial >= 2,
        "expected at least the two seed entries in the initial tail, got {initial}"
    );

    // Drain initial tail frames.
    let mut max_seq = 0u64;
    let mut saw_seed = false;
    for _ in 0..initial {
        let mut frame = String::new();
        tokio::time::timeout(Duration::from_secs(2), reader.read_line(&mut frame))
            .await
            .expect("initial tail frame timeout")
            .unwrap();
        let entry: serde_json::Value = serde_json::from_str(frame.trim()).unwrap();
        if let Some(seq) = entry["seq"].as_u64() {
            max_seq = max_seq.max(seq);
        }
        if entry["message"]
            .as_str()
            .is_some_and(|m| m.contains("seed-two"))
        {
            saw_seed = true;
        }
    }
    assert!(saw_seed, "initial tail should include seed-two");
    assert!(max_seq > 0, "entries should carry monotonic seq");

    // Push a live entry after subscribe; stream must deliver it.
    {
        let k = kernel.read().await;
        k.event_log().info("test", "live-after-sub");
    }

    let mut live = String::new();
    tokio::time::timeout(Duration::from_secs(2), reader.read_line(&mut live))
        .await
        .expect("live frame should arrive within 2s")
        .unwrap();
    let live_entry: serde_json::Value = serde_json::from_str(live.trim()).unwrap();
    assert!(
        live_entry["message"]
            .as_str()
            .is_some_and(|m| m.contains("live-after-sub")),
        "expected live-after-sub, got: {live_entry}"
    );
    let live_seq = live_entry["seq"].as_u64().unwrap_or(0);
    assert!(
        live_seq > max_seq,
        "live seq {live_seq} should exceed max initial seq {max_seq}"
    );

    let _ = shutdown_tx.send(true);
}
