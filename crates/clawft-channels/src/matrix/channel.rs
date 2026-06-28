//! Matrix channel adapter implementation.
//!
//! # WARNING: Planning stub -- does NOT transmit messages.
//!
//! This adapter is a compile-time placeholder. It is **not**
//! production-ready:
//!
//! - `start()` never long-polls `/sync`, never auto-joins rooms, and
//!   never parses `m.room.message` events. It waits for cancellation.
//! - `send()` does not `PUT` to
//!   `/_matrix/client/v3/rooms/{room}/send/m.room.message/{txn}`. It
//!   fabricates a synthetic `${ts}` event id and returns it. Outbound
//!   messages are silently dropped.
//!
//! The real client-server runtime is tracked as Task 5 in
//! `.planning/reviews/0.7.0-release-gate/05-channels.md`. Do **not**
//! enable the `matrix` feature in production until that task ships.
//!
//! Implements [`ChannelAdapter`] for Matrix messaging via the
//! client-server API. Supports threaded rooms and text payloads.

use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use clawft_plugin::error::PluginError;
use clawft_plugin::message::MessagePayload;
use clawft_plugin::traits::{ChannelAdapter, ChannelAdapterHost};

use super::types::MatrixAdapterConfig;

/// Matrix channel adapter using the client-server API.
///
/// Connects to a Matrix homeserver for sending and receiving messages.
/// Supports threaded conversations via Matrix room threads.
pub struct MatrixChannelAdapter {
    config: MatrixAdapterConfig,
}

impl MatrixChannelAdapter {
    /// Create a new Matrix channel adapter.
    pub fn new(config: MatrixAdapterConfig) -> Self {
        Self { config }
    }

    /// Check if a user ID is in the allow list.
    pub fn is_user_allowed(&self, user_id: &str) -> bool {
        if self.config.allowed_users.is_empty() {
            return true;
        }
        self.config.allowed_users.iter().any(|u| u == user_id)
    }
}

#[async_trait]
impl ChannelAdapter for MatrixChannelAdapter {
    fn name(&self) -> &str {
        "matrix"
    }

    fn display_name(&self) -> &str {
        "Matrix"
    }

    fn supports_threads(&self) -> bool {
        true // Matrix supports threaded conversations
    }

    fn supports_media(&self) -> bool {
        true // Matrix supports media via content repository
    }

    async fn start(
        &self,
        _host: Arc<dyn ChannelAdapterHost>,
        cancel: CancellationToken,
    ) -> Result<(), PluginError> {
        warn!(
            "matrix channel adapter is a planning stub: `/sync` long-poll \
             and `PUT /rooms/.../send/m.room.message/{{txn}}` are not \
             implemented; outbound messages will be silently dropped. \
             See .planning/reviews/0.7.0-release-gate/05-channels.md \
             task 5."
        );
        info!("Matrix channel adapter starting");

        if self.config.homeserver_url.is_empty() {
            return Err(PluginError::LoadFailed(
                "matrix adapter: homeserver_url is required".into(),
            ));
        }
        if self.config.access_token.is_empty() {
            return Err(PluginError::LoadFailed(
                "matrix adapter: access_token is required".into(),
            ));
        }
        if self.config.user_id.is_empty() {
            return Err(PluginError::LoadFailed(
                "matrix adapter: user_id is required".into(),
            ));
        }

        // In production, this would:
        // 1. Perform an initial /sync to get the since token
        // 2. Auto-join rooms listed in config
        // 3. Long-poll /sync for new events
        // 4. Parse m.room.message events and deliver via host
        debug!(
            homeserver = %self.config.homeserver_url,
            user = %self.config.user_id,
            rooms = self.config.auto_join_rooms.len(),
            "matrix sync loop would start here (stub)"
        );

        cancel.cancelled().await;
        info!("Matrix channel adapter shutting down");
        Ok(())
    }

