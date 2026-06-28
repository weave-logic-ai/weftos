//! WhatsApp channel adapter implementation.
//!
//! Implements [`ChannelAdapter`] for WhatsApp Cloud API communication:
//!
//! - Inbound: an `axum` HTTP server listens on
//!   [`WhatsAppAdapterConfig::webhook_bind_addr`]. `GET /webhook`
//!   handles the Meta verification handshake (`hub.mode=subscribe`,
//!   matching `hub.verify_token`, echoing `hub.challenge`).
//!   `POST /webhook` verifies the `X-Hub-Signature-256` header against
//!   the configured app secret using HMAC-SHA256, parses the inbound
//!   message envelope, and delivers each text message onto the agent
//!   pipeline via [`ChannelAdapterHost`].
//! - Outbound: [`ChannelAdapter::send`] POSTs to
//!   `{api_url}/{api_version}/{phone_number_id}/messages` with the
//!   Cloud API payload `{messaging_product, to, type, text}`. The
//!   response's `messages[0].id` (the `wamid.*`) is returned to the
//!   caller.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    Router,
    body::Bytes,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use clawft_plugin::traits::CancellationToken;
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;
use tracing::{debug, error, info, warn};

use clawft_plugin::error::PluginError;
use clawft_plugin::message::MessagePayload;
use clawft_plugin::traits::{ChannelAdapter, ChannelAdapterHost};

use super::types::WhatsAppAdapterConfig;

type HmacSha256 = Hmac<Sha256>;

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

    /// Verify the Meta `X-Hub-Signature-256` header value against `body`
    /// using HMAC-SHA256(`app_secret`, body). The header is expected to
    /// look like `sha256=<hex digest>`.
    pub fn verify_signature(app_secret: &str, header: &str, body: &[u8]) -> bool {
        let Some(hex_digest) = header.strip_prefix("sha256=") else {
            return false;
        };
        let Ok(provided) = hex::decode(hex_digest) else {
            return false;
        };
        let Ok(mut mac) = HmacSha256::new_from_slice(app_secret.as_bytes()) else {
            return false;
        };
        mac.update(body);
        mac.verify_slice(&provided).is_ok()
    }
}

// ---------------------------------------------------------------------------
// Webhook envelope types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct VerifyParams {
    #[serde(rename = "hub.mode")]
    mode: Option<String>,
    #[serde(rename = "hub.verify_token")]
    verify_token: Option<String>,
    #[serde(rename = "hub.challenge")]
    challenge: Option<String>,
}

/// Inbound webhook envelope. We only consume the fields we need; the
/// rest is ignored to keep this resilient against Meta adding fields.
#[derive(Debug, Deserialize)]
struct WebhookEnvelope {
    #[serde(default)]
    entry: Vec<EntryItem>,
}

#[derive(Debug, Deserialize)]
struct EntryItem {
    #[serde(default)]
    changes: Vec<ChangeItem>,
}

#[derive(Debug, Deserialize)]
struct ChangeItem {
    #[serde(default)]
    value: ChangeValue,
}

#[derive(Debug, Default, Deserialize)]
struct ChangeValue {
    #[serde(default)]
    messages: Vec<InboundMessage>,
}

#[derive(Debug, Deserialize)]
struct InboundMessage {
    /// `wamid.*` -- WhatsApp message id.
    #[allow(dead_code)]
    id: Option<String>,
    /// Sender's phone number (E.164 without the leading `+`).
    from: Option<String>,
    /// `text`, `image`, `audio`, ...
    #[serde(rename = "type")]
    msg_type: Option<String>,
    text: Option<TextBody>,
}

#[derive(Debug, Deserialize)]
struct TextBody {
    body: Option<String>,
}

