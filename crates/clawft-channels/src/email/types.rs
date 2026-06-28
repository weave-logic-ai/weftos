//! Email channel configuration and message types.
//!
//! Uses [`SecretString`] from `clawft-types` for all credential fields
//! to prevent accidental exposure in logs or serialized output.

use serde::{Deserialize, Serialize};

use clawft_types::secret::SecretString;

/// Authentication method for the email channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EmailAuth {
    /// Username/password authentication. Credentials are stored via
    /// [`SecretString`] and resolved from environment variables.
    Password {
        /// IMAP username (often the email address itself).
        username: String,
        /// IMAP password (via SecretString -- never logged or serialized).
        password: SecretString,
    },
    /// OAuth2 authentication for providers like Gmail.
    /// All secrets reference environment variable names, not raw values.
    #[serde(rename = "oauth2")]
    OAuth2 {
        /// Env var name for the OAuth2 client ID.
        client_id_env: String,
        /// Env var name for the OAuth2 client secret.
        client_secret_env: String,
        /// Env var name for the OAuth2 refresh token.
        refresh_token_env: String,
        /// Token endpoint URL.
        #[serde(default = "default_google_token_url")]
        token_url: String,
    },
}

fn default_google_token_url() -> String {
    "https://oauth2.googleapis.com/token".into()
}

impl Default for EmailAuth {
    fn default() -> Self {
        Self::Password {
            username: String::new(),
            password: SecretString::default(),
        }
    }
}

/// Configuration for the email channel adapter.
///
/// All credential fields use [`SecretString`] to prevent accidental
/// exposure. OAuth2 fields reference environment variable names.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailAdapterConfig {
    /// IMAP server hostname.
    #[serde(default, alias = "imapHost")]
    pub imap_host: String,

    /// IMAP server port.
    #[serde(default = "default_imap_port", alias = "imapPort")]
    pub imap_port: u16,

    /// SMTP server hostname.
    #[serde(default, alias = "smtpHost")]
    pub smtp_host: String,

    /// SMTP server port.
    #[serde(default = "default_smtp_port", alias = "smtpPort")]
    pub smtp_port: u16,

    /// Email address used for sending and as the IMAP identity.
    #[serde(default, alias = "emailAddress")]
    pub email_address: String,

    /// Authentication configuration.
    #[serde(default)]
    pub auth: EmailAuth,

    /// IMAP mailbox to monitor.
    #[serde(default = "default_mailbox")]
    pub mailbox: String,

    /// Poll interval in seconds.
    #[serde(default = "default_poll_interval", alias = "pollIntervalSecs")]
    pub poll_interval_secs: u64,

    /// Allowed sender email addresses. Empty = allow all.
    #[serde(default, alias = "allowedSenders")]
    pub allowed_senders: Vec<String>,

    /// Use TLS for IMAP connection.
    #[serde(default = "default_true", alias = "imapUseTls")]
    pub imap_use_tls: bool,

    /// Use STARTTLS for SMTP connection.
    #[serde(default = "default_true", alias = "smtpUseTls")]
    pub smtp_use_tls: bool,

    /// Maximum body characters to process.
    #[serde(default = "default_max_body_chars", alias = "maxBodyChars")]
    pub max_body_chars: usize,
}

fn default_imap_port() -> u16 {
    993
}
fn default_smtp_port() -> u16 {
    587
}
fn default_mailbox() -> String {
    "INBOX".into()
}
fn default_poll_interval() -> u64 {
    60
}
fn default_true() -> bool {
    true
}
fn default_max_body_chars() -> usize {
    12000
}

impl Default for EmailAdapterConfig {
    fn default() -> Self {
        Self {
            imap_host: String::new(),
            imap_port: default_imap_port(),
            smtp_host: String::new(),
            smtp_port: default_smtp_port(),
            email_address: String::new(),
            auth: EmailAuth::default(),
            mailbox: default_mailbox(),
            poll_interval_secs: default_poll_interval(),
            allowed_senders: Vec::new(),
            imap_use_tls: true,
            smtp_use_tls: true,
            max_body_chars: default_max_body_chars(),
        }
    }
}

