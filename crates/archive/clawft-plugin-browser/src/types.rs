//! Types for browser CDP automation.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Sandbox configuration for headless Chrome automation.
///
/// This config is enforced at runtime to limit the browser's
/// capabilities and prevent security issues.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserSandboxConfig {
    /// Domains the browser is allowed to navigate to. Empty = block all.
    #[serde(default)]
    pub allowed_domains: Vec<String>,

    /// Maximum number of concurrent browser pages per agent.
    #[serde(default = "default_max_concurrent_pages")]
    pub max_concurrent_pages: u32,

    /// Maximum browser session lifetime before forced termination.
    #[serde(
        default = "default_session_lifetime",
        with = "duration_serde"
    )]
    pub session_lifetime: Duration,

    /// Maximum memory for the browser process (MB).
    #[serde(default = "default_max_memory_mb")]
    pub max_memory_mb: u64,

    /// Whether to clear cookies/storage between sessions.
    #[serde(default = "default_clear_state")]
    pub clear_state_between_sessions: bool,
}

fn default_max_concurrent_pages() -> u32 {
    2
}

fn default_session_lifetime() -> Duration {
    Duration::from_secs(300)
}

fn default_max_memory_mb() -> u64 {
    512
}

fn default_clear_state() -> bool {
    true
}

impl Default for BrowserSandboxConfig {
    fn default() -> Self {
        Self {
            allowed_domains: Vec::new(),
            max_concurrent_pages: default_max_concurrent_pages(),
            session_lifetime: default_session_lifetime(),
            max_memory_mb: default_max_memory_mb(),
            clear_state_between_sessions: default_clear_state(),
        }
    }
}

/// Serde helper for Duration as seconds.
mod duration_serde {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        duration.as_secs().serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let secs = u64::deserialize(deserializer)?;
        Ok(Duration::from_secs(secs))
    }
}

/// Blocked URL schemes that must never be navigated to.
pub const BLOCKED_SCHEMES: &[&str] = &["file", "data", "javascript"];

/// Validate a URL against the sandbox configuration.
///
/// Returns an error message if the URL is not allowed.
pub fn validate_url(url: &str, config: &BrowserSandboxConfig) -> Result<(), String> {
    // Parse the URL
    let parsed = url::Url::parse(url).map_err(|e| format!("invalid URL: {e}"))?;

    // Check blocked schemes
    let scheme = parsed.scheme().to_lowercase();
    if BLOCKED_SCHEMES.contains(&scheme.as_str()) {
        return Err(format!("blocked URL scheme: '{scheme}://'"));
    }

    // Check allowed domains (empty = block all navigation)
    if config.allowed_domains.is_empty() {
        return Err("no allowed domains configured".to_string());
    }

    if let Some(host) = parsed.host_str() {
        let host_lower = host.to_lowercase();
        let allowed = config.allowed_domains.iter().any(|d| {
            let d_lower = d.to_lowercase();
            host_lower == d_lower || host_lower.ends_with(&format!(".{d_lower}"))
        });
        if !allowed {
            return Err(format!("domain '{host}' not in allowed_domains"));
        }
    } else {
        return Err("URL has no host".to_string());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let config = BrowserSandboxConfig::default();
        assert!(config.allowed_domains.is_empty());
        assert_eq!(config.max_concurrent_pages, 2);
        assert_eq!(config.session_lifetime, Duration::from_secs(300));
        assert_eq!(config.max_memory_mb, 512);
        assert!(config.clear_state_between_sessions);
    }

    #[test]
    fn config_serde_roundtrip() {
        let config = BrowserSandboxConfig {
            allowed_domains: vec!["example.com".into()],
            max_concurrent_pages: 3,
            session_lifetime: Duration::from_secs(600),
            max_memory_mb: 1024,
            clear_state_between_sessions: false,
        };
        let json = serde_json::to_string(&config).unwrap();
        let restored: BrowserSandboxConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.allowed_domains, vec!["example.com"]);
        assert_eq!(restored.max_concurrent_pages, 3);
        assert_eq!(restored.session_lifetime, Duration::from_secs(600));
        assert_eq!(restored.max_memory_mb, 1024);
        assert!(!restored.clear_state_between_sessions);
    }

    #[test]
    fn validate_url_blocks_file_scheme() {
        let config = BrowserSandboxConfig {
            allowed_domains: vec!["example.com".into()],
            ..Default::default()
        };
        let result = validate_url("file:///etc/passwd", &config);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("blocked URL scheme"));
    }

    #[test]
    fn validate_url_blocks_data_scheme() {
        let config = BrowserSandboxConfig {
            allowed_domains: vec!["example.com".into()],
            ..Default::default()
        };
        let result = validate_url("data:text/html,<h1>evil</h1>", &config);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("blocked URL scheme"));
    }

    #[test]
    fn validate_url_blocks_javascript_scheme() {
        let config = BrowserSandboxConfig {
            allowed_domains: vec!["example.com".into()],
            ..Default::default()
        };
        let result = validate_url("javascript:alert(1)", &config);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("blocked URL scheme"));
    }

    #[test]
    fn validate_url_blocks_unlisted_domain() {
        let config = BrowserSandboxConfig {
            allowed_domains: vec!["example.com".into()],
            ..Default::default()
        };
        let result = validate_url("https://evil.com/steal", &config);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not in allowed_domains"));
    }

    #[test]
    fn validate_url_allows_listed_domain() {
        let config = BrowserSandboxConfig {
            allowed_domains: vec!["example.com".into()],
            ..Default::default()
        };
        assert!(validate_url("https://example.com/page", &config).is_ok());
    }

    #[test]
    fn validate_url_allows_subdomain() {
        let config = BrowserSandboxConfig {
            allowed_domains: vec!["example.com".into()],
            ..Default::default()
        };
        assert!(validate_url("https://www.example.com/page", &config).is_ok());
        assert!(validate_url("https://api.example.com/v1", &config).is_ok());
    }

    #[test]
    fn validate_url_empty_domains_blocks_all() {
        let config = BrowserSandboxConfig::default();
        let result = validate_url("https://example.com", &config);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no allowed domains"));
    }

    #[test]
    fn validate_url_invalid_url() {
        let config = BrowserSandboxConfig {
            allowed_domains: vec!["example.com".into()],
            ..Default::default()
        };
        let result = validate_url("not a url", &config);
        assert!(result.is_err());
    }
}
