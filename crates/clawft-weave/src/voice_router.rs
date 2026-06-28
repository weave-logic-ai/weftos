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

use std::collections::{HashMap, HashSet};
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
    /// Per-principal permission grid (WEFT-208 / SC-4). Defaults to
    /// "every voice principal is Level 0", which only allows chat
    /// dispatch — commands are refused outright.
    pub permissions: VoicePermissions,
}

/// Voice-router permission table (WEFT-208 / SC-4).
///
/// Mirrors [`clawft_types::config::VoicePermissionConfig`] without
/// coupling the router to the full config crate. The router clamps
/// out-of-range levels (anything > 2) to [`VoiceLevel::Level0`] at
/// resolve time so a bad config can never accidentally privilege a
/// principal.
#[derive(Debug, Clone, Default)]
pub struct VoicePermissions {
    /// Level applied to any principal not explicitly listed.
    pub default_level: VoiceLevel,
    /// Per-principal overrides, keyed by substrate `actor_id`.
    pub principal_levels: HashMap<String, VoiceLevel>,
    /// Allowlist of command verbs Level 1 principals may dispatch.
    pub safe_commands: HashSet<String>,
}

impl VoicePermissions {
    /// Resolve the permission level for a given principal. `None` and
    /// unknown principals fall back to [`Self::default_level`].
    pub fn level_for(&self, principal: Option<&str>) -> VoiceLevel {
        match principal {
            Some(id) => self
                .principal_levels
                .get(id)
                .copied()
                .unwrap_or(self.default_level),
            None => self.default_level,
        }
    }

    /// Build a permissions table from raw config integer levels (0/1/2);
    /// anything outside that range is clamped to Level 0.
    pub fn from_raw(
        default_level: u8,
        principal_levels: impl IntoIterator<Item = (String, u8)>,
        safe_commands: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            default_level: VoiceLevel::from_raw(default_level),
            principal_levels: principal_levels
                .into_iter()
                .map(|(k, v)| (k, VoiceLevel::from_raw(v)))
                .collect(),
            safe_commands: safe_commands.into_iter().collect(),
        }
    }
}

/// Voice principal trust level (WEFT-208 / SC-4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VoiceLevel {
    /// Read-only — chat allowed, commands refused outright. Default for
    /// unauthenticated voice (no `actor_id` on the publish line) and
    /// for principals not in the override table.
    #[default]
    Level0,
    /// Authenticated voice — chat + commands listed in
    /// [`VoicePermissions::safe_commands`].
    Level1,
    /// Privileged voice — chat + any command. Dispatched commands are
    /// still subject to the standard kernel governance gate, so a
    /// missing per-verb grant on the kernel side will still reject
    /// the call.
    Level2,
}

impl VoiceLevel {
    /// Clamp an out-of-range raw level (anything > 2) to [`Self::Level0`].
    pub fn from_raw(value: u8) -> Self {
        match value {
            1 => Self::Level1,
            2 => Self::Level2,
            _ => Self::Level0,
        }
    }

    /// Numeric form, suitable for tracing fields.
    pub fn as_u8(self) -> u8 {
        match self {
            Self::Level0 => 0,
            Self::Level1 => 1,
            Self::Level2 => 2,
        }
    }
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
    /// Voice principal as reported by the substrate publish line's
    /// `actor_id` (set by `clawft-service-whisper` to the source
    /// sensor node id). `None` when the publish line carries no
    /// `actor_id` — treated as an unauthenticated voice principal by
    /// the permission gate.
    pub principal: Option<String>,
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
    async fn dispatch_command(&self, method: String, params: Value) -> Result<Value, String>;
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
        S: FnOnce(Option<&str>, &str) -> Result<mpsc::Receiver<Vec<u8>>, String>,
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
    let Some(envelope) = decode_publish_envelope(bytes) else {
        // Either a notify-kind line, or malformed JSON — both safely
        // ignored. The substrate subscription stream interleaves both
        // shapes; only publish-kind lines carry transcript bodies.
        return;
    };
    let payload = envelope.payload;
    let principal = envelope.actor_id;
    let Some(text_raw) = payload.text() else {
        debug!("voice consumer: skipping transcript with no text field");
        return;
    };
    let text = text_raw.trim();
    if text.is_empty() {
        debug!("voice consumer: skipping empty transcript");
        return;
    }

    let level = config.permissions.level_for(principal.as_deref());

