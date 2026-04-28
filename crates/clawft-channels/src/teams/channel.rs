//! Microsoft Teams channel adapter implementation.
//!
//! # WARNING: Planning stub -- does NOT transmit messages.
//!
//! This adapter is a compile-time placeholder. It is **not**
//! production-ready:
//!
//! - `start()` never registers a Bot Framework webhook and never
//!   acquires an Azure AD client-credentials token. It logs a `debug!`
//!   line and waits for cancellation.
//! - `send()` does not POST to
//!   `/teams/{team-id}/channels/{channel-id}/messages`. It fabricates a
//!   synthetic `teams-{ts}` id and returns it. Outbound messages are
//!   silently dropped.
//!
//! The real Bot Framework runtime is tracked as Task 7 in
//! `.planning/reviews/0.7.0-release-gate/05-channels.md`. Do **not**
//! enable the `teams` feature in production until that task ships.
//!
//! Implements [`ChannelAdapter`] for Microsoft Teams via the
//! Bot Framework and Graph API. Uses Azure AD client credentials
//! for authentication.

use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use clawft_plugin::error::PluginError;
use clawft_plugin::message::MessagePayload;
use clawft_plugin::traits::{ChannelAdapter, ChannelAdapterHost};

use super::types::TeamsAdapterConfig;

/// Microsoft Teams channel adapter.
///
/// Connects to Teams via the Bot Framework for inbound messages
/// and the Graph API for outbound messages. Uses Azure AD
/// client credentials flow for authentication.
pub struct TeamsChannelAdapter {
    config: TeamsAdapterConfig,
}

impl TeamsChannelAdapter {
    /// Create a new Teams channel adapter.
    pub fn new(config: TeamsAdapterConfig) -> Self {
        Self { config }
    }

    /// Check if a user principal name is in the allow list.
    pub fn is_user_allowed(&self, upn: &str) -> bool {
        if self.config.allowed_users.is_empty() {
            return true;
        }
        let upn_lower = upn.to_lowercase();
        self.config
            .allowed_users
            .iter()
            .any(|u| u.to_lowercase() == upn_lower)
    }
}

#[async_trait]
impl ChannelAdapter for TeamsChannelAdapter {
    fn name(&self) -> &str {
        "teams"
    }

    fn display_name(&self) -> &str {
        "Microsoft Teams"
    }

    fn supports_threads(&self) -> bool {
        true // Teams supports threaded conversations
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
            "teams channel adapter is a planning stub: Azure AD token \
             acquisition and Graph API POST are not implemented; \
             outbound messages will be silently dropped. See \
             .planning/reviews/0.7.0-release-gate/05-channels.md task 7."
        );
        info!("Teams channel adapter starting");

        if self.config.tenant_id.is_empty() {
            return Err(PluginError::LoadFailed(
                "teams adapter: tenant_id is required".into(),
            ));
        }
        if self.config.client_id.is_empty() {
            return Err(PluginError::LoadFailed(
                "teams adapter: client_id is required".into(),
            ));
        }
        if self.config.client_secret.is_empty() {
            return Err(PluginError::LoadFailed(
                "teams adapter: client_secret is required".into(),
            ));
        }

        // In production, this would:
        // 1. Acquire an Azure AD token via client credentials
        // 2. Register a webhook endpoint with the Bot Framework
        // 3. Process incoming Activity objects
        // 4. Deliver parsed messages to host
        debug!(
            tenant = %self.config.tenant_id,
            client = %self.config.client_id,
            graph_url = %self.config.graph_url,
            "teams bot framework loop would start here (stub)"
        );

        cancel.cancelled().await;
        info!("Teams channel adapter shutting down");
        Ok(())
    }

    async fn send(
        &self,
        target: &str,
        payload: &MessagePayload,
    ) -> Result<String, PluginError> {
        let content = payload.as_text().ok_or_else(|| {
            PluginError::ExecutionFailed(
                "teams: only text payloads supported".into(),
            )
        })?;

        if self.config.tenant_id.is_empty() {
            return Err(PluginError::ExecutionFailed(
                "teams: tenant_id not configured".into(),
            ));
        }

        // In production, this would POST to the Graph API:
        // POST /teams/{team-id}/channels/{channel-id}/messages
        debug!(
            channel = %target,
            content_len = content.len(),
            "sending Teams message (stub)"
        );

        let msg_id = format!(
            "teams-{}",
            chrono::Utc::now().timestamp_millis()
        );
        Ok(msg_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_config() -> TeamsAdapterConfig {
        TeamsAdapterConfig {
            tenant_id: "test-tenant".into(),
            client_id: "test-client".into(),
            client_secret: "test-secret".into(),
            bot_app_id: "test-bot".into(),
            ..Default::default()
        }
    }

    #[test]
    fn name_is_teams() {
        let adapter = TeamsChannelAdapter::new(make_config());
        assert_eq!(adapter.name(), "teams");
    }

    #[test]
    fn display_name() {
        let adapter = TeamsChannelAdapter::new(make_config());
        assert_eq!(adapter.display_name(), "Microsoft Teams");
    }

    #[test]
    fn supports_threads_yes() {
        let adapter = TeamsChannelAdapter::new(make_config());
        assert!(adapter.supports_threads());
    }

    #[test]
    fn supports_media_no() {
        let adapter = TeamsChannelAdapter::new(make_config());
        assert!(!adapter.supports_media());
    }

    #[test]
    fn user_filtering() {
        let mut config = make_config();
        config.allowed_users = vec!["admin@company.com".into()];
        let adapter = TeamsChannelAdapter::new(config);

        assert!(adapter.is_user_allowed("admin@company.com"));
        assert!(adapter.is_user_allowed("ADMIN@COMPANY.COM"));
        assert!(!adapter.is_user_allowed("stranger@evil.com"));
    }

    #[test]
    fn empty_allow_list_allows_all() {
        let adapter = TeamsChannelAdapter::new(make_config());
        assert!(adapter.is_user_allowed("anyone@company.com"));
    }

    #[tokio::test]
    async fn send_text_message() {
        let adapter = TeamsChannelAdapter::new(make_config());
        let payload = MessagePayload::text("Hello Teams");
        let result = adapter.send("channel-123", &payload).await;
        assert!(result.is_ok());
        assert!(result.unwrap().starts_with("teams-"));
    }

    #[tokio::test]
    async fn send_non_text_fails() {
        let adapter = TeamsChannelAdapter::new(make_config());
        let payload =
            MessagePayload::structured(serde_json::json!({}));
        let result = adapter.send("channel-123", &payload).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn start_validates_tenant_id() {
        let mut config = make_config();
        config.tenant_id = String::new();
        let adapter = TeamsChannelAdapter::new(config);

        let host = Arc::new(MockHost);
        let cancel = CancellationToken::new();
        let result = adapter.start(host, cancel).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("tenant_id"));
    }

    #[tokio::test]
    async fn start_validates_client_id() {
        let mut config = make_config();
        config.client_id = String::new();
        let adapter = TeamsChannelAdapter::new(config);

        let host = Arc::new(MockHost);
        let cancel = CancellationToken::new();
        let result = adapter.start(host, cancel).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("client_id"));
    }

    #[tokio::test]
    async fn start_validates_client_secret() {
        let mut config = make_config();
        config.client_secret = "".into();
        let adapter = TeamsChannelAdapter::new(config);

        let host = Arc::new(MockHost);
        let cancel = CancellationToken::new();
        let result = adapter.start(host, cancel).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("client_secret"));
    }

    #[tokio::test]
    async fn start_shuts_down_on_cancel() {
        let adapter = TeamsChannelAdapter::new(make_config());
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
