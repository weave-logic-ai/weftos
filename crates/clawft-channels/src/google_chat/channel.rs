//! Google Chat channel adapter (real Pub/Sub pull + Chat REST send).
//!
//! Implements [`ChannelAdapter`] from `clawft-plugin` for Google
//! Workspace Chat. This adapter ships real network I/O behind the
//! `google-chat` feature flag.
//!
//! # Runtime
//!
//! - `start()` validates config and then spawns a long-lived loop that
//!   POSTs to `pubsub.googleapis.com/v1/{subscription}:pull` with
//!   `maxMessages = pull_max_messages`. Each received Pub/Sub message
//!   carries a base64-encoded Google Chat event (`MESSAGE`, `ADDED_TO_SPACE`,
//!   etc.) in `message.data`; the adapter decodes it, extracts the
//!   sender / space / text, calls
//!   [`ChannelAdapterHost::deliver_inbound`], and acknowledges the
//!   message via `…:acknowledge`. Pull failures use exponential
//!   backoff; the loop exits cleanly on `CancellationToken::cancelled`.
//! - `send()` POSTs the supplied text to
//!   `chat.googleapis.com/v1/{space}/messages` with
//!   `Authorization: Bearer <token>`. Returns the response's `name`
//!   (e.g. `spaces/AAAAA/messages/BBBBBB.CCCCCC`) as the channel
//!   message ID.
//! - `stop()` is implicit: callers cancel the `CancellationToken`
//!   passed to `start()` and the pull loop unwinds.
//!
//! # Authentication
//!
//! In 0.7.0 the adapter expects a pre-issued OAuth2 access token in an
//! environment variable named by `bearer_token_env` (default
//! `GOOGLE_CHAT_ACCESS_TOKEN`). The env var is re-read on every HTTP
//! call so external rotation (e.g. a sidecar refreshing
//! `gcloud auth application-default print-access-token`) is picked up
//! without restart. Service-account JWT signing (RS256) is a 0.8.x
//! follow-up — see `service_account_key_path` on
//! [`super::types::GoogleChatAdapterConfig`].
//!
//! # Test injection
//!
//! Both base URLs (`chat_base_url`, `pubsub_base_url`) can be
//! overridden in config so the unit tests below can point the adapter
//! at a `wiremock::MockServer`. Token acquisition is abstracted via
//! the [`TokenSource`] trait so tests can inject a static token without
//! touching the real env.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use clawft_plugin::error::PluginError;
use clawft_plugin::message::MessagePayload;
use clawft_plugin::traits::{ChannelAdapter, ChannelAdapterHost};

use super::types::GoogleChatAdapterConfig;

const DEFAULT_CHAT_BASE_URL: &str = "https://chat.googleapis.com";
const DEFAULT_PUBSUB_BASE_URL: &str = "https://pubsub.googleapis.com";

// ---------------------------------------------------------------------------
// Token source abstraction
// ---------------------------------------------------------------------------

/// Provides a current OAuth2 bearer access token for Google APIs.
///
/// Implementations are responsible for refresh / rotation; the adapter
/// simply calls `token()` before each HTTP call and uses whatever
/// string comes back.
#[async_trait]
pub trait TokenSource: Send + Sync {
    /// Return the current bearer token (without the `Bearer ` prefix).
    async fn token(&self) -> Result<String, PluginError>;
}

/// Reads the bearer token from the environment variable named by the
/// adapter config. Re-reads on every call so external rotation is
/// picked up automatically.
pub struct EnvTokenSource {
    env_var: String,
}

impl EnvTokenSource {
    pub fn new(env_var: impl Into<String>) -> Self {
        Self {
            env_var: env_var.into(),
        }
    }
}

#[async_trait]
impl TokenSource for EnvTokenSource {
    async fn token(&self) -> Result<String, PluginError> {
        std::env::var(&self.env_var).map_err(|_| {
            PluginError::ExecutionFailed(format!(
                "google_chat: env var `{}` is unset",
                self.env_var
            ))
        })
    }
}

// ---------------------------------------------------------------------------
// Pub/Sub + Chat REST DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct PullRequest {
    #[serde(rename = "maxMessages")]
    max_messages: u32,
}

