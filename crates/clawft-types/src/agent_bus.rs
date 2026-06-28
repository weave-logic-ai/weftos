//! Inter-agent communication types.
//!
//! Defines [`InterAgentMessage`] for agent-to-agent communication
//! and [`MessagePayload`] for structured/binary content transport.
//!
//! These types are used by the [`AgentBus`](clawft_core::agent_bus::AgentBus)
//! for per-agent inbox delivery with TTL enforcement.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// A message exchanged between agents via the [`AgentBus`].
///
/// Each message has a unique ID, sender/recipient agent IDs, a task
/// description, and an arbitrary JSON payload. Messages can optionally
/// reference a parent message via `reply_to` for request/response patterns.
///
/// # TTL enforcement
///
/// Messages have a time-to-live. If undelivered within this duration,
/// they are dropped and logged at `warn` level.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterAgentMessage {
    /// Unique message identifier.
    pub id: Uuid,

    /// Agent ID of the sender.
    pub from_agent: String,

    /// Agent ID of the recipient.
    pub to_agent: String,

    /// Task description or intent.
    pub task: String,

    /// Arbitrary JSON payload.
    pub payload: Value,

    /// If this is a reply, the ID of the original message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<Uuid>,

    /// Time-to-live: message expires if undelivered within this duration.
    #[serde(
        serialize_with = "serialize_duration_secs",
        deserialize_with = "deserialize_duration_secs"
    )]
    pub ttl: Duration,

    /// Timestamp when the message was created (milliseconds since epoch).
    #[serde(default = "now_millis")]
    pub created_at_ms: i64,
}

impl InterAgentMessage {
    /// Create a new inter-agent message.
    pub fn new(
        from_agent: impl Into<String>,
        to_agent: impl Into<String>,
        task: impl Into<String>,
        payload: Value,
        ttl: Duration,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            from_agent: from_agent.into(),
            to_agent: to_agent.into(),
            task: task.into(),
            payload,
            reply_to: None,
            ttl,
            created_at_ms: now_millis(),
        }
    }

    /// Create a reply to an existing message.
    pub fn reply(original: &InterAgentMessage, payload: Value, ttl: Duration) -> Self {
        Self {
            id: Uuid::new_v4(),
            from_agent: original.to_agent.clone(),
            to_agent: original.from_agent.clone(),
            task: format!("reply to: {}", original.task),
            payload,
            reply_to: Some(original.id),
            ttl,
            created_at_ms: now_millis(),
        }
    }

    /// Check whether this message has expired based on its TTL.
    pub fn is_expired(&self) -> bool {
        let elapsed_ms = now_millis() - self.created_at_ms;
        elapsed_ms > self.ttl.as_millis() as i64
    }
}

/// Structured and binary payload types for future canvas/voice support.
///
/// This enum provides forward-compatibility for rich content types
/// beyond plain text.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessagePayload {
    /// Plain text content.
    Text { content: String },

    /// Structured JSON content.
    Structured { content: Value },

    /// Binary content with MIME type (e.g., image/png, audio/wav).
    Binary {
        mime_type: String,
        #[serde(with = "base64_bytes")]
        data: Vec<u8>,
    },
}

/// Errors from the agent bus.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum AgentBusError {
    /// The target agent is not registered on the bus.
    #[error("agent not found: {0}")]
    AgentNotFound(String),

    /// The agent's inbox is full (backpressure).
    #[error("inbox full for agent: {0}")]
    InboxFull(String),

    /// The message has expired (TTL exceeded).
    #[error("message expired (ttl: {ttl:?})")]
    MessageExpired { ttl: Duration },
}

fn now_millis() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn serialize_duration_secs<S>(d: &Duration, s: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    s.serialize_u64(d.as_secs())
}

fn deserialize_duration_secs<'de, D>(d: D) -> Result<Duration, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let secs = u64::deserialize(d)?;
    Ok(Duration::from_secs(secs))
}

/// Base64 serialization for binary payloads.
mod base64_bytes {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S>(data: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Simple hex encoding (no external base64 dep needed).
        let hex: String = data.iter().map(|b| format!("{b:02x}")).collect();
        hex.serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let hex = String::deserialize(deserializer)?;
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).map_err(serde::de::Error::custom))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inter_agent_message_new() {
        let msg = InterAgentMessage::new(
            "agent-a",
            "agent-b",
            "summarize document",
            serde_json::json!({"doc_id": 42}),
            Duration::from_secs(60),
        );
        assert_eq!(msg.from_agent, "agent-a");
        assert_eq!(msg.to_agent, "agent-b");
        assert_eq!(msg.task, "summarize document");
        assert!(msg.reply_to.is_none());
        assert!(!msg.is_expired());
    }

    #[test]
    fn inter_agent_message_reply() {
        let original = InterAgentMessage::new(
            "agent-a",
            "agent-b",
            "task",
            Value::Null,
            Duration::from_secs(60),
        );
        let reply = InterAgentMessage::reply(
            &original,
            serde_json::json!({"result": "done"}),
            Duration::from_secs(30),
        );
        assert_eq!(reply.from_agent, "agent-b");
        assert_eq!(reply.to_agent, "agent-a");
        assert_eq!(reply.reply_to, Some(original.id));
    }

    #[test]
    fn inter_agent_message_serde_roundtrip() {
        let msg = InterAgentMessage::new(
            "a",
            "b",
            "test",
            serde_json::json!({"key": "value"}),
            Duration::from_secs(120),
        );
        let json = serde_json::to_string(&msg).unwrap();
        let restored: InterAgentMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.from_agent, "a");
        assert_eq!(restored.to_agent, "b");
        assert_eq!(restored.task, "test");
        assert_eq!(restored.ttl, Duration::from_secs(120));
    }

    #[test]
    fn message_payload_text() {
        let p = MessagePayload::Text {
            content: "hello".into(),
        };
        let json = serde_json::to_string(&p).unwrap();
        let restored: MessagePayload = serde_json::from_str(&json).unwrap();
        match restored {
            MessagePayload::Text { content } => assert_eq!(content, "hello"),
            _ => panic!("expected Text variant"),
        }
    }

    #[test]
    fn message_payload_structured() {
        let p = MessagePayload::Structured {
            content: serde_json::json!({"key": "value"}),
        };
        let json = serde_json::to_string(&p).unwrap();
        let restored: MessagePayload = serde_json::from_str(&json).unwrap();
        match restored {
            MessagePayload::Structured { content } => {
                assert_eq!(content["key"], "value");
            }
            _ => panic!("expected Structured variant"),
        }
    }

    #[test]
    fn message_payload_binary() {
        let p = MessagePayload::Binary {
            mime_type: "image/png".into(),
            data: vec![0x89, 0x50, 0x4e, 0x47],
        };
        let json = serde_json::to_string(&p).unwrap();
        let restored: MessagePayload = serde_json::from_str(&json).unwrap();
        match restored {
            MessagePayload::Binary { mime_type, data } => {
                assert_eq!(mime_type, "image/png");
                assert_eq!(data, vec![0x89, 0x50, 0x4e, 0x47]);
            }
            _ => panic!("expected Binary variant"),
        }
    }

    #[test]
    fn agent_bus_error_display() {
        let err = AgentBusError::AgentNotFound("agent-x".into());
        assert_eq!(err.to_string(), "agent not found: agent-x");

        let err = AgentBusError::InboxFull("agent-y".into());
        assert_eq!(err.to_string(), "inbox full for agent: agent-y");

        let err = AgentBusError::MessageExpired {
            ttl: Duration::from_secs(30),
        };
        assert_eq!(err.to_string(), "message expired (ttl: 30s)");
    }
}
