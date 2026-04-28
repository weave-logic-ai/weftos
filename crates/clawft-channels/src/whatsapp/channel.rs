//! WhatsApp channel adapter implementation.
//!
//! # WARNING: Planning stub -- does NOT transmit messages.
//!
//! This adapter is a compile-time placeholder. It is **not**
//! production-ready:
//!
//! - `start()` never starts a webhook listener and never verifies
//!   `X-Hub-Signature-256` headers. It waits for cancellation.
//! - `send()` does not POST to
//!   `/v18.0/{phone_number_id}/messages`. It fabricates a synthetic
//!   `wamid.{ts}` id and returns it. Outbound messages are silently
//!   dropped.
//!
//! The real Cloud API runtime (webhook receiver, signature verify,
//! outbound POST, 429 backoff) is tracked as Task 2 in
//! `.planning/reviews/0.7.0-release-gate/05-channels.md`. Do **not**
//! enable the `whatsapp` feature in production until that task ships.
//!
//! Implements [`ChannelAdapter`] for WhatsApp Cloud API communication.
//! Uses webhook for inbound messages and REST API for outbound.

use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use clawft_plugin::error::PluginError;
use clawft_plugin::message::MessagePayload;
use clawft_plugin::traits::{ChannelAdapter, ChannelAdapterHost};

use super::types::WhatsAppAdapterConfig;

/// WhatsApp channel adapter using the Cloud API.
pub struct WhatsAppChannelAdapter {
    config: WhatsAppAdapterConfig,
}

impl WhatsAppChannelAdapter {
    /// Create a new WhatsApp channel adapter.
    pub fn new(config: WhatsAppAdapterConfig) -> Self {
        Self { config }
    }

    /// Check if a phone number is in the allow list.
    pub fn is_number_allowed(&self, number: &str) -> bool {
        if self.config.allowed_numbers.is_empty() {
            return true;
        }
        self.config.allowed_numbers.iter().any(|n| n == number)
    }
}

#[async_trait]
impl ChannelAdapter for WhatsAppChannelAdapter {
    fn name(&self) -> &str {
        "whatsapp"
    }

    fn display_name(&self) -> &str {
        "WhatsApp"
    }

    fn supports_threads(&self) -> bool {
        false
    }

    fn supports_media(&self) -> bool {
        true
    }

    async fn start(
        &self,
        _host: Arc<dyn ChannelAdapterHost>,
        cancel: CancellationToken,
    ) -> Result<(), PluginError> {
        warn!(
            "whatsapp channel adapter is a planning stub: webhook \
             listener and Cloud API POST are not implemented; outbound \
             messages will be silently dropped. See \
             .planning/reviews/0.7.0-release-gate/05-channels.md task 2."
        );
        info!("WhatsApp channel adapter starting");

        if self.config.phone_number_id.is_empty() {
            return Err(PluginError::LoadFailed(
                "whatsapp adapter: phone_number_id is required".into(),
            ));
        }
        if self.config.access_token.is_empty() {
            return Err(PluginError::LoadFailed(
                "whatsapp adapter: access_token is required".into(),
            ));
        }

        // In production, this would set up a webhook listener for
        // incoming message notifications from the WhatsApp Cloud API.
        cancel.cancelled().await;
        info!("WhatsApp channel adapter shutting down");
        Ok(())
    }

    async fn send(
        &self,
        target: &str,
        payload: &MessagePayload,
    ) -> Result<String, PluginError> {
        let content = payload.as_text().ok_or_else(|| {
            PluginError::ExecutionFailed("whatsapp: only text payloads supported".into())
        })?;

        if self.config.phone_number_id.is_empty() {
            return Err(PluginError::ExecutionFailed(
                "whatsapp: phone_number_id not configured".into(),
            ));
        }

        // In production, this would POST to the WhatsApp Cloud API:
        // POST /{api_version}/{phone_number_id}/messages
        debug!(
            to = %target,
            content_len = content.len(),
            "sending WhatsApp message (stub)"
        );

        let msg_id = format!("wamid.{}", chrono::Utc::now().timestamp_millis());
        Ok(msg_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_config() -> WhatsAppAdapterConfig {
        WhatsAppAdapterConfig {
            phone_number_id: "12345".into(),
            access_token: "test-token".into(),
            ..Default::default()
        }
    }

    #[test]
    fn name_is_whatsapp() {
        let adapter = WhatsAppChannelAdapter::new(make_config());
        assert_eq!(adapter.name(), "whatsapp");
    }

    #[test]
    fn display_name() {
        let adapter = WhatsAppChannelAdapter::new(make_config());
        assert_eq!(adapter.display_name(), "WhatsApp");
    }

    #[test]
    fn number_filtering() {
        let mut config = make_config();
        config.allowed_numbers = vec!["+1234567890".into()];
        let adapter = WhatsAppChannelAdapter::new(config);

        assert!(adapter.is_number_allowed("+1234567890"));
        assert!(!adapter.is_number_allowed("+9876543210"));
    }

    #[test]
    fn empty_allow_list_allows_all() {
        let adapter = WhatsAppChannelAdapter::new(make_config());
        assert!(adapter.is_number_allowed("+anyone"));
    }

    #[tokio::test]
    async fn send_text_message() {
        let adapter = WhatsAppChannelAdapter::new(make_config());
        let payload = MessagePayload::text("Hello from bot");
        let result = adapter.send("+1234567890", &payload).await;
        assert!(result.is_ok());
        assert!(result.unwrap().starts_with("wamid."));
    }

    #[tokio::test]
    async fn send_non_text_fails() {
        let adapter = WhatsAppChannelAdapter::new(make_config());
        let payload = MessagePayload::structured(serde_json::json!({}));
        let result = adapter.send("+1234567890", &payload).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn start_shuts_down_on_cancel() {
        let adapter = WhatsAppChannelAdapter::new(make_config());
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
