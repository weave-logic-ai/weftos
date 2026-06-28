//! IRC channel configuration types.

use serde::{Deserialize, Serialize};

/// Valid authentication methods for IRC.
const VALID_AUTH_METHODS: &[&str] = &["none", "nickserv", "sasl"];

/// Configuration for the IRC channel adapter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IrcAdapterConfig {
    /// IRC server hostname (e.g. `"irc.libera.chat"`).
    #[serde(default)]
    pub server: String,

    /// IRC server port (default 6697 for TLS, 6667 for plaintext).
    #[serde(default = "default_port")]
    pub port: u16,

    /// Whether to use TLS for the connection (default `true`).
    #[serde(default = "default_use_tls", alias = "useTls")]
    pub use_tls: bool,

    /// Bot nickname on the IRC network.
    #[serde(default)]
    pub nickname: String,

    /// Channels to join on connect (e.g. `["#general", "#dev"]`).
    #[serde(default)]
    pub channels: Vec<String>,

    /// Authentication method: `"none"`, `"nickserv"`, or `"sasl"`.
    #[serde(default = "default_auth_method", alias = "authMethod")]
    pub auth_method: String,

    /// Environment variable name that holds the IRC password.
    ///
    /// Follows the `SecretRef` pattern -- the password is never stored
    /// directly in configuration; only the env var name is stored.
    #[serde(default, alias = "passwordEnv")]
    pub password_env: Option<String>,

    /// Allowed sender nicknames. Empty = allow all senders.
    #[serde(default, alias = "allowedSenders")]
    pub allowed_senders: Vec<String>,

    /// Delay in seconds before reconnecting after a disconnect (default 5).
    #[serde(default = "default_reconnect_delay_secs", alias = "reconnectDelaySecs")]
    pub reconnect_delay_secs: u64,
}

fn default_port() -> u16 {
    6697
}

fn default_use_tls() -> bool {
    true
}

fn default_auth_method() -> String {
    "none".into()
}

fn default_reconnect_delay_secs() -> u64 {
    5
}

impl Default for IrcAdapterConfig {
    fn default() -> Self {
        Self {
            server: String::new(),
            port: default_port(),
            use_tls: default_use_tls(),
            nickname: String::new(),
            channels: Vec::new(),
            auth_method: default_auth_method(),
            password_env: None,
            allowed_senders: Vec::new(),
            reconnect_delay_secs: default_reconnect_delay_secs(),
        }
    }
}

/// Validate the IRC adapter configuration.
///
/// Checks:
/// - `server` is non-empty and free of shell metacharacters
/// - `nickname` is non-empty and free of shell metacharacters
/// - `auth_method` is one of `"none"`, `"nickserv"`, `"sasl"`
/// - `password_env` is set when `auth_method` requires authentication
/// - Channel names start with `#` or `&`
pub fn validate_config(config: &IrcAdapterConfig) -> Result<(), String> {
    // Server is required.
    if config.server.is_empty() {
        return Err("irc adapter: server is required".into());
    }
    sanitize_irc_argument(&config.server)
        .map_err(|e| format!("irc adapter: invalid server: {e}"))?;

    // Nickname is required.
    if config.nickname.is_empty() {
        return Err("irc adapter: nickname is required".into());
    }
    sanitize_irc_argument(&config.nickname)
        .map_err(|e| format!("irc adapter: invalid nickname: {e}"))?;

    // Auth method must be one of the valid values.
    if !VALID_AUTH_METHODS.contains(&config.auth_method.as_str()) {
        return Err(format!(
            "irc adapter: auth_method must be one of {:?}, got {:?}",
            VALID_AUTH_METHODS, config.auth_method
        ));
    }

    // Password env is required when using authenticated methods.
    if (config.auth_method == "nickserv" || config.auth_method == "sasl")
        && config.password_env.is_none()
    {
        return Err(format!(
            "irc adapter: password_env is required when auth_method is {:?}",
            config.auth_method
        ));
    }

    // Validate channel names (must start with # or &).
    for ch in &config.channels {
        if !ch.starts_with('#') && !ch.starts_with('&') {
            return Err(format!(
                "irc adapter: channel name must start with '#' or '&', got {:?}",
                ch
            ));
        }
        sanitize_channel_name(ch).map_err(|e| format!("irc adapter: invalid channel name: {e}"))?;
    }

    Ok(())
}

