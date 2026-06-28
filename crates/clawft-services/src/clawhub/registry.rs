//! ClawHub skill registry client and types.
//!
//! Implements the REST API contract (Contract #20):
//!
//! ```text
//! GET  /api/v1/skills/search?q=&limit=&offset=
//! GET  /api/v1/skills/{id}/download
//! POST /api/v1/skills/publish
//!
//! Auth: Bearer <token>
//! Response: { "ok": bool, "data": T, "error": Option<String>, "pagination": {...} }
//! ```

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors from ClawHub API operations.
#[derive(Error, Debug)]
pub enum ClawHubError {
    /// Network or HTTP transport failure.
    #[error("HTTP request failed: {0}")]
    Http(String),

    /// The server returned a non-success status code.
    #[error("server error (HTTP {status}): {body}")]
    ServerError {
        /// HTTP status code.
        status: u16,
        /// Response body (possibly truncated).
        body: String,
    },

    /// Failed to parse the response body as JSON.
    #[error("failed to parse response: {0}")]
    ParseError(String),

    /// The API returned `ok: false` with an error message.
    #[error("API error: {0}")]
    ApiError(String),

    /// The server is unreachable (connection refused, DNS failure, etc.).
    #[error("server unreachable at {url}: {reason}")]
    Unreachable {
        /// The URL that was attempted.
        url: String,
        /// Why the connection failed.
        reason: String,
    },

    /// I/O error during file operations.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// ClawHub API response wrapper.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiResponse<T> {
    /// Whether the request succeeded.
    pub ok: bool,
    /// Response data (present on success).
    pub data: Option<T>,
    /// Error message (present on failure).
    pub error: Option<String>,
    /// Pagination info (present for list/search endpoints).
    pub pagination: Option<Pagination>,
}

/// Pagination metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pagination {
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
}

/// A skill entry in the ClawHub registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillEntry {
    /// Unique skill identifier.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Skill description.
    pub description: String,
    /// Semantic version.
    pub version: String,
    /// Author identifier.
    pub author: String,
    /// Star count.
    pub stars: u32,
    /// Content hash (SHA-256) for verification.
    pub content_hash: String,
    /// Whether the skill is signed.
    pub signed: bool,
    /// Signature (if signed).
    pub signature: Option<String>,
    /// Publication timestamp (ISO 8601).
    pub published_at: String,
    /// Tags for categorization.
    pub tags: Vec<String>,
}

/// Request body for publishing a skill.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishRequest {
    /// Skill name.
    pub name: String,
    /// Skill description.
    pub description: String,
    /// Semantic version.
    pub version: String,
    /// Skill content (base64-encoded archive).
    pub content: String,
    /// Content hash (SHA-256).
    pub content_hash: String,
    /// Digital signature.
    pub signature: Option<String>,
    /// Public key used for signing (hex).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_key: Option<String>,
    /// Tags.
    pub tags: Vec<String>,
}

/// Result of installing a skill.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillInstallResult {
    /// Whether installation succeeded.
    pub success: bool,
    /// Installed skill path.
    pub install_path: Option<String>,
    /// Security scan results (if scan was run).
    pub security_scan_passed: Option<bool>,
    /// Error message (if failed).
    pub error: Option<String>,
}

/// ClawHub client configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClawHubConfig {
    /// Registry API base URL.
    pub api_url: String,
    /// API token for authentication.
    pub api_token: Option<String>,
    /// Whether to allow unsigned skills (local dev only).
    pub allow_unsigned: bool,
}

impl Default for ClawHubConfig {
    fn default() -> Self {
        Self {
            api_url: "https://hub.clawft.dev/api/v1".to_string(),
            api_token: None,
            allow_unsigned: false,
        }
    }
}

impl ClawHubConfig {
    /// Build a config from environment variables with sensible defaults.
    ///
    /// - `CLAWHUB_API_URL` overrides the default API URL.
    /// - `CLAWHUB_API_TOKEN` sets the auth token.
    pub fn from_env() -> Self {
        let api_url = std::env::var("CLAWHUB_API_URL")
            .unwrap_or_else(|_| "http://localhost:3000/api/v1".to_string());
        let api_token = std::env::var("CLAWHUB_API_TOKEN").ok();

        Self {
            api_url,
            api_token,
            allow_unsigned: false,
        }
    }
}

/// ClawHub registry client.
pub struct ClawHubClient {
    config: ClawHubConfig,
    http: reqwest::Client,
}

