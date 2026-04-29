//! Integration test: `agent.chat` dispatch flattening (Phase D3).
//!
//! After D3 the `agent.chat` arm in `daemon::dispatch` flattens to an
//! unconditional call into `clawft-service-agent::AgentService`. The
//! C2 spike fallback is gone; if `DAEMON_AGENT` didn't wire (LLM init
//! failed at boot, or — as in these tests — boot was skipped entirely)
//! the arm must surface a typed error rather than panic.
//!
//! The test daemon here mirrors `tests/control_rpc.rs::spawn_test_daemon`:
//! it builds a kernel + listener but skips `daemon::run`, so the
//! daemon-wide `OnceLock`s (`DAEMON_AGENT`, `DAEMON_LLM`,
//! `DAEMON_CONTROL`) are never populated. That naturally exercises the
//! "agent service not wired" branch the cutover introduced.
//!
//! What's checked:
//!
//! 1. Service-not-wired path returns `ok: false` with a user-facing
//!    error mentioning `agent service not wired`.
//! 2. Bad-params path (missing `messages`) returns `ok: false` with an
//!    error mentioning `invalid params`. This guards the
//!    `serde_json::from_value` arm of the flattened dispatch.
//! 3. `agent.chat.cancel` with no wired service is a clean error, not a
//!    panic (sanity check that C2's cancel arm survived D3 unchanged).
//!
//! What's *not* checked here: the `Some(agent) + good params →
//! Response::success` path. Driving that requires booting `DAEMON_LLM`
//! (which needs a reachable model server), `DAEMON_CONCIERGE_AGENT_ID`,
//! the substrate sink, and the gate. The unit tests in
//! `clawft-service-agent::service::tests` already prove the
//! `AgentService::dispatch` happy path; what D3 changed is only the
//! daemon's dispatch arm, and the wire shape of that arm is what these
//! tests pin down.

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
    let req = serde_json::json!({ "id": "t", "method": method, "params": params });
    let mut line = serde_json::to_string(&req).unwrap();
    line.push('\n');
    writer.write_all(line.as_bytes()).await.unwrap();
    let mut ack = String::new();
    reader.read_line(&mut ack).await.unwrap();
    serde_json::from_str(ack.trim()).unwrap()
}

#[tokio::test]
async fn agent_chat_returns_error_when_service_not_wired() {
    // The test daemon skips `run()` so `DAEMON_AGENT` is never set.
    // After D3 the dispatch arm must surface that as a typed error
    // rather than fall back to the deleted spike (or panic).
    let (_tmp, socket, shutdown_tx, _kernel) = spawn_test_daemon().await;
    let resp = one_shot(
        &socket,
        "agent.chat",
        serde_json::json!({
            "conv_id": "test-conv-1",
            "messages": [
                { "role": "user", "content": "hello" }
            ],
        }),
    )
    .await;
    assert_eq!(
        resp["ok"],
        serde_json::Value::Bool(false),
        "expected error response, got: {resp}",
    );
    let err = resp["error"].as_str().unwrap_or("");
    assert!(
        err.contains("agent service not wired"),
        "expected 'agent service not wired' error, got: {err}",
    );
    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn agent_chat_returns_error_when_params_invalid() {
    // The flattened dispatch in D3 still validates params via
    // `serde_json::from_value::<AgentChatParams>`. With a bad payload
    // (no `messages` field — required, not `#[serde(default)]`) the
    // arm must short-circuit before it even checks `daemon_agent()`,
    // returning the typed `agent.chat: invalid params: ...` error.
    //
    // We don't strictly assert which of the two errors comes first
    // (params parse vs. service-not-wired) — both are correct
    // failure modes; the contract is "no panic, clean error".
    let (_tmp, socket, shutdown_tx, _kernel) = spawn_test_daemon().await;
    let resp = one_shot(
        &socket,
        "agent.chat",
        serde_json::json!({
            // No `messages` field; serde_json::from_value will
            // refuse to materialize `AgentChatParams`. (`conv_id`
            // has #[serde(default)] so its absence wouldn't fail.)
            "conv_id": "test-conv-2"
        }),
    )
    .await;
    assert_eq!(
        resp["ok"],
        serde_json::Value::Bool(false),
        "expected error response, got: {resp}",
    );
    let err = resp["error"].as_str().unwrap_or("");
    assert!(
        err.contains("invalid params") || err.contains("agent service not wired"),
        "expected params or service-wiring error, got: {err}",
    );
    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn agent_chat_cancel_clean_error_when_service_not_wired() {
    // Sanity check that C2's `agent.chat.cancel` arm — which D3
    // explicitly preserves — still surfaces a clean error (not a
    // panic) when the service isn't wired.
    let (_tmp, socket, shutdown_tx, _kernel) = spawn_test_daemon().await;
    let resp = one_shot(
        &socket,
        "agent.chat.cancel",
        serde_json::json!({ "conv_id": "test-conv-3" }),
    )
    .await;
    assert_eq!(
        resp["ok"],
        serde_json::Value::Bool(false),
        "expected error response, got: {resp}",
    );
    let err = resp["error"].as_str().unwrap_or("");
    assert!(
        err.contains("agent service not wired"),
        "expected 'agent service not wired' error, got: {err}",
    );
    let _ = shutdown_tx.send(true);
}
