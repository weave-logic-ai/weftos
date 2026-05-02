//! Tests for the Telegram channel plugin.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use clawft_types::error::ChannelError;
use clawft_types::event::{InboundMessage, OutboundMessage};

use crate::traits::{Channel, ChannelFactory, ChannelHost, ChannelStatus, Command};

use super::channel::{TelegramChannel, TelegramChannelFactory};
use super::types;

// ── Mock host ────────────────────────────────────────────────────────────

/// Mock host that collects delivered inbound messages.
struct MockHost {
    messages: tokio::sync::Mutex<Vec<InboundMessage>>,
}

impl MockHost {
    fn new() -> Self {
        Self {
            messages: tokio::sync::Mutex::new(vec![]),
        }
    }
}

#[async_trait]
impl ChannelHost for MockHost {
    async fn deliver_inbound(&self, msg: InboundMessage) -> Result<(), ChannelError> {
        self.messages.lock().await.push(msg);
        Ok(())
    }

    async fn register_command(&self, _cmd: Command) -> Result<(), ChannelError> {
        Ok(())
    }

    async fn publish_inbound(
        &self,
        _channel: &str,
        _sender_id: &str,
        _chat_id: &str,
        _content: &str,
        _media: Vec<String>,
        _metadata: HashMap<String, serde_json::Value>,
    ) -> Result<(), ChannelError> {
        Ok(())
    }
}

// ── is_allowed ───────────────────────────────────────────────────────────

#[test]
fn is_allowed_empty_list_allows_everyone() {
    let ch = TelegramChannel::new("tok".into(), vec![]);
    assert!(ch.is_allowed("123"));
    assert!(ch.is_allowed("anyone"));
    assert!(ch.is_allowed(""));
}

#[test]
fn is_allowed_with_list_allows_only_listed() {
    let ch = TelegramChannel::new("tok".into(), vec!["100".into(), "200".into()]);
    assert!(ch.is_allowed("100"));
    assert!(ch.is_allowed("200"));
    assert!(!ch.is_allowed("300"));
    assert!(!ch.is_allowed(""));
}

// ── metadata ─────────────────────────────────────────────────────────────

#[test]
fn metadata_values() {
    let ch = TelegramChannel::new("tok".into(), vec![]);
    let meta = ch.metadata();
    assert_eq!(meta.name, "telegram");
    assert_eq!(meta.display_name, "Telegram Bot");
    assert!(!meta.supports_threads);
    assert!(meta.supports_media);
}

// ── name ─────────────────────────────────────────────────────────────────

#[test]
fn name_is_telegram() {
    let ch = TelegramChannel::new("tok".into(), vec![]);
    assert_eq!(ch.name(), "telegram");
}

// ── status ───────────────────────────────────────────────────────────────

#[test]
fn initial_status_is_stopped() {
    let ch = TelegramChannel::new("tok".into(), vec![]);
    assert_eq!(ch.status(), ChannelStatus::Stopped);
}

// ── send (chat_id parsing) ───────────────────────────────────────────────

#[tokio::test]
async fn send_rejects_non_numeric_chat_id() {
    let ch = TelegramChannel::new("tok".into(), vec![]);
    let msg = OutboundMessage {
        channel: "telegram".into(),
        chat_id: "not-a-number".into(),
        content: "hello".into(),
        reply_to: None,
        media: vec![],
        metadata: HashMap::new(),
    };
    let result = ch.send(&msg).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        matches!(err, ChannelError::SendFailed(_)),
        "expected SendFailed, got: {err:?}"
    );
}

#[tokio::test]
async fn send_rejects_non_numeric_reply_to() {
    let ch = TelegramChannel::new("tok".into(), vec![]);
    let msg = OutboundMessage {
        channel: "telegram".into(),
        chat_id: "42".into(),
        content: "hello".into(),
        reply_to: Some("abc".into()),
        media: vec![],
        metadata: HashMap::new(),
    };
    let result = ch.send(&msg).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        matches!(err, ChannelError::SendFailed(_)),
        "expected SendFailed, got: {err:?}"
    );
}

// ── factory ──────────────────────────────────────────────────────────────

#[test]
fn factory_channel_name() {
    let factory = TelegramChannelFactory;
    assert_eq!(factory.channel_name(), "telegram");
}

