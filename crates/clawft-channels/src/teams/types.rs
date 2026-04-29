//! Microsoft Teams channel configuration types.

use serde::{Deserialize, Serialize};

use clawft_types::secret::SecretString;

/// Configuration for the Microsoft Teams channel adapter.
///
/// Connects to Microsoft Teams via the Bot Framework. Outbound
/// messages POST to `{service_url}/v3/conversations/{convoId}/activities`
/// after acquiring an Azure AD client-credentials access token from the
/// `oauth_token_url`. Inbound activities arrive on a local axum webhook
/// bound to `webhook_bind`.
///
/// **Field naming**: per WEFT-156 the canonical Bot Framework field
/// names are `app_id` and `app_password`; `client_id`/`client_secret`
/// are accepted as serde aliases for back-compat with the pre-0.7
/// stub config schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamsAdapterConfig {
    /// Azure AD tenant ID.
    #[serde(default, alias = "tenantId")]
    pub tenant_id: String,

    /// Bot Framework / Azure AD application (MicrosoftAppId).
    /// Accepts the legacy `client_id` / `clientId` keys as aliases.
    #[serde(default, alias = "clientId", alias = "client_id", alias = "appId")]
    pub app_id: String,

    /// Bot Framework / Azure AD client secret (MicrosoftAppPassword).
    /// Accepts the legacy `client_secret` / `clientSecret` keys.
    #[serde(
        default,
        alias = "clientSecret",
        alias = "client_secret",
        alias = "appPassword"
    )]
    pub app_password: SecretString,

    /// Bot Framework `serviceUrl` for the conversation. Stored from
    /// inbound activities and used as the base for outbound POSTs.
    /// May be overridden in config for single-tenant deployments.
    #[serde(default, alias = "serviceUrl")]
    pub service_url: String,

    /// Local socket address to bind the inbound activity webhook
    /// listener (e.g. `127.0.0.1:3978`). Empty disables the listener.
    #[serde(default, alias = "webhookBind")]
    pub webhook_bind: String,

    /// OAuth2 token endpoint. Defaults to the Microsoft v2 endpoint
    /// for the configured tenant; overridable for tests.
    #[serde(default, alias = "oauthTokenUrl")]
    pub oauth_token_url: String,

    /// Teams to monitor. Empty = all teams the bot is added to.
    #[serde(default)]
    pub teams: Vec<String>,

    /// Channels within teams to monitor. Empty = all channels.
    #[serde(default)]
    pub channels: Vec<String>,

    /// Allowed user principal names. Empty = allow all.
    #[serde(default, alias = "allowedUsers")]
    pub allowed_users: Vec<String>,

    /// Microsoft Graph API base URL (kept for compatibility; not
    /// used by the Bot Framework outbound path, which targets
    /// `service_url`).
    #[serde(default = "default_graph_url", alias = "graphUrl")]
    pub graph_url: String,
}

fn default_graph_url() -> String {
    "https://graph.microsoft.com/v1.0".into()
}

impl TeamsAdapterConfig {
    /// Returns the OAuth2 token endpoint, falling back to the
    /// Microsoft v2 endpoint scoped to `tenant_id` when not
    /// explicitly configured.
    pub fn token_endpoint(&self) -> String {
        if !self.oauth_token_url.is_empty() {
            return self.oauth_token_url.clone();
        }
        format!(
            "https://login.microsoftonline.com/{}/oauth2/v2.0/token",
            self.tenant_id
        )
    }

    /// Back-compat alias: the legacy `client_id` field name.
    pub fn client_id(&self) -> &str {
        &self.app_id
    }

    /// Back-compat alias: the legacy `client_secret` field name.
    pub fn client_secret(&self) -> &SecretString {
        &self.app_password
    }
}

impl Default for TeamsAdapterConfig {
    fn default() -> Self {
        Self {
            tenant_id: String::new(),
            app_id: String::new(),
            app_password: SecretString::default(),
            service_url: String::new(),
            webhook_bind: String::new(),
            oauth_token_url: String::new(),
            teams: Vec::new(),
            channels: Vec::new(),
            allowed_users: Vec::new(),
            graph_url: default_graph_url(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values() {
        let cfg = TeamsAdapterConfig::default();
        assert!(cfg.tenant_id.is_empty());
        assert!(cfg.app_id.is_empty());
        assert!(cfg.app_password.is_empty());
        assert!(cfg.service_url.is_empty());
        assert!(cfg.webhook_bind.is_empty());
        assert!(cfg.teams.is_empty());
        assert!(cfg.channels.is_empty());
        assert!(cfg.allowed_users.is_empty());
        assert_eq!(
            cfg.graph_url,
            "https://graph.microsoft.com/v1.0"
        );
    }

    #[test]
    fn app_password_uses_secret_string() {
        let cfg = TeamsAdapterConfig {
            app_password: SecretString::new("super-secret"),
            ..Default::default()
        };
        let debug = format!("{:?}", cfg);
        assert!(!debug.contains("super-secret"));
        assert!(debug.contains("REDACTED"));
    }

    #[test]
    fn token_endpoint_default() {
        let cfg = TeamsAdapterConfig {
            tenant_id: "abc-123".into(),
            ..Default::default()
        };
        assert_eq!(
            cfg.token_endpoint(),
            "https://login.microsoftonline.com/abc-123/oauth2/v2.0/token"
        );
    }

    #[test]
    fn token_endpoint_override() {
        let cfg = TeamsAdapterConfig {
            tenant_id: "abc-123".into(),
            oauth_token_url: "http://localhost:9999/token".into(),
            ..Default::default()
        };
        assert_eq!(cfg.token_endpoint(), "http://localhost:9999/token");
    }

    #[test]
    fn config_serde_roundtrip_canonical_keys() {
        let json = r#"{
            "tenantId": "tenant-abc",
            "appId": "app-123",
            "appPassword": "secret-xyz",
            "serviceUrl": "https://smba.trafficmanager.net/amer/",
            "webhookBind": "127.0.0.1:3978",
            "teams": ["team-1"],
            "channels": ["channel-1"],
            "allowedUsers": ["user@company.com"]
        }"#;
        let cfg: TeamsAdapterConfig =
            serde_json::from_str(json).unwrap();
        assert_eq!(cfg.tenant_id, "tenant-abc");
        assert_eq!(cfg.app_id, "app-123");
        assert_eq!(cfg.service_url, "https://smba.trafficmanager.net/amer/");
        assert_eq!(cfg.webhook_bind, "127.0.0.1:3978");
        assert_eq!(cfg.teams, vec!["team-1"]);
        assert_eq!(cfg.channels, vec!["channel-1"]);
        assert_eq!(cfg.allowed_users, vec!["user@company.com"]);
    }

    #[test]
    fn config_serde_roundtrip_legacy_keys() {
        // The pre-0.7 stub used clientId/clientSecret. Accept those
        // verbatim so existing config files keep loading.
        let json = r#"{
            "tenantId": "tenant-abc",
            "clientId": "client-123",
            "clientSecret": "secret-xyz"
        }"#;
        let cfg: TeamsAdapterConfig =
            serde_json::from_str(json).unwrap();
        assert_eq!(cfg.app_id, "client-123");
        assert_eq!(cfg.client_id(), "client-123");
        assert_eq!(cfg.client_secret().expose(), "secret-xyz");
    }
}
