//! Provider error types for clawft-llm.
//!
//! All provider operations return [`Result<T>`] which uses [`ProviderError`]
//! as the error type.

use thiserror::Error;

/// Errors that can occur when interacting with an LLM provider.
#[derive(Error, Debug)]
pub enum ProviderError {
    /// The HTTP request to the provider failed.
    #[error("request failed: {0}")]
    RequestFailed(String),

    /// Authentication with the provider was rejected (HTTP 401/403).
    #[error("authentication failed: {0}")]
    AuthFailed(String),

    /// The provider returned a rate-limit response (HTTP 429).
    #[error("rate limited: retry after {retry_after_ms}ms")]
    RateLimited {
        /// Suggested wait time before retrying, in milliseconds.
        retry_after_ms: u64,
    },

    /// The requested model does not exist on the provider.
    #[error("model not found: {0}")]
    ModelNotFound(String),

    /// The server could not be reached (connection refused, DNS, reset
    /// before any response). Distinct from [`RequestFailed`](Self::RequestFailed)
    /// so callers can tell "nothing answered" from "the server said no".
    #[error("connection failed: {0}")]
    Connect(String),

    /// The provider has not been configured (e.g. missing API key).
    #[error("provider not configured: {0}")]
    NotConfigured(String),

    /// The provider returned a response that could not be parsed.
    #[error("invalid response: {0}")]
    InvalidResponse(String),

    /// The request timed out.
    #[error("timeout")]
    Timeout,

    /// An HTTP-level error from reqwest.
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    /// A JSON serialization/deserialization error.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// The provider returned an HTTP 5xx server error.
    #[error("server error {status}: {body}")]
    ServerError {
        /// HTTP status code (500, 502, 503, 504, etc.).
        status: u16,
        /// Response body text.
        body: String,
    },

    /// All providers in the failover chain have been exhausted.
    #[error("all providers exhausted: {}", attempts.join("; "))]
    AllProvidersExhausted {
        /// Per-provider error summaries in order of attempt.
        attempts: Vec<String>,
    },
}

/// A convenience type alias for provider operations.
pub type Result<T> = std::result::Result<T, ProviderError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_request_failed() {
        let err = ProviderError::RequestFailed("connection reset".into());
        assert_eq!(err.to_string(), "request failed: connection reset");
    }

    #[test]
    fn display_auth_failed() {
        let err = ProviderError::AuthFailed("invalid token".into());
        assert_eq!(err.to_string(), "authentication failed: invalid token");
    }

    #[test]
    fn display_rate_limited() {
        let err = ProviderError::RateLimited {
            retry_after_ms: 5000,
        };
        assert_eq!(err.to_string(), "rate limited: retry after 5000ms");
    }

    #[test]
    fn display_model_not_found() {
        let err = ProviderError::ModelNotFound("gpt-5-turbo".into());
        assert_eq!(err.to_string(), "model not found: gpt-5-turbo");
    }

    #[test]
    fn display_not_configured() {
        let err = ProviderError::NotConfigured("set OPENAI_API_KEY env var".into());
        assert_eq!(
            err.to_string(),
            "provider not configured: set OPENAI_API_KEY env var"
        );
    }

    #[test]
    fn display_invalid_response() {
        let err = ProviderError::InvalidResponse("missing choices field".into());
        assert_eq!(err.to_string(), "invalid response: missing choices field");
    }

    #[test]
    fn display_timeout() {
        let err = ProviderError::Timeout;
        assert_eq!(err.to_string(), "timeout");
    }

    #[test]
    fn json_error_from_conversion() {
        let bad_json = serde_json::from_str::<serde_json::Value>("not json");
        let serde_err = bad_json.unwrap_err();
        let provider_err: ProviderError = serde_err.into();
        assert!(provider_err.to_string().starts_with("json error:"));
    }

    #[test]
    fn result_type_alias_works() {
        let ok: Result<i32> = Ok(42);
        assert!(ok.is_ok());

        let err: Result<i32> = Err(ProviderError::Timeout);
        assert!(err.is_err());
    }

    #[test]
    fn display_server_error() {
        let err = ProviderError::ServerError {
            status: 503,
            body: "service unavailable".into(),
        };
        assert_eq!(err.to_string(), "server error 503: service unavailable");
    }

    #[test]
    fn display_all_providers_exhausted() {
        let err = ProviderError::AllProvidersExhausted {
            attempts: vec!["openai: rate limited".into(), "anthropic: timeout".into()],
        };
        let msg = err.to_string();
        assert!(msg.contains("all providers exhausted"));
        assert!(msg.contains("openai: rate limited"));
        assert!(msg.contains("anthropic: timeout"));
    }
}
