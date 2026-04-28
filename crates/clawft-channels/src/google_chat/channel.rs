//! Google Chat channel adapter implementation (skeleton).
//!
//! # WARNING: Planning stub -- does NOT transmit messages.
//!
//! This adapter is a compile-time placeholder. It is **not**
//! production-ready:
//!
//! - `start()` never subscribes to Pub/Sub or the Chat API. It logs a
//!   `debug!` line and waits for cancellation.
//! - `send()` does not POST to `chat.spaces.messages.create`. It
//!   fabricates a synthetic `spaces/{target}/messages/gchat-{ts}` id
//!   and returns it. Outbound messages are silently dropped.
//!
//! The real Workspace API runtime (service-account credentials, OAuth2
//! via Workstream F6, Pub/Sub subscription) is tracked as Task 6 in
//! `.planning/reviews/0.7.0-release-gate/05-channels.md`. Do **not**
//! enable the `google-chat` feature in production until that task ships.
//!
//! Implements [`ChannelAdapter`] for Google Chat via the Chat API.
//! Full OAuth2 integration is deferred to Workstream F6; this
//! skeleton provides the adapter structure and configuration.

use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use clawft_plugin::error::PluginError;
use clawft_plugin::message::MessagePayload;
use clawft_plugin::traits::{ChannelAdapter, ChannelAdapterHost};

use super::types::GoogleChatAdapterConfig;

/// Google Chat channel adapter (skeleton).
///
/// The adapter structure and trait implementation are in place.
/// Actual API calls require OAuth2 service account credentials
/// which are planned for Workstream F6.
pub struct GoogleChatChannelAdapter {
    config: GoogleChatAdapterConfig,
}

impl GoogleChatChannelAdapter {
    /// Create a new Google Chat channel adapter.
    pub fn new(config: GoogleChatAdapterConfig) -> Self {
        Self { config }
    }

    /// Check if a user email is in the allow list.
    pub fn is_user_allowed(&self, user_email: &str) -> bool {
        if self.config.allowed_users.is_empty() {
            return true;
        }
        let email_lower = user_email.to_lowercase();
        self.config
            .allowed_users
            .iter()
            .any(|u| u.to_lowercase() == email_lower)
    }
}

#[async_trait]
impl ChannelAdapter for GoogleChatChannelAdapter {
    fn name(&self) -> &str {
        "google_chat"
    }

    fn display_name(&self) -> &str {
        "Google Chat"
    }

    fn supports_threads(&self) -> bool {
        true // Google Chat supports threaded messages
    }

    fn supports_media(&self) -> bool {
        false // Media support is a future enhancement
    }

    async fn start(
        &self,
        _host: Arc<dyn ChannelAdapterHost>,
        cancel: CancellationToken,
    ) -> Result<(), PluginError> {
        warn!(
            "google_chat channel adapter is a planning stub: Pub/Sub \
             event subscription and `chat.spaces.messages.create` POST \
             are not implemented; outbound messages will be silently \
             dropped. See \
             .planning/reviews/0.7.0-release-gate/05-channels.md task 6."
        );
        info!("Google Chat channel adapter starting");

        if self.config.project_id.is_empty() {
            return Err(PluginError::LoadFailed(
                "google_chat adapter: project_id is required".into(),
            ));
        }

        // OAuth2 integration deferred to Workstream F6.
        // In production, this would:
        // 1. Load service account credentials
        // 2. Exchange for an OAuth2 access token
        // 3. Subscribe to space events via the Chat API
        // 4. Process incoming messages
        debug!(
            project_id = %self.config.project_id,
            spaces = self.config.spaces.len(),
            "google chat event loop would start here (stub, blocked on F6 OAuth2)"
        );

        cancel.cancelled().await;
        info!("Google Chat channel adapter shutting down");
        Ok(())
    }

    async fn send(
        &self,
        target: &str,
        payload: &MessagePayload,
    ) -> Result<String, PluginError> {
        let content = payload.as_text().ok_or_else(|| {
            PluginError::ExecutionFailed(
                "google_chat: only text payloads supported".into(),
            )
        })?;

        if self.config.project_id.is_empty() {
            return Err(PluginError::ExecutionFailed(
                "google_chat: project_id not configured".into(),
            ));
        }

        // In production, this would POST to the Chat API:
        // POST /v1/{target}/messages
        debug!(
            space = %target,
            content_len = content.len(),
            "sending Google Chat message (stub, blocked on F6 OAuth2)"
        );

        let msg_id = format!(
            "spaces/{}/messages/gchat-{}",
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

    fn make_config() -> GoogleChatAdapterConfig {
        GoogleChatAdapterConfig {
            project_id: "test-project-123".into(),
            ..Default::default()
        }
    }

    #[test]
    fn name_is_google_chat() {
        let adapter = GoogleChatChannelAdapter::new(make_config());
        assert_eq!(adapter.name(), "google_chat");
    }

    #[test]
    fn display_name() {
        let adapter = GoogleChatChannelAdapter::new(make_config());
        assert_eq!(adapter.display_name(), "Google Chat");
    }

    #[test]
    fn supports_threads_yes() {
        let adapter = GoogleChatChannelAdapter::new(make_config());
        assert!(adapter.supports_threads());
    }

    #[test]
    fn supports_media_no() {
        let adapter = GoogleChatChannelAdapter::new(make_config());
        assert!(!adapter.supports_media());
    }

    #[test]
    fn user_filtering() {
        let mut config = make_config();
        config.allowed_users = vec!["admin@company.com".into()];
        let adapter = GoogleChatChannelAdapter::new(config);

        assert!(adapter.is_user_allowed("admin@company.com"));
        assert!(adapter.is_user_allowed("ADMIN@COMPANY.COM"));
        assert!(!adapter.is_user_allowed("stranger@evil.com"));
    }

    #[test]
    fn empty_allow_list_allows_all() {
        let adapter = GoogleChatChannelAdapter::new(make_config());
        assert!(adapter.is_user_allowed("anyone@anywhere.com"));
    }

    #[tokio::test]
    async fn send_text_message() {
        let adapter = GoogleChatChannelAdapter::new(make_config());
        let payload = MessagePayload::text("Hello");
        let result = adapter.send("AAAA", &payload).await;
        assert!(result.is_ok());
        assert!(result.unwrap().contains("gchat-"));
    }

    #[tokio::test]
    async fn send_non_text_fails() {
        let adapter = GoogleChatChannelAdapter::new(make_config());
        let payload =
            MessagePayload::structured(serde_json::json!({}));
        let result = adapter.send("AAAA", &payload).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn start_validates_project_id() {
        let mut config = make_config();
        config.project_id = String::new();
        let adapter = GoogleChatChannelAdapter::new(config);

        let host = Arc::new(MockHost);
        let cancel = CancellationToken::new();
        let result = adapter.start(host, cancel).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("project_id"));
    }

    #[tokio::test]
    async fn start_shuts_down_on_cancel() {
        let adapter = GoogleChatChannelAdapter::new(make_config());
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
