//! Signal channel configuration types.

use serde::{Deserialize, Serialize};

/// Configuration for the Signal channel adapter.
///
/// `signal-cli` is invoked in `daemon` mode with a TCP JSON-RPC socket;
/// `start()` connects a [`tokio::net::TcpStream`] to that socket, reads
/// newline-delimited JSON-RPC events, and writes outbound `send` calls
/// over the same connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalAdapterConfig {
    /// Phone number registered with Signal (e.g. `"+1234567890"`).
    #[serde(default, alias = "phoneNumber")]
    pub phone_number: String,

    /// Path to the `signal-cli` binary.
    ///
    /// Defaults to the value of the `SIGNAL_CLI_PATH` env var at startup,
    /// or `"signal-cli"` if the env var is unset (i.e. resolved through
    /// `PATH`).
    #[serde(default = "default_signal_cli_path", alias = "signalCliPath")]
    pub signal_cli_path: String,

    /// TCP bind address for `signal-cli daemon --tcp <addr>`
    /// (default `"127.0.0.1:7583"`).
    #[serde(default = "default_daemon_bind_addr", alias = "daemonBindAddr")]
    pub daemon_bind_addr: String,

    /// Optional `--config` directory for `signal-cli` state (account
    /// data, attachments, etc.). When `None`, `signal-cli` uses its
    /// default location (`~/.local/share/signal-cli`).
    #[serde(default, alias = "dataDir")]
    pub data_dir: Option<String>,

    /// Subprocess timeout in seconds (default 30).
    ///
    /// Used as both the daemon-startup readiness deadline and the
    /// per-request response deadline.
    #[serde(default = "default_timeout_secs", alias = "timeoutSecs")]
    pub timeout_secs: u64,

    /// Allowed phone numbers. Empty = allow all.
    #[serde(default, alias = "allowedNumbers")]
    pub allowed_numbers: Vec<String>,
}

fn default_signal_cli_path() -> String {
    std::env::var("SIGNAL_CLI_PATH").unwrap_or_else(|_| "signal-cli".into())
}
fn default_daemon_bind_addr() -> String {
    "127.0.0.1:7583".into()
}
fn default_timeout_secs() -> u64 {
    30
}

impl Default for SignalAdapterConfig {
    fn default() -> Self {
        Self {
            phone_number: String::new(),
            signal_cli_path: default_signal_cli_path(),
            daemon_bind_addr: default_daemon_bind_addr(),
            data_dir: None,
            timeout_secs: default_timeout_secs(),
            allowed_numbers: Vec::new(),
        }
    }
}

/// Sanitize a string argument for safe use as a subprocess argument.
///
/// Rejects arguments containing shell metacharacters that could
/// enable command injection. Returns `Err` with a description if
/// the argument is unsafe.
pub fn sanitize_argument(arg: &str) -> Result<&str, String> {
    // Reject empty arguments.
    if arg.is_empty() {
        return Err("empty argument".into());
    }

    // Reject shell metacharacters.
    const BANNED_CHARS: &[char] = &[
        ';', '|', '&', '$', '`', '(', ')', '{', '}', '<', '>', '!',
        '\n', '\r', '\0',
    ];

    for ch in BANNED_CHARS {
        if arg.contains(*ch) {
            return Err(format!(
                "argument contains forbidden character: {:?}",
                ch
            ));
        }
    }

    Ok(arg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values() {
        // SIGNAL_CLI_PATH may be set in the environment of CI runners;
        // tolerate either the env-supplied value or the literal default.
        let cfg = SignalAdapterConfig::default();
        let expected_path = std::env::var("SIGNAL_CLI_PATH")
            .unwrap_or_else(|_| "signal-cli".into());
        assert_eq!(cfg.signal_cli_path, expected_path);
        assert_eq!(cfg.daemon_bind_addr, "127.0.0.1:7583");
        assert!(cfg.data_dir.is_none());
        assert_eq!(cfg.timeout_secs, 30);
    }

    #[test]
    fn sanitize_clean_argument() {
        assert!(sanitize_argument("+1234567890").is_ok());
        assert!(sanitize_argument("hello world").is_ok());
        assert!(sanitize_argument("normal-text_123").is_ok());
    }

    #[test]
    fn sanitize_rejects_semicolon() {
        let result = sanitize_argument("hello; rm -rf /");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains(";"));
    }

    #[test]
    fn sanitize_rejects_pipe() {
        assert!(sanitize_argument("hello | cat /etc/passwd").is_err());
    }

    #[test]
    fn sanitize_rejects_ampersand() {
        assert!(sanitize_argument("hello & whoami").is_err());
    }

    #[test]
    fn sanitize_rejects_dollar() {
        assert!(sanitize_argument("$HOME").is_err());
    }

    #[test]
    fn sanitize_rejects_backtick() {
        assert!(sanitize_argument("`id`").is_err());
    }

    #[test]
    fn sanitize_rejects_subshell() {
        assert!(sanitize_argument("$(whoami)").is_err());
    }

    #[test]
    fn sanitize_rejects_newline() {
        assert!(sanitize_argument("hello\nworld").is_err());
    }

    #[test]
    fn sanitize_rejects_null_byte() {
        assert!(sanitize_argument("hello\0world").is_err());
    }

    #[test]
    fn sanitize_rejects_empty() {
        assert!(sanitize_argument("").is_err());
    }
}
