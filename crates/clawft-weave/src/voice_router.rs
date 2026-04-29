//! Voice transcript consumer / router (WEFT-555, M5-W).
//!
//! ## What this module does
//!
//! Bridges the substrate-side STT pipeline (sensor → PCM → whisper →
//! transcript topic) into the agent surface that humans actually
//! interact with:
//!
//! ```text
//!   sensor (ESP32)        whisper service                voice_router
//!   ┌─────────────┐       ┌────────────────┐             ┌──────────┐
//!   │ pcm_chunk   │──────▶│ /inference     │────────────▶│ subscribe│
//!   │ (substrate) │       │ → transcript   │  substrate  │ + route  │
//!   └─────────────┘       └────────────────┘   (this     └────┬─────┘
//!                                                topic)       │
//!                                                ┌────────────┴────┐
//!                                                ▼                 ▼
//!                                       agent.chat dispatch    daemon.dispatch
//!                                       (concierge-bot)        (weft <verb> ...)
//! ```
//!
//! Per ADR-053 the substrate-side `clawft-service-whisper` is the
//! canonical STT path; the consumer is intentionally decoupled from
//! the backend so swapping STT engines (whisper, sherpa-onnx, cloud
//! Whisper) does not change the agent-facing surface.
//!
//! ## Routing
//!
//! For each transcript line published on
//! `config.voice.consumer.transcript_topic`, the consumer:
//!
//! 1. Parses the substrate update line as a publish-kind delta.
//! 2. Extracts `value.text` (whisper's transcript shape).
//! 3. If the text starts with `config.voice.consumer.command_prefix`
//!    (`"weft "` by default), strips the prefix and dispatches the
//!    remainder as `<method> <args...>` through [`CommandHandler`].
//! 4. Otherwise dispatches the text as a one-shot user turn through
//!    [`ChatHandler`] against `config.voice.consumer.chat_target_agent`
//!    (`concierge-bot` by default), with metadata
//!    `{ source: "voice", transcript_topic, confidence }`.
//!
//! ## Security gating (WEFT-207/208/209/210/211)
//!
//! This module implements the routing seam; the 5 P0 voice security
//! controls slot in here:
//!
//! - **WEFT-207** (sensor enrollment) — gate on the source node's
//!   enrollment status before accepting a transcript.
//! - **WEFT-208** (command authorization) — gate on per-verb
//!   permissions before dispatching a `weft <verb>` transcript.
//! - **WEFT-209** (rate limit / floods) — token-bucket limiter around
//!   the dispatch path so a stuck mic can't DoS the agent loop.
//! - **WEFT-210** (audit log) — append every routed transcript to the
//!   substrate audit chain with source attribution.
//! - **WEFT-211** (privacy / redaction) — pre-dispatch redaction pass
//!   on the transcript text before either path consumes it.
//!
//! Every gate is a strict superset of "do nothing extra"; the consumer
//! ships in this commit with no gating beyond the on/off flag, and
//! each control replaces a clearly-marked stub below.
//!
//! ## Testability
//!
//! The router is generic over [`ChatHandler`] and [`CommandHandler`]
//! so tests can wire in stubs that record dispatch calls without
//! booting the full LLM service / kernel RPC surface. The production
//! daemon wires
//! [`DaemonAgentChatHandler`] (calls `daemon_agent().dispatch()`) and
//! [`DaemonCommandHandler`] (calls into the daemon's `dispatch`
//! function via an injected closure).

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::{mpsc, watch};
use tracing::{debug, info, warn};

/// Configuration snapshot the router consumes at spawn time. Mirrors
/// the load-bearing fields of [`clawft_types::config::VoiceConsumerConfig`]
/// without coupling the trait surface to the full config shape — keeps
/// unit tests cheap.
#[derive(Debug, Clone)]
pub struct VoiceRouterConfig {
    /// Substrate path to subscribe to.
    pub transcript_topic: String,
    /// Agent id that receives non-command transcripts.
    pub chat_target_agent: String,
    /// Stable conversation id for the voice session.
    pub conv_id: String,
    /// Command prefix; empty string disables command routing.
    pub command_prefix: String,
    /// Optional caller id for the substrate subscription. Production
    /// wires the daemon's own node-id; tests use `None` (or any
    /// non-`None` value — capture-tier subscribe accepts any caller).
    pub subscriber_id: Option<String>,
}

