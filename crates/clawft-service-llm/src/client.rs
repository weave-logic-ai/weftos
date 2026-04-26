//! HTTP client for an OpenAI-compatible chat-completions endpoint
//! (typically a local `llama-server`).
//!
//! Mirrors `clawft-service-whisper`'s `client.rs` shape:
//!
//! - [`LlmClient::health`]   — `GET /health` once.
//! - [`LlmClient::complete`] — `POST /v1/chat/completions` with the
//!   in-flight semaphore (permits=1) so we never pipeline a second
//!   request against `llama-server`'s single-batch core and stall on
//!   the OS accept backlog.
//!
//! No substrate or kernel knowledge — that lives in `clawft-weave`.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::Semaphore;
use tracing::{debug, warn};

use crate::{DEFAULT_LLM_MODEL, DEFAULT_LLM_SERVICE_URL, LLM_SERVICE_URL_ENV};

/// Configuration for [`LlmClient`].
#[derive(Debug, Clone)]
pub struct LlmConfig {
    /// Base URL of the LLM service (e.g. `http://127.0.0.1:8111`).
    /// No trailing slash.
    pub base_url: String,
    /// Model name to send in the request body. `llama-server` ignores
    /// this for routing (it serves whatever model it loaded), but it's
    /// echoed in the response and shows up in logs.
    pub model: String,
    /// Request timeout for a single completion. Generation can take
    /// many seconds for large outputs; default 120s covers up to a
    /// few hundred tokens on a moderately quantized 35B at low
    /// throughput.
    pub request_timeout: Duration,
    /// How long [`LlmClient::wait_for_healthy`] will poll before
    /// giving up.
    pub health_deadline: Duration,
    /// Default sampling temperature when the per-call value is `None`.
    pub default_temperature: f32,
    /// Default `max_tokens` when the per-call value is `None`.
    /// Set deliberately conservative so a forgotten cap doesn't pin
    /// the server for minutes on a single request.
    pub default_max_tokens: u32,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_LLM_SERVICE_URL.to_string(),
            model: DEFAULT_LLM_MODEL.to_string(),
            request_timeout: Duration::from_secs(120),
            health_deadline: Duration::from_secs(10),
            default_temperature: 0.2,
            default_max_tokens: 512,
        }
    }
}

impl LlmConfig {
    /// Build a config honoring the `LLM_SERVICE_URL` env var.
    ///
    /// Falls back to [`DEFAULT_LLM_SERVICE_URL`] if unset or empty.
    pub fn from_env() -> Self {
        let base_url = std::env::var(LLM_SERVICE_URL_ENV)
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_LLM_SERVICE_URL.to_string());
        Self {
            base_url,
            ..Default::default()
        }
    }
}

/// One message in a chat completion conversation.
///
/// `role` is one of `"system"`, `"user"`, `"assistant"`. We don't
/// gate on the value here — the server validates and rejects.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    /// Role: `system` / `user` / `assistant`.
    pub role: String,
    /// Message content.
    pub content: String,
}

impl ChatMessage {
    /// Convenience constructor for a system prompt.
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            content: content.into(),
        }
    }
    /// Convenience constructor for a user turn.
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: content.into(),
        }
    }
    /// Convenience constructor for an assistant turn.
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".into(),
            content: content.into(),
        }
    }
}

/// Wire shape for `POST /v1/chat/completions` request body.
#[derive(Debug, Clone, Serialize)]
pub struct ChatRequest {
    /// Model name (echoed; not used for routing on llama-server).
    pub model: String,
    /// Conversation so far. The last entry should normally be `user`.
    pub messages: Vec<ChatMessage>,
    /// Sampling temperature.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Hard cap on generated tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Streaming flag. Always `false` for [`LlmClient::complete`];
    /// streaming would land as a separate method that flips this to
    /// `true` and parses SSE.
    pub stream: bool,
}

/// One choice in a chat completion response.
#[derive(Debug, Clone, Deserialize)]
pub struct ChatChoice {
    /// The assistant message produced for this choice.
    pub message: ChatMessage,
    /// `"stop"`, `"length"`, etc. Optional because some servers omit
    /// it on early returns.
    #[serde(default)]
    pub finish_reason: Option<String>,
}

