//! Wire-format types for the `agent.chat` RPC.
//!
//! These types are the canonical home for the `agent.chat` request/
//! response shape consumed by the daemon (`clawft-weave`), the agent
//! service (`clawft-service-agent`), and panels / tools that talk to
//! either. Hosting them in `clawft-types` (the foundation crate that
//! every other clawft crate already depends on) lets `clawft-weave`
//! avoid importing service crates just for serde, and keeps the wire
//! shape under a single ownership boundary.
//!
//! History: prior to WEFT-498, `clawft-weave::protocol::AgentChat*` and
//! `clawft-service-agent::protocol::AgentChat*` carried duplicate 1:1
//! mirrors of these definitions plus a `From` bridge between them. The
//! mirrors existed because `clawft-service-agent` was added late in the
//! agent-core-v1 phasing and re-exporting upstream from `clawft-weave`
//! would have inverted the dep direction. With this module in place,
//! both crates re-export from here and the bridge impls collapse to
//! identity.
//!
//! `default_conv_id` is the single ephemeral-id generator the wire
//! format relies on; both panels and the daemon must agree on the
//! `ephemeral-{ts:013}-{n:06}` shape to keep legacy panel behaviour
//! through Phase A of the agent-core rollout.

use serde::{Deserialize, Serialize};

/// One message in an `agent.chat` conversation.
///
/// `role` is one of `"system"` / `"user"` / `"assistant"`. The daemon
/// prepends its own system prompt; any `system` from the panel is
/// appended after it.
///
/// WEFT-350: optional [`audio`](AgentChatMessage::audio) lets the voice
/// path attach a substrate-pointed utterance so the sink can persist
/// [`crate::turn_content::TurnContent::Audio`] / `Mixed`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentChatMessage {
    /// `system` / `user` / `assistant`.
    pub role: String,
    /// Message content (STT transcript for voice turns).
    pub content: String,
    /// Optional substrate-pointed audio for this message (WEFT-350).
    /// When set on the trailing user message, the chat loop records a
    /// multimodal turn (`Mixed` when content is non-empty, else `Audio`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<crate::turn_content::AudioRef>,
}

impl AgentChatMessage {
    /// Build a text-only chat message (no audio attachment).
    pub fn text(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
            audio: None,
        }
    }
}

/// Metadata key stamped by `AgentService` (WEFT-332) so
/// `AgentLoop::handle_turn` uses a per-caller principal for
/// `EffectGate::check` instead of the boot-time concierge id.
///
/// Value is a JSON string (the kernel-issued agent_id UUID, or a
/// namespaced synthetic id when no registry is wired).
pub const GATE_AGENT_ID_META_KEY: &str = "gate_agent_id";

/// Build the stable registration *name* for a multi-tenant chat
/// principal (WEFT-332).
///
/// Kernel `AgentRegistry` mints a UUID as `agent_id`; this name is the
/// human-readable / lookup key so the same caller reuses one principal
/// across turns. Format: `agent.chat:user:<caller_id>`.
pub fn caller_principal_name(caller_id: &str) -> String {
    format!("agent.chat:user:{caller_id}")
}

/// Parameters for the `agent.chat` RPC.
///
/// The panel sends the full conversation each turn; cross-request
/// state is owned by the substrate-backed `ConversationSink`
/// (agent-core-v1 Phase C3).
///
/// The `conv_id` field defaults to an ephemeral id when omitted so
/// legacy panels keep working through Phase A; Phase C callers supply
/// a stable id and observe per-conv mutex behaviour in
/// `clawft_service_agent::AgentService`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentChatParams {
    /// Full conversation history. Last entry should be `user`.
    pub messages: Vec<AgentChatMessage>,
    /// Sampling temperature; daemon default when None.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Hard cap on generated tokens per LLM call inside the loop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Conversation identifier. Drives the per-conv `DashMap<ConvId, _>`
    /// in `clawft_service_agent::AgentService` and the substrate JSONL
    /// path `derived/chat/<conv_id>/turns/<ulid>`. Defaults to an
    /// ephemeral id so legacy panels keep working.
    #[serde(default = "default_conv_id")]
    pub conv_id: String,
    /// Free-form per-turn metadata carried to the daemon agent loop.
    ///
    /// Threaded into `InboundMessage.metadata` by
    /// `clawft_service_agent`'s `inbound_from_params`, giving the daemon
    /// loop the same per-turn context the in-process REPL injects
    /// directly. Documented known keys consumed by `loop_core`:
    /// - `skill_instructions: String` — injected as a system note.
    /// - `allowed_tools: [String]` — tool-subset filter.
    /// - `model: String` — model override, carried to the router.
    /// - `tool_choice: string | object` — OpenAI `tool_choice` override
    ///   (`"auto"`/`"none"`/`"required"` or
    ///   `{"type":"function","function":{"name":…}}`). Requests a tool
    ///   selection for a turn; absent leaves it to the model. Enforcement
    ///   is provider-side (llama-server honors it only probabilistically
    ///   when a system prompt is present).
    /// - `provenance: object` — diagnostic (e.g.
    ///   `{"impulse_source":"agent.chat"}`) for the witness/audit trail.
    /// - [`GATE_AGENT_ID_META_KEY`] — set by the service (not panels);
    ///   per-caller principal for gate checks (WEFT-332).
    /// - `audio: object` — WEFT-350 substrate audio ref
    ///   (`substrate_path`, `mime`, `duration_ms`) when STT attaches
    ///   utterance audio without message-level `AgentChatMessage.audio`.
    /// - `audio_substrate_path` / `audio_mime` / `audio_duration_ms` —
    ///   flattened alternate form of the audio ref for thin adapters.
    ///
    /// Additive and fully backward-compatible: absent on the wire when
    /// `None`, and older callers that omit it deserialize to `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Map<String, serde_json::Value>>,
    /// Caller / user identity for multi-tenant chat (WEFT-332).
    ///
    /// Opaque user or tenant id supplied by the panel, CLI, or voice
    /// path. When present (non-empty after trim):
    /// - `InboundMessage.sender_id` becomes this value so
    ///   [`PermissionResolver`](crate) per-user policy applies;
    /// - `AgentService` lazily registers a principal named
    ///   [`caller_principal_name`] in the kernel `AgentRegistry` and
    ///   stamps its `agent_id` under [`GATE_AGENT_ID_META_KEY`].
    ///
    /// Absent / empty keeps the legacy single-tenant path
    /// (`sender_id = "panel"`, boot-time concierge principal).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller_id: Option<String>,
}

/// Default conversation id when the caller omits `conv_id`.
///
/// Generates an ephemeral, timestamp-prefixed monotonic string so
/// successive default-id calls within the same millisecond do not
/// collide. Callers that need stable per-conversation behaviour must
/// supply their own id; this default exists only to keep the legacy
/// panel wire format working through Phase A of the agent-core
/// rollout.
pub fn default_conv_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("ephemeral-{ts:013}-{n:06}")
}

/// Summary of one tool call the agent executed during a chat turn.
///
/// Renders as a collapsible bubble in the panel between user and
/// assistant turns.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentChatToolCall {
    /// Tool name (e.g. `"read_file"`, `"list_directory"`).
    pub name: String,
    /// JSON-stringified arguments, truncated for UI preview.
    pub arguments_preview: String,
    /// Tool result, truncated for UI preview.
    pub result_preview: String,
    /// True when the tool ran without error.
    pub success: bool,
}

/// Summary of one subagent spawned during a chat turn (M4 D8).
///
/// Emitted when the agent's tool loop calls `agent_spawn`. Lets the
/// CLI / GUI render "agent kicked off N subagents (M done, K running)"
/// without re-deriving the spawn tree from the forest. `status` is the
/// stringified `TaskStatus` (`"running"` / `"completed"` / `"failed"` /
/// `"exhausted"` / `"cancelled"` / `"denied"`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpawnedTaskSummary {
    /// Spawn task id (`SpawnHandle.task_id`).
    pub task_id: String,
    /// Child conversation id (`"sub:<parent>:<ulid>"`).
    pub child_conv_id: String,
    /// Task status at the time the parent turn finished.
    pub status: String,
}