/// Inbound chat request synthesized from a single transcript.
///
/// Production hands this to `clawft_service_agent::AgentService::dispatch`
/// after wrapping it in the wire-format `AgentChatParams`. Tests use
/// it as the assertion shape for the `ChatHandler` stub.
#[derive(Debug, Clone)]
pub struct VoiceChatTurn {
    /// Target agent id (e.g. `concierge-bot`).
    pub target_agent: String,
    /// Stable conversation id for the voice session.
    pub conv_id: String,
    /// Transcript text (already prefix-stripped on the command path —
    /// chat path passes through unchanged).
    pub text: String,
    /// Source attribution metadata. Populated with the transcript
    /// topic and confidence (when whisper provides one).
    pub metadata: VoiceTurnMetadata,
}

/// Metadata attached to a voice-originated turn. Surfaced via the
/// `system`-role message the chat handler injects so the agent loop's
/// system prompt sees the source attribution without changing the
/// `agent.chat` wire shape.
#[derive(Debug, Clone)]
pub struct VoiceTurnMetadata {
    /// Always `"voice"` today; future audio-source plugins (e.g.
    /// dictation, multi-mic) supply alternative sources.
    pub source: &'static str,
    /// Substrate path the transcript was published on.
    pub transcript_topic: String,
    /// Whisper-reported confidence, or `None` when the response_format
    /// in use does not carry a confidence field.
    pub confidence: Option<f64>,
}

/// Trait for dispatching a transcript into an agent's conversation.
/// The router does not own the agent service; daemon and tests inject
/// their own implementations.
#[async_trait]
pub trait ChatHandler: Send + Sync + 'static {
    /// Push `turn` onto the target agent's chat surface. Errors are
    /// logged + dropped; one bad transcript does not abort the loop.
    async fn dispatch_chat(&self, turn: VoiceChatTurn) -> Result<(), String>;
}

/// Trait for dispatching a parsed `weft <verb> <args>` transcript
/// through the daemon's RPC surface. The router does not own the
/// kernel; daemon and tests inject their own implementations.
#[async_trait]
pub trait CommandHandler: Send + Sync + 'static {
    /// Run `method` with `params` against the daemon's dispatch
    /// surface. Subject to the placeholder permission gate in
    /// [`VoiceRouter`]; production gates (WEFT-208) wrap this trait
    /// rather than the router.
    async fn dispatch_command(
        &self,
        method: String,
        params: Value,
    ) -> Result<Value, String>;
}

/// Spawned voice-router handle. Drop / call [`Self::shutdown`] to
/// terminate the background task.
pub struct VoiceRouter {
    shutdown: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}

impl VoiceRouter {
    /// Spawn the router on the caller's tokio runtime.
    ///
    /// `subscribe_fn` is a factory the router invokes once at boot to
    /// open the substrate subscription. Returning the receiver this
    /// way lets the caller (production: `daemon::run`; tests:
    /// `voice_consumer_smoke`) own the substrate handle without
    /// forcing this module to depend on `clawft-kernel` directly.
    pub fn spawn<S>(
        config: VoiceRouterConfig,
        subscribe_fn: S,
        chat: Arc<dyn ChatHandler>,
        commands: Arc<dyn CommandHandler>,
    ) -> Result<Self, String>
    where
        S: FnOnce(
            Option<&str>,
            &str,
        ) -> Result<mpsc::Receiver<Vec<u8>>, String>,
    {
        let rx = subscribe_fn(config.subscriber_id.as_deref(), &config.transcript_topic)?;
        info!(
            topic = %config.transcript_topic,
            chat_target = %config.chat_target_agent,
            conv_id = %config.conv_id,
            command_prefix = %config.command_prefix,
            "voice consumer: subscribed to transcript topic"
        );

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let task = tokio::spawn(run_loop(rx, config, chat, commands, shutdown_rx));
        Ok(Self {
            shutdown: shutdown_tx,
            task,
        })
    }

    /// Signal shutdown and await the background task.
    pub async fn shutdown(self) {
        let _ = self.shutdown.send(true);
        let _ = self.task.await;
    }
}

async fn run_loop(
    mut rx: mpsc::Receiver<Vec<u8>>,
    config: VoiceRouterConfig,
    chat: Arc<dyn ChatHandler>,
    commands: Arc<dyn CommandHandler>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            changed = shutdown_rx.changed() => {
                if changed.is_ok() && *shutdown_rx.borrow() {
                    debug!("voice consumer: shutdown requested");
                    break;
                }
            }
            line = rx.recv() => {
                let Some(bytes) = line else {
                    debug!("voice consumer: substrate sender dropped, exiting");
                    break;
                };
                handle_line(&bytes, &config, chat.as_ref(), commands.as_ref()).await;
            }
        }
    }
}

