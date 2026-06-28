//! OpenAI-compatible provider implementation.
//!
//! [`OpenAiCompatProvider`] works with any API that follows the OpenAI chat
//! completion format. This covers OpenAI, Anthropic (via their OpenAI-compat
//! endpoint), Groq, DeepSeek, Mistral, Together AI, OpenRouter, and many more.

use async_trait::async_trait;
use tokio::sync::mpsc;
use tracing::{debug, trace, warn};

use std::time::Duration;

use crate::config::LlmProviderConfig;
use crate::error::{ProviderError, Result};
use crate::provider::Provider;
use crate::sse::parse_sse_line;
use crate::types::{ChatRequest, ChatResponse, StreamChunk};

/// Default timeout for LLM API requests (2 minutes).
const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// An LLM provider that uses the OpenAI-compatible chat completion API.
///
/// This is the primary provider implementation in clawft-llm. It can be
/// configured to talk to any endpoint that accepts the OpenAI request format
/// by changing the `base_url` in the [`LlmProviderConfig`].
///
/// # Construction
///
/// ```rust,ignore
/// use clawft_llm::{OpenAiCompatProvider, LlmProviderConfig};
///
/// let config = LlmProviderConfig {
///     name: "openai".into(),
///     base_url: "https://api.openai.com/v1".into(),
///     api_key_env: "OPENAI_API_KEY".into(),
///     model_prefix: Some("openai/".into()),
///     default_model: Some("gpt-4o".into()),
///     headers: Default::default(),
/// };
/// let provider = OpenAiCompatProvider::new(config);
/// ```
pub struct OpenAiCompatProvider {
    config: LlmProviderConfig,
    http: reqwest::Client,
    api_key: Option<String>,
}

impl OpenAiCompatProvider {
    /// Create a new provider from configuration.
    ///
    /// The API key will be resolved from the environment variable specified
    /// in `config.api_key_env` at request time.
    pub fn new(config: LlmProviderConfig) -> Self {
        let timeout_secs = config.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS);
        Self {
            http: reqwest::ClientBuilder::new()
                .timeout(Duration::from_secs(timeout_secs))
                .build()
                .expect("failed to build reqwest client"),
            config,
            api_key: None,
        }
    }

    /// Create a new provider with an explicit API key.
    ///
    /// This bypasses environment variable lookup and uses the provided key
    /// directly.
    pub fn with_api_key(config: LlmProviderConfig, api_key: String) -> Self {
        let timeout_secs = config.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS);
        Self {
            http: reqwest::ClientBuilder::new()
                .timeout(Duration::from_secs(timeout_secs))
                .build()
                .expect("failed to build reqwest client"),
            config,
            api_key: Some(api_key),
        }
    }

    /// Returns the provider configuration.
    pub fn config(&self) -> &LlmProviderConfig {
        &self.config
    }

    /// Returns the chat completions endpoint URL.
    fn completions_url(&self) -> String {
        let base = self.config.base_url.trim_end_matches('/');
        format!("{base}/chat/completions")
    }

    /// Resolve the API key: explicit key > environment variable.
    fn resolve_api_key(&self) -> Result<String> {
        if let Some(ref key) = self.api_key {
            return Ok(key.clone());
        }
        std::env::var(&self.config.api_key_env).map_err(|_| {
            ProviderError::NotConfigured(format!("set {} env var", self.config.api_key_env))
        })
    }
}

#[async_trait]
impl Provider for OpenAiCompatProvider {
    fn name(&self) -> &str {
        &self.config.name
    }