// ---------------------------------------------------------------------------
// Server state
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct WebhookState {
    verify_token: String,
    app_secret: String,
    host: Arc<dyn ChannelAdapterHost>,
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
        host: Arc<dyn ChannelAdapterHost>,
        cancel: CancellationToken,
    ) -> Result<(), PluginError> {
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
        if self.config.verify_token.is_empty() {
            return Err(PluginError::LoadFailed(
                "whatsapp adapter: verify_token is required".into(),
            ));
        }
        if self.config.app_secret.is_empty() {
            return Err(PluginError::LoadFailed(
                "whatsapp adapter: app_secret is required".into(),
            ));
        }

        let state = WebhookState {
            verify_token: self.config.verify_token.expose().to_string(),
            app_secret: self.config.app_secret.expose().to_string(),
            host,
        };

        let app = build_router(state);

        let listener = tokio::net::TcpListener::bind(&self.config.webhook_bind_addr)
            .await
            .map_err(|e| {
                PluginError::LoadFailed(format!(
                    "whatsapp adapter: bind {} failed: {}",
                    self.config.webhook_bind_addr, e
                ))
            })?;
        let local = listener
            .local_addr()
            .map(|a| a.to_string())
            .unwrap_or_else(|_| self.config.webhook_bind_addr.clone());
        info!(addr = %local, "whatsapp webhook listening");

        let cancel_for_shutdown = cancel.clone();
        let serve_result = axum::serve(listener, app)
            .with_graceful_shutdown(async move { cancel_for_shutdown.cancelled().await })
            .await;

        match serve_result {
            Ok(()) => {
                info!("WhatsApp channel adapter shutting down");
                Ok(())
            }
            Err(e) => {
                error!(error = %e, "whatsapp webhook server error");
                Err(PluginError::ExecutionFailed(format!(
                    "whatsapp webhook serve failed: {e}"
                )))
            }
        }
    }

    async fn send(&self, target: &str, payload: &MessagePayload) -> Result<String, PluginError> {
        let content = payload.as_text().ok_or_else(|| {
            PluginError::ExecutionFailed("whatsapp: only text payloads supported".into())
        })?;

        if self.config.phone_number_id.is_empty() {
            return Err(PluginError::ExecutionFailed(
                "whatsapp: phone_number_id not configured".into(),
            ));
        }
        if self.config.access_token.is_empty() {
            return Err(PluginError::ExecutionFailed(
                "whatsapp: access_token not configured".into(),
            ));
        }

        let url = format!(
            "{}/{}/{}/messages",
            self.config.api_url.trim_end_matches('/'),
            self.config.api_version,
            self.config.phone_number_id,
        );
        let body = serde_json::json!({
            "messaging_product": "whatsapp",
            "to": target,
            "type": "text",
            "text": { "body": content },
        });

        debug!(to = %target, content_len = content.len(), "POST whatsapp message");

        let resp = reqwest::Client::new()
            .post(&url)
            .bearer_auth(self.config.access_token.expose())
            .json(&body)
            .send()
            .await
            .map_err(|e| PluginError::ExecutionFailed(format!("whatsapp send: HTTP error: {e}")))?;

        let status = resp.status();
        let body_text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(PluginError::ExecutionFailed(format!(
                "whatsapp send: status {status}: {body_text}"
            )));
        }

        let parsed: serde_json::Value = serde_json::from_str(&body_text).map_err(|e| {
            PluginError::ExecutionFailed(format!(
                "whatsapp send: malformed response: {e}: {body_text}"
            ))
        })?;
        let id = parsed
            .get("messages")
            .and_then(|m| m.get(0))
            .and_then(|m| m.get("id"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                PluginError::ExecutionFailed(format!(
                    "whatsapp send: response missing messages[0].id: {body_text}"
                ))
            })?
            .to_string();

        Ok(id)
    }
}

// ---------------------------------------------------------------------------
// HTTP routes
// ---------------------------------------------------------------------------

fn build_router(state: WebhookState) -> Router {
    Router::new()
        .route("/webhook", get(handle_verify).post(handle_inbound))
        .with_state(state)
}

async fn handle_verify(
    State(state): State<WebhookState>,
    Query(params): Query<VerifyParams>,
) -> Response {
    if params.mode.as_deref() != Some("subscribe") {
        return (StatusCode::BAD_REQUEST, "bad mode").into_response();
    }
    if params.verify_token.as_deref() != Some(state.verify_token.as_str()) {
        warn!("whatsapp webhook: verify_token mismatch on GET /webhook");
        return (StatusCode::FORBIDDEN, "verify_token mismatch").into_response();
    }
    let challenge = params.challenge.unwrap_or_default();
    (StatusCode::OK, challenge).into_response()
}