    if !config.command_prefix.is_empty()
        && let Some(cmd_body) = strip_prefix_ci(text, &config.command_prefix)
    {
        let trimmed = cmd_body.trim();
        if !trimmed.is_empty() {
            route_command(trimmed, config, principal.as_deref(), level, commands).await;
            return;
        }
    }

    // Chat path is allowed at every level today. The grid leaves room
    // to deny chat at a future "Level -1" (full mute), but per the
    // voice security spec Level 0 is "read-only", which still allows
    // sending a chat query — only command dispatch is gated off.
    let metadata = VoiceTurnMetadata {
        source: "voice",
        transcript_topic: config.transcript_topic.clone(),
        confidence: payload.confidence,
        principal,
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
    config: &VoiceRouterConfig,
    principal: Option<&str>,
    level: VoiceLevel,
    commands: &dyn CommandHandler,
) {
    let mut parts = body.split_whitespace();
    let Some(method) = parts.next() else {
        return;
    };
    let args: Vec<&str> = parts.collect();

    // WEFT-208 / SC-4 permission gate.
    //
    // Level 0: command dispatch is refused outright.
    // Level 1: only verbs in `safe_commands` allowed.
    // Level 2: every verb falls through to the kernel governance gate
    //          (which has the final say via gate.check on the kernel
    //          side; the router does not second-guess it).
    match level {
        VoiceLevel::Level0 => {
            warn!(
                event = "voice.permission.denied",
                principal = principal.unwrap_or("<unknown>"),
                requested_command = method,
                level = level.as_u8(),
                reason = "level-0 forbids command dispatch",
                "voice consumer: command refused by permission gate"
            );
            return;
        }
        VoiceLevel::Level1 => {
            if !config.permissions.safe_commands.contains(method) {
                warn!(
                    event = "voice.permission.denied",
                    principal = principal.unwrap_or("<unknown>"),
                    requested_command = method,
                    level = level.as_u8(),
                    reason = "command not in level-1 safe-commands allowlist",
                    "voice consumer: command refused by permission gate"
                );
                return;
            }
        }
        VoiceLevel::Level2 => {
            // Falls through to the existing kernel-side governance gate
            // wrapped by the daemon's `CommandHandler` impl.
        }
    }

    // Single args-array param shape. Verbs that need richer params
    // are responsible for parsing the array; the alternative (a real
    // CLI parser) lives in the panel and is intentionally out of
    // scope for the routing seam.
    let params = serde_json::json!({ "args": args });
    match commands.dispatch_command(method.to_string(), params).await {
        Ok(_) => {
            info!(
                method,
                principal = principal.unwrap_or("<unknown>"),
                level = level.as_u8(),
                "voice consumer: command dispatched"
            );
        }
        Err(e) => {
            warn!(
                method,
                principal = principal.unwrap_or("<unknown>"),
                level = level.as_u8(),
                err = %e,
                "voice consumer: command dispatch failed"
            );
        }
    }
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

/// Decoded publish-line envelope: the transcript payload + the source
/// principal (`actor_id`) the substrate publisher attached.
struct PublishEnvelope {
    payload: TranscriptPayload,
    actor_id: Option<String>,
}

/// Substrate update-line shape (see
/// `clawft_kernel::substrate_service::build_update_line`):
///
/// ```json
/// {"path":"…","tick":N,"kind":"publish|notify","value":{…},"actor_id":…}
/// ```
///
/// Returns the parsed `value` payload + top-level `actor_id` when
/// `kind == "publish"`. Notify lines and malformed JSON return `None`;
/// both are safely ignored upstream so the subscription stream stays
/// alive across them.
fn decode_publish_envelope(line: &[u8]) -> Option<PublishEnvelope> {
    let end = if line.last() == Some(&b'\n') {
        line.len() - 1
    } else {
        line.len()
    };
    let v: Value = serde_json::from_slice(&line[..end]).ok()?;
    if v.get("kind")?.as_str()? != "publish" {
        return None;
    }
    let actor_id = v
        .get("actor_id")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string());
    let value = v.get("value")?.clone();
    let payload: TranscriptPayload = serde_json::from_value(value).ok()?;
    Some(PublishEnvelope { payload, actor_id })
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
        async fn dispatch_command(&self, method: String, params: Value) -> Result<Value, String> {
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
            permissions: VoicePermissions::default(),
        }
    }

    fn publish_line(value: Value) -> Vec<u8> {
        publish_line_from(value, None)
    }

