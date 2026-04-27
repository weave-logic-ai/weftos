//! Wire-format types for `agent.chat`.
//!
//! These mirror the types in `clawft_weave::protocol` 1:1. The
//! duplication is **intentional and temporary**: this crate is the
//! canonical home for the agent.chat shape, and `clawft-weave` will
//! re-export from here once Phase C2 lands the daemon wiring (see
//! `docs/plans/agent-core-v1.md` Phase C2). C2's commit will delete
//! the duplicates from `clawft-weave::protocol::AgentChat*` and
//! replace them with a `pub use clawft_service_agent::protocol::*;`.
//!
//! Why mirror instead of re-export today: `clawft-weave` is the
//! downstream consumer of `clawft-service-agent` (Phase C2). Re-
//! exporting from `clawft-weave` here would create the wrong dep
//! direction; making the new crate own the canonical shape and have
//! `clawft-weave` adapt is the cleaner layering.
//!
//! The `serde` field shapes here MUST stay byte-compatible with
//! `clawft-weave::protocol`. The
//! `agent_chat_params_omitted_conv_id_gets_default` test in this
//! module pins the "no `conv_id`" wire shape; the matching test
//! lives in `clawft-weave::protocol`'s tests.

use serde::{Deserialize, Serialize};

/// One message in an `agent.chat` conversation.
///
/// Mirror of `clawft_weave::protocol::AgentChatMessage`. `role` is one
/// of `"system"` / `"user"` / `"assistant"`; the daemon prepends its
/// own system prompt and any `system` from the panel is appended after.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentChatMessage {
    /// `system` / `user` / `assistant`.
    pub role: String,
    /// Message content.
    pub content: String,
}

/// Parameters for `agent.chat`.
///
/// Mirror of `clawft_weave::protocol::AgentChatParams`. The panel sends
/// the full conversation each turn; cross-request state is owned by
/// the substrate-backed `ConversationSink` (Phase C3).
///
/// The `conv_id` field defaults to an ephemeral ULID-shaped id when
/// omitted so legacy panels keep working through Phase A; Phase C
/// callers supply a stable id and observe per-conv mutex behaviour
/// in [`crate::service::AgentService`].
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
    /// in [`crate::service::AgentService`] and (Phase C3) the substrate
    /// JSONL path `derived/chat/<conv_id>/turns/<ulid>`. Defaults to an
    /// ephemeral id so legacy panels keep working.
    #[serde(default = "default_conv_id")]
    pub conv_id: String,
}

/// Default conversation id when the caller omits `conv_id`.
///
/// Generates an ephemeral, timestamp-prefixed monotonic string so
/// successive default-id calls within the same millisecond do not
/// collide. Phase C will require callers to supply a stable id; until
/// then this keeps the legacy panel wire format working.
///
/// Algorithmically identical to
/// `clawft_weave::protocol::default_conv_id`; both must produce
/// `ephemeral-`-prefixed strings so the C2 cutover preserves panel
/// behaviour.
fn default_conv_id() -> String {
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
/// Mirror of `clawft_weave::protocol::AgentChatToolCall`. Renders as a
/// collapsible bubble in the panel between user and assistant turns.
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

/// Result of `agent.chat`.
///
/// Mirror of `clawft_weave::protocol::AgentChatResult`. Several fields
/// (`tool_calls`, `prompt_tokens`, `completion_tokens`, `model`,
/// `identity_source`) cannot be populated end-to-end from
/// [`OutboundMessage`](clawft_types::event::OutboundMessage) today —
/// `OutboundMessage` is a generic bus envelope without token counts
/// or tool-call summaries. The C1 skeleton's
/// [`crate::service::AgentService::dispatch`] returns best-effort
/// defaults (empty vec, 0, None) and traces a `debug!` for every
/// missing field; richer plumbing lands when the loop's result type
/// is enriched in C2/D3.
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
    /// Echoed model name (best-effort).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Identity descriptor surfaced to the panel — diagnostic for the
    /// drift-warning path. Spike emits the loaded source (e.g.
    /// `"docs-fallback"`); C1 leaves it `None` and lets the daemon
    /// inject the value at the wire boundary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_source: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_chat_params_omitted_conv_id_gets_default() {
        // Legacy wire format (no `conv_id`) must deserialize cleanly;
        // the default fills in an ephemeral id. This MUST stay aligned
        // with clawft-weave's matching test so the C2 cutover doesn't
        // change panel behaviour.
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
    }

    #[test]
    fn agent_chat_params_default_conv_ids_are_distinct() {
        // Successive default-id calls within the same millisecond must
        // not collide — the atomic counter component differentiates.
        let a = default_conv_id();
        let b = default_conv_id();
        assert_ne!(a, b);
    }

    #[test]
    fn agent_chat_result_serialises_known_shortfalls_as_defaults() {
        // Documents the C1 known shortfall: tool_calls, tokens, model,
        // identity_source default to empty/zero/None until the loop
        // result type is enriched in C2/D3. The panel must tolerate
        // these defaults for the cutover to be a flag flip.
        let r = AgentChatResult {
            assistant_text: "hi".into(),
            tool_calls: Vec::new(),
            finish_reason: "stop".into(),
            iterations: 1,
            prompt_tokens: 0,
            completion_tokens: 0,
            model: None,
            identity_source: None,
        };
        let json = serde_json::to_string(&r).unwrap();
        // model + identity_source are skipped when None.
        assert!(!json.contains("\"model\""));
        assert!(!json.contains("\"identity_source\""));
        // tool_calls is always present (no skip on empty Vec).
        assert!(json.contains("\"tool_calls\":[]"));
    }
}