/// One gate denial that contributed to a WEFT-345 escalation streak.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GateDenialRecord {
    /// Tool name that the gate refused (e.g. `"write_file"`).
    pub tool: String,
    /// Human-readable deny reason from the effect gate.
    pub reason: String,
}

/// Well-known `finish_reason` when the tool loop escalates after
/// consecutive gate denials (WEFT-345).
///
/// Matches the wire string panels / RPC clients should switch on.
pub const FINISH_REASON_ESCALATE_TO_HUMAN: &str = "escalate_to_human";

/// Well-known `finish_reason` when the tool loop halts on a gate
/// `Defer` decision so the panel can prompt the user (WEFT-258
/// interactive defer).
///
/// Matches the wire string panels switch on, alongside the structured
/// [`DeferredActionEvent`] / top-level `{ "deferred": true, "reason" }`
/// shape.
pub const FINISH_REASON_DEFERRED: &str = "deferred";

/// Canonical governance decision name for the WEFT-345 path — same
/// spelling as `clawft_kernel::governance::GovernanceDecision::EscalateToHuman`.
pub const GOVERNANCE_DECISION_ESCALATE_TO_HUMAN: &str = "EscalateToHuman";

/// Governance escalation event when the agent loop hits the consecutive
/// gate-denial threshold (WEFT-345).
///
/// Surfaced on [`AgentLoopResultMeta::escalation`] / [`AgentChatResult::escalation`]
/// so panels can render an escalation prompt (W-UI/15) and a future
/// allow/abort/refine RPC can resume the turn. Until that RPC lands,
/// the loop still **halts** the turn after emitting this event — it
/// no longer fails silently with only a log line.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EscalateToHumanEvent {
    /// Always [`GOVERNANCE_DECISION_ESCALATE_TO_HUMAN`].
    pub decision: String,
    /// Number of consecutive gate denials that triggered escalation.
    pub denial_count: u32,
    /// Conversation the loop was processing.
    pub conv_id: String,
    /// Ordered denials (oldest first) that formed the streak.
    pub denials: Vec<GateDenialRecord>,
    /// Human-readable summary for logs / panel fallback text.
    pub summary: String,
}

impl EscalateToHumanEvent {
    /// Build a WEFT-345 escalation event from a denial streak.
    pub fn from_denials(conv_id: impl Into<String>, denials: Vec<GateDenialRecord>) -> Self {
        let conv_id = conv_id.into();
        let denial_count = denials.len() as u32;
        let last = denials.last();
        let summary = match last {
            Some(d) => format!(
                "agent: gate denied tool calls {denial_count}x; escalating to human \
                 (EscalateToHuman). Last: `{}`: {}",
                d.tool, d.reason
            ),
            None => format!(
                "agent: gate denied tool calls {denial_count}x; escalating to human \
                 (EscalateToHuman)."
            ),
        };
        Self {
            decision: GOVERNANCE_DECISION_ESCALATE_TO_HUMAN.to_string(),
            denial_count,
            conv_id,
            denials,
            summary,
        }
    }
}

/// Interactive defer event when the effect gate returns `Defer` for a
/// tool call (WEFT-258).
///
/// The agent loop **halts** the turn and surfaces this on
/// [`AgentLoopResultMeta::deferred`] / [`AgentChatResult::deferred`] so
/// the chat panel can render a reason + approve/deny/input affordance
/// and resume the conversation with the user-supplied decision.
///
/// Wire shape mirrors the tool-result envelope
/// `{ "deferred": true, "reason": ... }` so panels can switch on either
/// the structured event or a top-level `deferred: true` flag.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeferredActionEvent {
    /// Always `true` — wire-shape compatibility with
    /// `{ "deferred": true, "reason" }`.
    pub deferred: bool,
    /// Human-readable reason from the effect gate.
    pub reason: String,
    /// Tool name that was deferred (e.g. `"write_file"`).
    pub tool: String,
    /// Conversation the loop was processing.
    pub conv_id: String,
    /// Truncated tool arguments for the panel preview.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub arguments_preview: String,
    /// Human-readable summary for logs / assistant_text fallback.
    pub summary: String,
}

impl DeferredActionEvent {
    /// Build a WEFT-258 interactive-defer event.
    pub fn new(
        conv_id: impl Into<String>,
        tool: impl Into<String>,
        reason: impl Into<String>,
        arguments_preview: impl Into<String>,
    ) -> Self {
        let conv_id = conv_id.into();
        let tool = tool.into();
        let reason = reason.into();
        let arguments_preview = arguments_preview.into();
        let summary = format!(
            "agent: tool `{tool}` deferred for human review — {reason}. \
             Approve, deny, or reply with guidance to resume."
        );
        Self {
            deferred: true,
            reason,
            tool,
            conv_id,
            arguments_preview,
            summary,
        }
    }
}

/// `AgentChatResult::finish_reason` for a turn the agent loop flagged as
/// failed (`OutboundMessage::is_error`). `assistant_text` still carries the
/// readable error for chat panels; scripted callers (one-shot
/// `weft agent -m`) treat it as a failure.
pub const FINISH_REASON_ERROR: &str = "error";

impl AgentChatResult {
    /// True when the loop reported this turn as failed.
    pub fn is_error(&self) -> bool {
        self.finish_reason == FINISH_REASON_ERROR
    }
}

/// Result of `agent.chat`.
///
/// Since WEFT-328 the loop threads a real [`AgentLoopResultMeta`] through
/// `OutboundMessage.metadata` under [`AGENT_LOOP_RESULT_META_KEY`], so
/// `tool_calls`, `prompt_tokens`, `completion_tokens`, `model`, and
/// `identity_source` carry actual values on the normal agent-loop path.
/// When the meta key is absent (non-loop outbound, older producer) the
/// agent service's `dispatch` falls back to C1-compatible defaults
/// (empty tool_calls, 0 tokens, `None` model/identity_source).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentChatResult {
    /// Final assistant text after the tool loop terminates.
    pub assistant_text: String,
    /// Tool calls executed during the loop, in order.
    pub tool_calls: Vec<AgentChatToolCall>,
    /// Why the loop terminated: `"stop"`, `"length"`,
    /// `"max_iterations"`, etc.
    pub finish_reason: String,
    /// Number of LLM round-trips inside the loop.
    pub iterations: u32,
    /// Cumulative prompt tokens across iterations.
    pub prompt_tokens: u32,
    /// Cumulative completion tokens across iterations.
    pub completion_tokens: u32,
    /// Echoed model name (best-effort; from the provider response).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Identity descriptor surfaced to the panel — diagnostic for the
    /// drift-warning path (e.g. `"clawft"`, `"docs-fallback"`). Sourced
    /// from the turn's identity loader via the loop meta (WEFT-328).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_source: Option<String>,
    /// Model reasoning captured across the tool loop (Hermes `<think>` /
    /// llama.cpp `reasoning_content`), when the provider produced any —
    /// surfaced for observability (`weft voice watch`), never part of the
    /// spoken/printed reply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    /// Subagents spawned during the turn (M4 D8). Absent on the wire
    /// when empty; older clients that omit it deserialize to an empty
    /// vec, so the field is fully backward-compatible.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spawned_tasks: Vec<SpawnedTaskSummary>,
    /// WEFT-345: present when the tool loop stopped after consecutive
    /// gate denials and emitted `EscalateToHuman`. Absent on the wire
    /// when `None` so older panels ignore the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub escalation: Option<EscalateToHumanEvent>,
    /// WEFT-258: present when the tool loop halted on a gate `Defer`
    /// so the panel can prompt-and-resume. Absent on the wire when
    /// `None`. Also detectable via `finish_reason == "deferred"` and
    /// the event's `deferred: true` field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deferred: Option<DeferredActionEvent>,
}