/// A parsed inbound email message.
#[derive(Debug, Clone)]
pub struct ParsedEmail {
    /// Sender email address.
    pub from: String,
    /// Recipient email address.
    pub to: String,
    /// Email subject line.
    pub subject: String,
    /// Plain-text body (truncated to `max_body_chars`).
    pub body: String,
    /// Unique message ID from headers.
    pub message_id: String,
    /// Optional In-Reply-To header for threading.
    pub in_reply_to: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_values() {
        let cfg = EmailAdapterConfig::default();
        assert_eq!(cfg.imap_port, 993);
        assert_eq!(cfg.smtp_port, 587);
        assert_eq!(cfg.mailbox, "INBOX");
        assert_eq!(cfg.poll_interval_secs, 60);
        assert!(cfg.allowed_senders.is_empty());
        assert!(cfg.imap_use_tls);
        assert!(cfg.smtp_use_tls);
        assert_eq!(cfg.max_body_chars, 12000);
    }

    #[test]
    fn config_serde_roundtrip() {
        let cfg = EmailAdapterConfig {
            imap_host: "imap.gmail.com".into(),
            imap_port: 993,
            smtp_host: "smtp.gmail.com".into(),
            smtp_port: 587,
            email_address: "user@gmail.com".into(),
            auth: EmailAuth::Password {
                username: "user@gmail.com".into(),
                password: SecretString::new("secret123"),
            },
            mailbox: "INBOX".into(),
            poll_interval_secs: 30,
            allowed_senders: vec!["boss@company.com".into()],
            imap_use_tls: true,
            smtp_use_tls: true,
            max_body_chars: 8000,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let restored: EmailAdapterConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.imap_host, "imap.gmail.com");
        assert_eq!(restored.smtp_port, 587);
        assert_eq!(restored.allowed_senders, vec!["boss@company.com"]);
        assert_eq!(restored.max_body_chars, 8000);
    }

    #[test]
    fn oauth2_config_serde() {
        let json = r#"{
            "imapHost": "imap.gmail.com",
            "smtpHost": "smtp.gmail.com",
            "emailAddress": "user@gmail.com",
            "auth": {
                "type": "oauth2",
                "client_id_env": "GMAIL_CLIENT_ID",
                "client_secret_env": "GMAIL_CLIENT_SECRET",
                "refresh_token_env": "GMAIL_REFRESH_TOKEN"
            },
            "allowedSenders": ["boss@company.com"]
        }"#;
        let cfg: EmailAdapterConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.imap_host, "imap.gmail.com");
        match &cfg.auth {
            EmailAuth::OAuth2 {
                client_id_env,
                client_secret_env,
                refresh_token_env,
                token_url,
            } => {
                assert_eq!(client_id_env, "GMAIL_CLIENT_ID");
                assert_eq!(client_secret_env, "GMAIL_CLIENT_SECRET");
                assert_eq!(refresh_token_env, "GMAIL_REFRESH_TOKEN");
                assert_eq!(token_url, "https://oauth2.googleapis.com/token");
            }
            _ => panic!("expected OAuth2 auth"),
        }
    }

    #[test]
    fn password_auth_does_not_leak_secret() {
        let auth = EmailAuth::Password {
            username: "user".into(),
            password: SecretString::new("super-secret"),
        };
        let debug_output = format!("{:?}", auth);
        assert!(!debug_output.contains("super-secret"));
        assert!(debug_output.contains("REDACTED"));
    }

    #[test]
    fn parsed_email_fields() {
        let email = ParsedEmail {
            from: "alice@example.com".into(),
            to: "bot@example.com".into(),
            subject: "Help request".into(),
            body: "I need help with my account".into(),
            message_id: "<msg-001@example.com>".into(),
            in_reply_to: None,
        };
        assert_eq!(email.from, "alice@example.com");
        assert_eq!(email.subject, "Help request");
        assert!(email.in_reply_to.is_none());
    }

    #[test]
    fn parsed_email_with_reply() {
        let email = ParsedEmail {
            from: "alice@example.com".into(),
            to: "bot@example.com".into(),
            subject: "Re: Help request".into(),
            body: "Thanks for the update".into(),
            message_id: "<msg-002@example.com>".into(),
            in_reply_to: Some("<msg-001@example.com>".into()),
        };
        assert_eq!(email.in_reply_to.as_deref(), Some("<msg-001@example.com>"));
    }
}