async fn handle_line(
    bytes: &[u8],
    config: &VoiceRouterConfig,
    chat: &dyn ChatHandler,
    commands: &dyn CommandHandler,
) {
    let Some(payload) = decode_publish_value(bytes) else {
        // Either a notify-kind line, or malformed JSON — both safely
        // ignored. The substrate subscription stream interleaves both
        // shapes; only publish-kind lines carry transcript bodies.
        return;
    };
    let Some(text_raw) = payload.text() else {
        debug!("voice consumer: skipping transcript with no text field");
        return;
    };
    let text = text_raw.trim();
    if text.is_empty() {
        debug!("voice consumer: skipping empty transcript");
        return;
    }

    if !config.command_prefix.is_empty()
        && let Some(cmd_body) = strip_prefix_ci(text, &config.command_prefix)
    {
        let trimmed = cmd_body.trim();
        if !trimmed.is_empty() {
            route_command(trimmed, config, commands).await;
            return;
        }
    }

    let metadata = VoiceTurnMetadata {
        source: "voice",
        transcript_topic: config.transcript_topic.clone(),
        confidence: payload.confidence,
    };
    let turn = VoiceChatTurn {
        target_agent: config.chat_target_agent.clone(),
        conv_id: config.conv_id.clone(),
        text: text.to_string(),
        metadata,
    };
    if let Err(e) = chat.dispatch_chat(turn).await {
        warn!(err = %e, "voice consumer: chat dispatch failed");
    }
}

async fn route_command(
    body: &str,
    _config: &VoiceRouterConfig,
    commands: &dyn CommandHandler,
) {
    let mut parts = body.split_whitespace();
    let Some(method) = parts.next() else {
        return;
    };
    let args: Vec<&str> = parts.collect();
    // Placeholder permission gate (WEFT-208 will replace with the
    // real per-verb authz check + audit hook).
    if !permission_stub_allows(method) {
        warn!(method, "voice consumer: command refused by placeholder gate");
        return;
    }
    // Single args-array param shape. Verbs that need richer params
    // are responsible for parsing the array; the alternative (a real
    // CLI parser) lives in the panel and is intentionally out of
    // scope for the routing seam.
    let params = serde_json::json!({ "args": args });
    match commands.dispatch_command(method.to_string(), params).await {
        Ok(_) => {
            info!(method, "voice consumer: command dispatched");
        }
        Err(e) => {
            warn!(method, err = %e, "voice consumer: command dispatch failed");
        }
    }
}

/// Placeholder permission gate. Replaced by WEFT-208 (per-verb
/// authorization and audit). Returns `true` for every method today;
/// voice command routing is opt-in via `voice.consumer.enabled` so a
/// misbehaving transcript can only reach this path when the operator
/// has explicitly turned the consumer on.
fn permission_stub_allows(_method: &str) -> bool {
    true
}

/// Strip a case-insensitive prefix. Whisper output capitalization is
/// model-dependent; treating the prefix as case-insensitive avoids
/// surprises like `"Weft status"` falling through to chat.
fn strip_prefix_ci<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    if prefix.is_empty() {
        return None;
    }
    let prefix_lower = prefix.to_lowercase();
    let text_lower = text.to_lowercase();
    if text_lower.starts_with(&prefix_lower) {
        Some(&text[prefix.len()..])
    } else {
        None
    }
}

/// Shape of the whisper-published transcript value (mirrors
/// `clawft_service_whisper::service::handle_inference_result`'s
/// payload).
#[derive(Debug, Deserialize)]
struct TranscriptPayload {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    confidence: Option<f64>,
}

impl TranscriptPayload {
    fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }
}

