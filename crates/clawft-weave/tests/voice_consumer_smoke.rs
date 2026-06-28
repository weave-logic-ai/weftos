//! Integration test: voice transcript consumer (WEFT-555 / M5-W).
//!
//! Verifies the substrate-side STT → agent-conversation / command-
//! dispatch bridge:
//!
//! 1. **Chat path** — a synthetic transcript published to the configured
//!    substrate topic lands as a recorded `VoiceChatTurn` on the
//!    in-process chat handler within 2 seconds.
//! 2. **Command path** — a transcript starting with `weft ` lands as a
//!    recorded command dispatch (method + params) on the in-process
//!    command handler within 2 seconds.
//!
//! Booting the full daemon (with `DAEMON_AGENT` wired) requires a live
//! LLM service and the kernel `agent_registry` setup; both are
//! out-of-scope for a smoke test. This test instead drives the
//! `voice_router::VoiceRouter` directly against a real
//! `clawft_kernel::SubstrateService`, with stub handlers in place of
//! the daemon's `AgentService` / `dispatch` wiring. The daemon-side
//! glue (`DaemonAgentChatHandler`, `DaemonCommandHandler`) is exercised
//! via the unit tests in `voice_router` and the daemon's existing
//! `agent_chat_dispatch` integration test; this smoke test pins the
//! end-to-end substrate → router → handler shape.

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use clawft_kernel::SubstrateService;
use clawft_weave::voice_router::{
    ChatHandler, CommandHandler, VoiceChatTurn, VoiceLevel, VoicePermissions, VoiceRouter,
    VoiceRouterConfig,
};
use serde_json::{Value, json};
use tokio::time::timeout;

const TOPIC: &str = "substrate/_derived/transcript/n-test/mic";

#[derive(Default)]
struct RecordingChat {
    calls: Mutex<Vec<VoiceChatTurn>>,
}

#[async_trait]
impl ChatHandler for RecordingChat {
    async fn dispatch_chat(&self, turn: VoiceChatTurn) -> Result<(), String> {
        self.calls.lock().unwrap().push(turn);
        Ok(())
    }
}

#[derive(Default)]
struct RecordingCommands {
    calls: Mutex<Vec<(String, Value)>>,
}

#[async_trait]
impl CommandHandler for RecordingCommands {
    async fn dispatch_command(&self, method: String, params: Value) -> Result<Value, String> {
        self.calls.lock().unwrap().push((method, params));
        Ok(json!({"ok": true}))
    }
}

fn router_config() -> VoiceRouterConfig {
    // Smoke test predates SC-4; grant Level 2 by default so the
    // command-path assertion still exercises the routing seam end-to-
    // end. SC-4-specific permission gating has dedicated tests in the
    // voice_router unit-test module.
    let mut permissions = VoicePermissions::default();
    permissions.default_level = VoiceLevel::Level2;
    VoiceRouterConfig {
        transcript_topic: TOPIC.into(),
        chat_target_agent: "concierge-bot".into(),
        conv_id: "voice-smoke".into(),
        command_prefix: "weft ".into(),
        subscriber_id: Some("daemon".into()),
        permissions,
    }
}

