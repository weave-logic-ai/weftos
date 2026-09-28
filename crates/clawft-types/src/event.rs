//! Message event types for the channel bus.
//!
//! [`InboundMessage`] represents user input arriving from a channel,
//! while [`OutboundMessage`] represents agent responses heading back out.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// An inbound message received from a chat channel.
///
/// Carries the raw user input plus channel-specific metadata.
/// Use [`session_key`](InboundMessage::session_key) to derive a
/// stable session identifier from the channel + chat_id pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboundMessage {
    /// Channel name (e.g. "telegram", "slack", "discord").
    pub channel: String,

    /// Sender identifier within the channel.
    pub sender_id: String,

    /// Chat / conversation identifier within the channel.
    pub chat_id: String,

    /// Message text content.
    pub content: String,

    /// When the message was received.
    #[serde(default = "Utc::now")]
    pub timestamp: DateTime<Utc>,

    /// URLs or identifiers for attached media.
    #[serde(default)]
    pub media: Vec<String>,

    /// Arbitrary channel-specific metadata.
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl InboundMessage {
    /// Unique key for session identification: `"{channel}:{chat_id}"`.
    pub fn session_key(&self) -> String {
        format!("{}:{}", self.channel, self.chat_id)
    }
}

/// An outbound message to send to a chat channel.
///
/// Produced by the agent pipeline and dispatched to the
/// appropriate channel adapter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboundMessage {
    /// Target channel name.
    pub channel: String,

    /// Target chat / conversation identifier.
    pub chat_id: String,

    /// Message text content.
    pub content: String,

    /// Optional message ID to reply to.
    #[serde(default)]
    pub reply_to: Option<String>,

    /// URLs or identifiers for attached media.
    #[serde(default)]
    pub media: Vec<String>,

    /// Arbitrary channel-specific metadata.
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Metadata key the agent loop sets to `true` when a turn failed (provider
/// error, delegation failure, refused delegation, …). The content is still a
/// readable message for chat UIs; scripted callers such as one-shot
/// `weft agent -m` read this flag to exit non-zero instead of treating the
/// text as a successful reply.
pub const OUTBOUND_ERROR_KEY: &str = "error";

impl OutboundMessage {
    /// True when the producing turn failed (see [`OUTBOUND_ERROR_KEY`]).
    pub fn is_error(&self) -> bool {
        self.metadata
            .get(OUTBOUND_ERROR_KEY)
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    }

    /// Mark this message as the result of a failed turn.
    pub fn mark_error(&mut self) {
        self.metadata
            .insert(OUTBOUND_ERROR_KEY.to_string(), serde_json::Value::Bool(true));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outbound_error_flag_round_trips() {
        let mut msg = OutboundMessage {
            channel: "cli".into(),
            chat_id: "c".into(),
            content: "Delegation failed".into(),
            reply_to: None,
            media: vec![],
            metadata: HashMap::new(),
        };
        assert!(!msg.is_error());
        msg.mark_error();
        assert!(msg.is_error());
        let back: OutboundMessage = serde_json::from_str(&serde_json::to_string(&msg).unwrap()).unwrap();
        assert!(back.is_error());
    }

    #[test]
    fn inbound_session_key() {
        let msg = InboundMessage {
            channel: "telegram".into(),
            sender_id: "user123".into(),
            chat_id: "chat456".into(),
            content: "hello".into(),
            timestamp: Utc::now(),
            media: vec![],
            metadata: HashMap::new(),
        };
        assert_eq!(msg.session_key(), "telegram:chat456");
    }

    #[test]
    fn inbound_serde_roundtrip() {
        let msg = InboundMessage {
            channel: "slack".into(),
            sender_id: "U12345".into(),
            chat_id: "C67890".into(),
            content: "test message".into(),
            timestamp: Utc::now(),
            media: vec!["https://example.com/image.png".into()],
            metadata: {
                let mut m = HashMap::new();
                m.insert("thread_ts".into(), serde_json::json!("123.456"));
                m
            },
        };
        let json = serde_json::to_string(&msg).unwrap();
        let restored: InboundMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.channel, "slack");
        assert_eq!(restored.sender_id, "U12345");
        assert_eq!(restored.chat_id, "C67890");
        assert_eq!(restored.content, "test message");
        assert_eq!(restored.media.len(), 1);
        assert!(restored.metadata.contains_key("thread_ts"));
    }

    #[test]
    fn inbound_defaults_on_missing_fields() {
        let json = r#"{
            "channel": "discord",
            "sender_id": "u1",
            "chat_id": "c1",
            "content": "hi"
        }"#;
        let msg: InboundMessage = serde_json::from_str(json).unwrap();
        assert!(msg.media.is_empty());
        assert!(msg.metadata.is_empty());
    }

    #[test]
    fn outbound_serde_roundtrip() {
        let msg = OutboundMessage {
            channel: "telegram".into(),
            chat_id: "chat456".into(),
            content: "reply".into(),
            reply_to: Some("msg789".into()),
            media: vec![],
            metadata: HashMap::new(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let restored: OutboundMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.channel, "telegram");
        assert_eq!(restored.reply_to.as_deref(), Some("msg789"));
    }

    #[test]
    fn outbound_reply_to_optional() {
        let json = r#"{
            "channel": "slack",
            "chat_id": "c1",
            "content": "msg"
        }"#;
        let msg: OutboundMessage = serde_json::from_str(json).unwrap();
        assert!(msg.reply_to.is_none());
        assert!(msg.media.is_empty());
    }
}
