//! WhatsApp channel configuration and message types.

use serde::{Deserialize, Serialize};

use clawft_types::secret::SecretString;

/// Configuration for the WhatsApp channel adapter.
///
/// All credential fields use [`SecretString`] to prevent accidental exposure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhatsAppAdapterConfig {
    /// WhatsApp Business phone number ID.
    #[serde(default, alias = "phoneNumberId")]
    pub phone_number_id: String,

    /// App access token (via SecretString).
    #[serde(default, alias = "accessToken")]
    pub access_token: SecretString,

    /// Webhook verify token (via SecretString -- not plaintext).
    ///
    /// Used during the GET `/webhook` handshake from Meta to confirm
    /// ownership of the receiving endpoint.
    #[serde(default, alias = "verifyToken")]
    pub verify_token: SecretString,

    /// Meta app secret (via SecretString).
    ///
    /// Used to verify the `X-Hub-Signature-256` HMAC-SHA256 header on
    /// inbound webhook POSTs from Meta.
    #[serde(default, alias = "appSecret")]
    pub app_secret: SecretString,

    /// Bind address for the inbound webhook HTTP listener.
    ///
    /// Defaults to `127.0.0.1:0` (loopback, ephemeral port). Operators
    /// fronting the listener with a reverse proxy / tunnel typically
    /// override this to a fixed port.
    #[serde(default = "default_webhook_bind", alias = "webhookBindAddr")]
    pub webhook_bind_addr: String,

    /// Cloud API base URL.
    #[serde(default = "default_api_url", alias = "apiUrl")]
    pub api_url: String,

    /// API version.
    #[serde(default = "default_api_version", alias = "apiVersion")]
    pub api_version: String,

    /// Allowed phone numbers. Empty = allow all.
    #[serde(default, alias = "allowedNumbers")]
    pub allowed_numbers: Vec<String>,
}

fn default_api_url() -> String {
    "https://graph.facebook.com".into()
}
fn default_api_version() -> String {
    "v18.0".into()
}
fn default_webhook_bind() -> String {
    "127.0.0.1:0".into()
}

impl Default for WhatsAppAdapterConfig {
    fn default() -> Self {
        Self {
            phone_number_id: String::new(),
            access_token: SecretString::default(),
            verify_token: SecretString::default(),
            app_secret: SecretString::default(),
            webhook_bind_addr: default_webhook_bind(),
            api_url: default_api_url(),
            api_version: default_api_version(),
            allowed_numbers: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values() {
        let cfg = WhatsAppAdapterConfig::default();
        assert_eq!(cfg.api_url, "https://graph.facebook.com");
        assert_eq!(cfg.api_version, "v18.0");
        assert_eq!(cfg.webhook_bind_addr, "127.0.0.1:0");
        assert!(cfg.allowed_numbers.is_empty());
    }

    #[test]
    fn app_secret_uses_secret_string() {
        let cfg = WhatsAppAdapterConfig {
            app_secret: SecretString::new("super-secret"),
            ..Default::default()
        };
        let debug = format!("{:?}", cfg);
        assert!(!debug.contains("super-secret"));
        assert!(debug.contains("REDACTED"));
    }

    #[test]
    fn verify_token_uses_secret_string() {
        let cfg = WhatsAppAdapterConfig {
            verify_token: SecretString::new("my-secret"),
            ..Default::default()
        };
        let debug = format!("{:?}", cfg);
        assert!(!debug.contains("my-secret"));
        assert!(debug.contains("REDACTED"));
    }

    #[test]
    fn config_serde_roundtrip() {
        let json = r#"{
            "phoneNumberId": "12345",
            "accessToken": "token123",
            "verifyToken": "verify123",
            "allowedNumbers": ["+1234567890"]
        }"#;
        let cfg: WhatsAppAdapterConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.phone_number_id, "12345");
        assert_eq!(cfg.allowed_numbers, vec!["+1234567890"]);
    }
}