#[test]
fn factory_build_success() {
    let factory = TelegramChannelFactory;
    let config = serde_json::json!({
        "token": "123:ABC",
        "allowed_users": ["100", "200"]
    });
    let channel = factory.build(&config);
    assert!(channel.is_ok());
    let ch = channel.unwrap();
    assert_eq!(ch.name(), "telegram");
    assert!(ch.is_allowed("100"));
    assert!(ch.is_allowed("200"));
    assert!(!ch.is_allowed("300"));
}

#[test]
fn factory_build_missing_token_errors() {
    let factory = TelegramChannelFactory;
    let config = serde_json::json!({
        "allowed_users": ["100"]
    });
    let result = factory.build(&config);
    match result {
        Err(ChannelError::Other(msg)) => {
            assert!(msg.contains("token"), "error should mention token: {msg}");
        }
        Err(other) => panic!("expected ChannelError::Other, got: {other:?}"),
        Ok(_) => panic!("expected error, got Ok"),
    }
}

#[test]
fn factory_build_empty_config_errors() {
    let factory = TelegramChannelFactory;
    let config = serde_json::json!({});
    let result = factory.build(&config);
    assert!(result.is_err());
}

#[test]
fn factory_build_token_not_string_errors() {
    let factory = TelegramChannelFactory;
    let config = serde_json::json!({"token": 12345});
    let result = factory.build(&config);
    assert!(result.is_err());
}

#[test]
fn factory_build_no_allowed_users_defaults_to_empty() {
    let factory = TelegramChannelFactory;
    let config = serde_json::json!({"token": "123:ABC"});
    let channel = factory.build(&config).unwrap();
    // Empty allowed_users means everyone is allowed
    assert!(channel.is_allowed("anyone"));
}

#[test]
fn factory_build_invalid_allowed_users_defaults_to_empty() {
    let factory = TelegramChannelFactory;
    let config = serde_json::json!({
        "token": "123:ABC",
        "allowed_users": "not-an-array"
    });
    let channel = factory.build(&config).unwrap();
    // Malformed allowed_users falls back to empty (everyone allowed)
    assert!(channel.is_allowed("anyone"));
}

// ── process_update ───────────────────────────────────────────────────────

#[tokio::test]
async fn process_update_delivers_text_message() {
    let ch = TelegramChannel::new("tok".into(), vec![]);
    let mock_host = Arc::new(MockHost::new());
    let host: Arc<dyn ChannelHost> = mock_host.clone();

    let update = types::Update {
        update_id: 1,
        message: Some(types::Message {
            message_id: 42,
            from: Some(types::User {
                id: 999,
                is_bot: false,
                first_name: "Alice".into(),
                username: Some("alice".into()),
            }),
            chat: types::Chat {
                id: 100,
                chat_type: "private".into(),
                title: None,
                username: Some("alice".into()),
            },
            text: Some("Hello bot".into()),
            date: 1700000000,
        }),
    };

    ch.process_update(&update, &host).await.unwrap();

    let msgs = mock_host.messages.lock().await;
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].channel, "telegram");
    assert_eq!(msgs[0].sender_id, "999");
    assert_eq!(msgs[0].chat_id, "100");
    assert_eq!(msgs[0].content, "Hello bot");
    assert_eq!(
        msgs[0].metadata.get("first_name"),
        Some(&serde_json::Value::String("Alice".into()))
    );
    assert_eq!(
        msgs[0].metadata.get("username"),
        Some(&serde_json::Value::String("alice".into()))
    );
    assert_eq!(
        msgs[0].metadata.get("chat_type"),
        Some(&serde_json::Value::String("private".into()))
    );
}

#[tokio::test]
async fn process_update_skips_non_message_update() {
    let ch = TelegramChannel::new("tok".into(), vec![]);
    let mock_host = Arc::new(MockHost::new());
    let host: Arc<dyn ChannelHost> = mock_host.clone();

    let update = types::Update {
        update_id: 2,
        message: None,
    };

    ch.process_update(&update, &host).await.unwrap();
    assert!(mock_host.messages.lock().await.is_empty());
}

#[tokio::test]
async fn process_update_skips_message_without_text() {
    let ch = TelegramChannel::new("tok".into(), vec![]);
    let mock_host = Arc::new(MockHost::new());
    let host: Arc<dyn ChannelHost> = mock_host.clone();

    let update = types::Update {
        update_id: 3,
        message: Some(types::Message {
            message_id: 50,
            from: None,
            chat: types::Chat {
                id: 1,
                chat_type: "private".into(),
                title: None,
                username: None,
            },
            text: None,
            date: 1700000001,
        }),
    };

    ch.process_update(&update, &host).await.unwrap();
    assert!(mock_host.messages.lock().await.is_empty());
}