/// Well-known `OutboundMessage.metadata` key under which the daemon
/// agent loop stashes the enriched turn result (M4 D8 / WEFT-328).
///
/// `OutboundMessage` is a generic bus envelope with no first-class slots
/// for tool-call summaries, token counts, model, or identity source.
/// Rather than widen the envelope, the loop serializes an
/// [`AgentLoopResultMeta`] into the free-form `metadata` map under this
/// key; the agent service's `result_from_outbound` reads it back.
/// Mirrors the `AgentChatParams.metadata` precedent used inbound.
pub const AGENT_LOOP_RESULT_META_KEY: &str = "agent_loop_result";

/// Enriched loop-result summary threaded from the tool loop (in
/// `clawft-core`) to the agent service's `result_from_outbound` via
/// `OutboundMessage.metadata[AGENT_LOOP_RESULT_META_KEY]` (M4 D8 /
/// WEFT-328).
///
/// Every field is `#[serde(default)]` so a missing key or a partial
/// object degrades to the pre-M4 / C1 defaults rather than failing to
/// deserialize — keeping the panel wire shape backward-compatible.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentLoopResultMeta {
    /// Tool calls executed during the loop, in order.
    #[serde(default)]
    pub tool_calls: Vec<AgentChatToolCall>,
    /// Why the loop terminated (`"stop"`, `"max_iterations"`, …).
    #[serde(default)]
    pub finish_reason: String,
    /// Number of LLM round-trips inside the loop.
    #[serde(default)]
    pub iterations: u32,
    /// Subagents spawned during the turn.
    #[serde(default)]
    pub spawned_tasks: Vec<SpawnedTaskSummary>,
    /// Model that actually answered (from the provider response), when the
    /// pipeline reported one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Cumulative prompt tokens across the loop's LLM calls.
    #[serde(default)]
    pub prompt_tokens: u32,
    /// Cumulative completion tokens across the loop's LLM calls.
    #[serde(default)]
    pub completion_tokens: u32,
    /// Reasoning captured across the loop (see
    /// [`AgentChatResult::reasoning`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    /// Identity loader source for this turn (e.g. `"clawft"`). Populated
    /// when a `SystemPromptBuilder` successfully loads identity; absent
    /// when no builder is attached or the load failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_source: Option<String>,
    /// WEFT-345: consecutive gate-denial → `EscalateToHuman` event.
    /// Absent when the turn ended normally.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub escalation: Option<EscalateToHumanEvent>,
    /// WEFT-258: gate `Defer` → interactive human review event.
    /// Absent when the turn ended without a deferral.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deferred: Option<DeferredActionEvent>,
}

/// One externally-produced turn for the `agent.turn.record` RPC.
///
/// Unlike `agent.chat`, the daemon does not run the LLM loop for these —
/// the caller (e.g. the `weft voice talk` Talk-Mode loop) already produced
/// the exchange and only needs it recorded through the daemon's
/// `ConversationSink` so the substrate JSONL + `KernelTurnAnchor`
/// side-effects (witness chain / HNSW / causal graph / session tier)
/// apply exactly as they do for `agent.chat` turns.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedTurn {
    /// `user` / `assistant` / `system`.
    pub role: String,
    /// Turn text (for voice: the STT transcript or the spoken answer).
    pub content: String,
    /// Wall-clock ms when the turn was produced; the daemon stamps
    /// receipt time when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ts_ms: Option<u64>,
    /// Optional per-utterance voice decomposition (Wave 1 §W1.2). Carried
    /// verbatim through `index_turn` to the sibling `voice_analysis` node
    /// metadata key (served verbatim by `conversation.graph`); its `emotion`
    /// sub-blob overrides the classification emotion axis with `tier:"voice"`
    /// (the voice > llm > keyword confidence hierarchy). Absent for text turns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_analysis: Option<serde_json::Value>,
}

/// Parameters for the `agent.turn.record` RPC.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTurnRecordParams {
    /// Conversation identifier (same keyspace as `agent.chat` conv_ids;
    /// voice uses its Talk-Mode conv_id, e.g. `weft-talk`).
    pub conv_id: String,
    /// Producing channel, for provenance (e.g. `"voice.talk"`).
    #[serde(default)]
    pub channel: String,
    /// Turns to record, in order. Must be non-empty.
    pub turns: Vec<RecordedTurn>,
}

/// Result of `agent.turn.record`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTurnRecordResult {
    /// How many turns were durably published to the substrate.
    pub recorded: usize,
}

// ── agent.chat_stream (WEFT-253) ─────────────────────────────────
//
// Progressive companion to `agent.chat`. Same request params; the
// daemon publishes incremental frames at
// `substrate/_derived/chat/<conv_id>/stream` while the turn runs, and
// the RPC response is the final [`AgentChatResult`] (identical shape
// to `agent.chat`). Panels poll the stream path (or relay it via
// substrate snapshot) to render tokens as they land, then apply the
// final response when the oneshot reply arrives.
//
// Native + WASM both work because the progressive channel is the
// substrate (already polled on both transports); the RPC itself stays
// request/response — no connection-takeover required on the panel
// wire. True per-token LLM streaming lands later as a daemon-side
// enhancement that writes the same frame shape mid-generation.

/// Substrate path for the progressive stream frame of a conversation.
///
/// Sibling of `status` / `meta` under the same `_derived/chat/<conv>/`
/// parent so the daemon's existing `chat` `DerivedWriteGrant` covers
/// it. Heartbeat's 2s `Replace` at `status` never clobbers stream
/// text.
pub fn chat_stream_path(conv_id: &str) -> String {
    format!("substrate/_derived/chat/{conv_id}/stream")
}

// ── Interactive Defer UX (WEFT-331) ──────────────────────────────
//
// When the effect gate returns `GateDecision::Defer`, the agent loop
// can suspend until a human decides allow / deny / cancel via
// `agent.chat.defer_decide`. The pending prompt is published on the
// progressive stream frame (`phase = "awaiting_defer"`) and on the
// dedicated substrate path [`chat_defer_path`].

/// Default human-decision timeout for an interactive defer (ms).
///
/// On expiry the loop applies the **default-deny** path: the tool
/// does not run and the LLM receives a structured
/// `{"denied":true,"reason":"defer timed out"}` result so the turn
/// continues without hanging forever. Documented here and in
/// `docs/plans/wave-0l-WEFT-331-result.md`.
pub const DEFER_DEFAULT_TIMEOUT_MS: u64 = 120_000;

/// Stream / heartbeat phase while the loop is suspended on a defer.
pub const STREAM_PHASE_AWAITING_DEFER: &str = "awaiting_defer";

/// Wire string for a human **allow** decision on a deferred tool.
pub const DEFER_DECISION_ALLOW: &str = "allow";
/// Wire string for a human **deny** decision on a deferred tool.
pub const DEFER_DECISION_DENY: &str = "deny";
/// Wire string for a human **cancel** decision on a deferred tool.
pub const DEFER_DECISION_CANCEL: &str = "cancel";

/// Human decision returned via `agent.chat.defer_decide` (WEFT-331).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeferUserDecision {
    /// Proceed with the deferred tool (sandbox + dispatch still apply).
    Allow,
    /// Refuse the tool; loop continues with a structured deny result.
    Deny,
    /// Abort this defer without approving; treated as deny for the tool
    /// result so the model can re-plan (does not cancel the whole turn
    /// unless the panel also fires `agent.chat.cancel`).
    Cancel,
}

impl DeferUserDecision {
    /// Parse a wire decision string (`allow` / `deny` / `cancel`).
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            DEFER_DECISION_ALLOW => Some(Self::Allow),
            DEFER_DECISION_DENY => Some(Self::Deny),
            DEFER_DECISION_CANCEL => Some(Self::Cancel),
            _ => None,
        }
    }

    /// Wire spelling for this decision.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => DEFER_DECISION_ALLOW,
            Self::Deny => DEFER_DECISION_DENY,
            Self::Cancel => DEFER_DECISION_CANCEL,
        }
    }
}

