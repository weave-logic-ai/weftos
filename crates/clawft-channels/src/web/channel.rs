//! Web channel implementation.

use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use tracing::debug;

use crate::traits::{
    Channel, ChannelFactory, ChannelHost, ChannelMetadata, ChannelStatus, MessageId,
};
use clawft_types::error::ChannelError;
use clawft_types::event::OutboundMessage;

/// Callback trait for publishing outbound messages to browser clients.
///
/// The gateway wires this to the `TopicBroadcaster` at construction time.
#[async_trait]
pub trait WebPublisher: Send + Sync {
    /// Publish a JSON message to a named topic.
    async fn publish(&self, topic: &str, message: serde_json::Value);
}

/// A channel that delivers outbound messages to browser clients via
/// a [`WebPublisher`] (backed by the WebSocket/SSE broadcaster).
///
/// Inbound messages arrive via the REST API's `/api/sessions/{key}/messages`
/// endpoint, which publishes directly to the message bus. The web channel's
/// `start()` method is therefore a no-op that waits for cancellation.
///
/// # Authentication gate (WEFT-163)
///
/// In 0.6.x the channel's [`is_allowed`](Channel::is_allowed) returned
/// `true` unconditionally on the assumption that "auth is handled by the
/// API middleware". That was wrong when the gateway was running without
/// an auth middleware mounted: every anonymous inbound was accepted.
///
/// Now the channel takes an `auth_enabled` flag that mirrors whether
/// the gateway's auth middleware is wired (D-7 / M2-A). When the
/// middleware is enabled the channel trusts that the inbound was
/// validated upstream and returns `true`; when it is disabled the
/// channel denies all inbound, refusing to fall back to legacy
/// permissive behavior.
pub struct WebChannel {
    publisher: Arc<dyn WebPublisher>,
    /// Whether the gateway's auth middleware is mounted.
    auth_enabled: bool,
}

impl WebChannel {
    /// Create a new web channel with the given publisher.
    ///
    /// `auth_enabled` must reflect whether the gateway's auth
    /// middleware will gate inbound REST/WebSocket requests. When
    /// `false`, [`is_allowed`](Channel::is_allowed) denies every
    /// sender (the channel refuses to be permissive when the gateway
    /// is itself permissive). Callers that still need the legacy
    /// always-allow behavior must pass `true`, which signals that
    /// the gateway has authenticated the inbound upstream.
    pub fn new(publisher: Arc<dyn WebPublisher>, auth_enabled: bool) -> Self {
        Self {
            publisher,
            auth_enabled,
        }
    }
}

#[async_trait]
impl Channel for WebChannel {
    fn name(&self) -> &str {
        "web"
    }

    fn metadata(&self) -> ChannelMetadata {
        ChannelMetadata {
            name: "web".into(),
            display_name: "Web Dashboard".into(),
            supports_threads: true,
            supports_media: false,
        }
    }

    fn status(&self) -> ChannelStatus {
        // The web channel is always "running" when registered.
        ChannelStatus::Running
    }

    fn is_allowed(&self, _sender_id: &str) -> bool {
        // WEFT-163: defer to the gateway's auth middleware. When the
        // middleware is mounted (`auth_enabled == true`), the inbound
        // has already been gated by a Bearer token; we trust the
        // upstream check and return `true`. When the middleware is
        // disabled, deny all inbound — the channel will not paper
        // over a permissive gateway.
        self.auth_enabled
    }

    async fn start(
        &self,
        _host: Arc<dyn ChannelHost>,
        cancel: CancellationToken,
    ) -> Result<(), ChannelError> {
        // No polling loop needed — inbound arrives via REST API.
        // Just wait for shutdown.
        cancel.cancelled().await;
        Ok(())
    }

    async fn send(&self, msg: &OutboundMessage) -> Result<MessageId, ChannelError> {
        let topic = format!("sessions:{}", msg.chat_id);
        let payload = serde_json::json!({
            "type": "message",
            "role": "assistant",
            "content": &msg.content,
            "session_key": &msg.chat_id,
            "channel": "web",
            "timestamp": chrono::Utc::now().to_rfc3339(),
        });

        debug!(
            topic = %topic,
            chat_id = %msg.chat_id,
            "web channel publishing to broadcaster"
        );

        self.publisher.publish(&topic, payload).await;

        // Also publish to the general sessions topic.
        self.publisher
            .publish(
                "sessions",
                serde_json::json!({
                    "type": "message_added",
                    "session_key": &msg.chat_id,
                }),
            )
            .await;

        let msg_id = format!("web-{}", uuid::Uuid::new_v4());
        Ok(MessageId(msg_id))
    }
}

