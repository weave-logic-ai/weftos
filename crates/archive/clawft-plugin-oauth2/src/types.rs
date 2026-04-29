//! Types for OAuth2 tool operations.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// OAuth2 provider presets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderPreset {
    /// Google (Gmail, Calendar, Chat, Drive).
    Google,
    /// Microsoft (Azure AD, Graph API).
    Microsoft,
    /// Custom provider with explicit endpoints.
    Custom,
}

/// Configuration for an OAuth2 provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuth2ProviderConfig {
    /// Provider name (used as key for token storage).
    pub name: String,

    /// Provider preset for default endpoint URLs.
    #[serde(default = "default_preset")]
    pub preset: ProviderPreset,

    /// OAuth2 client ID.
    pub client_id: String,

    /// Reference to the client secret (env var name, not plaintext).
    /// The actual secret is read from the environment at runtime.
    pub client_secret_ref: SecretRef,

    /// Authorization endpoint URL (required for Custom preset).
    #[serde(default)]
    pub auth_url: Option<String>,

    /// Token endpoint URL (required for Custom preset).
    #[serde(default)]
    pub token_url: Option<String>,

    /// OAuth2 scopes to request.
    #[serde(default)]
    pub scopes: Vec<String>,

    /// Redirect URI for the authorization code flow.
    #[serde(default = "default_redirect_uri")]
    pub redirect_uri: String,
}

fn default_preset() -> ProviderPreset {
    ProviderPreset::Custom
}

fn default_redirect_uri() -> String {
    "http://localhost:8085/callback".to_string()
}

/// Reference to a secret value stored in an environment variable.
///
/// The client secret is NEVER stored in plaintext in configuration.
/// Instead, this struct holds the name of the environment variable
/// that contains the actual secret.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretRef {
    /// Name of the environment variable containing the secret.
    pub env_var: String,
}

impl SecretRef {
    /// Resolve the secret from the environment.
    pub fn resolve(&self) -> Result<String, String> {
        std::env::var(&self.env_var)
            .map_err(|_| format!("environment variable '{}' not set", self.env_var))
    }
}

impl OAuth2ProviderConfig {
    /// Get the authorization URL for this provider.
    pub fn auth_endpoint(&self) -> Result<String, String> {
        match self.preset {
            ProviderPreset::Google => {
                Ok("https://accounts.google.com/o/oauth2/v2/auth".to_string())
            }
            ProviderPreset::Microsoft => {
                Ok("https://login.microsoftonline.com/common/oauth2/v2.0/authorize".to_string())
            }
            ProviderPreset::Custom => self
                .auth_url
                .clone()
                .ok_or_else(|| "auth_url required for custom provider".to_string()),
        }
    }

    /// Get the token URL for this provider.
    pub fn token_endpoint(&self) -> Result<String, String> {
        match self.preset {
            ProviderPreset::Google => {
                Ok("https://oauth2.googleapis.com/token".to_string())
            }
            ProviderPreset::Microsoft => {
                Ok("https://login.microsoftonline.com/common/oauth2/v2.0/token".to_string())
            }
            ProviderPreset::Custom => self
                .token_url
                .clone()
                .ok_or_else(|| "token_url required for custom provider".to_string()),
        }
    }
}

/// Stored OAuth2 tokens.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredTokens {
    /// Access token.
    pub access_token: String,

    /// Refresh token (if available).
    #[serde(default)]
    pub refresh_token: Option<String>,

    /// Token type (usually "Bearer").
    #[serde(default = "default_token_type")]
    pub token_type: String,

    /// Expiration timestamp (Unix seconds).
    #[serde(default)]
    pub expires_at: Option<i64>,

    /// Scopes granted by the server.
    #[serde(default)]
    pub scopes: Vec<String>,

    /// Provider name this token belongs to.
    pub provider: String,
}

fn default_token_type() -> String {
    "Bearer".to_string()
}

impl StoredTokens {
    /// Check if the access token has expired (with a 60-second buffer).
    pub fn is_expired(&self) -> bool {
        match self.expires_at {
            Some(expires_at) => {
                let now = chrono::Utc::now().timestamp();
                now >= (expires_at - 60) // 60-second buffer
            }
            None => false, // No expiry info means we assume valid
        }
    }
}