#[tokio::test]
async fn process_update_rejects_disallowed_user() {
    let ch = TelegramChannel::new("tok".into(), vec!["100".into()]);
    let mock_host = Arc::new(MockHost::new());
    let host: Arc<dyn ChannelHost> = mock_host.clone();

    let update = types::Update {
        update_id: 4,
        message: Some(types::Message {
            message_id: 60,
            from: Some(types::User {
                id: 999, // not in allowed list
                is_bot: false,
                first_name: "Mallory".into(),
                username: None,
            }),
            chat: types::Chat {
                id: 1,
                chat_type: "private".into(),
                title: None,
                username: None,
            },
            text: Some("sneaky".into()),
            date: 1700000002,
        }),
    };

    ch.process_update(&update, &host).await.unwrap();
    assert!(mock_host.messages.lock().await.is_empty());
}

#[tokio::test]
async fn process_update_message_without_from() {
    let ch = TelegramChannel::new("tok".into(), vec![]);
    let mock_host = Arc::new(MockHost::new());
    let host: Arc<dyn ChannelHost> = mock_host.clone();

    let update = types::Update {
        update_id: 5,
        message: Some(types::Message {
            message_id: 70,
            from: None, // channel post
            chat: types::Chat {
                id: -100,
                chat_type: "channel".into(),
                title: Some("News".into()),
                username: None,
            },
            text: Some("announcement".into()),
            date: 1700000003,
        }),
    };

    ch.process_update(&update, &host).await.unwrap();
    let msgs = mock_host.messages.lock().await;
    assert_eq!(msgs.len(), 1);
    // sender_id should be empty string when from is None
    assert_eq!(msgs[0].sender_id, "");
    assert_eq!(msgs[0].content, "announcement");
}

// ── allow_from_match metadata (WEFT-162) ────────────────────────────────

/// Sender matched in a non-empty allow-list → metadata has
/// `allow_from_match: true`.
#[tokio::test]
async fn process_update_emits_allow_from_match_for_listed_user() {
    let ch = TelegramChannel::new("tok".into(), vec!["100".into(), "200".into()]);
    let mock_host = Arc::new(MockHost::new());
    let host: Arc<dyn ChannelHost> = mock_host.clone();

    let update = types::Update {
        update_id: 10,
        message: Some(types::Message {
            message_id: 80,
            from: Some(types::User {
                id: 100,
                is_bot: false,
                first_name: "Alice".into(),
                username: Some("alice".into()),
            }),
            chat: types::Chat {
                id: 100,
                chat_type: "private".into(),
                title: None,
                username: Some("alice".into()),
            },
            text: Some("hi".into()),
            date: 1700000010,
        }),
    };

    ch.process_update(&update, &host).await.unwrap();

    let msgs = mock_host.messages.lock().await;
    assert_eq!(msgs.len(), 1);
    assert_eq!(
        msgs[0].metadata.get("allow_from_match"),
        Some(&serde_json::Value::Bool(true)),
        "matched allow-list user must emit allow_from_match=true"
    );
}

/// Empty allow-list → message is delivered (everyone allowed) but no
/// `allow_from_match` metadata is attached.
#[tokio::test]
async fn process_update_no_allow_from_match_when_list_empty() {
    let ch = TelegramChannel::new("tok".into(), vec![]);
    let mock_host = Arc::new(MockHost::new());
    let host: Arc<dyn ChannelHost> = mock_host.clone();

    let update = types::Update {
        update_id: 11,
        message: Some(types::Message {
            message_id: 81,
            from: Some(types::User {
                id: 999,
                is_bot: false,
                first_name: "Anyone".into(),
                username: None,
            }),
            chat: types::Chat {
                id: 999,
                chat_type: "private".into(),
                title: None,
                username: None,
            },
            text: Some("hello".into()),
            date: 1700000011,
        }),
    };

    ch.process_update(&update, &host).await.unwrap();

    let msgs = mock_host.messages.lock().await;
    assert_eq!(msgs.len(), 1);
    assert!(
        !msgs[0].metadata.contains_key("allow_from_match"),
        "empty allow-list must not emit allow_from_match"
    );
}
