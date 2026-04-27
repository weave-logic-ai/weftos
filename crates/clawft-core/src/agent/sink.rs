//! Conversation sink seam.
//!
//! [`ConversationSink`] is the substrate-bound write hook used by
//! [`AgentLoop::handle_turn`](super::loop_core::AgentLoop::handle_turn)
//! to persist per-turn JSONL and publish status heartbeats. The shape
//! of [`Turn`] matches the substrate JSONL layout from
//! `chat-agent-v1.md` §11.5 (one record per role event:
//! user / assistant / tool).
//!
//! Phase C3 will land a substrate-backed implementation in
//! `clawft-service-agent`. Today the in-memory [`InMemorySink`] is
//! the only impl — it satisfies the trait contract for tests and is
//! the default attached to [`AgentLoop`](super::loop_core::AgentLoop).
//!
//! ## Lock semantics
//!
//! [`ConversationSink::lock_conversation`] is the entry point for the
//! per-conv mutex used to serialise turns inside the same conversation
//! (see `chat-agent-v1.md` §10). The real implementation lives in the
//! `clawft-service-agent::AgentService` (Phase C1's
//! `DashMap<ConvId, Mutex<()>>`); the sink just exposes it so the
//! loop has a single seam. [`InMemorySink`] treats the call as a
//! no-op because tests run sequentially anyway.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;

/// One per-turn record. Mirrors the substrate JSONL line.
///
/// Tool-call intermediates and the final assistant response each get
/// their own [`Turn`]; the role discriminates. `tool_calls` is set on
/// assistant turns that invoked tools; `tool_call_id` is set on
/// `role == "tool"` turns to bind the result to its dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    /// Stable per-turn identifier. The sink is responsible for
    /// monotonic ULIDs in the substrate impl; in-memory tests can
    /// supply any unique string.
    pub turn_id: String,
    /// One of `"user" | "assistant" | "tool" | "system"`.
    pub role: String,
    /// Text payload — the message body.
    pub content: String,
    /// JSON tool_calls array on assistant turns that invoked tools.
    pub tool_calls: Option<Vec<serde_json::Value>>,
    /// Tool call ID this turn is responding to (only `role == "tool"`).
    pub tool_call_id: Option<String>,
    /// Wall-clock millisecond timestamp the sink saw the turn.
    pub ts_ms: u64,
}

/// Per-conversation persistence seam.
///
/// Implementations:
/// - [`InMemorySink`] (default; HashMap-backed, test-only).
/// - Substrate sink (Phase C3, in `clawft-service-agent`).
#[async_trait]
pub trait ConversationSink: Send + Sync + 'static {
    /// Acquire the conversation lock. The substrate impl awaits the
    /// per-conv `Mutex<()>` from the `AgentService` DashMap so two
    /// turns in the same conversation can never interleave. The
    /// in-memory impl is a no-op.
    async fn lock_conversation(&self, conv_id: &str);

    /// Append a [`Turn`] to the conversation log. Errors return a
    /// `String` so the trait stays cheap to implement; callers map
    /// to richer errors at the boundary.
    async fn append_turn(&self, conv_id: &str, turn: Turn) -> Result<(), String>;

    /// Publish a status event (heartbeat, "thinking", "tool_running",
    /// "done", …) to the substrate `derived/chat/<conv>/status`
    /// topic. The in-memory impl drops the payload.
    async fn publish_status(
        &self,
        conv_id: &str,
        status: &str,
        payload: serde_json::Value,
    ) -> Result<(), String>;

    /// Load the full conversation history. Substrate impl reads from
    /// `derived/chat/<conv>/turns/`; in-memory returns the HashMap
    /// entry.
    async fn load_history(&self, conv_id: &str) -> Result<Vec<Turn>, String>;
}

/// Test-only [`ConversationSink`] backed by a `Mutex<HashMap>`.
///
/// Mirrors the trait shape without any of the substrate machinery.
/// Used as the default sink for [`AgentLoop`] until Phase C3 lands
/// the real impl.
#[derive(Debug, Default)]
pub struct InMemorySink {
    turns: Mutex<HashMap<String, Vec<Turn>>>,
}