/// Substrate update-line shape (see
/// `clawft_kernel::substrate_service::build_update_line`):
///
/// ```json
/// {"path":"…","tick":N,"kind":"publish|notify","value":{…},"actor_id":…}
/// ```
///
/// Returns the parsed `value` payload when `kind == "publish"`. Notify
/// lines and malformed JSON return `None`; both are safely ignored
/// upstream so the subscription stream stays alive across them.
fn decode_publish_value(line: &[u8]) -> Option<TranscriptPayload> {
    let end = if line.last() == Some(&b'\n') {
        line.len() - 1
    } else {
        line.len()
    };
    let v: Value = serde_json::from_slice(&line[..end]).ok()?;
    if v.get("kind")?.as_str()? != "publish" {
        return None;
    }
    let value = v.get("value")?.clone();
    serde_json::from_value(value).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

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
        async fn dispatch_command(
            &self,
            method: String,
            params: Value,
        ) -> Result<Value, String> {
            self.calls.lock().unwrap().push((method, params));
            Ok(json!({"ok": true}))
        }
    }

    fn cfg() -> VoiceRouterConfig {
        VoiceRouterConfig {
            transcript_topic: "substrate/_derived/transcript/n-test/mic".into(),
            chat_target_agent: "concierge-bot".into(),
            conv_id: "voice-test".into(),
            command_prefix: "weft ".into(),
            subscriber_id: Some("daemon".into()),
        }
    }

    fn publish_line(value: Value) -> Vec<u8> {
        let v = json!({
            "path": "substrate/_derived/transcript/n-test/mic",
            "tick": 1,
            "kind": "publish",
            "value": value,
            "actor_id": null,
        });
        serde_json::to_vec(&v).unwrap()
    }

    #[tokio::test]
    async fn chat_path_routes_to_chat_handler() {
        let chat = Arc::new(RecordingChat::default());
        let cmd = Arc::new(RecordingCommands::default());
        let line = publish_line(json!({
            "text": "what time is it",
            "confidence": null,
        }));
        handle_line(&line, &cfg(), chat.as_ref(), cmd.as_ref()).await;
        let calls = chat.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].text, "what time is it");
        assert_eq!(calls[0].target_agent, "concierge-bot");
        assert_eq!(calls[0].conv_id, "voice-test");
        assert_eq!(calls[0].metadata.source, "voice");
        assert_eq!(
            calls[0].metadata.transcript_topic,
            "substrate/_derived/transcript/n-test/mic"
        );
        assert!(cmd.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn command_path_routes_to_command_handler() {
        let chat = Arc::new(RecordingChat::default());
        let cmd = Arc::new(RecordingCommands::default());
        let line = publish_line(json!({"text": "weft status now"}));
        handle_line(&line, &cfg(), chat.as_ref(), cmd.as_ref()).await;
        assert!(chat.calls.lock().unwrap().is_empty());
        let calls = cmd.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "status");
        assert_eq!(calls[0].1, json!({"args": ["now"]}));
    }

    #[tokio::test]
    async fn case_insensitive_prefix_match() {
        let chat = Arc::new(RecordingChat::default());
        let cmd = Arc::new(RecordingCommands::default());
        // Whisper sometimes capitalizes the first word.
        let line = publish_line(json!({"text": "Weft Hello"}));
        handle_line(&line, &cfg(), chat.as_ref(), cmd.as_ref()).await;
        assert!(chat.calls.lock().unwrap().is_empty());
        assert_eq!(cmd.calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn empty_transcript_dropped() {
        let chat = Arc::new(RecordingChat::default());
        let cmd = Arc::new(RecordingCommands::default());
        let line = publish_line(json!({"text": "   "}));
        handle_line(&line, &cfg(), chat.as_ref(), cmd.as_ref()).await;
        assert!(chat.calls.lock().unwrap().is_empty());
        assert!(cmd.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn notify_line_ignored() {
        let chat = Arc::new(RecordingChat::default());
        let cmd = Arc::new(RecordingCommands::default());
        let v = json!({
            "path": "substrate/_derived/transcript/n-test/mic",
            "tick": 1,
            "kind": "notify",
            "value": null,
            "actor_id": null,
        });
        let line = serde_json::to_vec(&v).unwrap();
        handle_line(&line, &cfg(), chat.as_ref(), cmd.as_ref()).await;
        assert!(chat.calls.lock().unwrap().is_empty());
        assert!(cmd.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn malformed_json_ignored() {
        let chat = Arc::new(RecordingChat::default());
        let cmd = Arc::new(RecordingCommands::default());
        handle_line(b"not json", &cfg(), chat.as_ref(), cmd.as_ref()).await;
        assert!(chat.calls.lock().unwrap().is_empty());
        assert!(cmd.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn confidence_threaded_through() {
        let chat = Arc::new(RecordingChat::default());
        let cmd = Arc::new(RecordingCommands::default());
        let line = publish_line(json!({
            "text": "ok",
            "confidence": 0.92,
        }));
        handle_line(&line, &cfg(), chat.as_ref(), cmd.as_ref()).await;
        let calls = chat.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert!((calls[0].metadata.confidence.unwrap() - 0.92).abs() < 1e-9);
    }

    #[tokio::test]
    async fn empty_command_prefix_disables_command_routing() {
        let chat = Arc::new(RecordingChat::default());
        let cmd = Arc::new(RecordingCommands::default());
        let mut c = cfg();
        c.command_prefix = String::new();
        let line = publish_line(json!({"text": "weft status"}));
        handle_line(&line, &c, chat.as_ref(), cmd.as_ref()).await;
        assert_eq!(chat.calls.lock().unwrap().len(), 1);
        assert!(cmd.calls.lock().unwrap().is_empty());
    }
}