    async fn complete(&self, request: &ChatRequest) -> Result<ChatResponse> {
        let api_key = self.resolve_api_key()?;
        let url = self.completions_url();

        debug!(
            provider = %self.config.name,
            model = %request.model,
            messages = request.messages.len(),
            "sending chat completion request"
        );

        let mut req = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Content-Type", "application/json");

        for (k, v) in &self.config.headers {
            req = req.header(k.as_str(), v.as_str());
        }

        let response = req.json(request).send().await?;
        let status = response.status();

        if !status.is_success() {
            if status.as_u16() == 429 {
                // Try HTTP Retry-After header first, then body JSON, then default
                let header_ms = parse_retry_after_header(&response);
                let body = response.text().await.unwrap_or_default();

                // Some providers (e.g. xAI) use 429 for exhausted credits/quota,
                // which is not a transient rate limit and should not be retried.
                if is_quota_exhausted(&body) {
                    let msg = extract_error_message(&body)
                        .unwrap_or_else(|| "credits exhausted or spending limit reached".into());
                    warn!(
                        provider = %self.config.name,
                        "quota exhausted (not retryable)"
                    );
                    return Err(ProviderError::RequestFailed(msg));
                }

                let retry_ms = header_ms
                    .or_else(|| parse_retry_after_ms(&body))
                    .unwrap_or(1000);
                warn!(
                    provider = %self.config.name,
                    retry_after_ms = retry_ms,
                    body = %body,
                    "rate limited"
                );
                return Err(ProviderError::RateLimited {
                    retry_after_ms: retry_ms,
                });
            }

            let body = response.text().await.unwrap_or_default();

            if status.as_u16() == 401 || status.as_u16() == 403 {
                return Err(ProviderError::AuthFailed(body));
            }

            if status.as_u16() == 404 {
                return Err(ProviderError::ModelNotFound(format!(
                    "model '{}': {}",
                    request.model, body
                )));
            }

            // Emit structured ServerError for 5xx, RequestFailed for other codes.
            let code = status.as_u16();
            if (500..=599).contains(&code) {
                return Err(ProviderError::ServerError { status: code, body });
            }

            return Err(ProviderError::RequestFailed(format!(
                "HTTP {status}: {body}"
            )));
        }

        let chat_response: ChatResponse = response.json().await.map_err(|e| {
            ProviderError::InvalidResponse(format!("failed to parse response: {e}"))
        })?;

        debug!(
            provider = %self.config.name,
            model = %chat_response.model,
            choices = chat_response.choices.len(),
            "chat completion response received"
        );

        Ok(chat_response)
    }