/// Pending interactive-defer prompt shown in the panel (WEFT-331).
///
/// Published on [`AgentChatStreamFrame::defer`] / [`chat_defer_path`]
/// while the tool loop is suspended awaiting `agent.chat.defer_decide`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeferPromptEvent {
    /// Stable id for this defer (keyed by the decision RPC).
    pub defer_id: String,
    /// Conversation the loop is processing.
    pub conv_id: String,
    /// Tool name the gate deferred (e.g. `"write_file"`).
    pub tool: String,
    /// Human-readable defer reason from the effect gate.
    pub reason: String,
    /// Truncated JSON arguments for the panel preview.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub arguments_preview: String,
    /// How long the loop will wait (ms) before default-deny.
    #[serde(default = "default_defer_timeout_ms")]
    pub timeout_ms: u64,
    /// Wall-clock ms when the prompt was opened (panel age label).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ts_ms: Option<u64>,
}

fn default_defer_timeout_ms() -> u64 {
    DEFER_DEFAULT_TIMEOUT_MS
}

/// Parameters for the `agent.chat.defer_decide` RPC (WEFT-331).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentChatDeferDecideParams {
    /// Conversation that owns the pending defer.
    pub conv_id: String,
    /// Id from the published [`DeferPromptEvent`].
    pub defer_id: String,
    /// One of [`DEFER_DECISION_ALLOW`] / [`DEFER_DECISION_DENY`] /
    /// [`DEFER_DECISION_CANCEL`].
    pub decision: String,
}

/// Result of `agent.chat.defer_decide`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentChatDeferDecideResult {
    /// True when a pending waiter accepted this decision.
    pub accepted: bool,
    /// Echo of the defer id.
    pub defer_id: String,
    /// Echo of the decision (or `"none"` when nothing was pending).
    pub decision: String,
    /// Optional diagnostic when `accepted == false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Substrate path for the pending interactive-defer prompt.
///
/// Sibling of `status` / `stream` under `_derived/chat/<conv>/` so the
/// existing chat `DerivedWriteGrant` covers it. The panel may poll this
/// path (or read `defer` off the stream frame) while a turn is in flight.
pub fn chat_defer_path(conv_id: &str) -> String {
    format!("substrate/_derived/chat/{conv_id}/defer")
}

/// One progressive frame written to [`chat_stream_path`].
///
/// The panel treats `text` as the **accumulated** assistant draft
/// (not a delta), so a missed frame is self-healing — the next frame
/// carries the full string so far. `seq` is a monotonic counter the
/// panel can use to ignore stale out-of-order reads.
///
/// WEFT-350 voice path also reads:
/// - [`delta`](AgentChatStreamFrame::delta) — partial token(s) since the
///   previous frame (TTS / token consumers that want increments).
/// - [`speakable`](AgentChatStreamFrame::speakable) — a completed
///   sentence/clause ready for TTS (empty when no boundary yet).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentChatStreamFrame {
    /// Accumulated assistant text so far (empty while thinking /
    /// tool-running before the first token).
    #[serde(default)]
    pub text: String,
    /// Coarse phase for the heartbeat-style status label:
    /// `"thinking"` / `"generating"` / `"done"` / `"error"` /
    /// [`STREAM_PHASE_AWAITING_DEFER`].
    pub phase: String,
    /// Monotonic frame counter within this turn.
    pub seq: u64,
    /// True once the turn has terminated (success or error). The
    /// panel freezes the draft and waits for the RPC reply (or
    /// surfaces `error` when set).
    #[serde(default)]
    pub done: bool,
    /// Optional tool name when `phase == "tool"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// Set when `done && phase == "error"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Wall-clock ms the frame was published (panel age label).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ts_ms: Option<u64>,
    /// WEFT-331: present when `phase == awaiting_defer` so the panel
    /// can render the allow / deny / cancel prompt without a second
    /// substrate read. Absent on the wire when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub defer: Option<DeferPromptEvent>,
    /// Partial text since the previous progressive frame (WEFT-350).
    /// Absent on thinking / terminal frames that carry no new tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta: Option<String>,
    /// Completed speakable unit (sentence / clause) ready for TTS
    /// (WEFT-350). Absent when no sentence boundary has closed yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speakable: Option<String>,
}

impl AgentChatStreamFrame {
    /// Build a "thinking / waiting for model" frame.
    pub fn thinking(seq: u64) -> Self {
        Self {
            text: String::new(),
            phase: "thinking".into(),
            seq,
            done: false,
            tool_name: None,
            error: None,
            ts_ms: None,
            defer: None,
            delta: None,
            speakable: None,
        }
    }

    /// Build a progressive draft frame (`phase = "generating"`).
    pub fn generating(seq: u64, text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            phase: "generating".into(),
            seq,
            done: false,
            tool_name: None,
            error: None,
            ts_ms: None,
            defer: None,
            delta: None,
            speakable: None,
        }
    }

    /// Progressive frame with partial-token `delta` and optional TTS
    /// `speakable` unit (WEFT-350 voice path).
    pub fn generating_voice(
        seq: u64,
        text: impl Into<String>,
        delta: impl Into<String>,
        speakable: Option<String>,
    ) -> Self {
        let delta = delta.into();
        Self {
            text: text.into(),
            phase: "generating".into(),
            seq,
            done: false,
            tool_name: None,
            error: None,
            ts_ms: None,
            defer: None,
            delta: if delta.is_empty() {
                None
            } else {
                Some(delta)
            },
            speakable,
        }
    }

    /// Build the terminal success frame (full text, `done = true`).
    pub fn done_ok(seq: u64, text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            phase: "done".into(),
            seq,
            done: true,
            tool_name: None,
            error: None,
            ts_ms: None,
            defer: None,
            delta: None,
            speakable: None,
        }
    }

    /// Build the terminal error frame.
    pub fn done_err(seq: u64, error: impl Into<String>) -> Self {
        Self {
            text: String::new(),
            phase: "error".into(),
            seq,
            done: true,
            tool_name: None,
            error: Some(error.into()),
            ts_ms: None,
            defer: None,
            delta: None,
            speakable: None,
        }
    }

    /// Build an interactive-defer wait frame (WEFT-331).
    ///
    /// `done` stays `false` — the turn is still in flight; the panel
    /// shows the prompt and must call `agent.chat.defer_decide`.
    pub fn awaiting_defer(seq: u64, prompt: DeferPromptEvent) -> Self {
        let tool_name = Some(prompt.tool.clone());
        Self {
            text: String::new(),
            phase: STREAM_PHASE_AWAITING_DEFER.into(),
            seq,
            done: false,
            tool_name,
            error: None,
            ts_ms: prompt.ts_ms,
            defer: Some(prompt),
            delta: None,
            speakable: None,
        }
    }
}

// ── Speakable unit tracker (WEFT-350 voice TTS handoff) ──────────

/// Tracks accumulated assistant text and emits completed speakable
/// units (sentence / clause boundaries) for the STT→chat_stream→TTS
/// pipeline.
///
/// Boundaries: `.` `!` `?` `…` optionally followed by closing quotes /
/// brackets, then whitespace or end-of-string. Also flushes on `\n`.
#[derive(Debug, Default, Clone)]
pub struct SpeakableTracker {
    /// Text already emitted as speakable units (prefix of accumulated).
    emitted_upto: usize,
    /// Last full accumulated text seen (for delta computation).
    last_text: String,
}

/// One progressive stream step produced by [`SpeakableTracker::push`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamStep {
    /// Accumulated text so far.
    pub text: String,
    /// New characters since the previous push.
    pub delta: String,
    /// Newly closed speakable unit(s), joined; `None` if no boundary.
    pub speakable: Option<String>,
}