/// Authorization flow state for CSRF protection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorizationState {
    /// Random state parameter for CSRF protection.
    pub state: String,

    /// PKCE code verifier (used in exchange step).
    pub pkce_verifier: String,

    /// Provider name.
    pub provider: String,

    /// Timestamp when this state was created.
    pub created_at: i64,
}

/// Result of starting an authorization flow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorizeResult {
    /// URL the user should open in their browser.
    pub authorize_url: String,

    /// State token (echoed back in the callback for CSRF validation).
    pub state: String,
}

/// Result of an authenticated REST request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestResponse {
    /// HTTP status code.
    pub status: u16,

    /// Response headers.
    pub headers: HashMap<String, String>,

    /// Response body (as string).
    pub body: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_preset_serde() {
        let presets = vec![
            ProviderPreset::Google,
            ProviderPreset::Microsoft,
            ProviderPreset::Custom,
        ];
        for preset in &presets {
            let json = serde_json::to_string(preset).unwrap();
            let restored: ProviderPreset = serde_json::from_str(&json).unwrap();
            assert_eq!(&restored, preset);
        }
    }

    #[test]
    fn google_endpoints() {
        let config = OAuth2ProviderConfig {
            name: "google".to_string(),
            preset: ProviderPreset::Google,
            client_id: "test".to_string(),
            client_secret_ref: SecretRef {
                env_var: "GOOGLE_CLIENT_SECRET".to_string(),
            },
            auth_url: None,
            token_url: None,
            scopes: vec!["email".to_string()],
            redirect_uri: default_redirect_uri(),
        };
        assert!(config.auth_endpoint().unwrap().contains("google"));
        assert!(config.token_endpoint().unwrap().contains("googleapis.com"));
    }

    #[test]
    fn microsoft_endpoints() {
        let config = OAuth2ProviderConfig {
            name: "microsoft".to_string(),
            preset: ProviderPreset::Microsoft,
            client_id: "test".to_string(),
            client_secret_ref: SecretRef {
                env_var: "MS_CLIENT_SECRET".to_string(),
            },
            auth_url: None,
            token_url: None,
            scopes: vec!["User.Read".to_string()],
            redirect_uri: default_redirect_uri(),
        };
        assert!(config.auth_endpoint().unwrap().contains("microsoftonline"));
        assert!(config.token_endpoint().unwrap().contains("microsoftonline"));
    }

    #[test]
    fn custom_requires_urls() {
        let config = OAuth2ProviderConfig {
            name: "custom".to_string(),
            preset: ProviderPreset::Custom,
            client_id: "test".to_string(),
            client_secret_ref: SecretRef {
                env_var: "SECRET".to_string(),
            },
            auth_url: None,
            token_url: None,
            scopes: vec![],
            redirect_uri: default_redirect_uri(),
        };
        assert!(config.auth_endpoint().is_err());
        assert!(config.token_endpoint().is_err());
    }

    #[test]
    fn stored_tokens_expired() {
        let tokens = StoredTokens {
            access_token: "test".to_string(),
            refresh_token: None,
            token_type: "Bearer".to_string(),
            expires_at: Some(0), // expired long ago
            scopes: vec![],
            provider: "test".to_string(),
        };
        assert!(tokens.is_expired());
    }

    #[test]
    fn stored_tokens_not_expired() {
        let future = chrono::Utc::now().timestamp() + 3600;
        let tokens = StoredTokens {
            access_token: "test".to_string(),
            refresh_token: None,
            token_type: "Bearer".to_string(),
            expires_at: Some(future),
            scopes: vec![],
            provider: "test".to_string(),
        };
        assert!(!tokens.is_expired());
    }

    #[test]
    fn stored_tokens_no_expiry_not_expired() {
        let tokens = StoredTokens {
            access_token: "test".to_string(),
            refresh_token: None,
            token_type: "Bearer".to_string(),
            expires_at: None,
            scopes: vec![],
            provider: "test".to_string(),
        };
        assert!(!tokens.is_expired());
    }

    #[test]
    fn secret_ref_resolve_missing_var() {
        let secret = SecretRef {
            env_var: "CLAWFT_TEST_NONEXISTENT_VAR_12345".to_string(),
        };
        assert!(secret.resolve().is_err());
    }
}