/// Wait up to `deadline` for `predicate` to hold, polling every 25ms.
/// Returns `true` if the predicate held, `false` on timeout.
async fn wait_until<F>(deadline: Duration, mut predicate: F) -> bool
where
    F: FnMut() -> bool,
{
    let start = std::time::Instant::now();
    while start.elapsed() < deadline {
        if predicate() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    predicate()
}

#[tokio::test]
async fn chat_path_lands_transcript_on_handler() {
    let substrate = SubstrateService::new();
    let chat = Arc::new(RecordingChat::default());
    let cmd = Arc::new(RecordingCommands::default());

    let substrate_for_subscribe = substrate.clone();
    let router = VoiceRouter::spawn(
        router_config(),
        |caller, path| {
            substrate_for_subscribe
                .subscribe(caller, path)
                .map(|(_id, rx)| rx)
                .map_err(|e| format!("subscribe: {e}"))
        },
        chat.clone(),
        cmd.clone(),
    )
    .expect("spawn voice router");

    // Give the subscription task a moment to register before we
    // publish — the SubstrateService creates the path entry on first
    // subscribe and we want our publish to fan out to the consumer.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Publish a synthetic transcript directly to the topic. The shape
    // mirrors what `clawft_service_whisper::service::handle_inference_result`
    // emits.
    let tick = substrate.publish(
        Some("daemon"),
        TOPIC,
        json!({
            "text": "what is the weather",
            "start_ms": 0,
            "end_ms": 2000,
            "confidence": 0.91,
            "lang": "en",
            "seq": 1,
        }),
    );
    assert!(tick > 0, "publish should bump the tick");

    // The consumer needs at most a handful of tokio yields to drain
    // the subscription line and call into the handler. 2s gives wide
    // margin for a slow CI runner.
    let landed = wait_until(Duration::from_secs(2), || {
        !chat.calls.lock().unwrap().is_empty()
    })
    .await;
    assert!(landed, "chat handler did not receive transcript within 2s");

    let calls = chat.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].text, "what is the weather");
    assert_eq!(calls[0].target_agent, "concierge-bot");
    assert_eq!(calls[0].conv_id, "voice-smoke");
    assert_eq!(calls[0].metadata.source, "voice");
    assert_eq!(calls[0].metadata.transcript_topic, TOPIC);
    assert!(
        (calls[0].metadata.confidence.unwrap() - 0.91).abs() < 1e-9,
        "confidence threaded through"
    );
    assert!(
        cmd.calls.lock().unwrap().is_empty(),
        "non-command transcript must not reach the command handler"
    );

    // Ensure shutdown is clean.
    timeout(Duration::from_secs(2), router.shutdown())
        .await
        .expect("voice router shutdown should not hang");
}

#[tokio::test]
async fn command_path_dispatches_through_command_handler() {
    let substrate = SubstrateService::new();
    let chat = Arc::new(RecordingChat::default());
    let cmd = Arc::new(RecordingCommands::default());

    let substrate_for_subscribe = substrate.clone();
    let router = VoiceRouter::spawn(
        router_config(),
        |caller, path| {
            substrate_for_subscribe
                .subscribe(caller, path)
                .map(|(_id, rx)| rx)
                .map_err(|e| format!("subscribe: {e}"))
        },
        chat.clone(),
        cmd.clone(),
    )
    .expect("spawn voice router");

    tokio::time::sleep(Duration::from_millis(50)).await;

    // "weft hello" should strip the prefix and dispatch verb=hello.
    let _ = substrate.publish(
        Some("daemon"),
        TOPIC,
        json!({
            "text": "weft hello",
            "start_ms": 0,
            "end_ms": 1500,
            "confidence": null,
            "lang": "en",
            "seq": 2,
        }),
    );

    let landed = wait_until(Duration::from_secs(2), || {
        !cmd.calls.lock().unwrap().is_empty()
    })
    .await;
    assert!(landed, "command handler did not receive dispatch within 2s");

    let calls = cmd.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "hello");
    assert_eq!(calls[0].1, json!({"args": []}));
    assert!(
        chat.calls.lock().unwrap().is_empty(),
        "command transcript must not also fire chat dispatch"
    );

    timeout(Duration::from_secs(2), router.shutdown())
        .await
        .expect("voice router shutdown should not hang");
}

#[tokio::test]
async fn command_path_passes_remainder_as_args() {
    let substrate = SubstrateService::new();
    let chat = Arc::new(RecordingChat::default());
    let cmd = Arc::new(RecordingCommands::default());

    let substrate_for_subscribe = substrate.clone();
    let router = VoiceRouter::spawn(
        router_config(),
        |caller, path| {
            substrate_for_subscribe
                .subscribe(caller, path)
                .map(|(_id, rx)| rx)
                .map_err(|e| format!("subscribe: {e}"))
        },
        chat.clone(),
        cmd.clone(),
    )
    .expect("spawn voice router");

    tokio::time::sleep(Duration::from_millis(50)).await;

    let _ = substrate.publish(
        Some("daemon"),
        TOPIC,
        json!({
            "text": "weft kernel.status verbose",
            "start_ms": 0,
            "end_ms": 2000,
            "confidence": null,
            "lang": "en",
            "seq": 3,
        }),
    );

    let landed = wait_until(Duration::from_secs(2), || {
        !cmd.calls.lock().unwrap().is_empty()
    })
    .await;
    assert!(landed, "command not dispatched within 2s");

    let calls = cmd.calls.lock().unwrap();
    assert_eq!(calls[0].0, "kernel.status");
    assert_eq!(calls[0].1, json!({"args": ["verbose"]}));

    timeout(Duration::from_secs(2), router.shutdown())
        .await
        .expect("voice router shutdown should not hang");
}