impl SpeakableTracker {
    /// Create an empty tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Ingest the latest **accumulated** draft and return delta +
    /// any newly closed speakable unit(s).
    pub fn push(&mut self, accumulated: &str) -> StreamStep {
        let delta = if accumulated.starts_with(&self.last_text) {
            accumulated[self.last_text.len()..].to_string()
        } else if self.last_text.is_empty() {
            accumulated.to_string()
        } else {
            // Non-prefix growth (rewrite) — treat full string as delta.
            accumulated.to_string()
        };
        self.last_text = accumulated.to_string();

        let speakable = self.drain_speakable(accumulated);
        StreamStep {
            text: accumulated.to_string(),
            delta,
            speakable,
        }
    }

    /// Flush any remaining non-empty tail as a final speakable unit
    /// (call on stream `done`).
    pub fn flush(&mut self, accumulated: &str) -> Option<String> {
        if self.emitted_upto >= accumulated.len() {
            return None;
        }
        let rest = accumulated[self.emitted_upto..].trim();
        self.emitted_upto = accumulated.len();
        if rest.is_empty() {
            None
        } else {
            Some(rest.to_string())
        }
    }

    fn drain_speakable(&mut self, accumulated: &str) -> Option<String> {
        let bytes = accumulated.as_bytes();
        let mut start = self.emitted_upto;
        let mut units: Vec<String> = Vec::new();
        let mut i = start;
        while i < bytes.len() {
            let ch = bytes[i] as char;
            // UTF-8: only ASCII punctuation checked here; multi-byte
            // ellipsis is rare in LLM output (models emit "...").
            if matches!(ch, '.' | '!' | '?' | '\n') {
                let mut end = i + 1;
                // Swallow trailing closers: "')]}
                while end < bytes.len()
                    && matches!(bytes[end] as char, '"' | '\'' | ')' | ']' | '}')
                {
                    end += 1;
                }
                // Require whitespace or EOS after boundary (avoid
                // "3.14" / "e.g." mid-token splits when next is alnum).
                let ok = end >= bytes.len()
                    || (bytes[end] as char).is_whitespace()
                    || ch == '\n';
                if ok {
                    let unit = accumulated[start..end].trim();
                    if !unit.is_empty() {
                        units.push(unit.to_string());
                    }
                    // Skip whitespace after boundary.
                    while end < bytes.len() && (bytes[end] as char).is_whitespace() {
                        end += 1;
                    }
                    start = end;
                    i = end;
                    continue;
                }
            }
            i += 1;
        }
        self.emitted_upto = start;
        if units.is_empty() {
            None
        } else {
            Some(units.join(" "))
        }
    }
}

/// Cascade helper: split full assistant text into progressive frames
/// with delta + speakable fields for the voice path (WEFT-350).
///
/// Word-ish chunks (whitespace boundaries, max `chunk_chars`) keep
/// frame count bounded while still feeding TTS sentence units early.
pub fn cascade_stream_frames(full: &str, chunk_chars: usize) -> Vec<AgentChatStreamFrame> {
    let chunk = chunk_chars.max(2);
    let chars: Vec<char> = full.chars().collect();
    let mut tracker = SpeakableTracker::new();
    let mut frames = Vec::new();
    let mut seq: u64 = 1;
    let mut emitted = 0usize;
    while emitted < chars.len() {
        let mut take = 0usize;
        while emitted + take < chars.len() && take < chunk {
            take += 1;
            if take >= 2 && chars[emitted + take - 1].is_whitespace() {
                break;
            }
        }
        emitted += take;
        let prefix: String = chars[..emitted].iter().collect();
        let step = tracker.push(&prefix);
        frames.push(AgentChatStreamFrame::generating_voice(
            seq,
            step.text,
            step.delta,
            step.speakable,
        ));
        seq = seq.saturating_add(1);
    }
    // Terminal flush of any trailing clause without punctuation.
    if let Some(tail) = tracker.flush(full)
        && let Some(last) = frames.last_mut()
    {
        // Attach residual speakable onto the last generating frame
        // if it had none; otherwise emit an extra frame.
        if last.speakable.is_none() {
            last.speakable = Some(tail);
        } else {
            frames.push(AgentChatStreamFrame::generating_voice(
                seq,
                full,
                "",
                Some(tail),
            ));
            seq = seq.saturating_add(1);
        }
    }
    frames.push(AgentChatStreamFrame::done_ok(seq, full));
    frames
}

// ── Typed agent.chat errors (WEFT-334) ───────────────────────────
//
// v1 surfaced failures only as `Response::error("agent.chat: <inner>")`
// strings. Panels had to substring-match to distinguish timeout from
// LLM failure from gate denial. This enum is the v1.1 structured
// surface: wire JSON carries an `error_kind` discriminator alongside
// the legacy string `error` field so old clients keep working.

/// Structured error variants for `agent.chat` / `agent.chat_stream`
/// (WEFT-334).
///
/// # Wire format
///
/// Serializes as a JSON object with an `error_kind` tag and a
/// human-readable `message`:
///
/// ```json
/// { "error_kind": "timeout", "message": "operation timed out: llm_call" }
/// ```
///
/// On the RPC envelope the daemon also mirrors `message` into the
/// legacy string field `Response.error` (prefixed with the method
/// name, e.g. `"agent.chat: operation timed out: llm_call"`) and
/// places `error_kind` at the top level so panels can branch without
/// parsing the body. Clients that only read `error` as a string remain
/// wire-compatible.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "error_kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum AgentChatError {
    /// The turn exceeded a deadline (LLM call, dispatch select, etc.).
    #[error("{message}")]
    Timeout {
        /// Human-readable detail.
        message: String,
    },
    /// Effect / capability gate refused the action.
    #[error("{message}")]
    GateDeny {
        /// Human-readable detail (reason / tool name when known).
        message: String,
    },
    /// Upstream LLM / provider failure (5xx, bad request, rate limit).
    #[error("{message}")]
    LlmError {
        /// Human-readable detail.
        message: String,
    },
    /// Per-conversation cancel token tripped mid-turn.
    #[error("{message}")]
    Cancelled {
        /// Human-readable detail.
        message: String,
    },
    /// Cost circuit-breaker tripped (WEFT-322).
    #[error("{message}")]
    BudgetExceeded {
        /// Human-readable detail.
        message: String,
    },
    /// Agent service is draining and refused new work.
    #[error("{message}")]
    ShuttingDown {
        /// Human-readable detail.
        message: String,
    },
    /// LLM client failed to wire at daemon boot (`DAEMON_AGENT` unset).
    #[error("{message}")]
    NotWired {
        /// Human-readable detail.
        message: String,
    },
    /// Request params failed to deserialize / failed validation.
    #[error("{message}")]
    InvalidParams {
        /// Human-readable detail.
        message: String,
    },
    /// Catch-all for unclassified internal failures.
    #[error("{message}")]
    Internal {
        /// Human-readable detail.
        message: String,
    },
}