impl InMemorySink {
    /// Create an empty sink.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of turns recorded for `conv_id`. Useful for tests.
    pub fn turn_count(&self, conv_id: &str) -> usize {
        self.turns
            .lock()
            .map(|g| g.get(conv_id).map(|v| v.len()).unwrap_or(0))
            .unwrap_or(0)
    }
}

#[async_trait]
impl ConversationSink for InMemorySink {
    async fn lock_conversation(&self, _conv_id: &str) {
        // No-op for the in-memory impl. The real per-conv mutex
        // lives in clawft-service-agent (Phase C1).
    }

    async fn append_turn(&self, conv_id: &str, turn: Turn) -> Result<(), String> {
        let mut guard = self
            .turns
            .lock()
            .map_err(|_| "InMemorySink mutex poisoned".to_string())?;
        guard.entry(conv_id.to_string()).or_default().push(turn);
        Ok(())
    }

    async fn publish_status(
        &self,
        _conv_id: &str,
        _status: &str,
        _payload: serde_json::Value,
    ) -> Result<(), String> {
        // Tests don't observe status; drop on the floor.
        Ok(())
    }

    async fn load_history(&self, conv_id: &str) -> Result<Vec<Turn>, String> {
        let guard = self
            .turns
            .lock()
            .map_err(|_| "InMemorySink mutex poisoned".to_string())?;
        Ok(guard.get(conv_id).cloned().unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[allow(dead_code)]
    fn _coerce(_sink: &dyn ConversationSink) {}

    fn make_turn(role: &str, content: &str) -> Turn {
        Turn {
            turn_id: format!("test-{role}"),
            role: role.into(),
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
            ts_ms: 1_700_000_000_000,
        }
    }

    #[tokio::test]
    async fn round_trip_one_turn() {
        let sink = InMemorySink::new();
        sink.lock_conversation("c1").await;
        sink.append_turn("c1", make_turn("user", "hello"))
            .await
            .unwrap();

        let history = sink.load_history("c1").await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].role, "user");
        assert_eq!(history[0].content, "hello");
    }

    #[tokio::test]
    async fn empty_history_for_unknown_conv() {
        let sink = InMemorySink::new();
        let history = sink.load_history("nope").await.unwrap();
        assert!(history.is_empty());
    }

    #[tokio::test]
    async fn turns_are_isolated_per_conversation() {
        let sink = InMemorySink::new();
        sink.append_turn("a", make_turn("user", "from a"))
            .await
            .unwrap();
        sink.append_turn("b", make_turn("user", "from b"))
            .await
            .unwrap();

        let a = sink.load_history("a").await.unwrap();
        let b = sink.load_history("b").await.unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(b.len(), 1);
        assert_eq!(a[0].content, "from a");
        assert_eq!(b[0].content, "from b");
    }

    #[tokio::test]
    async fn append_preserves_order() {
        let sink = InMemorySink::new();
        sink.append_turn("c", make_turn("user", "1")).await.unwrap();
        sink.append_turn("c", make_turn("assistant", "2"))
            .await
            .unwrap();
        sink.append_turn("c", make_turn("tool", "3")).await.unwrap();

        let history = sink.load_history("c").await.unwrap();
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].content, "1");
        assert_eq!(history[1].content, "2");
        assert_eq!(history[2].content, "3");
    }

    #[tokio::test]
    async fn publish_status_is_a_noop() {
        let sink = InMemorySink::new();
        let r = sink
            .publish_status("c1", "thinking", json!({"step": 1}))
            .await;
        assert!(r.is_ok());
    }

    #[tokio::test]
    async fn turn_count_helper_reports_length() {
        let sink = InMemorySink::new();
        assert_eq!(sink.turn_count("x"), 0);
        sink.append_turn("x", make_turn("user", "hi"))
            .await
            .unwrap();
        assert_eq!(sink.turn_count("x"), 1);
    }

    #[tokio::test]
    async fn turn_with_tool_calls_round_trips() {
        let sink = InMemorySink::new();
        let turn = Turn {
            turn_id: "t1".into(),
            role: "assistant".into(),
            content: "calling".into(),
            tool_calls: Some(vec![json!({"name": "echo"})]),
            tool_call_id: None,
            ts_ms: 1_700_000_000_000,
        };
        sink.append_turn("c1", turn.clone()).await.unwrap();
        let history = sink.load_history("c1").await.unwrap();
        assert_eq!(history[0], turn);
    }
}