/// Sanitize an IRC channel name.
///
/// Channel names may begin with `#` or `&` (valid IRC prefixes), but
/// the remainder must not contain protocol injection characters or
/// shell metacharacters.
pub fn sanitize_channel_name(name: &str) -> Result<&str, String> {
    if name.is_empty() {
        return Err("empty channel name".into());
    }

    // Characters banned in the body of a channel name.
    // Note: `#` and `&` are allowed as the leading prefix only.
    const BANNED_CHARS: &[char] = &[
        ';', '|', '&', '$', '`', '(', ')', '{', '}', '<', '>', '!', '\n', '\r', '\0', ' ', ',',
    ];

    // Check all characters after the first (prefix) character.
    let body = &name[name.chars().next().unwrap().len_utf8()..];
    for ch in BANNED_CHARS {
        if body.contains(*ch) {
            return Err(format!(
                "channel name contains forbidden character: {:?}",
                ch
            ));
        }
    }

    Ok(name)
}

/// Sanitize a string argument for safe use in IRC commands.
///
/// Rejects arguments containing characters that could enable
/// protocol injection (newlines, carriage returns, null bytes)
/// or shell metacharacters (for any subprocess interaction).
/// Returns `Err` with a description if the argument is unsafe.
pub fn sanitize_irc_argument(arg: &str) -> Result<&str, String> {
    if arg.is_empty() {
        return Err("empty argument".into());
    }

    // IRC protocol injection characters + shell metacharacters.
    const BANNED_CHARS: &[char] = &[
        ';', '|', '&', '$', '`', '(', ')', '{', '}', '<', '>', '!', '\n', '\r', '\0',
    ];

    for ch in BANNED_CHARS {
        if arg.contains(*ch) {
            return Err(format!("argument contains forbidden character: {:?}", ch));
        }
    }

    Ok(arg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values() {
        let cfg = IrcAdapterConfig::default();
        assert_eq!(cfg.port, 6697);
        assert!(cfg.use_tls);
        assert_eq!(cfg.auth_method, "none");
        assert_eq!(cfg.reconnect_delay_secs, 5);
        assert!(cfg.server.is_empty());
        assert!(cfg.nickname.is_empty());
        assert!(cfg.channels.is_empty());
        assert!(cfg.password_env.is_none());
        assert!(cfg.allowed_senders.is_empty());
    }

    #[test]
    fn validate_config_success() {
        let cfg = IrcAdapterConfig {
            server: "irc.libera.chat".into(),
            nickname: "clawft-bot".into(),
            channels: vec!["#general".into(), "#dev".into()],
            ..Default::default()
        };
        assert!(validate_config(&cfg).is_ok());
    }

    #[test]
    fn validate_config_empty_server() {
        let cfg = IrcAdapterConfig {
            nickname: "bot".into(),
            ..Default::default()
        };
        let err = validate_config(&cfg).unwrap_err();
        assert!(err.contains("server is required"));
    }

    #[test]
    fn validate_config_empty_nickname() {
        let cfg = IrcAdapterConfig {
            server: "irc.example.com".into(),
            ..Default::default()
        };
        let err = validate_config(&cfg).unwrap_err();
        assert!(err.contains("nickname is required"));
    }

    #[test]
    fn validate_config_invalid_auth_method() {
        let cfg = IrcAdapterConfig {
            server: "irc.example.com".into(),
            nickname: "bot".into(),
            auth_method: "kerberos".into(),
            ..Default::default()
        };
        let err = validate_config(&cfg).unwrap_err();
        assert!(err.contains("auth_method must be one of"));
    }

    #[test]
    fn validate_config_nickserv_without_password_env() {
        let cfg = IrcAdapterConfig {
            server: "irc.example.com".into(),
            nickname: "bot".into(),
            auth_method: "nickserv".into(),
            password_env: None,
            ..Default::default()
        };
        let err = validate_config(&cfg).unwrap_err();
        assert!(err.contains("password_env is required"));
    }

    #[test]
    fn validate_config_sasl_without_password_env() {
        let cfg = IrcAdapterConfig {
            server: "irc.example.com".into(),
            nickname: "bot".into(),
            auth_method: "sasl".into(),
            password_env: None,
            ..Default::default()
        };
        let err = validate_config(&cfg).unwrap_err();
        assert!(err.contains("password_env is required"));
    }

    #[test]
    fn validate_config_nickserv_with_password_env() {
        let cfg = IrcAdapterConfig {
            server: "irc.example.com".into(),
            nickname: "bot".into(),
            auth_method: "nickserv".into(),
            password_env: Some("IRC_PASSWORD".into()),
            ..Default::default()
        };
        assert!(validate_config(&cfg).is_ok());
    }

    #[test]
    fn validate_config_sasl_with_password_env() {
        let cfg = IrcAdapterConfig {
            server: "irc.example.com".into(),
            nickname: "bot".into(),
            auth_method: "sasl".into(),
            password_env: Some("IRC_PASSWORD".into()),
            ..Default::default()
        };
        assert!(validate_config(&cfg).is_ok());
    }

    #[test]
    fn validate_config_bad_channel_name() {
        let cfg = IrcAdapterConfig {
            server: "irc.example.com".into(),
            nickname: "bot".into(),
            channels: vec!["general".into()],
            ..Default::default()
        };
        let err = validate_config(&cfg).unwrap_err();
        assert!(err.contains("must start with '#' or '&'"));
    }

    #[test]
    fn validate_config_ampersand_channel() {
        let cfg = IrcAdapterConfig {
            server: "irc.example.com".into(),
            nickname: "bot".into(),
            channels: vec!["&local-channel".into()],
            ..Default::default()
        };
        assert!(validate_config(&cfg).is_ok());
    }

    #[test]
    fn sanitize_clean_argument() {
        assert!(sanitize_irc_argument("irc.libera.chat").is_ok());
        assert!(sanitize_irc_argument("clawft-bot").is_ok());
        assert!(sanitize_irc_argument("#general").is_ok());
        assert!(sanitize_irc_argument("normal_text-123").is_ok());
    }

    #[test]
    fn sanitize_rejects_semicolon() {
        let result = sanitize_irc_argument("server; rm -rf /");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains(";"));
    }

    #[test]
    fn sanitize_rejects_newline() {
        assert!(sanitize_irc_argument("hello\nworld").is_err());
    }

    #[test]
    fn sanitize_rejects_carriage_return() {
        assert!(sanitize_irc_argument("hello\rworld").is_err());
    }

    #[test]
    fn sanitize_rejects_null_byte() {
        assert!(sanitize_irc_argument("hello\0world").is_err());
    }

    #[test]
    fn sanitize_rejects_empty() {
        assert!(sanitize_irc_argument("").is_err());
    }

    #[test]
    fn sanitize_rejects_pipe() {
        assert!(sanitize_irc_argument("bot | cat /etc/passwd").is_err());
    }

    #[test]
    fn sanitize_rejects_dollar() {
        assert!(sanitize_irc_argument("$HOME").is_err());
    }

    #[test]
    fn sanitize_rejects_backtick() {
        assert!(sanitize_irc_argument("`id`").is_err());
    }

    #[test]
    fn validate_config_server_with_injection() {
        let cfg = IrcAdapterConfig {
            server: "irc.example.com; cat /etc/passwd".into(),
            nickname: "bot".into(),
            ..Default::default()
        };
        let err = validate_config(&cfg).unwrap_err();
        assert!(err.contains("invalid server"));
    }

    #[test]
    fn validate_config_nickname_with_injection() {
        let cfg = IrcAdapterConfig {
            server: "irc.example.com".into(),
            nickname: "bot`whoami`".into(),
            ..Default::default()
        };
        let err = validate_config(&cfg).unwrap_err();
        assert!(err.contains("invalid nickname"));
    }

    #[test]
    fn serde_roundtrip() {
        let cfg = IrcAdapterConfig {
            server: "irc.libera.chat".into(),
            port: 6697,
            use_tls: true,
            nickname: "clawft-bot".into(),
            channels: vec!["#general".into()],
            auth_method: "sasl".into(),
            password_env: Some("IRC_PASS".into()),
            allowed_senders: vec!["admin".into()],
            reconnect_delay_secs: 10,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let restored: IrcAdapterConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.server, "irc.libera.chat");
        assert_eq!(restored.port, 6697);
        assert!(restored.use_tls);
        assert_eq!(restored.nickname, "clawft-bot");
        assert_eq!(restored.channels, vec!["#general"]);
        assert_eq!(restored.auth_method, "sasl");
        assert_eq!(restored.password_env, Some("IRC_PASS".into()));
        assert_eq!(restored.allowed_senders, vec!["admin"]);
        assert_eq!(restored.reconnect_delay_secs, 10);
    }
}
