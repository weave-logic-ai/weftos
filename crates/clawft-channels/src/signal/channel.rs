//! Signal channel adapter implementation.
//!
//! # WARNING: Planning stub -- does NOT transmit messages.
//!
//! This adapter is a compile-time placeholder. It is **not**
//! production-ready:
//!
//! - `start()` never spawns `signal-cli daemon`. PID tracking,
//!   JSON-RPC reader, and auto-restart are unimplemented. It waits
//!   for cancellation.
//! - `send()` does not invoke `tokio::process::Command`. Argument
//!   sanitization runs (so the security envelope is exercised) but no
//!   process is spawned and no message is transmitted; the call returns
//!   a synthetic `signal-{ts}` id. Outbound messages are silently
//!   dropped.
//!
//! The real subprocess runtime is tracked as Task 3 in
//! `.planning/reviews/0.7.0-release-gate/05-channels.md`. Do **not**
//! enable the `signal` feature in production until that task ships.
//!
//! Implements [`ChannelAdapter`] for Signal messaging via `signal-cli`.
//! Uses `tokio::process::Command` for subprocess management with
//! argument sanitization to prevent command injection.

use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use clawft_plugin::error::PluginError;
use clawft_plugin::message::MessagePayload;
use clawft_plugin::traits::{ChannelAdapter, ChannelAdapterHost};

use super::types::{sanitize_argument, SignalAdapterConfig};

/// Signal channel adapter using `signal-cli` subprocess.
///
/// Runs `signal-cli` as a subprocess for sending and receiving messages.
/// All arguments are sanitized via [`sanitize_argument`] before being
/// passed to the subprocess to prevent command injection.
pub struct SignalChannelAdapter {
    config: SignalAdapterConfig,
}

impl SignalChannelAdapter {
    /// Create a new Signal channel adapter.
    pub fn new(config: SignalAdapterConfig) -> Self {
        Self { config }
    }

    /// Check if a phone number is in the allow list.
    pub fn is_number_allowed(&self, number: &str) -> bool {
        if self.config.allowed_numbers.is_empty() {
            return true;
        }
        self.config.allowed_numbers.iter().any(|n| n == number)
    }

    /// Validate the adapter configuration.
    fn validate_config(&self) -> Result<(), PluginError> {
        if self.config.phone_number.is_empty() {
            return Err(PluginError::LoadFailed(
                "signal adapter: phone_number is required".into(),
            ));
        }
        sanitize_argument(&self.config.phone_number).map_err(|e| {
            PluginError::LoadFailed(format!(
                "signal adapter: invalid phone_number: {e}"
            ))
        })?;
        sanitize_argument(&self.config.signal_cli_path).map_err(|e| {
            PluginError::LoadFailed(format!(
                "signal adapter: invalid signal_cli_path: {e}"
            ))
        })?;
        Ok(())
    }
}

#[async_trait]
impl ChannelAdapter for SignalChannelAdapter {
    fn name(&self) -> &str {
        "signal"
    }

    fn display_name(&self) -> &str {
        "Signal"
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
            "signal channel adapter is a planning stub: `signal-cli \
             daemon` subprocess is not spawned; outbound messages will \
             be silently dropped. See \
             .planning/reviews/0.7.0-release-gate/05-channels.md task 3."
        );
        info!("Signal channel adapter starting");

        self.validate_config()?;

        // In production, this would spawn `signal-cli daemon` as a
        // long-running subprocess and read its JSON-RPC output for
        // incoming messages. Each inbound message would be parsed and
        // forwarded to host.deliver_inbound().
        //
        // The subprocess PID would be tracked for graceful shutdown.
        debug!(
            phone = %self.config.phone_number,
            cli = %self.config.signal_cli_path,
            timeout_secs = self.config.timeout_secs,
            "signal-cli subprocess would start here (stub)"
        );

