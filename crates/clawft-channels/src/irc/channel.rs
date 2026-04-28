//! IRC channel adapter implementation.
//!
//! # WARNING: Planning stub -- does NOT transmit messages.
//!
//! This adapter is a compile-time placeholder. It is **not**
//! production-ready (existing source already mentions "skeleton" in
//! `IrcChannelAdapter`'s doc comment; this header makes the operator
//! impact explicit):
//!
//! - `start()` never opens a TCP/TLS socket, never sends NICK/USER/CAP,
//!   never JOINs the configured channels, and never reads PRIVMSG. See
//!   the `TODO` at line ~82.
//! - `send()` does not issue PRIVMSG. It fabricates a synthetic
//!   `irc-{target}-{ts}` id and returns it. See the `TODO` at line ~128.
//!   Outbound messages are silently dropped.
//!
//! The real RFC-2812 runtime (pending `irc` crate selection) is tracked
//! as Task 1 in `.planning/reviews/0.7.0-release-gate/05-channels.md`.
//! Do **not** enable the `irc` feature in production until that task
//! ships.
//!
//! Implements [`ChannelAdapter`] for IRC messaging.
//! This is a properly-typed skeleton -- the actual IRC protocol client
//! integration will be added when the `irc` crate is brought in as a
//! dependency.

use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use clawft_plugin::error::PluginError;
use clawft_plugin::message::MessagePayload;
use clawft_plugin::traits::{ChannelAdapter, ChannelAdapterHost};

use super::types::{validate_config, IrcAdapterConfig};

/// IRC channel adapter.
///
/// Connects to an IRC server and bridges messages between IRC channels
/// and the clawft agent pipeline.
///
/// **Status**: Skeleton implementation. The IRC protocol client will be
/// integrated when the `irc` crate is added as a dependency.
pub struct IrcChannelAdapter {
    config: IrcAdapterConfig,
}

impl IrcChannelAdapter {
    /// Create a new IRC channel adapter with the given configuration.
    pub fn new(config: IrcAdapterConfig) -> Self {
        Self { config }
    }

    /// Check if a sender nickname is in the allow list.
    ///
    /// If `allowed_senders` is empty, all senders are allowed.
    /// Otherwise, the sender must match one of the entries exactly.
    pub fn is_sender_allowed(&self, sender: &str) -> bool {
        if self.config.allowed_senders.is_empty() {
            return true;
        }
        self.config.allowed_senders.iter().any(|s| s == sender)
    }

    /// Validate the adapter configuration, returning a `PluginError`
    /// on failure.
    fn validate(&self) -> Result<(), PluginError> {
        validate_config(&self.config).map_err(PluginError::LoadFailed)
    }
}

#[async_trait]
impl ChannelAdapter for IrcChannelAdapter {
    fn name(&self) -> &str {
        "irc"
    }

    fn display_name(&self) -> &str {
        "IRC"
    }

    fn supports_threads(&self) -> bool {
        false
    }

    fn supports_media(&self) -> bool {
        false
    }

    async fn start(
        &self,
        _host: Arc<dyn ChannelAdapterHost>,
        cancel: CancellationToken,
    ) -> Result<(), PluginError> {
        warn!(
            "irc channel adapter is a planning stub: TCP/TLS dial, \
             NICK/USER/CAP, JOIN, and PRIVMSG are not implemented; \
             outbound messages will be silently dropped. See \
             .planning/reviews/0.7.0-release-gate/05-channels.md task 1."
        );
        info!("IRC channel adapter starting");

        self.validate()?;

        // TODO: Connect to the IRC server using an IRC client library.
        //
        // In production, this would:
        // 1. Establish a TCP (or TLS) connection to `self.config.server:self.config.port`
        // 2. Authenticate using the configured `auth_method`
        // 3. Join all channels listed in `self.config.channels`
        // 4. Listen for PRIVMSG events and forward them to host.deliver_inbound()
        // 5. Handle reconnection with `self.config.reconnect_delay_secs`
        //
        // The actual IRC protocol client will be integrated when the `irc`
        // crate is added as a dependency.
        debug!(
            server = %self.config.server,
            port = self.config.port,
            use_tls = self.config.use_tls,
            nickname = %self.config.nickname,
            channels = ?self.config.channels,
            auth_method = %self.config.auth_method,
            "IRC connection would be established here (stub)"
        );

        cancel.cancelled().await;
        info!("IRC channel adapter shutting down");
        Ok(())
    }

