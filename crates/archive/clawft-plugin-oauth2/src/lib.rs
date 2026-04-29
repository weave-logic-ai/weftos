//! Generic REST + OAuth2 helper tool plugin for clawft.
//!
//! Provides OAuth2 authorization code flow, client credentials flow,
//! token persistence, and an authenticated REST client. Supports
//! Google, Microsoft, and custom OAuth2 providers.
//!
//! # Security
//!
//! - OAuth2 `state` parameter for CSRF protection (mandatory).
//! - PKCE (Proof Key for Code Exchange) for public clients.
//! - Tokens stored with 0600 file permissions.
//! - `client_secret` accessed via `SecretRef` (env var, not plaintext).
//! - Rotated refresh tokens persisted immediately.
//!
//! # Feature Flag
//!
//! This crate is gated behind the workspace `plugin-oauth2` feature flag.

pub mod token_store;
pub mod types;

use std::collections::HashMap;

use async_trait::async_trait;
use clawft_plugin::{PluginError, Tool, ToolContext};
use rand::Rng;
use tracing::debug;

use token_store::TokenStore;
use types::{
    AuthorizationState, AuthorizeResult, OAuth2ProviderConfig, RestResponse, StoredTokens,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Generate a cryptographically random state string for CSRF protection.
fn generate_state() -> String {
    let mut rng = rand::thread_rng();
    let bytes: Vec<u8> = (0..32).map(|_| rng.r#gen()).collect();
    hex_encode(&bytes)
}

/// Generate a PKCE code verifier (43-128 character URL-safe string).
fn generate_pkce_verifier() -> String {
    let mut rng = rand::thread_rng();
    let bytes: Vec<u8> = (0..32).map(|_| rng.r#gen()).collect();
    base64_url_encode(&bytes)
}

/// Compute PKCE code challenge (S256) from verifier.
fn compute_pkce_challenge(verifier: &str) -> String {
    use sha2::Digest;
    let hash = sha2::Sha256::digest(verifier.as_bytes());
    base64_url_encode(&hash)
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn base64_url_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

// ---------------------------------------------------------------------------
// OAuth2AuthorizeTool
// ---------------------------------------------------------------------------

/// Tool that starts an OAuth2 authorization code flow.
pub struct OAuth2AuthorizeTool {
    config: OAuth2ProviderConfig,
    token_store: TokenStore,
}

impl OAuth2AuthorizeTool {
    pub fn new(config: OAuth2ProviderConfig, token_store: TokenStore) -> Self {
        Self {
            config,
            token_store,
        }
    }
}

#[async_trait]
impl Tool for OAuth2AuthorizeTool {
    fn name(&self) -> &str {
        "oauth2_authorize"
    }

    fn description(&self) -> &str {
        "Start an OAuth2 authorization code flow and return the authorization URL"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "scopes": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "OAuth2 scopes to request (overrides config default)"
                }
            },
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &dyn ToolContext,
    ) -> Result<serde_json::Value, PluginError> {
        let auth_url = self
            .config
            .auth_endpoint()
            .map_err(PluginError::ExecutionFailed)?;

        let scopes: Vec<String> = params
            .get("scopes")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_else(|| self.config.scopes.clone());

        let state = generate_state();
        let pkce_verifier = generate_pkce_verifier();
        let pkce_challenge = compute_pkce_challenge(&pkce_verifier);

        // Build authorization URL
        let mut url = url::Url::parse(&auth_url)
            .map_err(|e| PluginError::ExecutionFailed(format!("invalid auth URL: {e}")))?;

        url.query_pairs_mut()
            .append_pair("client_id", &self.config.client_id)
            .append_pair("redirect_uri", &self.config.redirect_uri)
            .append_pair("response_type", "code")
            .append_pair("scope", &scopes.join(" "))
            .append_pair("state", &state)
            .append_pair("code_challenge", &pkce_challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("access_type", "offline");

        // Store state for CSRF validation during callback
        let auth_state = AuthorizationState {
            state: state.clone(),
            pkce_verifier,
            provider: self.config.name.clone(),
            created_at: chrono::Utc::now().timestamp(),
        };

        self.token_store
            .store_auth_state(&auth_state)
            .map_err(PluginError::ExecutionFailed)?;

        let result = AuthorizeResult {
            authorize_url: url.to_string(),
            state,
        };

        debug!(provider = %self.config.name, "started authorization flow");
        serde_json::to_value(&result).map_err(PluginError::from)
    }
}

// ---------------------------------------------------------------------------
// OAuth2CallbackTool
// ---------------------------------------------------------------------------

/// Tool that handles the OAuth2 callback and exchanges the auth code for tokens.
pub struct OAuth2CallbackTool {
    config: OAuth2ProviderConfig,
    token_store: TokenStore,
}

impl OAuth2CallbackTool {
    pub fn new(config: OAuth2ProviderConfig, token_store: TokenStore) -> Self {
        Self {
            config,
            token_store,
        }
    }
}

#[async_trait]
impl Tool for OAuth2CallbackTool {
    fn name(&self) -> &str {
        "oauth2_callback"
    }

    fn description(&self) -> &str {
        "Handle OAuth2 callback with auth code and exchange for tokens"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "code": {
                    "type": "string",
                    "description": "Authorization code from the callback"
                },
                "state": {
                    "type": "string",
                    "description": "State parameter from the callback (for CSRF validation)"
                }
            },
            "required": ["code", "state"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &dyn ToolContext,
    ) -> Result<serde_json::Value, PluginError> {
        let code = params
            .get("code")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PluginError::ExecutionFailed("code is required".into()))?;
        let state = params
            .get("state")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PluginError::ExecutionFailed("state is required".into()))?;

        // Validate state (CSRF protection)
        let auth_state = self
            .token_store
            .consume_auth_state(&self.config.name)
            .map_err(PluginError::ExecutionFailed)?
            .ok_or_else(|| {
                PluginError::ExecutionFailed("no pending authorization state found".into())
            })?;

        if auth_state.state != state {
            return Err(PluginError::PermissionDenied(
                "state parameter mismatch (possible CSRF attack)".into(),
            ));
        }

        // Check state is not too old (10 minutes max)
        let age = chrono::Utc::now().timestamp() - auth_state.created_at;
        if age > 600 {
            return Err(PluginError::ExecutionFailed(
                "authorization state expired (>10 minutes)".into(),
            ));
        }

        // Exchange code for tokens
        let token_url = self
            .config
            .token_endpoint()
            .map_err(PluginError::ExecutionFailed)?;

        let client_secret = self
            .config
            .client_secret_ref
            .resolve()
            .map_err(PluginError::ExecutionFailed)?;

        let client = reqwest::Client::new();
        let response = client
            .post(&token_url)
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", &self.config.redirect_uri),
                ("client_id", &self.config.client_id),
                ("client_secret", &client_secret),
                ("code_verifier", &auth_state.pkce_verifier),
            ])
            .send()
            .await
            .map_err(|e| PluginError::ExecutionFailed(format!("token exchange failed: {e}")))?;

        let status = response.status();
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| PluginError::ExecutionFailed(format!("failed to parse token response: {e}")))?;

        if !status.is_success() {
            let error = body
                .get("error_description")
                .or_else(|| body.get("error"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            return Err(PluginError::ExecutionFailed(format!(
                "token exchange failed ({status}): {error}"
            )));
        }

        // Parse and store tokens
        let now = chrono::Utc::now().timestamp();
        let expires_in = body.get("expires_in").and_then(|v| v.as_i64());

        let tokens = StoredTokens {
            access_token: body["access_token"]
                .as_str()
                .ok_or_else(|| PluginError::ExecutionFailed("no access_token in response".into()))?
                .to_string(),
            refresh_token: body.get("refresh_token").and_then(|v| v.as_str()).map(String::from),
            token_type: body
                .get("token_type")
                .and_then(|v| v.as_str())
                .unwrap_or("Bearer")
                .to_string(),
            expires_at: expires_in.map(|e| now + e),
            scopes: body
                .get("scope")
                .and_then(|v| v.as_str())
                .map(|s| s.split(' ').map(String::from).collect())
                .unwrap_or_default(),
            provider: self.config.name.clone(),
        };

        self.token_store
            .store_tokens(&tokens)
            .map_err(PluginError::ExecutionFailed)?;

        debug!(provider = %self.config.name, "tokens stored successfully");

        Ok(serde_json::json!({
            "success": true,
            "provider": self.config.name,
            "token_type": tokens.token_type,
            "has_refresh_token": tokens.refresh_token.is_some(),
            "expires_at": tokens.expires_at
        }))
    }
}