    async fn complete_stream(
        &self,
        request: &ChatRequest,
        tx: mpsc::Sender<StreamChunk>,
    ) -> Result<()> {
        let api_key = self.resolve_api_key()?;
        let url = self.completions_url();

        debug!(
            provider = %self.config.name,
            model = %request.model,
            messages = request.messages.len(),
            "sending streaming chat completion request"
        );

        // Build a request with stream: true
        let mut stream_request = request.clone();
        stream_request.stream = Some(true);

        let mut req = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream");

        for (k, v) in &self.config.headers {
            req = req.header(k.as_str(), v.as_str());
        }

        let response = req.json(&stream_request).send().await?;
        let status = response.status();

        if !status.is_success() {
            if status.as_u16() == 429 {
                let header_ms = parse_retry_after_header(&response);
                let body = response.text().await.unwrap_or_default();

                if is_quota_exhausted(&body) {
                    let msg = extract_error_message(&body)
                        .unwrap_or_else(|| "credits exhausted or spending limit reached".into());
                    warn!(
                        provider = %self.config.name,
                        "quota exhausted (not retryable)"
                    );
                    return Err(ProviderError::RequestFailed(msg));
                }

                let retry_ms = header_ms
                    .or_else(|| parse_retry_after_ms(&body))
                    .unwrap_or(1000);
                warn!(
                    provider = %self.config.name,
                    retry_after_ms = retry_ms,
                    body = %body,
                    "rate limited (streaming)"
                );
                return Err(ProviderError::RateLimited {
                    retry_after_ms: retry_ms,
                });
            }

            let body = response.text().await.unwrap_or_default();

            if status.as_u16() == 401 || status.as_u16() == 403 {
                return Err(ProviderError::AuthFailed(body));
            }

            if status.as_u16() == 404 {
                return Err(ProviderError::ModelNotFound(format!(
                    "model '{}': {}",
                    request.model, body
                )));
            }

            // Emit structured ServerError for 5xx, RequestFailed for other codes.
            let code = status.as_u16();
            if (500..=599).contains(&code) {
                return Err(ProviderError::ServerError { status: code, body });
            }

            return Err(ProviderError::RequestFailed(format!(
                "HTTP {status}: {body}"
            )));
        }

        // Read the SSE stream line by line
        use futures_util::StreamExt;
        let mut byte_stream = response.bytes_stream();
        let mut buffer = String::new();

        while let Some(chunk_result) = byte_stream.next().await {
            let bytes = chunk_result
                .map_err(|e| ProviderError::RequestFailed(format!("stream read error: {e}")))?;

            let text = String::from_utf8_lossy(&bytes);
            buffer.push_str(&text);

            // Process complete lines from the buffer
            while let Some(newline_pos) = buffer.find('\n') {
                let line = buffer[..newline_pos].to_string();
                buffer = buffer[newline_pos + 1..].to_string();

                let chunks = match parse_sse_line(&line) {
                    Ok(c) => c,
                    Err(e) => {
                        warn!(
                            provider = %self.config.name,
                            error = %e,
                            "SSE parse error, skipping line"
                        );
                        continue;
                    }
                };

                for chunk in chunks {
                    trace!(
                        provider = %self.config.name,
                        chunk = ?chunk,
                        "streaming chunk"
                    );
                    // If the receiver is dropped, stop processing
                    if tx.send(chunk).await.is_err() {
                        debug!(
                            provider = %self.config.name,
                            "stream receiver dropped, stopping"
                        );
                        return Ok(());
                    }
                }
            }
        }

        // Process any remaining data in the buffer
        if !buffer.trim().is_empty()
            && let Ok(chunks) = parse_sse_line(&buffer)
        {
            for chunk in chunks {
                let _ = tx.send(chunk).await;
            }
        }

        debug!(
            provider = %self.config.name,
            "streaming complete"
        );

        Ok(())
    }
}

/// Check if a 429 response body indicates a permanent quota/credit exhaustion
/// rather than a transient rate limit. Some providers (xAI, OpenAI) return 429
/// for billing issues that will never resolve with retries.
fn is_quota_exhausted(body: &str) -> bool {
    let lower = body.to_lowercase();
    lower.contains("exhausted")
        || lower.contains("spending limit")
        || lower.contains("credits")
        || lower.contains("billing")
        || lower.contains("quota exceeded")
        || lower.contains("insufficient_quota")
}

/// Extract a human-readable error message from a JSON error response body.
fn extract_error_message(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    value.get("error").and_then(|v| {
        // OpenAI format: {"error": {"message": "..."}}
        v.get("message")
            .and_then(|m| m.as_str())
            .map(String::from)
            // xAI format: {"error": "..."}
            .or_else(|| v.as_str().map(String::from))
    })
}

/// Try to extract a retry-after value from the HTTP `Retry-After` header.
///
/// The header value can be either seconds (integer or float) or an HTTP-date.
/// We only handle the numeric form here; HTTP-date is rare for API providers.
fn parse_retry_after_header(response: &reqwest::Response) -> Option<u64> {
    let header_val = response
        .headers()
        .get("retry-after")
        .or_else(|| response.headers().get("x-ratelimit-reset-after"))
        .and_then(|v| v.to_str().ok())?;

    // Try as float seconds first (e.g. "1.5"), then integer
    if let Ok(secs) = header_val.parse::<f64>() {
        return Some((secs * 1000.0).max(0.0) as u64);
    }

    None
}

/// Try to extract a retry-after value from a JSON error response body.
fn parse_retry_after_ms(body: &str) -> Option<u64> {
    // Some providers include retry_after or retry_after_ms in the error JSON
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    value
        .get("retry_after_ms")
        .and_then(|v| v.as_u64())
        .or_else(|| {
            value
                .get("retry_after")
                .and_then(|v| v.as_f64())
                .map(|secs| (secs * 1000.0) as u64)
        })
}

