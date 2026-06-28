//! Email channel adapter implementation.
//!
//! # WARNING: Planning stub -- does NOT transmit messages.
//!
//! This adapter is a compile-time placeholder so config schemas, factory
//! wiring, and tests can run today. It is **not** production-ready:
//!
//! - `start()` never connects to IMAP. The poll loop logs a `debug!`
//!   line ("polling for new emails (stub)") and waits for cancellation.
//! - `send()` does not invoke SMTP. It fabricates a synthetic
//!   `<{ts}-{target}@{host}>` Message-ID and returns it. Outbound
//!   messages are silently dropped.
//!
//! The real IMAP/SMTP runtime (using the `imap` and `lettre` crates
//! behind the `email` feature) is tracked as Task 4 in
//! `.planning/reviews/0.7.0-release-gate/05-channels.md`. Do **not**
//! enable the `email` feature in production until that task ships.
//!
//! Implements [`ChannelAdapter`] from `clawft-plugin` for IMAP/SMTP
//! email communication. Polls an IMAP mailbox for new messages and
//! delivers them to the agent pipeline via [`ChannelAdapterHost`].
//!
//! Outbound messages are sent as email replies via SMTP.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use clawft_plugin::error::PluginError;
use clawft_plugin::message::MessagePayload;
use clawft_plugin::traits::{ChannelAdapter, ChannelAdapterHost};

use super::types::{EmailAdapterConfig, ParsedEmail};

/// Email channel adapter.
///
/// Connects to an IMAP server to receive email messages and uses SMTP
/// to send replies. Implements the [`ChannelAdapter`] plugin trait.
///
/// # Credential Handling
///
/// All credentials are stored via [`SecretString`](clawft_types::secret::SecretString)
/// or referenced as environment variable names (for OAuth2). No plaintext
/// secrets appear in config structs or log output.
pub struct EmailChannelAdapter {
    config: EmailAdapterConfig,
}

impl EmailChannelAdapter {
    /// Create a new email channel adapter from configuration.
    pub fn new(config: EmailAdapterConfig) -> Self {
        Self { config }
    }

    /// Check if a sender email is in the allow list.
    ///
    /// Returns `true` when the allow list is empty (everyone allowed)
    /// or when `sender` appears in the list (case-insensitive).
    pub fn is_sender_allowed(&self, sender: &str) -> bool {
        if self.config.allowed_senders.is_empty() {
            return true;
        }
        let sender_lower = sender.to_lowercase();
        self.config
            .allowed_senders
            .iter()
            .any(|s| s.to_lowercase() == sender_lower)
    }

    /// Build an inbound metadata map from a parsed email.
    pub fn build_metadata(email: &ParsedEmail) -> HashMap<String, serde_json::Value> {
        let mut metadata = HashMap::new();
        metadata.insert("subject".to_string(), serde_json::json!(email.subject));
        metadata.insert(
            "message_id".to_string(),
            serde_json::json!(email.message_id),
        );
        metadata.insert("to".to_string(), serde_json::json!(email.to));
        if let Some(ref reply_to) = email.in_reply_to {
            metadata.insert("in_reply_to".to_string(), serde_json::json!(reply_to));
        }
        metadata
    }

    /// Deliver a parsed email to the host pipeline.
    pub async fn deliver_email(
        &self,
        email: &ParsedEmail,
        host: &Arc<dyn ChannelAdapterHost>,
    ) -> Result<(), PluginError> {
        if !self.is_sender_allowed(&email.from) {
            warn!(
                sender = %email.from,
                "email from disallowed sender, ignoring"
            );
            return Ok(());
        }

        // Truncate body to max_body_chars.
        let body = if email.body.len() > self.config.max_body_chars {
            let truncated: String = email
                .body
                .chars()
                .take(self.config.max_body_chars)
                .collect();
            format!("{truncated}\n\n[truncated]")
        } else {
            email.body.clone()
        };

        let content = format!("Subject: {}\n\n{}", email.subject, body);
        let metadata = Self::build_metadata(email);

        host.deliver_inbound(
            "email",
            &email.from,
            &email.from,
            MessagePayload::text(content),
            metadata,
        )
        .await
    }
}