// ---------------------------------------------------------------------------
// OAuth2RefreshTool
// ---------------------------------------------------------------------------

/// Tool that refreshes an OAuth2 access token using the stored refresh token.
pub struct OAuth2RefreshTool {
    config: OAuth2ProviderConfig,
    token_store: TokenStore,
}

impl OAuth2RefreshTool {
    pub fn new(config: OAuth2ProviderConfig, token_store: TokenStore) -> Self {
        Self {
            config,
            token_store,
        }
    }
}

#[async_trait]
impl Tool for OAuth2RefreshTool {
    fn name(&self) -> &str {
        "oauth2_refresh"
    }

    fn description(&self) -> &str {
        "Refresh an OAuth2 access token using the stored refresh token"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        _params: serde_json::Value,
        _ctx: &dyn ToolContext,
    ) -> Result<serde_json::Value, PluginError> {
        let existing = self
            .token_store
            .load_tokens(&self.config.name)
            .map_err(PluginError::ExecutionFailed)?
            .ok_or_else(|| {
                PluginError::ExecutionFailed(format!(
                    "no stored tokens for provider '{}'",
                    self.config.name
                ))
            })?;

        let refresh_token = existing.refresh_token.as_deref().ok_or_else(|| {
            PluginError::ExecutionFailed("no refresh token available".into())
        })?;

        let token_url = self
            .config
            .token_endpoint()
            .map_err(PluginError::ExecutionFailed)?;

        let client_secret = self
            .config
            .client_secret_ref
            .resolve()
            .map_err(PluginError::ExecutionFailed)?;

        let client = reqwest::Client::new();
        let response = client
            .post(&token_url)
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
                ("client_id", &self.config.client_id),
                ("client_secret", &client_secret),
            ])
            .send()
            .await
            .map_err(|e| PluginError::ExecutionFailed(format!("refresh request failed: {e}")))?;

        let status = response.status();
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| PluginError::ExecutionFailed(format!("failed to parse response: {e}")))?;

        if !status.is_success() {
            let error = body
                .get("error_description")
                .or_else(|| body.get("error"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            return Err(PluginError::ExecutionFailed(format!(
                "token refresh failed ({status}): {error}"
            )));
        }

        let now = chrono::Utc::now().timestamp();
        let expires_in = body.get("expires_in").and_then(|v| v.as_i64());

        // Some providers rotate refresh tokens; persist the new one immediately
        let new_refresh = body
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or(existing.refresh_token);

        let tokens = StoredTokens {
            access_token: body["access_token"]
                .as_str()
                .ok_or_else(|| PluginError::ExecutionFailed("no access_token in response".into()))?
                .to_string(),
            refresh_token: new_refresh,
            token_type: body
                .get("token_type")
                .and_then(|v| v.as_str())
                .unwrap_or("Bearer")
                .to_string(),
            expires_at: expires_in.map(|e| now + e),
            scopes: existing.scopes,
            provider: self.config.name.clone(),
        };

        // Persist immediately (critical for rotated refresh tokens)
        self.token_store
            .store_tokens(&tokens)
            .map_err(PluginError::ExecutionFailed)?;

        debug!(provider = %self.config.name, "tokens refreshed");

        Ok(serde_json::json!({
            "success": true,
            "provider": self.config.name,
            "expires_at": tokens.expires_at
        }))
    }
}