/// Token-usage block from the response.
///
/// All fields default to 0 because some OpenAI-compat servers omit
/// the block entirely when token counts aren't tracked.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ChatUsage {
    /// Tokens in the prompt.
    #[serde(default)]
    pub prompt_tokens: u32,
    /// Tokens generated.
    #[serde(default)]
    pub completion_tokens: u32,
    /// Sum.
    #[serde(default)]
    pub total_tokens: u32,
}

/// Wire shape for `POST /v1/chat/completions` response body.
#[derive(Debug, Clone, Deserialize)]
pub struct ChatResponse {
    /// One or more completion choices. Practically always one for
    /// llama-server.
    pub choices: Vec<ChatChoice>,
    /// Token-usage block. May be absent on some servers.
    #[serde(default)]
    pub usage: ChatUsage,
    /// Echoed model name (best-effort; some servers omit).
    #[serde(default)]
    pub model: Option<String>,
}

/// Errors emitted by the LLM HTTP client.
#[derive(Debug, Error)]
pub enum LlmError {
    /// Underlying HTTP transport failure (DNS, connect, timeout, TLS).
    #[error("llm http transport: {0}")]
    Transport(String),
    /// Service returned 5xx (idempotent at low T — safe to retry once).
    #[error("llm service {status}: {body}")]
    Server {
        /// HTTP status code.
        status: u16,
        /// Response body (plain text from the server).
        body: String,
    },
    /// Service returned 4xx (malformed request — don't retry).
    #[error("llm client error {status}: {body}")]
    ClientError {
        /// HTTP status code.
        status: u16,
        /// Response body.
        body: String,
    },
    /// Service returned 503 `{"status":"loading model"}` — retry with
    /// backoff, mirrors whisper's contract.
    #[error("llm service loading")]
    Loading,
    /// Response body was not JSON in the expected shape.
    #[error("llm response malformed: {0}")]
    Malformed(String),
    /// Response carried an empty `choices` array — we have nothing to
    /// surface back to the caller.
    #[error("llm response had no choices")]
    NoChoices,
}

impl LlmError {
    /// Whether a caller should retry automatically.
    pub fn is_retriable(&self) -> bool {
        matches!(
            self,
            LlmError::Transport(_) | LlmError::Server { .. } | LlmError::Loading
        )
    }
}

/// HTTP client for the LLM service.
#[derive(Debug, Clone)]
pub struct LlmClient {
    config: LlmConfig,
    http: reqwest::Client,
    /// Backpressure: permits=1 matches `llama-server`'s single-batch
    /// processing core. We enforce it client-side so we never pipeline
    /// a second request against the server's busy slot.
    in_flight: Arc<Semaphore>,
}

impl LlmClient {
    /// Build a client with the supplied config.
    pub fn new(config: LlmConfig) -> Result<Self, LlmError> {
        let http = reqwest::Client::builder()
            .timeout(config.request_timeout)
            .build()
            .map_err(|e| LlmError::Transport(e.to_string()))?;
        Ok(Self {
            config,
            http,
            in_flight: Arc::new(Semaphore::new(1)),
        })
    }

    /// Read-only accessor; used by the daemon's wiring + structured logs.
    pub fn config(&self) -> &LlmConfig {
        &self.config
    }