    async fn send(
        &self,
        target: &str,
        payload: &MessagePayload,
    ) -> Result<String, PluginError> {
        // IRC only supports text payloads.
        let content = match payload {
            MessagePayload::Text { content } => content,
            MessagePayload::Binary { .. } => {
                return Err(PluginError::ExecutionFailed(
                    "irc: binary payloads are not supported (IRC is text-only)".into(),
                ));
            }
            MessagePayload::Structured { .. } => {
                return Err(PluginError::ExecutionFailed(
                    "irc: structured payloads are not supported (IRC is text-only)".into(),
                ));
            }
        };

        // TODO: Send the message using the connected IRC client.
        //
        // In production, this would issue a PRIVMSG command:
        //   PRIVMSG <target> :<content>
        //
        // For now, we return an error indicating the client is not
        // connected since no real IRC client is instantiated.
        debug!(
            to = %target,
            content_len = content.len(),
            "would send IRC PRIVMSG (stub -- client not connected)"
        );

        // Stub: return a synthetic message ID.
        // In a real implementation, this would only succeed when the
        // client is connected and the PRIVMSG has been sent.
        let msg_id = format!(
            "irc-{}-{}",
            target,
            chrono::Utc::now().timestamp_millis()
        );
        Ok(msg_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_config() -> IrcAdapterConfig {
        IrcAdapterConfig {
            server: "irc.libera.chat".into(),
            nickname: "clawft-bot".into(),
            channels: vec!["#general".into()],
            ..Default::default()
        }
    }

    // E5-T6: Factory/constructor builds channel from valid config.
    #[test]
    fn construct_from_valid_config() {
        let adapter = IrcChannelAdapter::new(make_config());
        assert_eq!(adapter.name(), "irc");
        assert_eq!(adapter.display_name(), "IRC");
    }

    #[test]
    fn name_is_irc() {
        let adapter = IrcChannelAdapter::new(make_config());
        assert_eq!(adapter.name(), "irc");
    }

    #[test]
    fn display_name_is_irc() {
        let adapter = IrcChannelAdapter::new(make_config());
        assert_eq!(adapter.display_name(), "IRC");
    }

    #[test]
    fn no_threads_or_media() {
        let adapter = IrcChannelAdapter::new(make_config());
        assert!(!adapter.supports_threads());
        assert!(!adapter.supports_media());
    }

    // E5-T8: is_sender_allowed respects allowed_senders (empty = allow all).
    #[test]
    fn sender_allowed_empty_list_allows_all() {
        let adapter = IrcChannelAdapter::new(make_config());
        assert!(adapter.is_sender_allowed("anyone"));
        assert!(adapter.is_sender_allowed("stranger"));
    }

    #[test]
    fn sender_allowed_with_filter() {
        let mut config = make_config();
        config.allowed_senders = vec!["admin".into(), "moderator".into()];
        let adapter = IrcChannelAdapter::new(config);

        assert!(adapter.is_sender_allowed("admin"));
        assert!(adapter.is_sender_allowed("moderator"));
        assert!(!adapter.is_sender_allowed("random-user"));
        assert!(!adapter.is_sender_allowed(""));
    }

    #[test]
    fn validate_config_success() {
        let adapter = IrcChannelAdapter::new(make_config());
        assert!(adapter.validate().is_ok());
    }

    // E5-T7: Validate rejects nickserv without password_env.
    #[test]
    fn validate_rejects_nickserv_without_password() {
        let config = IrcAdapterConfig {
            server: "irc.example.com".into(),
            nickname: "bot".into(),
            auth_method: "nickserv".into(),
            password_env: None,
            ..Default::default()
        };
        let adapter = IrcChannelAdapter::new(config);
        let err = adapter.validate().unwrap_err();
        assert!(err.to_string().contains("password_env"));
    }

    #[test]
    fn validate_rejects_empty_server() {
        let config = IrcAdapterConfig {
            nickname: "bot".into(),
            ..Default::default()
        };
        let adapter = IrcChannelAdapter::new(config);
        let err = adapter.validate().unwrap_err();
        assert!(err.to_string().contains("server"));
    }

    #[test]
    fn validate_rejects_empty_nickname() {
        let config = IrcAdapterConfig {
            server: "irc.example.com".into(),
            ..Default::default()
        };
        let adapter = IrcChannelAdapter::new(config);
        let err = adapter.validate().unwrap_err();
        assert!(err.to_string().contains("nickname"));
    }

    #[test]
    fn validate_rejects_invalid_auth_method() {
        let config = IrcAdapterConfig {
            server: "irc.example.com".into(),
            nickname: "bot".into(),
            auth_method: "invalid".into(),
            ..Default::default()
        };
        let adapter = IrcChannelAdapter::new(config);
        let err = adapter.validate().unwrap_err();
        assert!(err.to_string().contains("auth_method"));
    }

    // E5-T10: send returns error for non-text (Binary) payload.
    #[tokio::test]
    async fn send_binary_payload_fails() {
        let adapter = IrcChannelAdapter::new(make_config());
        let payload = MessagePayload::binary("audio/wav", vec![0u8; 16]);
        let result = adapter.send("#general", &payload).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("binary"));
    }

    #[tokio::test]
    async fn send_structured_payload_fails() {
        let adapter = IrcChannelAdapter::new(make_config());
        let payload = MessagePayload::structured(serde_json::json!({"key": "value"}));
        let result = adapter.send("#general", &payload).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("structured"));
    }

    // E5-T9: send returns a synthetic message ID for text payloads
    // (stub always succeeds since we generate synthetic IDs).
    #[tokio::test]
    async fn send_text_message_returns_id() {
        let adapter = IrcChannelAdapter::new(make_config());
        let payload = MessagePayload::text("Hello from bot");
        let result = adapter.send("#general", &payload).await;
        assert!(result.is_ok());
        let msg_id = result.unwrap();
        assert!(msg_id.starts_with("irc-#general-"));
    }

    #[tokio::test]
    async fn start_validates_config() {
        let config = IrcAdapterConfig {
            // Missing server and nickname -- should fail validation.
            ..Default::default()
        };
        let adapter = IrcChannelAdapter::new(config);

        let host = Arc::new(MockHost);
        let cancel = CancellationToken::new();
        let result = adapter.start(host, cancel).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn start_shuts_down_on_cancel() {
        let adapter = IrcChannelAdapter::new(make_config());
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        let host = Arc::new(MockHost);
        let handle = tokio::spawn(async move {
            adapter.start(host, cancel_clone).await
        });

        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        cancel.cancel();
        let result = handle.await.unwrap();
        assert!(result.is_ok());
    }

    struct MockHost;

    #[async_trait]
    impl ChannelAdapterHost for MockHost {
        async fn deliver_inbound(
            &self,
            _channel: &str,
            _sender_id: &str,
            _chat_id: &str,
            _payload: MessagePayload,
            _metadata: HashMap<String, serde_json::Value>,
        ) -> Result<(), PluginError> {
            Ok(())
        }
    }
}