// ---------------------------------------------------------------------------
// RestRequestTool
// ---------------------------------------------------------------------------

/// Tool that makes authenticated REST requests using stored OAuth2 tokens.
pub struct RestRequestTool {
    config: OAuth2ProviderConfig,
    token_store: TokenStore,
}

impl RestRequestTool {
    pub fn new(config: OAuth2ProviderConfig, token_store: TokenStore) -> Self {
        Self {
            config,
            token_store,
        }
    }
}

#[async_trait]
impl Tool for RestRequestTool {
    fn name(&self) -> &str {
        "rest_request"
    }

    fn description(&self) -> &str {
        "Make an authenticated REST request using stored OAuth2 tokens"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "method": {
                    "type": "string",
                    "enum": ["GET", "POST", "PUT", "PATCH", "DELETE"],
                    "description": "HTTP method"
                },
                "url": {
                    "type": "string",
                    "description": "Request URL"
                },
                "body": {
                    "type": "object",
                    "description": "Request body (for POST, PUT, PATCH)"
                },
                "headers": {
                    "type": "object",
                    "description": "Additional request headers",
                    "additionalProperties": { "type": "string" }
                }
            },
            "required": ["method", "url"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &dyn ToolContext,
    ) -> Result<serde_json::Value, PluginError> {
        let method = params
            .get("method")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PluginError::ExecutionFailed("method is required".into()))?;
        let url = params
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PluginError::ExecutionFailed("url is required".into()))?;

        // Load tokens
        let tokens = self
            .token_store
            .load_tokens(&self.config.name)
            .map_err(PluginError::ExecutionFailed)?
            .ok_or_else(|| {
                PluginError::ExecutionFailed(format!(
                    "no stored tokens for provider '{}'; run oauth2_authorize first",
                    self.config.name
                ))
            })?;

        if tokens.is_expired() {
            return Err(PluginError::ExecutionFailed(
                "access token expired; run oauth2_refresh first".into(),
            ));
        }

        let client = reqwest::Client::new();

        let http_method = match method.to_uppercase().as_str() {
            "GET" => reqwest::Method::GET,
            "POST" => reqwest::Method::POST,
            "PUT" => reqwest::Method::PUT,
            "PATCH" => reqwest::Method::PATCH,
            "DELETE" => reqwest::Method::DELETE,
            other => {
                return Err(PluginError::ExecutionFailed(format!(
                    "unsupported HTTP method: {other}"
                )));
            }
        };

        let mut request = client
            .request(http_method, url)
            .bearer_auth(&tokens.access_token);

        // Add custom headers
        if let Some(headers) = params.get("headers").and_then(|v| v.as_object()) {
            for (key, value) in headers {
                if let Some(val) = value.as_str() {
                    request = request.header(key, val);
                }
            }
        }

        // Add body
        if let Some(body) = params.get("body") {
            request = request.json(body);
        }

        let response = request
            .send()
            .await
            .map_err(|e| PluginError::ExecutionFailed(format!("REST request failed: {e}")))?;

        let status = response.status().as_u16();
        let mut resp_headers = HashMap::new();
        for (key, value) in response.headers() {
            if let Ok(val) = value.to_str() {
                resp_headers.insert(key.to_string(), val.to_string());
            }
        }

        let body = response
            .text()
            .await
            .map_err(|e| PluginError::ExecutionFailed(format!("failed to read response: {e}")))?;

        let result = RestResponse {
            status,
            headers: resp_headers,
            body,
        };

        serde_json::to_value(&result).map_err(PluginError::from)
    }
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

