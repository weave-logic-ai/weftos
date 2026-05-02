//! Error types for the clawft framework.
//!
//! Provides [`ClawftError`] as the top-level error type and [`ChannelError`]
//! for channel-specific failures. Both are non-exhaustive to allow future
//! extension without breaking downstream.

use thiserror::Error;

/// Top-level error type for the clawft framework.
///
/// Variants are grouped into recoverable (retry, timeout, rate-limit) and
/// fatal (config, plugin, I/O) categories to guide callers on whether
/// retrying is worthwhile.
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum ClawftError {
    // ── Recoverable ──────────────────────────────────────────────────
    /// A transient failure that may succeed on retry.
    #[error("retry required: {source} (attempt {attempts})")]
    Retry {
        /// The underlying error.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
        /// How many attempts have been made so far.
        attempts: u32,
    },

    /// An operation exceeded its deadline.
    #[error("operation timed out: {operation}")]
    Timeout {
        /// Human-readable name of the operation that timed out.
        operation: String,
    },

    /// A provider returned an error (e.g. bad request, server error).
    #[error("provider error: {message}")]
    Provider {
        /// Provider-supplied error message.
        message: String,
    },

    /// The provider is throttling requests.
    #[error("rate limited: retry after {retry_after_ms}ms")]
    RateLimited {
        /// Suggested wait time in milliseconds before retrying.
        retry_after_ms: u64,
    },

    // ── Fatal ────────────────────────────────────────────────────────
    /// Configuration is malformed or semantically invalid.
    #[error("invalid config: {reason}")]
    ConfigInvalid {
        /// What is wrong with the configuration.
        reason: String,
    },

    /// A plugin/extension could not be loaded.
    #[error("failed to load plugin: {plugin}")]
    PluginLoadFailed {
        /// Name or path of the plugin that failed.
        plugin: String,
    },

    /// Underlying I/O error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialization / deserialization error.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// A channel-layer error bubbled up.
    #[error("channel error: {0}")]
    Channel(String),

    /// A security boundary was violated (path traversal, oversized input, etc.)
    #[error("security violation: {reason}")]
    SecurityViolation {
        /// What policy was violated.
        reason: String,
    },

    /// The per-conversation cost circuit-breaker tripped (WEFT-322).
    ///
    /// Returned by [`AgentLoop::handle_turn`] when accumulated usage on
    /// `conv_id` would exceed `dimension`'s configured cap (`limit`).
    /// `used` is the value that caused the trip. The conversation is
    /// marked `circuit_open` in the budget store; subsequent calls
    /// fail-fast with this same error until reset via
    /// `agent.chat.reset_budget`.
    #[error(
        "conversation budget exceeded for `{conv_id}`: \
         {dimension} {used} >= limit {limit} (circuit_open until reset_budget)"
    )]
    ConversationBudgetExceeded {
        /// Conversation identifier whose budget tripped.
        conv_id: String,
        /// One of `"tokens"`, `"usd"`, or `"iterations"`.
        dimension: String,
        /// Configured cap that was reached.
        limit: f64,
        /// Accumulated value at the moment of the trip.
        used: f64,
    },
}

/// Channel-specific error type.
///
/// Used by channel implementations (Telegram, Slack, Discord, etc.)
/// to report failures in connecting, authenticating, or exchanging messages.
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum ChannelError {
    /// Failed to establish a connection to the channel backend.
    #[error("connection failed: {0}")]
    ConnectionFailed(String),

    /// Authentication / authorization was rejected.
    #[error("authentication failed: {0}")]
    AuthFailed(String),

    /// Sending a message failed.
    #[error("send failed: {0}")]
    SendFailed(String),

    /// Receiving a message failed.
    #[error("receive failed: {0}")]
    ReceiveFailed(String),

    /// The channel is not currently connected.
    #[error("not connected")]
    NotConnected,

    /// The requested channel was not found.
    #[error("channel not found: {0}")]
    NotFound(String),

    /// Catch-all for errors that do not fit other variants.
    #[error("{0}")]
    Other(String),
}

/// A convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, ClawftError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clawft_error_display() {
        let err = ClawftError::Timeout {
            operation: "llm_call".into(),
        };
        assert_eq!(err.to_string(), "operation timed out: llm_call");
    }

    #[test]
    fn clawft_error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
        let err: ClawftError = io_err.into();
        assert!(matches!(err, ClawftError::Io(_)));
        assert!(err.to_string().contains("missing"));
    }

    #[test]
    fn clawft_error_from_json() {
        let json_err = serde_json::from_str::<serde_json::Value>("{{bad}}").unwrap_err();
        let err: ClawftError = json_err.into();
        assert!(matches!(err, ClawftError::Json(_)));
    }

    #[test]
    fn channel_error_display() {
        let err = ChannelError::NotConnected;
        assert_eq!(err.to_string(), "not connected");

        let err = ChannelError::AuthFailed("bad token".into());
        assert_eq!(err.to_string(), "authentication failed: bad token");
    }

    #[test]
    fn retry_error_preserves_source() {
        let source: Box<dyn std::error::Error + Send + Sync> = "transient".into();
        let err = ClawftError::Retry {
            source,
            attempts: 3,
        };
        assert!(err.to_string().contains("attempt 3"));
        assert!(err.to_string().contains("transient"));
    }

    #[test]
    fn security_violation_display() {
        let err = ClawftError::SecurityViolation {
            reason: "path traversal detected".into(),
        };
        assert_eq!(
            err.to_string(),
            "security violation: path traversal detected"
        );
    }

    #[test]
    fn result_alias_works() {
        fn ok_fn() -> Result<i32> {
            Ok(42)
        }
        fn err_fn() -> Result<i32> {
            Err(ClawftError::Provider {
                message: "boom".into(),
            })
        }
        assert_eq!(ok_fn().unwrap(), 42);
        assert!(err_fn().is_err());
    }
}
