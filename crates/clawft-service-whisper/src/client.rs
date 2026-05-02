//! HTTP client for the whisper.cpp transcription service.
//!
//! Implements the consumer side of `whisper-service-api.md`:
//!
//! - [`WhisperClient::health`]    — `GET /health` with retry loop.
//! - [`WhisperClient::transcribe`] — `POST /inference` multipart,
//!   serialised by an internal semaphore (permits=1) so callers never
//!   pipeline two requests into a service that doesn't multiplex.
//!
//! The client has no substrate knowledge — that is entirely in
//! [`crate::service`]. Keeping the split keeps hermetic testing cheap
//! (we point `base_url` at `wiremock::MockServer::uri()`).

use std::sync::Arc;
use std::time::Duration;

use reqwest::multipart::{Form, Part};
use serde::Deserialize;
use thiserror::Error;
use tokio::sync::Semaphore;
use tracing::{debug, warn};

use crate::{DEFAULT_WHISPER_SERVICE_URL, WHISPER_SERVICE_URL_ENV};

/// Configuration for [`WhisperClient`].
#[derive(Debug, Clone)]
pub struct WhisperConfig {
    /// Base URL of the whisper service (e.g. `http://127.0.0.1:8080`).
    /// No trailing slash.
    pub base_url: String,
    /// Request timeout for a single `/inference` POST. Rule of thumb
    /// from the API doc §4: `wall = 1.3 * audio_s / rtfx + 2`. For
    /// 2s windows on an RTX 4070 that's ~2.2s; we allow 30s to cover
    /// cold loads + large-model fallbacks.
    pub request_timeout: Duration,
    /// How long [`WhisperClient::wait_for_healthy`] will poll before
    /// giving up.
    pub health_deadline: Duration,
    /// Whisper `language` form field (ISO 639-1 or `auto`).
    pub language: String,
    /// Whisper `no_context` form field. Defaults to `true` — each
    /// window is independent for streaming; the client (or a future
    /// stitcher stage) owns overlap dedup.
    pub no_context: bool,
    /// Whisper `temperature` — 0 makes retries idempotent per API §7.
    pub temperature: f32,
    /// Skip verbose_json's language-detection pass (cheapest path).
    pub no_language_probabilities: bool,
}

impl Default for WhisperConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_WHISPER_SERVICE_URL.to_string(),
            request_timeout: Duration::from_secs(30),
            health_deadline: Duration::from_secs(10),
            language: "en".to_string(),
            no_context: true,
            temperature: 0.0,
            no_language_probabilities: true,
        }
    }
}

impl WhisperConfig {
    /// Build a config honoring the `WHISPER_SERVICE_URL` env var.
    ///
    /// Falls back to [`DEFAULT_WHISPER_SERVICE_URL`] if unset or empty.
    pub fn from_env() -> Self {
        let base_url = std::env::var(WHISPER_SERVICE_URL_ENV)
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_WHISPER_SERVICE_URL.to_string());
        Self {
            base_url,
            ..Default::default()
        }
    }
}

/// JSON response shape for `response_format=json`.
///
/// Only `text` is required; verbose_json carries more (segments, words,
/// confidence) but costs us an encoder sweep we don't need for live
/// streaming. See journal Q4 for the observability tradeoff.
#[derive(Debug, Clone, Deserialize)]
pub struct InferenceResponse {
    /// Transcribed text. Whisper convention: leading space — the
    /// client [`WhisperClient::transcribe`] trims it before returning.
    pub text: String,
}

/// Errors emitted by the whisper HTTP client.
#[derive(Debug, Error)]
pub enum TranscribeError {
    /// Underlying HTTP transport failure (DNS, connect, timeout, TLS).
    #[error("whisper http transport: {0}")]
    Transport(String),
    /// Service returned 5xx (idempotent at T=0 — safe to retry).
    #[error("whisper service {status}: {body}")]
    Server {
        /// HTTP status code.
        status: u16,
        /// Response body (plain text from the server).
        body: String,
    },
    /// Service returned 4xx (malformed request — don't retry).
    #[error("whisper client error {status}: {body}")]
    ClientError {
        /// HTTP status code.
        status: u16,
        /// Response body.
        body: String,
    },
    /// Service returned 503 `{"status":"loading model"}` — retry with
    /// backoff.
    #[error("whisper service loading")]
    Loading,
    /// Response body was not JSON in the expected shape.
    #[error("whisper response malformed: {0}")]
    Malformed(String),
}

impl TranscribeError {
    /// Whether a caller should retry automatically.
    pub fn is_retriable(&self) -> bool {
        matches!(
            self,
            TranscribeError::Transport(_)
                | TranscribeError::Server { .. }
                | TranscribeError::Loading
        )
    }
}