    fn publish_line_from(value: Value, actor_id: Option<&str>) -> Vec<u8> {
        let v = json!({
            "path": "substrate/_derived/transcript/n-test/mic",
            "tick": 1,
            "kind": "publish",
            "value": value,
            "actor_id": actor_id,
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

    /// Cfg variant that grants the test principal Level 2 — used by
    /// the legacy router-shape tests that pre-date the SC-4 gate. The
    /// new SC-4-specific tests below build their own permission tables
    /// inline.
    fn cfg_level2() -> VoiceRouterConfig {
        let mut c = cfg();
        c.permissions.default_level = VoiceLevel::Level2;
        c
    }

    #[tokio::test]
    async fn command_path_routes_to_command_handler() {
        let chat = Arc::new(RecordingChat::default());
        let cmd = Arc::new(RecordingCommands::default());
        let line = publish_line(json!({"text": "weft status now"}));
        handle_line(&line, &cfg_level2(), chat.as_ref(), cmd.as_ref()).await;
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
        handle_line(&line, &cfg_level2(), chat.as_ref(), cmd.as_ref()).await;
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

    // ---------------------------------------------------------------
    // WEFT-208 / SC-4: voice permission gate tests.
    // ---------------------------------------------------------------

    fn cfg_with_permissions(perms: VoicePermissions) -> VoiceRouterConfig {
        let mut c = cfg();
        c.permissions = perms;
        c
    }

    fn perms(default: VoiceLevel, overrides: &[(&str, VoiceLevel)]) -> VoicePermissions {
        VoicePermissions {
            default_level: default,
            principal_levels: overrides
                .iter()
                .map(|(k, v)| ((*k).to_string(), *v))
                .collect(),
            safe_commands: ["status", "list", "help"]
                .into_iter()
                .map(String::from)
                .collect(),
        }
    }

    #[tokio::test]
    async fn level0_command_rejected_chat_allowed() {
        let chat = Arc::new(RecordingChat::default());
        let cmd = Arc::new(RecordingCommands::default());
        let c = cfg_with_permissions(perms(VoiceLevel::Level0, &[]));

        // Command path: rejected.
        let line = publish_line_from(json!({"text": "weft status"}), Some("n-unknown"));
        handle_line(&line, &c, chat.as_ref(), cmd.as_ref()).await;
        assert!(cmd.calls.lock().unwrap().is_empty());
        assert!(chat.calls.lock().unwrap().is_empty());

        // Chat path: allowed.
        let line = publish_line_from(json!({"text": "what is the time"}), Some("n-unknown"));
        handle_line(&line, &c, chat.as_ref(), cmd.as_ref()).await;
        assert!(cmd.calls.lock().unwrap().is_empty());
        let calls = chat.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].metadata.principal.as_deref(), Some("n-unknown"));
    }

    #[tokio::test]
    async fn level1_safe_command_allowed() {
        let chat = Arc::new(RecordingChat::default());
        let cmd = Arc::new(RecordingCommands::default());
        let c = cfg_with_permissions(perms(VoiceLevel::Level0, &[("n-mic1", VoiceLevel::Level1)]));
        let line = publish_line_from(json!({"text": "weft status"}), Some("n-mic1"));
        handle_line(&line, &c, chat.as_ref(), cmd.as_ref()).await;
        let calls = cmd.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "status");
        assert!(chat.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn level1_non_safe_command_rejected() {
        let chat = Arc::new(RecordingChat::default());
        let cmd = Arc::new(RecordingCommands::default());
        let c = cfg_with_permissions(perms(VoiceLevel::Level0, &[("n-mic1", VoiceLevel::Level1)]));
        let line = publish_line_from(json!({"text": "weft shutdown now"}), Some("n-mic1"));
        handle_line(&line, &c, chat.as_ref(), cmd.as_ref()).await;
        assert!(cmd.calls.lock().unwrap().is_empty());
        assert!(chat.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn level2_falls_through_to_governance_gate() {
        // The router does not consult `gate.check` directly; once the
        // SC-4 router-side gate clears, the call lands on the
        // CommandHandler trait, which the daemon implementation wraps
        // around the kernel's governance gate. The recording handler
        // here represents that downstream surface — observing a hit
        // proves the SC-4 gate did not block it.
        let chat = Arc::new(RecordingChat::default());
        let cmd = Arc::new(RecordingCommands::default());
        let c = cfg_with_permissions(perms(
            VoiceLevel::Level0,
            &[("n-admin", VoiceLevel::Level2)],
        ));
        let line = publish_line_from(json!({"text": "weft shutdown now"}), Some("n-admin"));
        handle_line(&line, &c, chat.as_ref(), cmd.as_ref()).await;
        let calls = cmd.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "shutdown");
        assert_eq!(calls[0].1, json!({"args": ["now"]}));
        assert!(chat.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn permissions_resolver_clamps_out_of_range_levels() {
        let p =
            VoicePermissions::from_raw(7, [("n-bad".to_string(), 99u8)], ["status".to_string()]);
        assert_eq!(p.default_level, VoiceLevel::Level0);
        assert_eq!(p.level_for(Some("n-bad")), VoiceLevel::Level0);
        assert_eq!(p.level_for(None), VoiceLevel::Level0);
    }

    /// Audit-emission test: a Level 0 principal attempting a command
    /// must produce a tracing event tagged
    /// `event = "voice.permission.denied"` carrying the principal,
    /// the requested verb, and the level. We capture the event via
    /// a custom tracing layer so the assertion is independent of any
    /// global subscriber configuration.
    #[test]
    fn permission_denied_audit_event_emits() {
        use std::sync::Mutex as StdMutex;
        use tracing::Subscriber;
        use tracing::field::{Field, Visit};
        use tracing::subscriber::with_default;
        use tracing_subscriber::Layer;
        use tracing_subscriber::layer::{Context, SubscriberExt};
        use tracing_subscriber::registry::LookupSpan;

        #[derive(Clone, Default)]
        struct CapturedEvent {
            event: Option<String>,
            principal: Option<String>,
            requested_command: Option<String>,
            level: Option<u64>,
            reason: Option<String>,
        }

        impl Visit for CapturedEvent {
            fn record_str(&mut self, field: &Field, value: &str) {
                match field.name() {
                    "event" => self.event = Some(value.to_string()),
                    "principal" => self.principal = Some(value.to_string()),
                    "requested_command" => self.requested_command = Some(value.to_string()),
                    "reason" => self.reason = Some(value.to_string()),
                    _ => {}
                }
            }
            fn record_u64(&mut self, field: &Field, value: u64) {
                if field.name() == "level" {
                    self.level = Some(value);
                }
            }
            fn record_i64(&mut self, field: &Field, value: i64) {
                if field.name() == "level" && value >= 0 {
                    self.level = Some(value as u64);
                }
            }
            fn record_debug(&mut self, _field: &Field, _value: &dyn std::fmt::Debug) {}
        }

        struct CapturingLayer {
            sink: Arc<StdMutex<Vec<CapturedEvent>>>,
        }

        impl<S> Layer<S> for CapturingLayer
        where
            S: Subscriber + for<'a> LookupSpan<'a>,
        {
            fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
                let mut captured = CapturedEvent::default();
                event.record(&mut captured);
                if captured.event.as_deref() == Some("voice.permission.denied") {
                    self.sink.lock().unwrap().push(captured);
                }
            }
        }

        let sink: Arc<StdMutex<Vec<CapturedEvent>>> = Arc::new(StdMutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry().with(CapturingLayer { sink: sink.clone() });

        let chat = Arc::new(RecordingChat::default());
        let cmd = Arc::new(RecordingCommands::default());
        let c = cfg_with_permissions(perms(VoiceLevel::Level0, &[]));

        let line = publish_line_from(json!({"text": "weft shutdown"}), Some("n-attacker"));
        let chat_clone = chat.clone();
        let cmd_clone = cmd.clone();
        let cfg_clone = c.clone();
        // Use a plain (non-tokio) test fn + a fresh current-thread
        // runtime so we own the subscriber installation site —
        // `with_default` only applies on the calling thread, and
        // `#[tokio::test]` wraps the call in its own runtime which
        // would prevent us from constructing a second one here.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        with_default(subscriber, || {
            rt.block_on(async {
                handle_line(&line, &cfg_clone, chat_clone.as_ref(), cmd_clone.as_ref()).await;
            });
        });

        // Command did not dispatch.
        assert!(cmd.calls.lock().unwrap().is_empty());

        let captured = sink.lock().unwrap();
        assert_eq!(
            captured.len(),
            1,
            "expected exactly one voice.permission.denied event, got {}",
            captured.len()
        );
        let e = &captured[0];
        assert_eq!(e.event.as_deref(), Some("voice.permission.denied"));
        assert_eq!(e.principal.as_deref(), Some("n-attacker"));
        assert_eq!(e.requested_command.as_deref(), Some("shutdown"));
        assert_eq!(e.level, Some(0));
        assert!(e.reason.is_some(), "denial event must include a reason");
    }
}