    async fn send(&self, target: &str, payload: &MessagePayload) -> Result<String, PluginError> {
        let content = payload.as_text().ok_or_else(|| {
            PluginError::ExecutionFailed("matrix: only text payloads supported currently".into())
        })?;

        if self.config.homeserver_url.is_empty() {
            return Err(PluginError::ExecutionFailed(
                "matrix: homeserver_url not configured".into(),
            ));
        }

        // In production, this would PUT to:
        // /_matrix/client/v3/rooms/{target}/send/m.room.message/{txnId}
        debug!(
            room = %target,
            content_len = content.len(),
            "sending Matrix message (stub)"
        );

        let event_id = format!("${}", chrono::Utc::now().timestamp_millis());
        Ok(event_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_config() -> MatrixAdapterConfig {
        MatrixAdapterConfig {
            homeserver_url: "https://matrix.test.org".into(),
            access_token: "syt_test_token".into(),
            user_id: "@bot:matrix.test.org".into(),
            ..Default::default()
        }
    }

    #[test]
    fn name_is_matrix() {
        let adapter = MatrixChannelAdapter::new(make_config());
        assert_eq!(adapter.name(), "matrix");
    }

    #[test]
    fn display_name() {
        let adapter = MatrixChannelAdapter::new(make_config());
        assert_eq!(adapter.display_name(), "Matrix");
    }

    #[test]
    fn supports_threads_and_media() {
        let adapter = MatrixChannelAdapter::new(make_config());
        assert!(adapter.supports_threads());
        assert!(adapter.supports_media());
    }

    #[test]
    fn user_filtering() {
        let mut config = make_config();
        config.allowed_users = vec!["@admin:matrix.org".into()];
        let adapter = MatrixChannelAdapter::new(config);

        assert!(adapter.is_user_allowed("@admin:matrix.org"));
        assert!(!adapter.is_user_allowed("@stranger:matrix.org"));
    }

    #[test]
    fn empty_allow_list_allows_all() {
        let adapter = MatrixChannelAdapter::new(make_config());
        assert!(adapter.is_user_allowed("@anyone:anywhere.org"));
    }

    #[tokio::test]
    async fn send_text_message() {
        let adapter = MatrixChannelAdapter::new(make_config());
        let payload = MessagePayload::text("Hello from bot");
        let result = adapter.send("!room1:matrix.test.org", &payload).await;
        assert!(result.is_ok());
        assert!(result.unwrap().starts_with('$'));
    }

    #[tokio::test]
    async fn send_non_text_fails() {
        let adapter = MatrixChannelAdapter::new(make_config());
        let payload = MessagePayload::structured(serde_json::json!({}));
        let result = adapter.send("!room1:matrix.test.org", &payload).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn start_validates_homeserver_url() {
        let mut config = make_config();
        config.homeserver_url = String::new();
        let adapter = MatrixChannelAdapter::new(config);

        let host = Arc::new(MockHost);
        let cancel = CancellationToken::new();
        let result = adapter.start(host, cancel).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("homeserver_url"));
    }

    #[tokio::test]
    async fn start_validates_access_token() {
        let mut config = make_config();
        config.access_token = "".into();
        let adapter = MatrixChannelAdapter::new(config);

        let host = Arc::new(MockHost);
        let cancel = CancellationToken::new();
        let result = adapter.start(host, cancel).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("access_token"));
    }

    #[tokio::test]
    async fn start_validates_user_id() {
        let mut config = make_config();
        config.user_id = String::new();
        let adapter = MatrixChannelAdapter::new(config);

        let host = Arc::new(MockHost);
        let cancel = CancellationToken::new();
        let result = adapter.start(host, cancel).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("user_id"));
    }

    #[tokio::test]
    async fn start_shuts_down_on_cancel() {
        let adapter = MatrixChannelAdapter::new(make_config());
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        let host = Arc::new(MockHost);
        let handle = tokio::spawn(async move { adapter.start(host, cancel_clone).await });

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