/// HTTP client for the whisper service.
#[derive(Debug, Clone)]
pub struct WhisperClient {
    config: WhisperConfig,
    http: reqwest::Client,
    /// Backpressure: permits=1 matches whisper's single-in-flight
    /// server mutex (API §1). We enforce it client-side so we never
    /// pipeline a second request against a loaded mutex and stall on
    /// the OS accept backlog.
    in_flight: Arc<Semaphore>,
}

impl WhisperClient {
    /// Build a client with the default config (or from env).
    pub fn new(config: WhisperConfig) -> Result<Self, TranscribeError> {
        let http = reqwest::Client::builder()
            .timeout(config.request_timeout)
            .build()
            .map_err(|e| TranscribeError::Transport(e.to_string()))?;
        Ok(Self {
            config,
            http,
            in_flight: Arc::new(Semaphore::new(1)),
        })
    }

    /// Read-only accessor; used by the service layer for structured
    /// logs.
    pub fn config(&self) -> &WhisperConfig {
        &self.config
    }

    /// Hit `GET /health` exactly once.
    ///
    /// Returns `Ok(true)` on 200 + `{"status":"ok"}`, `Ok(false)` on
    /// 503 + `{"status":"loading model"}`, or `Err` on transport
    /// failure.
    pub async fn health(&self) -> Result<bool, TranscribeError> {
        let url = format!("{}/health", self.config.base_url);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| TranscribeError::Transport(e.to_string()))?;
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| TranscribeError::Transport(e.to_string()))?;
        if status.is_success() {
            Ok(body.contains("\"ok\""))
        } else if status.as_u16() == 503 {
            Ok(false)
        } else {
            Err(TranscribeError::Server {
                status: status.as_u16(),
                body,
            })
        }
    }

    /// Poll `/health` until it returns ready or the deadline elapses.
    ///
    /// Backoff is a simple 500ms tick; the API doc §2.1 notes cold
    /// starts take "several seconds" so a 10s deadline (the default)
    /// is a reasonable upper bound. Returns `Ok(true)` when ready,
    /// `Ok(false)` on timeout — **not** an error, so the service
    /// enters a degraded-but-alive state instead of crashing.
    pub async fn wait_for_healthy(&self) -> bool {
        let start = tokio::time::Instant::now();
        let mut backoff = Duration::from_millis(200);
        while start.elapsed() < self.config.health_deadline {
            match self.health().await {
                Ok(true) => return true,
                Ok(false) => debug!("whisper: service still loading model"),
                Err(e) => debug!(error = %e, "whisper: health check failed"),
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_secs(2));
        }
        warn!(
            base_url = %self.config.base_url,
            deadline_ms = self.config.health_deadline.as_millis() as u64,
            "whisper: health probe timeout — service will run in degraded mode"
        );
        false
    }

    /// POST a WAV-wrapped clip to `/inference` and return the parsed
    /// JSON response.
    ///
    /// Acquires the in-flight semaphore; callers that want to apply
    /// their own scheduling (drop-oldest, say) can inspect
    /// `in_flight.available_permits()` before calling.
    pub async fn transcribe(&self, wav_bytes: Vec<u8>) -> Result<InferenceResponse, TranscribeError> {
        let _permit = self.in_flight.acquire().await.map_err(|_| {
            TranscribeError::Transport("whisper in-flight semaphore closed".into())
        })?;
        self.transcribe_unchecked(wav_bytes).await
    }

    /// Transcribe without acquiring the permit. Public for test harnesses
    /// that want to exercise the wire format without the semaphore's
    /// serialization. Production code paths MUST use
    /// [`Self::transcribe`].
    pub async fn transcribe_unchecked(
        &self,
        wav_bytes: Vec<u8>,
    ) -> Result<InferenceResponse, TranscribeError> {
        let url = format!("{}/inference", self.config.base_url);

        let part = Part::bytes(wav_bytes)
            .file_name("clip.wav")
            .mime_str("audio/wav")
            .map_err(|e| TranscribeError::Transport(e.to_string()))?;
        let form = Form::new()
            .part("file", part)
            .text("response_format", "json")
            .text("language", self.config.language.clone())
            .text("no_context", self.config.no_context.to_string())
            .text("temperature", self.config.temperature.to_string())
            .text(
                "no_language_probabilities",
                self.config.no_language_probabilities.to_string(),
            );

        let resp = self
            .http
            .post(&url)
            .multipart(form)
            .send()
            .await
            .map_err(|e| TranscribeError::Transport(e.to_string()))?;

        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| TranscribeError::Transport(e.to_string()))?;

        if status.as_u16() == 503 {
            // "loading model" — retry-worthy, not a failure.
            return Err(TranscribeError::Loading);
        }
        if !status.is_success() {
            let code = status.as_u16();
            if (400..500).contains(&code) {
                return Err(TranscribeError::ClientError {
                    status: code,
                    body,
                });
            }
            return Err(TranscribeError::Server { status: code, body });
        }

        let mut parsed: InferenceResponse = serde_json::from_str(&body).map_err(|e| {
            TranscribeError::Malformed(format!(
                "body was not InferenceResponse JSON ({e}): {body}"
            ))
        })?;
        // Whisper's leading-space convention — strip for symmetry.
        parsed.text = parsed.text.trim_start().to_string();
        Ok(parsed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn test_config(url: String) -> WhisperConfig {
        WhisperConfig {
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
        let client = WhisperClient::new(test_config(server.uri())).unwrap();
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
        let client = WhisperClient::new(test_config(server.uri())).unwrap();
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
        let client = WhisperClient::new(test_config(server.uri())).unwrap();
        let err = client.health().await.unwrap_err();
        assert!(matches!(err, TranscribeError::Server { .. }));
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
        let client = WhisperClient::new(test_config(server.uri())).unwrap();
        // Deadline is 2s; we expect `false`, not a panic.
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
        let client = WhisperClient::new(test_config(server.uri())).unwrap();
        assert!(client.wait_for_healthy().await);
    }

    #[tokio::test]
    async fn transcribe_happy_path_strips_leading_space() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/inference"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"text": " hello world"}"#,
            ))
            .mount(&server)
            .await;
        let client = WhisperClient::new(test_config(server.uri())).unwrap();
        let r = client.transcribe(vec![0u8; 44]).await.unwrap();
        assert_eq!(r.text, "hello world");
    }

    #[tokio::test]
    async fn transcribe_bubbles_loading() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/inference"))
            .respond_with(
                ResponseTemplate::new(503).set_body_string(r#"{"status":"loading model"}"#),
            )
            .mount(&server)
            .await;
        let client = WhisperClient::new(test_config(server.uri())).unwrap();
        let err = client.transcribe(vec![0u8; 44]).await.unwrap_err();
        assert!(matches!(err, TranscribeError::Loading));
        assert!(err.is_retriable());
    }

    #[tokio::test]
    async fn transcribe_client_error_not_retriable() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/inference"))
            .respond_with(ResponseTemplate::new(400).set_body_string(
                r#"{"error":"failed to read audio data"}"#,
            ))
            .mount(&server)
            .await;
        let client = WhisperClient::new(test_config(server.uri())).unwrap();
        let err = client.transcribe(vec![0u8; 44]).await.unwrap_err();
        assert!(matches!(err, TranscribeError::ClientError { status: 400, .. }));
        assert!(!err.is_retriable());
    }

    #[tokio::test]
    async fn transcribe_server_error_retriable() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/inference"))
            .respond_with(ResponseTemplate::new(502).set_body_string("bad gateway"))
            .mount(&server)
            .await;
        let client = WhisperClient::new(test_config(server.uri())).unwrap();
        let err = client.transcribe(vec![0u8; 44]).await.unwrap_err();
        assert!(err.is_retriable());
    }

    #[tokio::test]
    async fn transcribe_malformed_json_fails() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/inference"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
            .mount(&server)
            .await;
        let client = WhisperClient::new(test_config(server.uri())).unwrap();
        let err = client.transcribe(vec![0u8; 44]).await.unwrap_err();
        assert!(matches!(err, TranscribeError::Malformed(_)));
    }

    #[tokio::test]
    async fn in_flight_semaphore_is_one() {
        // Ensure the public client has exactly one permit — protects
        // the whisper single-mutex server contract (API §1).
        let client = WhisperClient::new(test_config("http://unused".into())).unwrap();
        assert_eq!(client.in_flight.available_permits(), 1);
    }

    #[test]
    fn config_from_env_uses_env_var() {
        // Use a safe local mutation: save + restore. This test runs
        // single-threaded against the process env so tests in this
        // module don't stomp each other.
        // SAFETY: `set_var` / `remove_var` are unsafe on Rust 2024 due
        // to potential races with threads reading env; we use them
        // only around this tightly scoped block.
        unsafe { std::env::set_var("WHISPER_SERVICE_URL", "http://other.host:9000") };
        let c = WhisperConfig::from_env();
        assert_eq!(c.base_url, "http://other.host:9000");
        unsafe { std::env::remove_var("WHISPER_SERVICE_URL") };
        let c2 = WhisperConfig::from_env();
        assert_eq!(c2.base_url, DEFAULT_WHISPER_SERVICE_URL);
    }
}