async fn handle_inbound(
    State(state): State<WebhookState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let sig = match headers
        .get("x-hub-signature-256")
        .and_then(|v| v.to_str().ok())
    {
        Some(s) => s,
        None => {
            warn!("whatsapp webhook: missing X-Hub-Signature-256");
            return (StatusCode::UNAUTHORIZED, "missing signature").into_response();
        }
    };

    if !WhatsAppChannelAdapter::verify_signature(&state.app_secret, sig, &body) {
        warn!("whatsapp webhook: signature verification failed");
        return (StatusCode::UNAUTHORIZED, "bad signature").into_response();
    }

    let envelope: WebhookEnvelope = match serde_json::from_slice(&body) {
        Ok(e) => e,
        Err(e) => {
            warn!(error = %e, "whatsapp webhook: malformed body");
            return (StatusCode::BAD_REQUEST, "bad body").into_response();
        }
    };

    for entry in envelope.entry {
        for change in entry.changes {
            for msg in change.value.messages {
                let from = match msg.from {
                    Some(f) if !f.is_empty() => f,
                    _ => continue,
                };
                // We currently only deliver text bodies onto the bus.
                if msg.msg_type.as_deref() != Some("text") {
                    debug!(
                        msg_type = msg.msg_type.as_deref().unwrap_or("unknown"),
                        "whatsapp webhook: skipping non-text inbound"
                    );
                    continue;
                }
                let text = msg.text.and_then(|t| t.body).unwrap_or_default();
                if text.is_empty() {
                    continue;
                }

                let mut metadata = HashMap::new();
                if let Some(id) = msg.id.clone() {
                    metadata.insert("wamid".to_string(), serde_json::json!(id));
                }

                if let Err(e) = state
                    .host
                    .deliver_inbound(
                        "whatsapp",
                        &from,
                        &from,
                        MessagePayload::text(text),
                        metadata,
                    )
                    .await
                {
                    error!(error = %e, "whatsapp webhook: deliver_inbound failed");
                }
            }
        }
    }

    (StatusCode::OK, "EVENT_RECEIVED").into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawft_types::secret::SecretString;
    use std::collections::HashMap;
    use tokio::sync::Mutex;

    fn make_config() -> WhatsAppAdapterConfig {
        WhatsAppAdapterConfig {
            phone_number_id: "12345".into(),
            access_token: SecretString::new("test-token"),
            verify_token: SecretString::new("verify-me"),
            app_secret: SecretString::new("app-secret"),
            ..Default::default()
        }
    }

    // -- Mock host -------------------------------------------------------

    #[derive(Default)]
    struct MockHost {
        delivered: Mutex<
            Vec<(
                String,
                String,
                String,
                MessagePayload,
                HashMap<String, serde_json::Value>,
            )>,
        >,
    }

    #[async_trait]
    impl ChannelAdapterHost for MockHost {
        async fn deliver_inbound(
            &self,
            channel: &str,
            sender_id: &str,
            chat_id: &str,
            payload: MessagePayload,
            metadata: HashMap<String, serde_json::Value>,
        ) -> Result<(), PluginError> {
            self.delivered.lock().await.push((
                channel.into(),
                sender_id.into(),
                chat_id.into(),
                payload,
                metadata,
            ));
            Ok(())
        }
    }

    // -- Trait surface ---------------------------------------------------

    #[test]
    fn name_is_whatsapp() {
        let adapter = WhatsAppChannelAdapter::new(make_config());
        assert_eq!(adapter.name(), "whatsapp");
        assert_eq!(adapter.display_name(), "WhatsApp");
        assert!(adapter.supports_media());
        assert!(!adapter.supports_threads());
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

    // -- Signature verify ------------------------------------------------

    fn sign(secret: &str, body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
    }

    #[test]
    fn verify_signature_accepts_valid() {
        let body = b"{\"object\":\"whatsapp_business_account\"}";
        let header = sign("app-secret", body);
        assert!(WhatsAppChannelAdapter::verify_signature(
            "app-secret",
            &header,
            body
        ));
    }

    #[test]
    fn verify_signature_rejects_wrong_secret() {
        let body = b"hello";
        let header = sign("other-secret", body);
        assert!(!WhatsAppChannelAdapter::verify_signature(
            "app-secret",
            &header,
            body
        ));
    }

    #[test]
    fn verify_signature_rejects_tampered_body() {
        let body = b"hello";
        let header = sign("app-secret", body);
        assert!(!WhatsAppChannelAdapter::verify_signature(
            "app-secret",
            &header,
            b"hello-tampered"
        ));
    }

    #[test]
    fn verify_signature_rejects_missing_prefix() {
        let body = b"hello";
        let mut mac = HmacSha256::new_from_slice(b"app-secret").unwrap();
        mac.update(body);
        let bare = hex::encode(mac.finalize().into_bytes());
        // Without the `sha256=` prefix, the verifier should reject.
        assert!(!WhatsAppChannelAdapter::verify_signature(
            "app-secret",
            &bare,
            body
        ));
    }

    #[test]
    fn verify_signature_rejects_non_hex() {
        assert!(!WhatsAppChannelAdapter::verify_signature(
            "app-secret",
            "sha256=not-hex!!",
            b"body"
        ));
    }

    // -- send() via mockito ---------------------------------------------

    #[tokio::test]
    async fn send_text_returns_message_id() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("POST", "/v18.0/12345/messages")
            .match_header("authorization", "Bearer test-token")
            .match_body(mockito::Matcher::PartialJson(serde_json::json!({
                "messaging_product": "whatsapp",
                "to": "+1234567890",
                "type": "text",
                "text": { "body": "Hello" }
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"messaging_product":"whatsapp","contacts":[{"input":"+1234567890","wa_id":"1234567890"}],"messages":[{"id":"wamid.ABC123"}]}"#,
            )
            .create_async()
            .await;

        let mut cfg = make_config();
        cfg.api_url = server.url();
        let adapter = WhatsAppChannelAdapter::new(cfg);
        let payload = MessagePayload::text("Hello");
        let id = adapter.send("+1234567890", &payload).await.unwrap();

        assert_eq!(id, "wamid.ABC123");
        m.assert_async().await;
    }

    #[tokio::test]
    async fn send_propagates_http_error() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/v18.0/12345/messages")
            .with_status(401)
            .with_body(r#"{"error":{"message":"Invalid OAuth"}}"#)
            .create_async()
            .await;

        let mut cfg = make_config();
        cfg.api_url = server.url();
        let adapter = WhatsAppChannelAdapter::new(cfg);
        let result = adapter
            .send("+1234567890", &MessagePayload::text("hi"))
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("401"), "{err}");
    }

    #[tokio::test]
    async fn send_non_text_fails() {
        let adapter = WhatsAppChannelAdapter::new(make_config());
        let payload = MessagePayload::structured(serde_json::json!({}));
        let result = adapter.send("+1234567890", &payload).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn send_without_phone_number_id_fails() {
        let mut cfg = make_config();
        cfg.phone_number_id = String::new();
        let adapter = WhatsAppChannelAdapter::new(cfg);
        let result = adapter
            .send("+1234567890", &MessagePayload::text("hi"))
            .await;
        assert!(result.is_err());
    }

    // -- Webhook router (in-process, no axum::serve) --------------------

    fn build_test_state(host: Arc<dyn ChannelAdapterHost>) -> WebhookState {
        WebhookState {
            verify_token: "verify-me".into(),
            app_secret: "app-secret".into(),
            host,
        }
    }

    #[tokio::test]
    async fn webhook_get_handshake_echoes_challenge() {
        use axum::body::to_bytes;
        use axum::http::Request;
        use tower::ServiceExt;

        let host = Arc::new(MockHost::default()) as Arc<dyn ChannelAdapterHost>;
        let app = build_router(build_test_state(host));

        let req = Request::builder()
            .uri("/webhook?hub.mode=subscribe&hub.verify_token=verify-me&hub.challenge=42")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(body.as_ref(), b"42");
    }

    #[tokio::test]
    async fn webhook_get_handshake_rejects_wrong_token() {
        use axum::http::Request;
        use tower::ServiceExt;

        let host = Arc::new(MockHost::default()) as Arc<dyn ChannelAdapterHost>;
        let app = build_router(build_test_state(host));

        let req = Request::builder()
            .uri("/webhook?hub.mode=subscribe&hub.verify_token=wrong&hub.challenge=42")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn webhook_post_valid_signature_delivers_message() {
        use axum::http::Request;
        use tower::ServiceExt;

        let host = Arc::new(MockHost::default());
        let host_dyn: Arc<dyn ChannelAdapterHost> = host.clone();
        let app = build_router(build_test_state(host_dyn));

        let body = serde_json::json!({
            "object": "whatsapp_business_account",
            "entry": [{
                "id": "BIZ_ID",
                "changes": [{
                    "value": {
                        "messaging_product": "whatsapp",
                        "messages": [{
                            "id": "wamid.HBg",
                            "from": "1234567890",
                            "type": "text",
                            "text": { "body": "Hi bot" }
                        }]
                    },
                    "field": "messages"
                }]
            }]
        })
        .to_string();
        let sig = sign("app-secret", body.as_bytes());

        let req = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header("content-type", "application/json")
            .header("x-hub-signature-256", sig)
            .body(axum::body::Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let delivered = host.delivered.lock().await;
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].0, "whatsapp");
        assert_eq!(delivered[0].1, "1234567890");
        assert_eq!(delivered[0].3.as_text().unwrap(), "Hi bot");
        assert_eq!(
            delivered[0].4.get("wamid"),
            Some(&serde_json::json!("wamid.HBg"))
        );
    }

    #[tokio::test]
    async fn webhook_post_bad_signature_rejected() {
        use axum::http::Request;
        use tower::ServiceExt;

        let host = Arc::new(MockHost::default());
        let host_dyn: Arc<dyn ChannelAdapterHost> = host.clone();
        let app = build_router(build_test_state(host_dyn));

        let body = r#"{"entry":[]}"#;
        // Sign with wrong secret.
        let bad_sig = sign("not-the-secret", body.as_bytes());

        let req = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header("content-type", "application/json")
            .header("x-hub-signature-256", bad_sig)
            .body(axum::body::Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        assert!(host.delivered.lock().await.is_empty());
    }

    #[tokio::test]
    async fn webhook_post_missing_signature_rejected() {
        use axum::http::Request;
        use tower::ServiceExt;

        let host = Arc::new(MockHost::default());
        let host_dyn: Arc<dyn ChannelAdapterHost> = host.clone();
        let app = build_router(build_test_state(host_dyn));

        let req = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(r#"{"entry":[]}"#))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn webhook_post_skips_non_text_messages() {
        use axum::http::Request;
        use tower::ServiceExt;

        let host = Arc::new(MockHost::default());
        let host_dyn: Arc<dyn ChannelAdapterHost> = host.clone();
        let app = build_router(build_test_state(host_dyn));

        let body = serde_json::json!({
            "entry": [{
                "changes": [{
                    "value": {
                        "messages": [{
                            "id": "wamid.IMG",
                            "from": "1234567890",
                            "type": "image"
                        }]
                    }
                }]
            }]
        })
        .to_string();
        let sig = sign("app-secret", body.as_bytes());

        let req = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header("content-type", "application/json")
            .header("x-hub-signature-256", sig)
            .body(axum::body::Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(host.delivered.lock().await.is_empty());
    }

    // -- start() lifecycle ----------------------------------------------

    #[tokio::test]
    async fn start_validates_required_fields() {
        let mut cfg = make_config();
        cfg.phone_number_id = String::new();
        let adapter = WhatsAppChannelAdapter::new(cfg);

        let host: Arc<dyn ChannelAdapterHost> = Arc::new(MockHost::default());
        let cancel = CancellationToken::new();
        let result = adapter.start(host, cancel).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("phone_number_id"));
    }

    #[tokio::test]
    async fn start_serves_and_shuts_down_on_cancel() {
        let adapter = WhatsAppChannelAdapter::new(make_config());
        let host: Arc<dyn ChannelAdapterHost> = Arc::new(MockHost::default());
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        let handle = tokio::spawn(async move { adapter.start(host, cancel_clone).await });
        // Give the listener a moment to bind.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        cancel.cancel();
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), handle)
            .await
            .expect("start did not exit on cancel")
            .unwrap();
        assert!(result.is_ok(), "start returned error: {:?}", result);
    }
}