impl ClawHubClient {
    /// Create a new client with the given config.
    pub fn new(config: ClawHubConfig) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_default();
        Self { config, http }
    }

    /// Build a request with optional auth header.
    fn auth_header(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(ref token) = self.config.api_token {
            builder.header("Authorization", format!("Bearer {token}"))
        } else {
            builder
        }
    }

    /// Search for skills by query.
    pub async fn search(
        &self,
        query: &str,
        limit: usize,
        offset: usize,
    ) -> Result<ApiResponse<Vec<SkillEntry>>, ClawHubError> {
        let url = format!(
            "{}/skills/search?q={}&limit={}&offset={}",
            self.config.api_url,
            urlencoding::encode(query),
            limit,
            offset,
        );

        let request = self.auth_header(self.http.get(&url));

        let response = request.send().await.map_err(|e| {
            if e.is_connect() || e.is_timeout() {
                ClawHubError::Unreachable {
                    url: url.clone(),
                    reason: e.to_string(),
                }
            } else {
                ClawHubError::Http(e.to_string())
            }
        })?;

        let status = response.status().as_u16();
        if status >= 400 {
            let body = response.text().await.unwrap_or_default();
            return Err(ClawHubError::ServerError {
                status,
                body: truncate_body(&body),
            });
        }

        let api_resp: ApiResponse<Vec<SkillEntry>> = response
            .json()
            .await
            .map_err(|e| ClawHubError::ParseError(e.to_string()))?;

        if !api_resp.ok
            && let Some(ref err) = api_resp.error
        {
            return Err(ClawHubError::ApiError(err.clone()));
        }

        Ok(api_resp)
    }

    /// Publish a skill to the registry.
    pub async fn publish(
        &self,
        request: &PublishRequest,
    ) -> Result<ApiResponse<SkillEntry>, ClawHubError> {
        // Client-side validation: require signature unless allow_unsigned.
        if request.signature.is_none() && !self.config.allow_unsigned {
            return Ok(ApiResponse {
                ok: false,
                data: None,
                error: Some(
                    "skill must be signed for publication. \
                     Use --allow-unsigned for local dev only."
                        .into(),
                ),
                pagination: None,
            });
        }

        if self.config.allow_unsigned && request.signature.is_none() {
            tracing::warn!(
                skill = %request.name,
                "publishing unsigned skill (--allow-unsigned flag used)"
            );
        }

        let url = format!("{}/skills/publish", self.config.api_url);

        let http_request = self.auth_header(self.http.post(&url)).json(request);

        let response = http_request.send().await.map_err(|e| {
            if e.is_connect() || e.is_timeout() {
                ClawHubError::Unreachable {
                    url: url.clone(),
                    reason: e.to_string(),
                }
            } else {
                ClawHubError::Http(e.to_string())
            }
        })?;

        let status = response.status().as_u16();
        if status >= 400 {
            let body = response.text().await.unwrap_or_default();
            return Err(ClawHubError::ServerError {
                status,
                body: truncate_body(&body),
            });
        }

        let api_resp: ApiResponse<SkillEntry> = response
            .json()
            .await
            .map_err(|e| ClawHubError::ParseError(e.to_string()))?;

        if !api_resp.ok
            && let Some(ref err) = api_resp.error
        {
            return Err(ClawHubError::ApiError(err.clone()));
        }

        Ok(api_resp)
    }

    /// Download skill content from the registry.
    ///
    /// Returns the raw bytes of the skill archive.
    pub async fn download(&self, skill_id: &str) -> Result<Vec<u8>, ClawHubError> {
        let url = format!(
            "{}/skills/{}/download",
            self.config.api_url,
            urlencoding::encode(skill_id),
        );

        let request = self.auth_header(self.http.get(&url));

        let response = request.send().await.map_err(|e| {
            if e.is_connect() || e.is_timeout() {
                ClawHubError::Unreachable {
                    url: url.clone(),
                    reason: e.to_string(),
                }
            } else {
                ClawHubError::Http(e.to_string())
            }
        })?;

        let status = response.status().as_u16();
        if status >= 400 {
            let body = response.text().await.unwrap_or_default();
            return Err(ClawHubError::ServerError {
                status,
                body: truncate_body(&body),
            });
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| ClawHubError::Http(e.to_string()))?;

        Ok(bytes.to_vec())
    }

    /// Install a skill from the registry by downloading and writing to disk.
    pub async fn install(
        &self,
        skill_id: &str,
        install_dir: &str,
    ) -> Result<SkillInstallResult, ClawHubError> {
        tracing::info!(
            skill = %skill_id,
            dir = %install_dir,
            "installing skill from ClawHub"
        );

        // Download the skill content.
        let content = self.download(skill_id).await?;

        // Create the install directory.
        let dest = std::path::Path::new(install_dir).join(skill_id);
        std::fs::create_dir_all(&dest)?;

        // Write content as SKILL.md (the server returns the skill content).
        let skill_file = dest.join("SKILL.md");
        std::fs::write(&skill_file, &content)?;

        Ok(SkillInstallResult {
            success: true,
            install_path: Some(dest.to_string_lossy().to_string()),
            security_scan_passed: None,
            error: None,
        })
    }

    /// Get the client configuration.
    pub fn config(&self) -> &ClawHubConfig {
        &self.config
    }
}