impl AgentChatError {
    /// Stable wire discriminator for this variant (`"timeout"`,
    /// `"gate_deny"`, `"llm_error"`, …). Matches the serde
    /// `rename_all = "snake_case"` tag value.
    pub fn error_kind(&self) -> &'static str {
        match self {
            Self::Timeout { .. } => "timeout",
            Self::GateDeny { .. } => "gate_deny",
            Self::LlmError { .. } => "llm_error",
            Self::Cancelled { .. } => "cancelled",
            Self::BudgetExceeded { .. } => "budget_exceeded",
            Self::ShuttingDown { .. } => "shutting_down",
            Self::NotWired { .. } => "not_wired",
            Self::InvalidParams { .. } => "invalid_params",
            Self::Internal { .. } => "internal",
        }
    }

    /// Borrow the human-readable message body.
    pub fn message(&self) -> &str {
        match self {
            Self::Timeout { message }
            | Self::GateDeny { message }
            | Self::LlmError { message }
            | Self::Cancelled { message }
            | Self::BudgetExceeded { message }
            | Self::ShuttingDown { message }
            | Self::NotWired { message }
            | Self::InvalidParams { message }
            | Self::Internal { message } => message.as_str(),
        }
    }

    /// Legacy `Response.error` string: `"{method}: {message}"`.
    ///
    /// Mirrors the pre-WEFT-334 `Response::error(format!("agent.chat: {e}"))`
    /// shape so older panels that only read the string field keep working.
    pub fn legacy_message(&self, method: &str) -> String {
        format!("{method}: {}", self.message())
    }

    /// Convenience constructors.
    pub fn timeout(message: impl Into<String>) -> Self {
        Self::Timeout {
            message: message.into(),
        }
    }

    /// Gate / capability denial.
    pub fn gate_deny(message: impl Into<String>) -> Self {
        Self::GateDeny {
            message: message.into(),
        }
    }

    /// LLM / provider failure.
    pub fn llm_error(message: impl Into<String>) -> Self {
        Self::LlmError {
            message: message.into(),
        }
    }

    /// Cancelled conversation.
    pub fn cancelled(message: impl Into<String>) -> Self {
        Self::Cancelled {
            message: message.into(),
        }
    }

    /// Cost budget circuit open.
    pub fn budget_exceeded(message: impl Into<String>) -> Self {
        Self::BudgetExceeded {
            message: message.into(),
        }
    }

    /// Service shutting down.
    pub fn shutting_down(message: impl Into<String>) -> Self {
        Self::ShuttingDown {
            message: message.into(),
        }
    }

    /// Agent service not wired.
    pub fn not_wired(message: impl Into<String>) -> Self {
        Self::NotWired {
            message: message.into(),
        }
    }

    /// Invalid request params.
    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::InvalidParams {
            message: message.into(),
        }
    }

    /// Unclassified internal error.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal {
            message: message.into(),
        }
    }

    /// Classify a free-form agent-loop / service error string into a
    /// structured variant.
    ///
    /// The agent-loop trait boundary still returns `String`
    /// (`AgentLoopHandle::handle_turn`); this maps well-known
    /// [`crate::ClawftError`] display prefixes onto the panel-facing
    /// kinds. Unrecognized text becomes [`Self::Internal`].
    pub fn classify_loop_error(err: &str) -> Self {
        let lower = err.to_ascii_lowercase();
        // Order matters: more specific prefixes first.
        if lower.contains("was cancelled") {
            return Self::cancelled(err);
        }
        if lower.contains("timed out") || lower.contains("timeout") {
            return Self::timeout(err);
        }
        if lower.contains("budget exceeded")
            || lower.contains("circuit_open")
            || lower.contains("conversation budget")
        {
            return Self::budget_exceeded(err);
        }
        if lower.contains("gate den")
            || lower.contains("permission denied")
            || lower.contains("security violation")
            || lower.contains("not allowed")
            || lower.contains("capability")
        {
            return Self::gate_deny(err);
        }
        if lower.contains("provider error")
            || lower.contains("rate limited")
            || lower.contains("llm")
            || lower.contains("openrouter")
            || lower.contains("llama")
        {
            return Self::llm_error(err);
        }
        if lower.contains("shutting down") {
            return Self::shutting_down(err);
        }
        if lower.contains("not wired") {
            return Self::not_wired(err);
        }
        Self::internal(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_chat_params_omitted_conv_id_gets_default() {
        let json = r#"{"messages":[{"role":"user","content":"hi"}]}"#;
        let params: AgentChatParams = serde_json::from_str(json).unwrap();
        assert!(
            params.conv_id.starts_with("ephemeral-"),
            "default conv_id must be ephemeral-shaped, got {:?}",
            params.conv_id
        );
        assert_eq!(params.messages.len(), 1);
    }

    #[test]
    fn agent_chat_params_explicit_conv_id_round_trips() {
        let json = r#"{"messages":[],"conv_id":"01HQ123ABCXYZ"}"#;
        let params: AgentChatParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.conv_id, "01HQ123ABCXYZ");
        assert!(params.caller_id.is_none());
    }

    #[test]
    fn agent_chat_params_caller_id_round_trips() {
        let json = r#"{"messages":[],"conv_id":"c1","caller_id":"alice"}"#;
        let params: AgentChatParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.caller_id.as_deref(), Some("alice"));
        let back = serde_json::to_value(&params).unwrap();
        assert_eq!(back["caller_id"], "alice");
    }

    #[test]
    fn caller_principal_name_namespaces_user() {
        assert_eq!(
            caller_principal_name("alice"),
            "agent.chat:user:alice"
        );
        assert_ne!(
            caller_principal_name("alice"),
            caller_principal_name("bob")
        );
    }

    #[test]
    fn agent_chat_params_default_conv_ids_are_distinct() {
        let a = default_conv_id();
        let b = default_conv_id();
        assert_ne!(a, b);
    }

    #[test]
    fn agent_chat_result_skips_optional_fields_when_none() {
        let r = AgentChatResult {
            assistant_text: "hi".into(),
            tool_calls: Vec::new(),
            finish_reason: "stop".into(),
            iterations: 1,
            prompt_tokens: 0,
            completion_tokens: 0,
            model: None,
            identity_source: None,
            reasoning: None,
            spawned_tasks: Vec::new(),
            escalation: None,
            deferred: None,
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(!json.contains("\"model\""));
        assert!(!json.contains("\"identity_source\""));
        assert!(!json.contains("\"reasoning\""));
        assert!(json.contains("\"tool_calls\":[]"));
        // Empty spawned_tasks must not appear on the wire (backward-compat).
        assert!(!json.contains("\"spawned_tasks\""));
        assert!(!json.contains("\"escalation\""));
        assert!(!json.contains("\"deferred\""));
    }

    #[test]
    fn chat_stream_path_is_sibling_of_status() {
        let p = chat_stream_path("panel-1");
        assert_eq!(p, "substrate/_derived/chat/panel-1/stream");
        assert!(p.starts_with("substrate/_derived/chat/"));
        assert!(p.ends_with("/stream"));
    }

    #[test]
    fn stream_frame_round_trips_and_helpers() {
        let f = AgentChatStreamFrame::generating(3, "hel");
        let v = serde_json::to_value(&f).unwrap();
        assert_eq!(v["phase"], "generating");
        assert_eq!(v["text"], "hel");
        assert_eq!(v["seq"], 3);
        assert_eq!(v["done"], false);
        let back: AgentChatStreamFrame = serde_json::from_value(v).unwrap();
        assert_eq!(back, f);

        let done = AgentChatStreamFrame::done_ok(4, "hello");
        assert!(done.done);
        assert_eq!(done.phase, "done");

        let err = AgentChatStreamFrame::done_err(5, "boom");
        assert!(err.done);
        assert_eq!(err.error.as_deref(), Some("boom"));
    }

    #[test]
    fn stream_frame_voice_fields_round_trip() {
        // WEFT-350: delta + speakable are optional and omit when None.
        let f = AgentChatStreamFrame::generating_voice(2, "Hello. ", "Hello. ", Some("Hello.".into()));
        let v = serde_json::to_value(&f).unwrap();
        assert_eq!(v["delta"], "Hello. ");
        assert_eq!(v["speakable"], "Hello.");
        let back: AgentChatStreamFrame = serde_json::from_value(v).unwrap();
        assert_eq!(back.delta.as_deref(), Some("Hello. "));
        assert_eq!(back.speakable.as_deref(), Some("Hello."));
        // Legacy frames without delta still deserialize.
        let legacy = r#"{"text":"x","phase":"generating","seq":1,"done":false}"#;
        let old: AgentChatStreamFrame = serde_json::from_str(legacy).unwrap();
        assert!(old.delta.is_none());
        assert!(old.speakable.is_none());
    }

    #[test]
    fn cascade_stream_frames_emits_deltas_and_done() {
        let frames = cascade_stream_frames("Hi. Bye.", 8);
        assert!(frames.len() >= 2);
        assert!(frames.last().unwrap().done);
        assert_eq!(frames.last().unwrap().phase, "done");
        let deltas: String = frames.iter().filter_map(|f| f.delta.clone()).collect();
        assert!(deltas.contains('H') || deltas.contains("Hi"), "deltas={deltas:?}");
        // Speakable should close at least the first sentence.
        let spoken: Vec<_> = frames.iter().filter_map(|f| f.speakable.clone()).collect();
        assert!(
            spoken.iter().any(|s| s.contains("Hi")),
            "speakable units={spoken:?}"
        );
    }

    #[test]
    fn agent_chat_result_serializes_spawned_tasks_when_present() {
        let r = AgentChatResult {
            assistant_text: "kicked off work".into(),
            tool_calls: Vec::new(),
            finish_reason: "stop".into(),
            iterations: 2,
            prompt_tokens: 0,
            completion_tokens: 0,
            model: None,
            identity_source: None,
            reasoning: None,
            spawned_tasks: vec![SpawnedTaskSummary {
                task_id: "task-1".into(),
                child_conv_id: "sub:P:01HQ".into(),
                status: "completed".into(),
            }],
            escalation: None,
            deferred: None,
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"spawned_tasks\""));
        assert!(json.contains("\"task-1\""));
        // Round-trips back to the same summary.
        let back: AgentChatResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.spawned_tasks.len(), 1);
        assert_eq!(back.spawned_tasks[0].status, "completed");
    }

    #[test]
    fn agent_chat_result_old_client_without_spawned_tasks_deserializes() {
        // A pre-M4 payload omits spawned_tasks entirely; it must
        // deserialize to an empty vec, not fail.
        let json = r#"{
            "assistant_text": "hi",
            "tool_calls": [],
            "finish_reason": "stop",
            "iterations": 1,
            "prompt_tokens": 0,
            "completion_tokens": 0
        }"#;
        let r: AgentChatResult = serde_json::from_str(json).unwrap();
        assert!(r.spawned_tasks.is_empty());
    }

    #[test]
    fn agent_loop_result_meta_round_trips_through_metadata_value() {
        // The loop stashes this as a serde_json::Value in
        // OutboundMessage.metadata; the service reads it back.
        let meta = AgentLoopResultMeta {
            tool_calls: vec![AgentChatToolCall {
                name: "agent_spawn".into(),
                arguments_preview: "{\"goal\":\"answer 2+2\"}".into(),
                result_preview: "{\"status\":\"completed\"}".into(),
                success: true,
            }],
            finish_reason: "stop".into(),
            iterations: 3,
            spawned_tasks: vec![SpawnedTaskSummary {
                task_id: "t1".into(),
                child_conv_id: "sub:P:01HQ".into(),
                status: "completed".into(),
            }],
            model: Some("hermes-4.3-36b".into()),
            prompt_tokens: 812,
            completion_tokens: 96,
            reasoning: Some("the user asked for a sum".into()),
            identity_source: Some("clawft".into()),
            escalation: None,
            deferred: None,
        };
        let value = serde_json::to_value(&meta).unwrap();
        let back: AgentLoopResultMeta = serde_json::from_value(value).unwrap();
        assert_eq!(back.iterations, 3);
        assert_eq!(back.tool_calls.len(), 1);
        assert_eq!(back.spawned_tasks.len(), 1);
        assert_eq!(back.spawned_tasks[0].task_id, "t1");
        assert_eq!(back.model.as_deref(), Some("hermes-4.3-36b"));
        assert_eq!(back.prompt_tokens, 812);
        assert_eq!(back.completion_tokens, 96);
        assert_eq!(back.reasoning.as_deref(), Some("the user asked for a sum"));
        assert_eq!(back.identity_source.as_deref(), Some("clawft"));
        assert!(back.escalation.is_none());
        assert!(back.deferred.is_none());
    }

    #[test]
    fn agent_loop_result_meta_partial_object_uses_defaults() {
        // A partial/absent object degrades to defaults rather than
        // failing to deserialize — C1 panel shape stays tolerant.
        let back: AgentLoopResultMeta = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(back.iterations, 0);
        assert!(back.finish_reason.is_empty());
        assert!(back.tool_calls.is_empty());
        assert!(back.spawned_tasks.is_empty());
        assert_eq!(back.prompt_tokens, 0);
        assert_eq!(back.completion_tokens, 0);
        assert!(back.model.is_none());
        assert!(back.identity_source.is_none());
        assert!(back.escalation.is_none());
        assert!(back.deferred.is_none());
    }

    #[test]
    fn escalate_to_human_event_from_denials() {
        let event = EscalateToHumanEvent::from_denials(
            "conv-1",
            vec![
                GateDenialRecord {
                    tool: "write_file".into(),
                    reason: "policy".into(),
                },
                GateDenialRecord {
                    tool: "exec_shell".into(),
                    reason: "high risk".into(),
                },
                GateDenialRecord {
                    tool: "write_file".into(),
                    reason: "policy".into(),
                },
            ],
        );
        assert_eq!(event.decision, GOVERNANCE_DECISION_ESCALATE_TO_HUMAN);
        assert_eq!(event.denial_count, 3);
        assert_eq!(event.conv_id, "conv-1");
        assert_eq!(event.denials.len(), 3);
        assert!(event.summary.contains("EscalateToHuman"));
        assert!(event.summary.contains("3x"));
    }

    #[test]
    fn agent_chat_result_escalation_round_trips() {
        let r = AgentChatResult {
            assistant_text: "escalated".into(),
            tool_calls: vec![],
            finish_reason: FINISH_REASON_ESCALATE_TO_HUMAN.into(),
            iterations: 3,
            prompt_tokens: 10,
            completion_tokens: 5,
            model: None,
            identity_source: None,
            reasoning: None,
            spawned_tasks: vec![],
            escalation: Some(EscalateToHumanEvent::from_denials(
                "c",
                vec![GateDenialRecord {
                    tool: "echo".into(),
                    reason: "blocked".into(),
                }],
            )),
            deferred: None,
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("EscalateToHuman"));
        let back: AgentChatResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.finish_reason, FINISH_REASON_ESCALATE_TO_HUMAN);
        assert_eq!(back.escalation.as_ref().unwrap().decision, "EscalateToHuman");
    }

    #[test]
    fn deferred_action_event_wire_shape() {
        let event = DeferredActionEvent::new(
            "conv-1",
            "write_file",
            "policy review pending",
            "{\"path\":\"x\"}",
        );
        assert!(event.deferred);
        assert_eq!(event.tool, "write_file");
        assert_eq!(event.reason, "policy review pending");
        assert!(event.summary.contains("write_file"));
        let v = serde_json::to_value(&event).unwrap();
        assert_eq!(v["deferred"], true);
        assert_eq!(v["reason"], "policy review pending");
    }

    #[test]
    fn agent_chat_result_deferred_round_trips() {
        let event = DeferredActionEvent::new("c", "echo", "needs review", "{}");
        let r = AgentChatResult {
            assistant_text: event.summary.clone(),
            tool_calls: vec![],
            finish_reason: FINISH_REASON_DEFERRED.into(),
            iterations: 1,
            prompt_tokens: 5,
            completion_tokens: 2,
            model: None,
            identity_source: None,
            reasoning: None,
            spawned_tasks: vec![],
            escalation: None,
            deferred: Some(event),
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"deferred\":true") || json.contains("\"deferred\": true"));
        let back: AgentChatResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.finish_reason, FINISH_REASON_DEFERRED);
        let d = back.deferred.expect("deferred event");
        assert!(d.deferred);
        assert_eq!(d.tool, "echo");
        assert_eq!(d.reason, "needs review");
    }

    #[test]
    fn agent_chat_result_deserializes_nonzero_token_and_identity_fields() {
        // WEFT-328: panel / clients must accept real (non-default) values
        // for the five fields plumbed through the loop meta.
        let json = r#"{
            "assistant_text": "done",
            "tool_calls": [{"name":"echo","arguments_preview":"{}","result_preview":"ok","success":true}],
            "finish_reason": "stop",
            "iterations": 2,
            "prompt_tokens": 40,
            "completion_tokens": 22,
            "model": "hermes-4.3-36b",
            "identity_source": "clawft"
        }"#;
        let r: AgentChatResult = serde_json::from_str(json).unwrap();
        assert_eq!(r.tool_calls.len(), 1);
        assert_eq!(r.prompt_tokens, 40);
        assert_eq!(r.completion_tokens, 22);
        assert_eq!(r.model.as_deref(), Some("hermes-4.3-36b"));
        assert_eq!(r.identity_source.as_deref(), Some("clawft"));
    }

    #[test]
    fn defer_user_decision_parses_wire_strings() {
        assert_eq!(
            DeferUserDecision::parse("allow"),
            Some(DeferUserDecision::Allow)
        );
        assert_eq!(
            DeferUserDecision::parse("DENY"),
            Some(DeferUserDecision::Deny)
        );
        assert_eq!(
            DeferUserDecision::parse("cancel"),
            Some(DeferUserDecision::Cancel)
        );
        assert!(DeferUserDecision::parse("maybe").is_none());
    }

    #[test]
    fn defer_prompt_and_decide_params_round_trip() {
        let prompt = DeferPromptEvent {
            defer_id: "d-1".into(),
            conv_id: "c-1".into(),
            tool: "write_file".into(),
            reason: "needs review".into(),
            arguments_preview: r#"{"path":"x"}"#.into(),
            timeout_ms: DEFER_DEFAULT_TIMEOUT_MS,
            ts_ms: Some(1),
        };
        let frame = AgentChatStreamFrame::awaiting_defer(7, prompt.clone());
        assert_eq!(frame.phase, STREAM_PHASE_AWAITING_DEFER);
        assert!(!frame.done);
        assert_eq!(frame.defer.as_ref().unwrap().defer_id, "d-1");
        let v = serde_json::to_value(&frame).unwrap();
        let back: AgentChatStreamFrame = serde_json::from_value(v).unwrap();
        assert_eq!(back, frame);

        let params = AgentChatDeferDecideParams {
            conv_id: "c-1".into(),
            defer_id: "d-1".into(),
            decision: DEFER_DECISION_ALLOW.into(),
        };
        let json = serde_json::to_string(&params).unwrap();
        let p2: AgentChatDeferDecideParams = serde_json::from_str(&json).unwrap();
        assert_eq!(p2.decision, "allow");
        assert_eq!(chat_defer_path("c-1"), "substrate/_derived/chat/c-1/defer");
    }

    // ── WEFT-334: AgentChatError ──────────────────────────────────

    #[test]
    fn agent_chat_error_timeout_wire_shape() {
        let err = AgentChatError::timeout("operation timed out: llm_call");
        let v = serde_json::to_value(&err).unwrap();
        assert_eq!(v["error_kind"], "timeout");
        assert_eq!(v["message"], "operation timed out: llm_call");
        assert_eq!(err.error_kind(), "timeout");
        let back: AgentChatError = serde_json::from_value(v).unwrap();
        assert_eq!(back, err);
    }

    #[test]
    fn agent_chat_error_gate_deny_wire_shape() {
        let err = AgentChatError::gate_deny("permission denied: write_file");
        let v = serde_json::to_value(&err).unwrap();
        assert_eq!(v["error_kind"], "gate_deny");
        assert_eq!(err.error_kind(), "gate_deny");
        let back: AgentChatError = serde_json::from_value(v).unwrap();
        assert_eq!(back, err);
    }

    #[test]
    fn agent_chat_error_llm_error_wire_shape() {
        let err = AgentChatError::llm_error("provider error: 502 bad gateway");
        let v = serde_json::to_value(&err).unwrap();
        assert_eq!(v["error_kind"], "llm_error");
        assert_eq!(err.error_kind(), "llm_error");
        let back: AgentChatError = serde_json::from_value(v).unwrap();
        assert_eq!(back, err);
    }

    #[test]
    fn agent_chat_error_cancelled_wire_shape() {
        let err = AgentChatError::cancelled("conversation `c1` was cancelled");
        let v = serde_json::to_value(&err).unwrap();
        assert_eq!(v["error_kind"], "cancelled");
        let back: AgentChatError = serde_json::from_value(v).unwrap();
        assert_eq!(back, err);
    }

    #[test]
    fn agent_chat_error_budget_exceeded_wire_shape() {
        let err = AgentChatError::budget_exceeded(
            "conversation budget exceeded for `c1`: tokens 100 >= limit 100 (circuit_open until reset_budget)",
        );
        let v = serde_json::to_value(&err).unwrap();
        assert_eq!(v["error_kind"], "budget_exceeded");
        let back: AgentChatError = serde_json::from_value(v).unwrap();
        assert_eq!(back, err);
    }

    #[test]
    fn agent_chat_error_shutting_down_wire_shape() {
        let err = AgentChatError::shutting_down("agent service shutting down");
        let v = serde_json::to_value(&err).unwrap();
        assert_eq!(v["error_kind"], "shutting_down");
        let back: AgentChatError = serde_json::from_value(v).unwrap();
        assert_eq!(back, err);
    }

    #[test]
    fn agent_chat_error_not_wired_wire_shape() {
        let err = AgentChatError::not_wired(
            "agent service not wired (LLM client init failed at boot)",
        );
        let v = serde_json::to_value(&err).unwrap();
        assert_eq!(v["error_kind"], "not_wired");
        let back: AgentChatError = serde_json::from_value(v).unwrap();
        assert_eq!(back, err);
    }

    #[test]
    fn agent_chat_error_invalid_params_wire_shape() {
        let err = AgentChatError::invalid_params("missing field `messages`");
        let v = serde_json::to_value(&err).unwrap();
        assert_eq!(v["error_kind"], "invalid_params");
        let back: AgentChatError = serde_json::from_value(v).unwrap();
        assert_eq!(back, err);
    }

    #[test]
    fn agent_chat_error_internal_wire_shape() {
        let err = AgentChatError::internal("unexpected state");
        let v = serde_json::to_value(&err).unwrap();
        assert_eq!(v["error_kind"], "internal");
        let back: AgentChatError = serde_json::from_value(v).unwrap();
        assert_eq!(back, err);
    }

    #[test]
    fn agent_chat_error_legacy_message_prefix() {
        let err = AgentChatError::timeout("operation timed out: llm_call");
        assert_eq!(
            err.legacy_message("agent.chat"),
            "agent.chat: operation timed out: llm_call"
        );
        assert_eq!(
            err.legacy_message("agent.chat_stream"),
            "agent.chat_stream: operation timed out: llm_call"
        );
    }

    #[test]
    fn agent_chat_error_classify_loop_error_variants() {
        assert_eq!(
            AgentChatError::classify_loop_error("operation timed out: llm_call").error_kind(),
            "timeout"
        );
        assert_eq!(
            AgentChatError::classify_loop_error("security violation: path traversal").error_kind(),
            "gate_deny"
        );
        assert_eq!(
            AgentChatError::classify_loop_error("provider error: 500").error_kind(),
            "llm_error"
        );
        assert_eq!(
            AgentChatError::classify_loop_error("conversation `x` was cancelled").error_kind(),
            "cancelled"
        );
        assert_eq!(
            AgentChatError::classify_loop_error(
                "conversation budget exceeded for `c`: tokens 1 >= limit 1 (circuit_open until reset_budget)"
            )
            .error_kind(),
            "budget_exceeded"
        );
        assert_eq!(
            AgentChatError::classify_loop_error("something completely novel").error_kind(),
            "internal"
        );
    }

    #[test]
    fn agent_chat_error_display_is_message_body() {
        let err = AgentChatError::llm_error("provider error: boom");
        assert_eq!(err.to_string(), "provider error: boom");
        assert_eq!(err.message(), "provider error: boom");
    }
}