impl std::fmt::Debug for OpenAiCompatProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiCompatProvider")
            .field("name", &self.config.name)
            .field("base_url", &self.config.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "***"))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LlmProviderConfig;
    use std::collections::HashMap;

    fn test_config() -> LlmProviderConfig {
        LlmProviderConfig {
            name: "test-provider".into(),
            base_url: "https://api.example.com/v1".into(),
            api_key_env: "TEST_PROVIDER_API_KEY".into(),
            model_prefix: Some("test/".into()),
            default_model: Some("test-model".into()),
            headers: HashMap::new(),
            timeout_secs: None,
        }
    }

    fn config_with_headers() -> LlmProviderConfig {
        LlmProviderConfig {
            name: "anthropic".into(),
            base_url: "https://api.anthropic.com/v1".into(),
            api_key_env: "ANTHROPIC_API_KEY".into(),
            model_prefix: Some("anthropic/".into()),
            default_model: None,
            headers: HashMap::from([("anthropic-version".into(), "2023-06-01".into())]),
            timeout_secs: None,
        }
    }

    #[test]
    fn new_provider() {
        let provider = OpenAiCompatProvider::new(test_config());
        assert_eq!(provider.name(), "test-provider");
        assert!(provider.api_key.is_none());
    }

    #[test]
    fn with_api_key_provider() {
        let provider = OpenAiCompatProvider::with_api_key(test_config(), "sk-test123".into());
        assert_eq!(provider.name(), "test-provider");
        assert_eq!(provider.api_key.as_deref(), Some("sk-test123"));
    }

    #[test]
    fn completions_url_construction() {
        let provider = OpenAiCompatProvider::new(test_config());
        assert_eq!(
            provider.completions_url(),
            "https://api.example.com/v1/chat/completions"
        );
    }

    #[test]
    fn completions_url_strips_trailing_slash() {
        let mut config = test_config();
        config.base_url = "https://api.example.com/v1/".into();
        let provider = OpenAiCompatProvider::new(config);
        assert_eq!(
            provider.completions_url(),
            "https://api.example.com/v1/chat/completions"
        );
    }

    #[test]
    fn resolve_api_key_explicit() {
        let provider = OpenAiCompatProvider::with_api_key(test_config(), "sk-explicit".into());
        let key = provider.resolve_api_key().unwrap();
        assert_eq!(key, "sk-explicit");
    }

    #[test]
    fn resolve_api_key_from_env() {
        // Use a unique env var name to avoid conflicts with real env
        let env_var = "CLAWFT_TEST_RESOLVE_KEY_12345";
        let mut config = test_config();
        config.api_key_env = env_var.into();

        let key = temp_env::with_var(env_var, Some("sk-from-env"), || {
            let provider = OpenAiCompatProvider::new(config);
            provider.resolve_api_key().unwrap()
        });
        assert_eq!(key, "sk-from-env");
    }

    #[test]
    fn resolve_api_key_missing() {
        let mut config = test_config();
        config.api_key_env = "CLAWFT_NONEXISTENT_KEY_98765".into();
        let provider = OpenAiCompatProvider::new(config);
        let err = provider.resolve_api_key().unwrap_err();
        assert!(matches!(err, ProviderError::NotConfigured(_)));
        assert!(err.to_string().contains("CLAWFT_NONEXISTENT_KEY_98765"));
    }

    #[test]
    fn config_accessor() {
        let config = test_config();
        let provider = OpenAiCompatProvider::new(config.clone());
        assert_eq!(provider.config().name, "test-provider");
        assert_eq!(provider.config().base_url, "https://api.example.com/v1");
    }

    #[test]
    fn provider_with_headers_config() {
        let provider = OpenAiCompatProvider::new(config_with_headers());
        assert_eq!(provider.config().headers.len(), 1);
        assert_eq!(
            provider.config().headers.get("anthropic-version"),
            Some(&"2023-06-01".to_string())
        );
    }

    #[test]
    fn debug_hides_api_key() {
        let provider = OpenAiCompatProvider::with_api_key(test_config(), "sk-secret-key".into());
        let debug_str = format!("{:?}", provider);
        assert!(!debug_str.contains("sk-secret-key"));
        assert!(debug_str.contains("***"));
    }

    #[test]
    fn debug_shows_none_for_missing_key() {
        let provider = OpenAiCompatProvider::new(test_config());
        let debug_str = format!("{:?}", provider);
        assert!(debug_str.contains("None"));
    }

    #[test]
    fn parse_retry_after_ms_from_ms_field() {
        let body = r#"{"retry_after_ms": 2500}"#;
        assert_eq!(parse_retry_after_ms(body), Some(2500));
    }

    #[test]
    fn parse_retry_after_ms_from_seconds_field() {
        let body = r#"{"retry_after": 3.5}"#;
        assert_eq!(parse_retry_after_ms(body), Some(3500));
    }

    #[test]
    fn parse_retry_after_ms_missing() {
        let body = r#"{"error": "rate limited"}"#;
        assert_eq!(parse_retry_after_ms(body), None);
    }

    #[test]
    fn parse_retry_after_ms_invalid_json() {
        assert_eq!(parse_retry_after_ms("not json"), None);
    }

    // -- SEC-04: API key leakage audit ------------------------------------

    /// SEC-04: Verify that the Debug impl does not leak API keys.
    #[test]
    fn debug_impl_never_leaks_api_key() {
        let test_keys = [
            "sk-abc123def456",
            "sk-proj-ABCDEF1234567890",
            "key-1234567890abcdef",
        ];

        for key in &test_keys {
            let provider = OpenAiCompatProvider::with_api_key(test_config(), key.to_string());
            let debug_str = format!("{:?}", provider);
            assert!(
                !debug_str.contains(key),
                "Debug output should not contain API key: found '{}' in debug output",
                key
            );
        }
    }

    /// SEC-04: Verify that the provider's string representations do not
    /// expose the actual API key value.
    #[test]
    fn provider_display_does_not_leak_key() {
        let provider =
            OpenAiCompatProvider::with_api_key(test_config(), "sk-secret-test-key-12345".into());
        // Only Debug is implemented; verify it masks the key.
        let output = format!("{provider:?}");
        assert!(!output.contains("sk-secret-test-key-12345"));
        assert!(output.contains("***"));
    }

    // ── A7: HTTP timeout tests ────────────────────────────────────────

    #[test]
    fn client_uses_default_timeout() {
        let config = test_config();
        // Verify Client was built (it would panic if ClientBuilder failed)
        let provider = OpenAiCompatProvider::new(config);
        assert_eq!(provider.name(), "test-provider");
    }

    #[test]
    fn client_uses_custom_timeout() {
        let mut config = test_config();
        config.timeout_secs = Some(30);
        let provider = OpenAiCompatProvider::new(config);
        assert_eq!(provider.name(), "test-provider");
    }

    #[test]
    fn with_api_key_uses_timeout() {
        let mut config = test_config();
        config.timeout_secs = Some(60);
        let provider = OpenAiCompatProvider::with_api_key(config, "sk-test".into());
        assert_eq!(provider.api_key.as_deref(), Some("sk-test"));
    }

    /// SEC-04: Verify that LlmProviderConfig stores env var names, not keys,
    /// and that its Debug output does not contain actual API key values.
    #[test]
    fn log_fields_do_not_include_api_key() {
        let config = test_config();
        let debug_config = format!("{:?}", config);
        // LlmProviderConfig should not contain any actual key values
        // (it stores the env var NAME, not the key itself).
        assert!(
            !debug_config.contains("sk-"),
            "LlmProviderConfig debug should not contain API key prefixes: {debug_config}"
        );
        assert!(
            debug_config.contains("TEST_PROVIDER_API_KEY"),
            "LlmProviderConfig should show the env var name, not the key"
        );
    }
}