#[async_trait]
impl ChannelAdapter for EmailChannelAdapter {
    fn name(&self) -> &str {
        "email"
    }

    fn display_name(&self) -> &str {
        "Email (IMAP/SMTP)"
    }

    fn supports_threads(&self) -> bool {
        true // Email threading via In-Reply-To / References headers
    }

    fn supports_media(&self) -> bool {
        false // Attachment support is a future enhancement
    }

    async fn start(
        &self,
        _host: Arc<dyn ChannelAdapterHost>,
        cancel: CancellationToken,
    ) -> Result<(), PluginError> {
        warn!(
            "email channel adapter is a planning stub: poll loop and \
             SMTP send are not implemented; outbound messages will be \
             silently dropped. See \
             .planning/reviews/0.7.0-release-gate/05-channels.md task 4."
        );
        info!(
            imap_host = %self.config.imap_host,
            mailbox = %self.config.mailbox,
            poll_interval_secs = self.config.poll_interval_secs,
            "email channel adapter starting"
        );

        // Validate configuration before entering the poll loop.
        if self.config.imap_host.is_empty() {
            return Err(PluginError::LoadFailed(
                "email adapter: imap_host is required".into(),
            ));
        }
        if self.config.email_address.is_empty() {
            return Err(PluginError::LoadFailed(
                "email adapter: email_address is required".into(),
            ));
        }

        let poll_interval = std::time::Duration::from_secs(self.config.poll_interval_secs);
        let mut interval = tokio::time::interval(poll_interval);
        // Skip the immediate first tick.
        interval.tick().await;

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    info!("email channel adapter shutting down");
                    return Ok(());
                }
                _ = interval.tick() => {
                    // In production, this would connect to IMAP, fetch new
                    // messages, parse them, and call self.deliver_email().
                    // The actual IMAP/SMTP integration requires the `imap`
                    // and `lettre` crates (behind the `email` feature flag).
                    debug!(
                        mailbox = %self.config.mailbox,
                        "polling for new emails (stub)"
                    );
                }
            }
        }
    }

    async fn send(&self, target: &str, payload: &MessagePayload) -> Result<String, PluginError> {
        let content = match payload.as_text() {
            Some(text) => text,
            None => {
                return Err(PluginError::ExecutionFailed(
                    "email adapter only supports text payloads".into(),
                ));
            }
        };

        if self.config.smtp_host.is_empty() {
            return Err(PluginError::ExecutionFailed(
                "email adapter: smtp_host is not configured".into(),
            ));
        }

        // In production, this would use `lettre` to send the email via SMTP.
        // The stub generates a message ID for testing purposes.
        info!(
            to = %target,
            smtp_host = %self.config.smtp_host,
            "sending email (stub)"
        );
        debug!(content_len = content.len(), "email content");

        let msg_id = format!(
            "<{}-{}@{}>",
            chrono::Utc::now().timestamp_millis(),
            target.replace('@', "-at-"),
            self.config
                .email_address
                .split('@')
                .nth(1)
                .unwrap_or("localhost")
        );

        Ok(msg_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use clawft_plugin::traits::ChannelAdapterHost;
    use tokio::sync::Mutex;

    // -- Mock host --

    struct MockAdapterHost {
        messages: Mutex<Vec<(String, String, String, MessagePayload)>>,
    }

    impl MockAdapterHost {
        fn new() -> Self {
            Self {
                messages: Mutex::new(vec![]),
            }
        }
    }

    #[async_trait]
    impl ChannelAdapterHost for MockAdapterHost {
        async fn deliver_inbound(
            &self,
            channel: &str,
            sender_id: &str,
            chat_id: &str,
            payload: MessagePayload,
            _metadata: HashMap<String, serde_json::Value>,
        ) -> Result<(), PluginError> {
            self.messages.lock().await.push((
                channel.into(),
                sender_id.into(),
                chat_id.into(),
                payload,
            ));
            Ok(())
        }
    }

    fn make_config() -> EmailAdapterConfig {
        EmailAdapterConfig {
            imap_host: "imap.test.com".into(),
            smtp_host: "smtp.test.com".into(),
            email_address: "bot@test.com".into(),
            ..Default::default()
        }
    }

    // -- Trait method tests --

    #[test]
    fn name_is_email() {
        let adapter = EmailChannelAdapter::new(make_config());
        assert_eq!(adapter.name(), "email");
    }

    #[test]
    fn display_name() {
        let adapter = EmailChannelAdapter::new(make_config());
        assert_eq!(adapter.display_name(), "Email (IMAP/SMTP)");
    }

    #[test]
    fn supports_threads() {
        let adapter = EmailChannelAdapter::new(make_config());
        assert!(adapter.supports_threads());
    }

    #[test]
    fn supports_media_false() {
        let adapter = EmailChannelAdapter::new(make_config());
        assert!(!adapter.supports_media());
    }

    // -- Sender filtering --

    #[test]
    fn empty_allow_list_allows_everyone() {
        let adapter = EmailChannelAdapter::new(make_config());
        assert!(adapter.is_sender_allowed("anyone@example.com"));
        assert!(adapter.is_sender_allowed(""));
    }

    #[test]
    fn allow_list_filters_senders() {
        let mut config = make_config();
        config.allowed_senders = vec!["boss@company.com".into(), "Admin@Corp.com".into()];
        let adapter = EmailChannelAdapter::new(config);

        assert!(adapter.is_sender_allowed("boss@company.com"));
        assert!(adapter.is_sender_allowed("BOSS@COMPANY.COM")); // case insensitive
        assert!(adapter.is_sender_allowed("admin@corp.com"));
        assert!(!adapter.is_sender_allowed("stranger@evil.com"));
    }

    // -- Email delivery --

    #[tokio::test]
    async fn deliver_email_sends_to_host() {
        let adapter = EmailChannelAdapter::new(make_config());
        let mock_host = Arc::new(MockAdapterHost::new());
        let host: Arc<dyn ChannelAdapterHost> = mock_host.clone();

        let email = ParsedEmail {
            from: "alice@example.com".into(),
            to: "bot@test.com".into(),
            subject: "Help request".into(),
            body: "I need help with my account".into(),
            message_id: "<msg-001@example.com>".into(),
            in_reply_to: None,
        };

        adapter.deliver_email(&email, &host).await.unwrap();

        let msgs = mock_host.messages.lock().await;
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].0, "email");
        assert_eq!(msgs[0].1, "alice@example.com");
        let text = msgs[0].3.as_text().unwrap();
        assert!(text.contains("Help request"));
        assert!(text.contains("I need help with my account"));
    }

    #[tokio::test]
    async fn deliver_email_skips_disallowed_sender() {
        let mut config = make_config();
        config.allowed_senders = vec!["boss@company.com".into()];
        let adapter = EmailChannelAdapter::new(config);
        let mock_host = Arc::new(MockAdapterHost::new());
        let host: Arc<dyn ChannelAdapterHost> = mock_host.clone();

        let email = ParsedEmail {
            from: "stranger@evil.com".into(),
            to: "bot@test.com".into(),
            subject: "Spam".into(),
            body: "Buy now!".into(),
            message_id: "<spam@evil.com>".into(),
            in_reply_to: None,
        };

        adapter.deliver_email(&email, &host).await.unwrap();
        assert!(mock_host.messages.lock().await.is_empty());
    }

    #[tokio::test]
    async fn deliver_email_truncates_long_body() {
        let mut config = make_config();
        config.max_body_chars = 20;
        let adapter = EmailChannelAdapter::new(config);
        let mock_host = Arc::new(MockAdapterHost::new());
        let host: Arc<dyn ChannelAdapterHost> = mock_host.clone();

        let email = ParsedEmail {
            from: "alice@example.com".into(),
            to: "bot@test.com".into(),
            subject: "Long email".into(),
            body: "A".repeat(100),
            message_id: "<long@example.com>".into(),
            in_reply_to: None,
        };

        adapter.deliver_email(&email, &host).await.unwrap();

        let msgs = mock_host.messages.lock().await;
        let text = msgs[0].3.as_text().unwrap();
        assert!(text.contains("[truncated]"));
    }

    // -- Metadata --

    #[test]
    fn build_metadata_includes_subject_and_message_id() {
        let email = ParsedEmail {
            from: "alice@example.com".into(),
            to: "bot@test.com".into(),
            subject: "Test subject".into(),
            body: "body text".into(),
            message_id: "<msg-001@test.com>".into(),
            in_reply_to: None,
        };

        let metadata = EmailChannelAdapter::build_metadata(&email);
        assert_eq!(metadata["subject"], serde_json::json!("Test subject"));
        assert_eq!(
            metadata["message_id"],
            serde_json::json!("<msg-001@test.com>")
        );
        assert_eq!(metadata["to"], serde_json::json!("bot@test.com"));
        assert!(!metadata.contains_key("in_reply_to"));
    }

    #[test]
    fn build_metadata_includes_in_reply_to() {
        let email = ParsedEmail {
            from: "alice@example.com".into(),
            to: "bot@test.com".into(),
            subject: "Re: Test".into(),
            body: "reply".into(),
            message_id: "<msg-002@test.com>".into(),
            in_reply_to: Some("<msg-001@test.com>".into()),
        };

        let metadata = EmailChannelAdapter::build_metadata(&email);
        assert_eq!(
            metadata["in_reply_to"],
            serde_json::json!("<msg-001@test.com>")
        );
    }

    // -- Send --

    #[tokio::test]
    async fn send_text_payload() {
        let adapter = EmailChannelAdapter::new(make_config());
        let payload = MessagePayload::text("Hello from the bot!");
        let result = adapter.send("user@example.com", &payload).await;
        assert!(result.is_ok());
        let msg_id = result.unwrap();
        assert!(msg_id.starts_with('<'));
        assert!(msg_id.ends_with('>'));
        assert!(msg_id.contains("test.com"));
    }

    #[tokio::test]
    async fn send_non_text_payload_fails() {
        let adapter = EmailChannelAdapter::new(make_config());
        let payload = MessagePayload::structured(serde_json::json!({"key": "value"}));
        let result = adapter.send("user@example.com", &payload).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn send_without_smtp_host_fails() {
        let mut config = make_config();
        config.smtp_host = String::new();
        let adapter = EmailChannelAdapter::new(config);
        let payload = MessagePayload::text("test");
        let result = adapter.send("user@example.com", &payload).await;
        assert!(result.is_err());
    }

    // -- Start validation --

    #[tokio::test]
    async fn start_without_imap_host_fails() {
        let mut config = make_config();
        config.imap_host = String::new();
        let adapter = EmailChannelAdapter::new(config);
        let host = Arc::new(MockAdapterHost::new());
        let cancel = CancellationToken::new();

        let result = adapter.start(host, cancel).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("imap_host"));
    }

    #[tokio::test]
    async fn start_without_email_address_fails() {
        let mut config = make_config();
        config.email_address = String::new();
        let adapter = EmailChannelAdapter::new(config);
        let host = Arc::new(MockAdapterHost::new());
        let cancel = CancellationToken::new();

        let result = adapter.start(host, cancel).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("email_address"));
    }

    #[tokio::test]
    async fn start_shuts_down_on_cancel() {
        let adapter = EmailChannelAdapter::new(make_config());
        let host = Arc::new(MockAdapterHost::new());
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        let handle = tokio::spawn(async move { adapter.start(host, cancel_clone).await });

        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        cancel.cancel();

        let result = handle.await.unwrap();
        assert!(result.is_ok());
    }
}