/// Factory that creates [`WebChannel`] instances.
///
/// Since the web channel requires a pre-built `WebPublisher` (not something
/// derivable from JSON config), the factory holds the publisher and passes
/// it into each built channel.
///
/// `auth_enabled` reflects whether the gateway's auth middleware is
/// mounted; it is propagated into every built channel and ultimately
/// gates [`Channel::is_allowed`] (WEFT-163).
pub struct WebChannelFactory {
    publisher: Arc<dyn WebPublisher>,
    auth_enabled: bool,
}

impl WebChannelFactory {
    /// Create a new factory with the given publisher.
    ///
    /// `auth_enabled` should be `true` when the gateway's auth
    /// middleware will gate inbound REST/WebSocket requests, and
    /// `false` otherwise. When `false`, every built channel will
    /// reject all inbound until the gateway middleware is enabled.
    pub fn new(publisher: Arc<dyn WebPublisher>, auth_enabled: bool) -> Self {
        Self {
            publisher,
            auth_enabled,
        }
    }
}

impl ChannelFactory for WebChannelFactory {
    fn channel_name(&self) -> &str {
        "web"
    }

    fn build(&self, _config: &serde_json::Value) -> Result<Arc<dyn Channel>, ChannelError> {
        Ok(Arc::new(WebChannel::new(
            self.publisher.clone(),
            self.auth_enabled,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct MockPublisher {
        messages: Mutex<Vec<(String, serde_json::Value)>>,
    }

    impl MockPublisher {
        fn new() -> Self {
            Self {
                messages: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl WebPublisher for MockPublisher {
        async fn publish(&self, topic: &str, message: serde_json::Value) {
            self.messages
                .lock()
                .unwrap()
                .push((topic.to_string(), message));
        }
    }

    #[test]
    fn web_channel_name() {
        let pub_ = Arc::new(MockPublisher::new());
        let ch = WebChannel::new(pub_, true);
        assert_eq!(ch.name(), "web");
    }

    #[test]
    fn web_channel_metadata() {
        let pub_ = Arc::new(MockPublisher::new());
        let ch = WebChannel::new(pub_, true);
        let meta = ch.metadata();
        assert_eq!(meta.name, "web");
        assert_eq!(meta.display_name, "Web Dashboard");
        assert!(meta.supports_threads);
    }

    #[test]
    fn web_channel_always_running() {
        let pub_ = Arc::new(MockPublisher::new());
        let ch = WebChannel::new(pub_, true);
        assert_eq!(ch.status(), ChannelStatus::Running);
    }

    /// WEFT-163: when the gateway's auth middleware is enabled
    /// (`auth_enabled = true`), `is_allowed` trusts the upstream
    /// authentication and admits the inbound.
    #[test]
    fn web_channel_is_allowed_when_auth_enabled() {
        let pub_ = Arc::new(MockPublisher::new());
        let ch = WebChannel::new(pub_, true);
        assert!(ch.is_allowed("authenticated-user"));
        assert!(ch.is_allowed(""));
    }

    /// WEFT-163: when the gateway's auth middleware is NOT enabled,
    /// `is_allowed` denies every sender — the channel refuses to be
    /// permissive when the gateway is permissive.
    #[test]
    fn web_channel_denies_when_auth_disabled() {
        let pub_ = Arc::new(MockPublisher::new());
        let ch = WebChannel::new(pub_, false);
        assert!(!ch.is_allowed("anyone"));
        assert!(!ch.is_allowed(""));
    }

    #[tokio::test]
    async fn web_channel_send_publishes() {
        let pub_ = Arc::new(MockPublisher::new());
        let ch = WebChannel::new(pub_.clone(), true);

        let msg = OutboundMessage {
            channel: "web".into(),
            chat_id: "voice".into(),
            content: "Hello!".into(),
            reply_to: None,
            media: vec![],
            metadata: HashMap::new(),
        };

        let result = ch.send(&msg).await;
        assert!(result.is_ok());

        let messages = pub_.messages.lock().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].0, "sessions:voice");
        assert_eq!(messages[0].1["role"], "assistant");
        assert_eq!(messages[0].1["content"], "Hello!");
        assert_eq!(messages[1].0, "sessions");
        assert_eq!(messages[1].1["type"], "message_added");
    }

    #[test]
    fn factory_builds_web_channel() {
        let pub_ = Arc::new(MockPublisher::new());
        let factory = WebChannelFactory::new(pub_, true);
        assert_eq!(factory.channel_name(), "web");

        let ch = factory.build(&serde_json::json!({})).unwrap();
        assert_eq!(ch.name(), "web");
    }

    /// WEFT-163: the factory propagates `auth_enabled = false` into
    /// the built channel so every built `WebChannel` denies inbound
    /// when the gateway is unauthenticated.
    #[test]
    fn factory_propagates_auth_disabled() {
        let pub_ = Arc::new(MockPublisher::new());
        let factory = WebChannelFactory::new(pub_, false);
        let ch = factory.build(&serde_json::json!({})).unwrap();
        assert!(
            !ch.is_allowed("anyone"),
            "factory built with auth_enabled=false must produce a denying channel"
        );
    }
}