        cancel.cancelled().await;
        info!("Signal channel adapter shutting down");
        Ok(())
    }

    async fn send(
        &self,
        target: &str,
        payload: &MessagePayload,
    ) -> Result<String, PluginError> {
        let content = payload.as_text().ok_or_else(|| {
            PluginError::ExecutionFailed(
                "signal: only text payloads supported".into(),
            )
        })?;

        if self.config.phone_number.is_empty() {
            return Err(PluginError::ExecutionFailed(
                "signal: phone_number not configured".into(),
            ));
        }

        // Sanitize the target before passing it as a subprocess argument.
        sanitize_argument(target).map_err(|e| {
            PluginError::ExecutionFailed(format!(
                "signal: invalid target number: {e}"
            ))
        })?;

        // In production, this would execute:
        // signal-cli -a <phone_number> send -m <content> <target>
        //
        // Using tokio::process::Command with a timeout from
        // self.config.timeout_secs.
        debug!(
            to = %target,
            content_len = content.len(),
            "sending Signal message (stub)"
        );

        let msg_id = format!(
            "signal-{}",
            chrono::Utc::now().timestamp_millis()
        );
        Ok(msg_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_config() -> SignalAdapterConfig {
        SignalAdapterConfig {
            phone_number: "+15551234567".into(),
            ..Default::default()
        }
    }

    #[test]
    fn name_is_signal() {
        let adapter = SignalChannelAdapter::new(make_config());
        assert_eq!(adapter.name(), "signal");
    }

    #[test]
    fn display_name() {
        let adapter = SignalChannelAdapter::new(make_config());
        assert_eq!(adapter.display_name(), "Signal");
    }

    #[test]
    fn no_threads_or_media() {
        let adapter = SignalChannelAdapter::new(make_config());
        assert!(!adapter.supports_threads());
        assert!(!adapter.supports_media());
    }

    #[test]
    fn number_filtering() {
        let mut config = make_config();
        config.allowed_numbers = vec!["+1234567890".into()];
        let adapter = SignalChannelAdapter::new(config);

        assert!(adapter.is_number_allowed("+1234567890"));
        assert!(!adapter.is_number_allowed("+9876543210"));
    }

    #[test]
    fn empty_allow_list_allows_all() {
        let adapter = SignalChannelAdapter::new(make_config());
        assert!(adapter.is_number_allowed("+anyone"));
    }

    #[test]
    fn validate_config_success() {
        let adapter = SignalChannelAdapter::new(make_config());
        assert!(adapter.validate_config().is_ok());
    }

    #[test]
    fn validate_config_empty_phone() {
        let mut config = make_config();
        config.phone_number = String::new();
        let adapter = SignalChannelAdapter::new(config);
        let err = adapter.validate_config().unwrap_err();
        assert!(err.to_string().contains("phone_number"));
    }

    #[test]
    fn validate_config_bad_phone_number() {
        let mut config = make_config();
        config.phone_number = "+1234; rm -rf /".into();
        let adapter = SignalChannelAdapter::new(config);
        let err = adapter.validate_config().unwrap_err();
        assert!(err.to_string().contains("phone_number"));
    }

    #[test]
    fn validate_config_bad_cli_path() {
        let mut config = make_config();
        config.signal_cli_path = "/bin/evil; cat /etc/passwd".into();
        let adapter = SignalChannelAdapter::new(config);
        let err = adapter.validate_config().unwrap_err();
        assert!(err.to_string().contains("signal_cli_path"));
    }

    #[tokio::test]
    async fn send_text_message() {
        let adapter = SignalChannelAdapter::new(make_config());
        let payload = MessagePayload::text("Hello from bot");
        let result = adapter.send("+1234567890", &payload).await;
        assert!(result.is_ok());
        assert!(result.unwrap().starts_with("signal-"));
    }

    #[tokio::test]
    async fn send_non_text_fails() {
        let adapter = SignalChannelAdapter::new(make_config());
        let payload =
            MessagePayload::structured(serde_json::json!({}));
        let result = adapter.send("+1234567890", &payload).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn send_to_unsafe_target_fails() {
        let adapter = SignalChannelAdapter::new(make_config());
        let payload = MessagePayload::text("test");
        let result =
            adapter.send("+1234; rm -rf /", &payload).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("target"));
    }

    #[tokio::test]
    async fn start_validates_phone_number() {
        let mut config = make_config();
        config.phone_number = String::new();
        let adapter = SignalChannelAdapter::new(config);

        let host = Arc::new(MockHost);
        let cancel = CancellationToken::new();
        let result = adapter.start(host, cancel).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn start_shuts_down_on_cancel() {
        let adapter = SignalChannelAdapter::new(make_config());
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