    /// Hit `GET /health` exactly once.
    ///
    /// Returns `Ok(true)` on 200, `Ok(false)` on 503 (loading model),
    /// or `Err` on transport / other failure.
    pub async fn health(&self) -> Result<bool, LlmError> {
        let url = format!("{}/health", self.config.base_url);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| LlmError::Transport(e.to_string()))?;
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| LlmError::Transport(e.to_string()))?;
        if status.is_success() {
            Ok(true)
        } else if status.as_u16() == 503 {
            Ok(false)
        } else {
            Err(LlmError::Server {
                status: status.as_u16(),
                body,
            })
        }
    }

    /// Poll `/health` until it returns ready or the deadline elapses.
    ///
    /// Returns `Ok(true)` when ready, `Ok(false)` on timeout — **not**
    /// an error, so the daemon can degrade gracefully (RPC calls will
    /// then surface a clear "service not ready" error per request).
    pub async fn wait_for_healthy(&self) -> bool {
        let start = tokio::time::Instant::now();
        let mut backoff = Duration::from_millis(200);
        while start.elapsed() < self.config.health_deadline {
            match self.health().await {
                Ok(true) => return true,
                Ok(false) => debug!("llm: service still loading model"),
                Err(e) => debug!(error = %e, "llm: health check failed"),
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_secs(2));
        }
        warn!(
            base_url = %self.config.base_url,
            deadline_ms = self.config.health_deadline.as_millis() as u64,
            "llm: health probe timeout — service will run in degraded mode"
        );
        false
    }

    /// POST a chat completion request and return the parsed response.
    ///
    /// Acquires the in-flight semaphore so concurrent callers serialize
    /// rather than racing the server's single-batch slot.
    ///
    /// The caller controls `temperature` and `max_tokens` per call;
    /// passing `None` uses the config defaults.
    pub async fn complete(
        &self,
        messages: Vec<ChatMessage>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
    ) -> Result<ChatResponse, LlmError> {
        let _permit = self
            .in_flight
            .acquire()
            .await
            .map_err(|_| LlmError::Transport("llm in-flight semaphore closed".into()))?;
        self.complete_unchecked(messages, temperature, max_tokens)
            .await
    }

    /// Send the request without acquiring the permit. Public for test
    /// harnesses that exercise the wire format without serialization;
    /// production code paths MUST use [`Self::complete`].
    pub async fn complete_unchecked(
        &self,
        messages: Vec<ChatMessage>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
    ) -> Result<ChatResponse, LlmError> {
        let url = format!("{}/v1/chat/completions", self.config.base_url);

        let body = ChatRequest {
            model: self.config.model.clone(),
            messages,
            temperature: Some(temperature.unwrap_or(self.config.default_temperature)),
            max_tokens: Some(max_tokens.unwrap_or(self.config.default_max_tokens)),
            stream: false,
        };

        let resp = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::Transport(e.to_string()))?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| LlmError::Transport(e.to_string()))?;

        if status.as_u16() == 503 {
            return Err(LlmError::Loading);
        }
        if !status.is_success() {
            let code = status.as_u16();
            if (400..500).contains(&code) {
                return Err(LlmError::ClientError {
                    status: code,
                    body: text,
                });
            }
            return Err(LlmError::Server {
                status: code,
                body: text,
            });
        }

        let parsed: ChatResponse = serde_json::from_str(&text).map_err(|e| {
            LlmError::Malformed(format!("body was not ChatResponse JSON ({e}): {text}"))
        })?;
        if parsed.choices.is_empty() {
            return Err(LlmError::NoChoices);
        }
        Ok(parsed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn test_config(url: String) -> LlmConfig {
        LlmConfig {
            base_url: url,
            request_timeout: Duration::from_secs(5),
            health_deadline: Duration::from_secs(2),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn health_ok_when_service_ready() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"status":"ok"}"#))
            .mount(&server)
            .await;
        let client = LlmClient::new(test_config(server.uri())).unwrap();
        assert!(client.health().await.unwrap());
    }

    #[tokio::test]
    async fn health_false_when_loading() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(
                ResponseTemplate::new(503).set_body_string(r#"{"status":"loading model"}"#),
            )
            .mount(&server)
            .await;
        let client = LlmClient::new(test_config(server.uri())).unwrap();
        assert!(!client.health().await.unwrap());
    }

    #[tokio::test]
    async fn health_errors_on_5xx_non_503() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
            .mount(&server)
            .await;
        let client = LlmClient::new(test_config(server.uri())).unwrap();
        let err = client.health().await.unwrap_err();
        assert!(matches!(err, LlmError::Server { .. }));
    }

    #[tokio::test]
    async fn wait_for_healthy_returns_false_on_timeout() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(
                ResponseTemplate::new(503).set_body_string(r#"{"status":"loading model"}"#),
            )
            .mount(&server)
            .await;
        let client = LlmClient::new(test_config(server.uri())).unwrap();
        let ok = client.wait_for_healthy().await;
        assert!(!ok);
    }

    #[tokio::test]
    async fn wait_for_healthy_returns_true_when_ready() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"status":"ok"}"#))
            .mount(&server)
            .await;
        let client = LlmClient::new(test_config(server.uri())).unwrap();
        assert!(client.wait_for_healthy().await);
    }

    #[tokio::test]
    async fn complete_happy_path_returns_assistant_text() {
        let server = MockServer::start().await;
        let body = r#"{
            "id": "chatcmpl-1",
            "object": "chat.completion",
            "created": 0,
            "model": "Qwen3.6-35B",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "hello back"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7}
        }"#;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(&server)
            .await;
        let client = LlmClient::new(test_config(server.uri())).unwrap();
        let r = client
            .complete(vec![ChatMessage::user("hi")], None, None)
            .await
            .unwrap();
        assert_eq!(r.choices.len(), 1);
        assert_eq!(r.choices[0].message.role, "assistant");
        assert_eq!(r.choices[0].message.content, "hello back");
        assert_eq!(r.usage.completion_tokens, 2);
    }

    #[tokio::test]
    async fn complete_handles_missing_usage_block() {
        // Some OpenAI-compat servers omit `usage`; we must default it.
        let server = MockServer::start().await;
        let body = r#"{
            "choices": [{
                "message": {"role": "assistant", "content": "ok"}
            }]
        }"#;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(&server)
            .await;
        let client = LlmClient::new(test_config(server.uri())).unwrap();
        let r = client
            .complete(vec![ChatMessage::user("hi")], None, None)
            .await
            .unwrap();
        assert_eq!(r.choices[0].message.content, "ok");
        assert_eq!(r.usage.total_tokens, 0);
    }

    #[tokio::test]
    async fn complete_empty_choices_is_explicit_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"choices": []}"#))
            .mount(&server)
            .await;
        let client = LlmClient::new(test_config(server.uri())).unwrap();
        let err = client
            .complete(vec![ChatMessage::user("hi")], None, None)
            .await
            .unwrap_err();
        assert!(matches!(err, LlmError::NoChoices));
    }

    #[tokio::test]
    async fn complete_bubbles_loading() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(503).set_body_string(r#"{"status":"loading model"}"#),
            )
            .mount(&server)
            .await;
        let client = LlmClient::new(test_config(server.uri())).unwrap();
        let err = client
            .complete(vec![ChatMessage::user("hi")], None, None)
            .await
            .unwrap_err();
        assert!(matches!(err, LlmError::Loading));
        assert!(err.is_retriable());
    }

    #[tokio::test]
    async fn complete_client_error_not_retriable() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(400).set_body_string(r#"{"error":"bad request"}"#),
            )
            .mount(&server)
            .await;
        let client = LlmClient::new(test_config(server.uri())).unwrap();
        let err = client
            .complete(vec![ChatMessage::user("hi")], None, None)
            .await
            .unwrap_err();
        assert!(matches!(err, LlmError::ClientError { status: 400, .. }));
        assert!(!err.is_retriable());
    }

    #[tokio::test]
    async fn complete_server_error_retriable() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(502).set_body_string("bad gateway"))
            .mount(&server)
            .await;
        let client = LlmClient::new(test_config(server.uri())).unwrap();
        let err = client
            .complete(vec![ChatMessage::user("hi")], None, None)
            .await
            .unwrap_err();
        assert!(err.is_retriable());
    }

    #[tokio::test]
    async fn complete_malformed_json_fails() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
            .mount(&server)
            .await;
        let client = LlmClient::new(test_config(server.uri())).unwrap();
        let err = client
            .complete(vec![ChatMessage::user("hi")], None, None)
            .await
            .unwrap_err();
        assert!(matches!(err, LlmError::Malformed(_)));
    }

    #[tokio::test]
    async fn in_flight_semaphore_is_one() {
        let client = LlmClient::new(test_config("http://unused".into())).unwrap();
        assert_eq!(client.in_flight.available_permits(), 1);
    }

    #[test]
    fn config_from_env_uses_env_var() {
        // SAFETY: set_var/remove_var are unsafe on Rust 2024 due to
        // races with reader threads; this test is single-threaded.
        unsafe { std::env::set_var(LLM_SERVICE_URL_ENV, "http://other.host:9000") };
        let c = LlmConfig::from_env();
        assert_eq!(c.base_url, "http://other.host:9000");
        unsafe { std::env::remove_var(LLM_SERVICE_URL_ENV) };
        let c2 = LlmConfig::from_env();
        assert_eq!(c2.base_url, DEFAULT_LLM_SERVICE_URL);
    }

    #[test]
    fn chat_message_role_constructors() {
        assert_eq!(ChatMessage::system("s").role, "system");
        assert_eq!(ChatMessage::user("u").role, "user");
        assert_eq!(ChatMessage::assistant("a").role, "assistant");
    }
}