#[derive(Debug, Deserialize)]
struct PullResponse {
    #[serde(default, rename = "receivedMessages")]
    received_messages: Vec<ReceivedMessage>,
}

#[derive(Debug, Deserialize)]
struct ReceivedMessage {
    #[serde(rename = "ackId")]
    ack_id: String,
    message: PubsubMessage,
}

#[derive(Debug, Deserialize)]
struct PubsubMessage {
    /// Base64-encoded payload (the Google Chat event JSON).
    #[serde(default)]
    data: Option<String>,
    #[serde(rename = "messageId", default)]
    _message_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct AcknowledgeRequest<'a> {
    #[serde(rename = "ackIds")]
    ack_ids: &'a [String],
}

#[derive(Debug, Serialize)]
struct CreateMessageRequest<'a> {
    text: &'a str,
}

#[derive(Debug, Deserialize)]
struct CreateMessageResponse {
    /// `spaces/{space}/messages/{id}.{thread}` — the canonical Chat
    /// message resource identifier returned by
    /// `spaces.messages.create`.
    name: String,
}

/// Decoded Google Chat event (the JSON inside Pub/Sub `data`). We only
/// care about the bits we need to deliver to the agent pipeline; the
/// real schema is much larger (see Google's
/// `chat.googleapis.com/v1/{space}/events`).
#[derive(Debug, Deserialize)]
struct ChatEvent {
    #[serde(default, rename = "type")]
    event_type: Option<String>,
    #[serde(default)]
    space: Option<ChatSpace>,
    #[serde(default)]
    message: Option<ChatMessage>,
    #[serde(default)]
    user: Option<ChatUser>,
}

#[derive(Debug, Deserialize)]
struct ChatSpace {
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatMessage {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    sender: Option<ChatUser>,
}

#[derive(Debug, Deserialize)]
struct ChatUser {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default, rename = "displayName")]
    display_name: Option<String>,
}

// ---------------------------------------------------------------------------
// Adapter
// ---------------------------------------------------------------------------

/// Google Chat channel adapter.
///
/// Real Pub/Sub pull-based inbound and `spaces.messages.create`
/// outbound. See the module-level docs for the runtime contract.
pub struct GoogleChatChannelAdapter {
    config: GoogleChatAdapterConfig,
    http: reqwest::Client,
    tokens: Arc<dyn TokenSource>,
}

impl GoogleChatChannelAdapter {
    /// Create a new adapter with the default
    /// [`EnvTokenSource`] (reads `config.bearer_token_env`).
    pub fn new(config: GoogleChatAdapterConfig) -> Self {
        let env_var = if config.bearer_token_env.is_empty() {
            "GOOGLE_CHAT_ACCESS_TOKEN".to_string()
        } else {
            config.bearer_token_env.clone()
        };
        let tokens: Arc<dyn TokenSource> = Arc::new(EnvTokenSource::new(env_var));
        Self::with_token_source(config, tokens)
    }