/// Truncate response body for error messages.
fn truncate_body(body: &str) -> String {
    if body.len() > 500 {
        format!("{}... (truncated)", &body[..500])
    } else {
        body.to_string()
    }
}

/// URL-encode a string for use in query parameters.
mod urlencoding {
    pub fn encode(s: &str) -> String {
        let mut encoded = String::with_capacity(s.len());
        for byte in s.bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    encoded.push(byte as char);
                }
                _ => {
                    encoded.push_str(&format!("%{byte:02X}"));
                }
            }
        }
        encoded
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let config = ClawHubConfig::default();
        assert!(!config.allow_unsigned);
        assert!(config.api_token.is_none());
    }

    #[test]
    fn config_from_env_uses_defaults() {
        // When env vars are not set, from_env uses localhost defaults.
        let config = ClawHubConfig::from_env();
        assert!(config.api_url.contains("localhost") || config.api_url.contains("hub.clawft.dev"));
    }

    #[test]
    fn api_response_serialization() {
        let response: ApiResponse<Vec<SkillEntry>> = ApiResponse {
            ok: true,
            data: Some(vec![]),
            error: None,
            pagination: Some(Pagination {
                total: 0,
                offset: 0,
                limit: 10,
            }),
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"ok\":true"));
    }

    #[test]
    fn publish_request_requires_signature() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let client = ClawHubClient::new(ClawHubConfig::default());
        let request = PublishRequest {
            name: "test-skill".into(),
            description: "A test skill".into(),
            version: "1.0.0".into(),
            content: "base64content".into(),
            content_hash: "abc123def456".into(),
            signature: None,
            public_key: None,
            tags: vec!["test".into()],
        };
        let result = rt.block_on(client.publish(&request)).unwrap();
        assert!(!result.ok, "unsigned publish should fail");
        assert!(result.error.unwrap().contains("signed"));
    }

    #[test]
    fn urlencoding_basic() {
        assert_eq!(urlencoding::encode("hello world"), "hello%20world");
        assert_eq!(urlencoding::encode("test"), "test");
        assert_eq!(urlencoding::encode("a+b"), "a%2Bb");
    }

    #[test]
    fn truncate_body_short() {
        assert_eq!(truncate_body("short"), "short");
    }

    #[test]
    fn truncate_body_long() {
        let long = "x".repeat(600);
        let result = truncate_body(&long);
        assert!(result.len() < 600);
        assert!(result.contains("truncated"));
    }

    #[test]
    fn skill_entry_serde_roundtrip() {
        let entry = SkillEntry {
            id: "skill-123".into(),
            name: "test-skill".into(),
            description: "A test".into(),
            version: "1.0.0".into(),
            author: "dev".into(),
            stars: 5,
            content_hash: "abcdef".into(),
            signed: true,
            signature: Some("sig123".into()),
            published_at: "2026-01-01T00:00:00Z".into(),
            tags: vec!["test".into()],
        };
        let json = serde_json::to_string(&entry).unwrap();
        let restored: SkillEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.name, "test-skill");
        assert_eq!(restored.stars, 5);
    }

    #[test]
    fn publish_request_with_public_key_serializes() {
        let request = PublishRequest {
            name: "test".into(),
            description: "desc".into(),
            version: "1.0.0".into(),
            content: "data".into(),
            content_hash: "hash".into(),
            signature: Some("sig".into()),
            public_key: Some("pubkey".into()),
            tags: vec![],
        };
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("public_key"));
    }

    #[test]
    fn publish_request_without_public_key_omits_field() {
        let request = PublishRequest {
            name: "test".into(),
            description: "desc".into(),
            version: "1.0.0".into(),
            content: "data".into(),
            content_hash: "hash".into(),
            signature: None,
            public_key: None,
            tags: vec![],
        };
        let json = serde_json::to_string(&request).unwrap();
        assert!(!json.contains("public_key"));
    }

    #[test]
    fn clawhub_error_display() {
        let err = ClawHubError::Unreachable {
            url: "http://localhost:3000".into(),
            reason: "connection refused".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("localhost"));
        assert!(msg.contains("connection refused"));
    }

    #[test]
    fn install_result_serde() {
        let result = SkillInstallResult {
            success: true,
            install_path: Some("/home/user/.clawft/skills/test".into()),
            security_scan_passed: Some(true),
            error: None,
        };
        let json = serde_json::to_string(&result).unwrap();
        let restored: SkillInstallResult = serde_json::from_str(&json).unwrap();
        assert!(restored.success);
        assert_eq!(
            restored.install_path.as_deref(),
            Some("/home/user/.clawft/skills/test")
        );
    }
}