/// Create all OAuth2 tools for a provider configuration.
pub fn all_oauth2_tools(
    config: OAuth2ProviderConfig,
    token_store: TokenStore,
) -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(OAuth2AuthorizeTool::new(config.clone(), token_store.clone())),
        Box::new(OAuth2CallbackTool::new(config.clone(), token_store.clone())),
        Box::new(OAuth2RefreshTool::new(config.clone(), token_store.clone())),
        Box::new(RestRequestTool::new(config, token_store)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_state_is_64_hex_chars() {
        let state = generate_state();
        assert_eq!(state.len(), 64);
        assert!(state.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn generate_pkce_verifier_is_nonempty() {
        let verifier = generate_pkce_verifier();
        assert!(!verifier.is_empty());
        assert!(verifier.len() >= 32);
    }

    #[test]
    fn pkce_challenge_is_deterministic() {
        let verifier = "test-verifier";
        let c1 = compute_pkce_challenge(verifier);
        let c2 = compute_pkce_challenge(verifier);
        assert_eq!(c1, c2);
    }

    #[test]
    fn pkce_challenge_differs_for_different_verifiers() {
        let c1 = compute_pkce_challenge("verifier-1");
        let c2 = compute_pkce_challenge("verifier-2");
        assert_ne!(c1, c2);
    }

    #[test]
    fn all_tools_returns_four() {
        let config = OAuth2ProviderConfig {
            name: "test".to_string(),
            preset: types::ProviderPreset::Google,
            client_id: "test-id".to_string(),
            client_secret_ref: types::SecretRef {
                env_var: "TEST_SECRET".to_string(),
            },
            auth_url: None,
            token_url: None,
            scopes: vec!["email".to_string()],
            redirect_uri: "http://localhost:8085/callback".to_string(),
        };
        let store = TokenStore::with_dir(std::path::PathBuf::from("/tmp/test"));
        let tools = all_oauth2_tools(config, store);
        assert_eq!(tools.len(), 4);

        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"oauth2_authorize"));
        assert!(names.contains(&"oauth2_callback"));
        assert!(names.contains(&"oauth2_refresh"));
        assert!(names.contains(&"rest_request"));
    }

    #[test]
    fn tool_schemas_are_objects() {
        let config = OAuth2ProviderConfig {
            name: "test".to_string(),
            preset: types::ProviderPreset::Google,
            client_id: "test-id".to_string(),
            client_secret_ref: types::SecretRef {
                env_var: "TEST_SECRET".to_string(),
            },
            auth_url: None,
            token_url: None,
            scopes: vec![],
            redirect_uri: "http://localhost:8085/callback".to_string(),
        };
        let store = TokenStore::with_dir(std::path::PathBuf::from("/tmp/test"));
        let tools = all_oauth2_tools(config, store);
        for tool in &tools {
            let schema = tool.parameters_schema();
            assert!(schema.is_object(), "schema not object for {}", tool.name());
        }
    }
}