    /// Create a new adapter with a caller-supplied [`TokenSource`].
    /// Used by the test suite to inject a static token without
    /// touching the process environment.
    pub fn with_token_source(
        config: GoogleChatAdapterConfig,
        tokens: Arc<dyn TokenSource>,
    ) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            config,
            http,
            tokens,
        }
    }

    /// Check if a user email/id is in the allow list.
    pub fn is_user_allowed(&self, user: &str) -> bool {
        if self.config.allowed_users.is_empty() {
            return true;
        }
        let lower = user.to_lowercase();
        self.config
            .allowed_users
            .iter()
            .any(|u| u.to_lowercase() == lower)
    }

    fn chat_base(&self) -> &str {
        self.config
            .chat_base_url
            .as_deref()
            .unwrap_or(DEFAULT_CHAT_BASE_URL)
    }

    fn pubsub_base(&self) -> &str {
        self.config
            .pubsub_base_url
            .as_deref()
            .unwrap_or(DEFAULT_PUBSUB_BASE_URL)
    }

    // -- Pub/Sub pull cycle ------------------------------------------------

    async fn pull_once(&self) -> Result<Vec<ReceivedMessage>, PluginError> {
        let token = self.tokens.token().await?;
        let url = format!(
            "{}/v1/{}:pull",
            self.pubsub_base(),
            self.config.pubsub_subscription
        );
        let body = PullRequest {
            max_messages: self.config.pull_max_messages.max(1),
        };
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                PluginError::ExecutionFailed(format!(
                    "google_chat: pubsub pull request failed: {e}"
                ))
            })?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(PluginError::ExecutionFailed(format!(
                "google_chat: pubsub pull HTTP {status}: {text}"
            )));
        }
        let parsed: PullResponse = resp.json().await.map_err(|e| {
            PluginError::ExecutionFailed(format!(
                "google_chat: pubsub pull response decode failed: {e}"
            ))
        })?;
        Ok(parsed.received_messages)
    }

    async fn ack(&self, ack_ids: &[String]) -> Result<(), PluginError> {
        if ack_ids.is_empty() {
            return Ok(());
        }
        let token = self.tokens.token().await?;
        let url = format!(
            "{}/v1/{}:acknowledge",
            self.pubsub_base(),
            self.config.pubsub_subscription
        );
        let body = AcknowledgeRequest { ack_ids };
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                PluginError::ExecutionFailed(format!("google_chat: pubsub ack request failed: {e}"))
            })?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(PluginError::ExecutionFailed(format!(
                "google_chat: pubsub ack HTTP {status}: {text}"
            )));
        }
        Ok(())
    }

    /// Decode a Pub/Sub `message.data` field (base64-encoded JSON) into
    /// a [`ChatEvent`].
    fn decode_event(data: &str) -> Result<ChatEvent, PluginError> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(data))
            .map_err(|e| {
                PluginError::ExecutionFailed(format!("google_chat: pubsub data is not base64: {e}"))
            })?;
        serde_json::from_slice::<ChatEvent>(&bytes).map_err(|e| {
            PluginError::ExecutionFailed(format!("google_chat: chat event JSON decode failed: {e}"))
        })
    }

    /// Translate a decoded Chat event into a `deliver_inbound` call.
    /// Returns `Ok(())` even when the event is one we choose to drop
    /// (non-message events, disallowed senders, missing text); the
    /// caller still acks the Pub/Sub message in those cases so we
    /// don't redeliver forever.
    async fn deliver_event(
        &self,
        event: ChatEvent,
        host: &Arc<dyn ChannelAdapterHost>,
    ) -> Result<(), PluginError> {
        // Only `MESSAGE` events carry user-authored text. Treat
        // missing event_type as `MESSAGE` for forward compatibility
        // (older webhook envelopes don't always include it).
        let kind = event.event_type.as_deref().unwrap_or("MESSAGE");
        if kind != "MESSAGE" {
            debug!(event_type = %kind, "google_chat: dropping non-message event");
            return Ok(());
        }

        let message = match event.message {
            Some(m) => m,
            None => {
                debug!("google_chat: MESSAGE event with no `message`, dropping");
                return Ok(());
            }
        };
        let text = match message.text {
            Some(t) if !t.is_empty() => t,
            _ => {
                debug!("google_chat: message has no text, dropping");
                return Ok(());
            }
        };

        // Sender resolution: prefer message.sender, fall back to
        // event.user. Allow-list check uses email when present, else
        // the resource name.
        let sender = message.sender.or(event.user).unwrap_or(ChatUser {
            name: None,
            email: None,
            display_name: None,
        });
        let sender_id = sender
            .email
            .clone()
            .or_else(|| sender.name.clone())
            .unwrap_or_else(|| "unknown".to_string());
        if !self.is_user_allowed(&sender_id) {
            warn!(
                sender = %sender_id,
                "google_chat: sender not in allow-list, dropping"
            );
            return Ok(());
        }

        // Space (chat) id: prefer the event-level space, fall back to
        // the message resource name (`spaces/X/messages/Y`).
        let chat_id = event
            .space
            .as_ref()
            .and_then(|s| s.name.clone())
            .or_else(|| {
                message
                    .name
                    .as_ref()
                    .and_then(|n| n.split('/').take(2).collect::<Vec<_>>().join("/").into())
            })
            .unwrap_or_else(|| "spaces/unknown".to_string());

        let mut metadata: HashMap<String, serde_json::Value> = HashMap::new();
        if let Some(ref msg_name) = message.name {
            metadata.insert("message_name".into(), serde_json::json!(msg_name));
        }
        if let Some(ref display) = sender.display_name {
            metadata.insert("display_name".into(), serde_json::json!(display));
        }
        metadata.insert("space".into(), serde_json::json!(chat_id));

        host.deliver_inbound(
            "google_chat",
            &sender_id,
            &chat_id,
            MessagePayload::text(text),
            metadata,
        )
        .await
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
        true
    }

    fn supports_media(&self) -> bool {
        false
    }

    async fn start(
        &self,
        host: Arc<dyn ChannelAdapterHost>,
        cancel: CancellationToken,
    ) -> Result<(), PluginError> {
        info!(
            project_id = %self.config.project_id,
            subscription = %self.config.pubsub_subscription,
            "google_chat channel adapter starting"
        );

        if self.config.project_id.is_empty() {
            return Err(PluginError::LoadFailed(
                "google_chat adapter: project_id is required".into(),
            ));
        }

        // Send-only mode: no subscription configured. Park until cancel.
        if self.config.pubsub_subscription.is_empty() {
            warn!(
                "google_chat: pubsub_subscription is empty -- inbound \
                 disabled (send-only mode)"
            );
            cancel.cancelled().await;
            info!("google_chat channel adapter shutting down");
            return Ok(());
        }

        let idle_sleep = Duration::from_millis(self.config.pull_idle_ms.max(1));
        let mut backoff = Duration::from_millis(500);
        let max_backoff = Duration::from_secs(30);

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    info!("google_chat channel adapter shutting down");
                    return Ok(());
                }
                pull_result = self.pull_once() => {
                    match pull_result {
                        Ok(messages) => {
                            backoff = Duration::from_millis(500);
                            if messages.is_empty() {
                                tokio::select! {
                                    _ = cancel.cancelled() => {
                                        info!("google_chat channel adapter shutting down");
                                        return Ok(());
                                    }
                                    _ = tokio::time::sleep(idle_sleep) => {}
                                }
                                continue;
                            }

                            let mut ack_ids: Vec<String> = Vec::with_capacity(messages.len());
                            for received in messages {
                                let ack_id = received.ack_id.clone();
                                if let Some(data) = received.message.data.as_deref() {
                                    match Self::decode_event(data) {
                                        Ok(event) => {
                                            if let Err(e) = self.deliver_event(event, &host).await {
                                                error!(error = %e, "google_chat: deliver failed");
                                            }
                                        }
                                        Err(e) => {
                                            warn!(
                                                error = %e,
                                                "google_chat: dropping undecodable pubsub message"
                                            );
                                        }
                                    }
                                } else {
                                    debug!("google_chat: pubsub message had no data, acking anyway");
                                }
                                ack_ids.push(ack_id);
                            }
                            if let Err(e) = self.ack(&ack_ids).await {
                                error!(error = %e, "google_chat: pubsub ack failed");
                            }
                        }
                        Err(e) => {
                            error!(error = %e, "google_chat: pubsub pull failed");
                            tokio::select! {
                                _ = cancel.cancelled() => {
                                    info!("google_chat channel adapter shutting down");
                                    return Ok(());
                                }
                                _ = tokio::time::sleep(backoff) => {}
                            }
                            backoff = (backoff * 2).min(max_backoff);
                        }
                    }
                }
            }
        }
    }

    async fn send(&self, target: &str, payload: &MessagePayload) -> Result<String, PluginError> {
        let content = payload.as_text().ok_or_else(|| {
            PluginError::ExecutionFailed("google_chat: only text payloads supported".into())
        })?;

        // Resolve target space: caller-supplied wins, fall back to
        // configured default. Accept both `spaces/AAAA` and bare
        // `AAAA` forms for ergonomic config.
        let raw_space = if target.is_empty() {
            self.config.default_space_id.as_str()
        } else {
            target
        };
        if raw_space.is_empty() {
            return Err(PluginError::ExecutionFailed(
                "google_chat: send target is empty and no default_space_id configured".into(),
            ));
        }
        let space = if raw_space.starts_with("spaces/") {
            raw_space.to_string()
        } else {
            format!("spaces/{raw_space}")
        };

        let token = self.tokens.token().await?;
        let url = format!("{}/v1/{}/messages", self.chat_base(), space);
        let body = CreateMessageRequest { text: content };

        debug!(space = %space, content_len = content.len(), "google_chat: sending message");

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                PluginError::ExecutionFailed(format!("google_chat: send request failed: {e}"))
            })?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(PluginError::ExecutionFailed(format!(
                "google_chat: send HTTP {status}: {text}"
            )));
        }
        let parsed: CreateMessageResponse = resp.json().await.map_err(|e| {
            PluginError::ExecutionFailed(format!("google_chat: send response decode failed: {e}"))
        })?;
        Ok(parsed.name)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use base64::engine::general_purpose::STANDARD as B64;
    use serde_json::json;
    use tokio::sync::Mutex;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // -- mock host --

    struct MockHost {
        messages: Mutex<Vec<(String, String, String, MessagePayload)>>,
    }
    impl MockHost {
        fn new() -> Self {
            Self {
                messages: Mutex::new(vec![]),
            }
        }
    }
    #[async_trait]
    impl ChannelAdapterHost for MockHost {
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

    // -- static token source --

    struct StaticToken(String);
    #[async_trait]
    impl TokenSource for StaticToken {
        async fn token(&self) -> Result<String, PluginError> {
            Ok(self.0.clone())
        }
    }

    fn base_config() -> GoogleChatAdapterConfig {
        GoogleChatAdapterConfig {
            project_id: "test-project".into(),
            pubsub_subscription: "projects/test-project/subscriptions/chat-events".into(),
            default_space_id: "spaces/AAAA".into(),
            pull_max_messages: 10,
            pull_idle_ms: 10,
            ..Default::default()
        }
    }

    fn build_adapter(config: GoogleChatAdapterConfig) -> GoogleChatChannelAdapter {
        let tokens: Arc<dyn TokenSource> = Arc::new(StaticToken("test-token".to_string()));
        GoogleChatChannelAdapter::with_token_source(config, tokens)
    }

    // -- trait surface --

    #[test]
    fn name_and_display() {
        let adapter = build_adapter(base_config());
        assert_eq!(adapter.name(), "google_chat");
        assert_eq!(adapter.display_name(), "Google Chat");
        assert!(adapter.supports_threads());
        assert!(!adapter.supports_media());
    }

    #[test]
    fn user_allow_list() {
        let mut config = base_config();
        config.allowed_users = vec!["admin@company.com".into()];
        let adapter = build_adapter(config);
        assert!(adapter.is_user_allowed("admin@company.com"));
        assert!(adapter.is_user_allowed("ADMIN@COMPANY.COM"));
        assert!(!adapter.is_user_allowed("stranger@evil.com"));
    }

    #[test]
    fn empty_allow_list_admits_all() {
        let adapter = build_adapter(base_config());
        assert!(adapter.is_user_allowed("anyone@anywhere.com"));
    }

    // -- decode_event --

    #[test]
    fn decode_event_parses_message_envelope() {
        let payload = json!({
            "type": "MESSAGE",
            "space": {"name": "spaces/AAAA"},
            "message": {
                "name": "spaces/AAAA/messages/MID.TID",
                "text": "Hello bot",
                "sender": {
                    "name": "users/123",
                    "email": "alice@example.com",
                    "displayName": "Alice"
                }
            }
        });
        let b64 = B64.encode(payload.to_string().as_bytes());
        let event = GoogleChatChannelAdapter::decode_event(&b64).unwrap();
        assert_eq!(event.event_type.as_deref(), Some("MESSAGE"));
        assert_eq!(
            event.message.as_ref().and_then(|m| m.text.as_deref()),
            Some("Hello bot")
        );
    }

    #[test]
    fn decode_event_rejects_garbage() {
        let err = GoogleChatChannelAdapter::decode_event("$$$not-base64$$$").unwrap_err();
        assert!(err.to_string().contains("base64"));
    }

    // -- start: validation paths --

    #[tokio::test]
    async fn start_requires_project_id() {
        let mut config = base_config();
        config.project_id = String::new();
        let adapter = build_adapter(config);
        let host: Arc<dyn ChannelAdapterHost> = Arc::new(MockHost::new());
        let cancel = CancellationToken::new();
        let result = adapter.start(host, cancel).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("project_id"));
    }

    #[tokio::test]
    async fn start_send_only_mode_parks_until_cancel() {
        let mut config = base_config();
        config.pubsub_subscription = String::new();
        let adapter = build_adapter(config);
        let host: Arc<dyn ChannelAdapterHost> = Arc::new(MockHost::new());
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();
        let handle = tokio::spawn(async move { adapter.start(host, cancel_clone).await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        cancel.cancel();
        let result = handle.await.unwrap();
        assert!(result.is_ok());
    }

    // -- send: real HTTP via wiremock --

    #[tokio::test]
    async fn send_posts_to_chat_api_and_returns_name() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/spaces/AAAA/messages"))
            .and(header("authorization", "Bearer test-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "spaces/AAAA/messages/MID.TID",
                "text": "Hello"
            })))
            .mount(&server)
            .await;

        let mut config = base_config();
        config.chat_base_url = Some(server.uri());
        let adapter = build_adapter(config);

        let payload = MessagePayload::text("Hello");
        let id = adapter.send("spaces/AAAA", &payload).await.unwrap();
        assert_eq!(id, "spaces/AAAA/messages/MID.TID");
    }

    #[tokio::test]
    async fn send_accepts_bare_space_id_and_uses_default() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/spaces/BBBB/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "spaces/BBBB/messages/Z.Z"
            })))
            .mount(&server)
            .await;

        let mut config = base_config();
        config.chat_base_url = Some(server.uri());
        let adapter = build_adapter(config);

        // Caller passes a bare id ("BBBB") -- adapter prepends "spaces/".
        let id = adapter
            .send("BBBB", &MessagePayload::text("hi"))
            .await
            .unwrap();
        assert_eq!(id, "spaces/BBBB/messages/Z.Z");
    }

    #[tokio::test]
    async fn send_uses_default_space_when_target_empty() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/spaces/AAAA/messages")) // default in base_config
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "spaces/AAAA/messages/D.D"
            })))
            .mount(&server)
            .await;

        let mut config = base_config();
        config.chat_base_url = Some(server.uri());
        let adapter = build_adapter(config);

        let id = adapter
            .send("", &MessagePayload::text("default"))
            .await
            .unwrap();
        assert_eq!(id, "spaces/AAAA/messages/D.D");
    }

    #[tokio::test]
    async fn send_propagates_http_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/spaces/AAAA/messages"))
            .respond_with(ResponseTemplate::new(401).set_body_string("nope"))
            .mount(&server)
            .await;

        let mut config = base_config();
        config.chat_base_url = Some(server.uri());
        let adapter = build_adapter(config);
        let result = adapter
            .send("spaces/AAAA", &MessagePayload::text("x"))
            .await;
        let err = result.unwrap_err().to_string();
        assert!(err.contains("401"), "expected 401 in error: {err}");
    }

    #[tokio::test]
    async fn send_rejects_non_text_payload() {
        let adapter = build_adapter(base_config());
        let payload = MessagePayload::structured(json!({"x": 1}));
        let result = adapter.send("spaces/AAAA", &payload).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn send_without_target_or_default_fails() {
        let mut config = base_config();
        config.default_space_id = String::new();
        let adapter = build_adapter(config);
        let result = adapter.send("", &MessagePayload::text("x")).await;
        let err = result.unwrap_err().to_string();
        assert!(err.contains("empty"), "expected empty-target error: {err}");
    }

    // -- pull + deliver round-trip via wiremock --

    #[tokio::test]
    async fn pull_loop_delivers_message_and_acks() {
        let server = MockServer::start().await;

        let chat_event = json!({
            "type": "MESSAGE",
            "space": {"name": "spaces/AAAA"},
            "message": {
                "name": "spaces/AAAA/messages/MID.TID",
                "text": "Ping from user",
                "sender": {
                    "name": "users/123",
                    "email": "alice@example.com",
                    "displayName": "Alice"
                }
            }
        });
        let data_b64 = B64.encode(chat_event.to_string().as_bytes());

        // First pull -> one message. Second pull (after ack) -> empty,
        // and we'll cancel before any further iteration.
        let pull_path = "/v1/projects/test-project/subscriptions/chat-events:pull";
        let ack_path = "/v1/projects/test-project/subscriptions/chat-events:acknowledge";

        Mock::given(method("POST"))
            .and(path(pull_path))
            .and(header("authorization", "Bearer test-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "receivedMessages": [{
                    "ackId": "ack-001",
                    "message": {
                        "data": data_b64,
                        "messageId": "psm-001"
                    }
                }]
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;

        // Subsequent pulls return empty so the loop idles until cancel.
        Mock::given(method("POST"))
            .and(path(pull_path))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "receivedMessages": []
            })))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path(ack_path))
            .and(header("authorization", "Bearer test-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;

        let mut config = base_config();
        config.pubsub_base_url = Some(server.uri());
        let adapter = build_adapter(config);

        let mock_host = Arc::new(MockHost::new());
        let host: Arc<dyn ChannelAdapterHost> = mock_host.clone();
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();
        let handle = tokio::spawn(async move { adapter.start(host, cancel_clone).await });

        // Give the loop time to pull once + ack.
        tokio::time::sleep(Duration::from_millis(200)).await;
        cancel.cancel();
        handle.await.unwrap().unwrap();

        let msgs = mock_host.messages.lock().await;
        assert_eq!(msgs.len(), 1, "expected exactly one delivered message");
        let (channel, sender, chat, payload) = &msgs[0];
        assert_eq!(channel, "google_chat");
        assert_eq!(sender, "alice@example.com");
        assert_eq!(chat, "spaces/AAAA");
        assert_eq!(payload.as_text(), Some("Ping from user"));

        // Confirm an ack was issued.
        let acks = server
            .received_requests()
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.url.path() == ack_path)
            .count();
        assert!(acks >= 1, "expected at least one ack call, got {acks}");
    }

    #[tokio::test]
    async fn pull_loop_filters_disallowed_sender() {
        let server = MockServer::start().await;
        let chat_event = json!({
            "type": "MESSAGE",
            "space": {"name": "spaces/AAAA"},
            "message": {
                "name": "spaces/AAAA/messages/MID",
                "text": "Hi",
                "sender": {"email": "stranger@evil.com"}
            }
        });
        let data_b64 = B64.encode(chat_event.to_string().as_bytes());
        let pull_path = "/v1/projects/test-project/subscriptions/chat-events:pull";
        let ack_path = "/v1/projects/test-project/subscriptions/chat-events:acknowledge";

        Mock::given(method("POST"))
            .and(path(pull_path))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "receivedMessages": [{
                    "ackId": "ack-evil",
                    "message": {"data": data_b64}
                }]
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(pull_path))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"receivedMessages": []})))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(ack_path))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;

        let mut config = base_config();
        config.pubsub_base_url = Some(server.uri());
        config.allowed_users = vec!["admin@company.com".into()];
        let adapter = build_adapter(config);

        let mock_host = Arc::new(MockHost::new());
        let host: Arc<dyn ChannelAdapterHost> = mock_host.clone();
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();
        let handle = tokio::spawn(async move { adapter.start(host, cancel_clone).await });
        tokio::time::sleep(Duration::from_millis(200)).await;
        cancel.cancel();
        handle.await.unwrap().unwrap();

        // Disallowed sender should not produce a delivered message,
        // but the Pub/Sub message is still acked.
        assert_eq!(mock_host.messages.lock().await.len(), 0);
    }

    // -- env-based token source --

    #[tokio::test]
    async fn env_token_source_reads_var() {
        // SAFETY: process-wide env. We use a unique name so concurrent
        // tests don't collide.
        let var = "GOOGLE_CHAT_TEST_TOKEN_ABCDEF";
        unsafe {
            std::env::set_var(var, "live-token");
        }
        let src = EnvTokenSource::new(var);
        assert_eq!(src.token().await.unwrap(), "live-token");
        unsafe {
            std::env::remove_var(var);
        }
    }

    #[tokio::test]
    async fn env_token_source_missing_var_errors() {
        let var = "GOOGLE_CHAT_TEST_TOKEN_NEVER_SET_QWERTY";
        unsafe {
            std::env::remove_var(var);
        }
        let src = EnvTokenSource::new(var);
        let err = src.token().await.unwrap_err().to_string();
        assert!(err.contains("unset"));
    }
}
