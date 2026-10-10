//! `[router]` in `~/.weftos/weave.toml`: the tailnet router (ADR-116 R1).
//!
//! ```toml
//! [router]
//! enabled = true                  # off by default
//! listen = "127.0.0.1:18000"      # loopback only; Tailscale Serve fronts it on :443
//! poll_secs = 5                   # how often each project's compose/ports.yaml mtime is checked
//! health_timeout_ms = 1500        # per upstream health / process-compose probe
//! ```
//!
//! A section that is present but invalid stops the router from starting (the
//! daemon keeps running) and names the key at fault.

use std::net::SocketAddr;

use serde::Deserialize;
use serde_json::Value;

/// Default bind address (ADR-116 §1).
pub const DEFAULT_LISTEN: &str = "127.0.0.1:18000";
/// Bounds on `poll_secs`.
pub const MIN_POLL_SECS: u64 = 1;
/// See [`MIN_POLL_SECS`].
pub const MAX_POLL_SECS: u64 = 300;
/// Bounds on `health_timeout_ms`.
pub const MIN_HEALTH_TIMEOUT_MS: u64 = 100;
/// See [`MIN_HEALTH_TIMEOUT_MS`].
pub const MAX_HEALTH_TIMEOUT_MS: u64 = 10_000;

/// The `[router]` section.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouterConfig {
    /// Master switch (default off).
    #[serde(default)]
    pub enabled: bool,
    /// Loopback socket address the router binds.
    #[serde(default = "default_listen")]
    pub listen: String,
    /// Seconds between manifest mtime polls.
    #[serde(default = "default_poll")]
    pub poll_secs: u64,
    /// Milliseconds an upstream health or process-compose probe may take.
    #[serde(default = "default_health_timeout")]
    pub health_timeout_ms: u64,
}

fn default_listen() -> String {
    DEFAULT_LISTEN.to_owned()
}
fn default_poll() -> u64 {
    5
}
fn default_health_timeout() -> u64 {
    1500
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self { enabled: false, listen: default_listen(), poll_secs: default_poll(), health_timeout_ms: default_health_timeout() }
    }
}

impl RouterConfig {
    /// Parse the `router` table of a merged config value (absent: disabled).
    pub fn from_config(root: &Value) -> Result<Self, String> {
        match root.get("router") {
            None => Ok(Self::default()),
            Some(v) => serde_json::from_value(v.clone()).map_err(|e| format!("[router]: {e}")),
        }
    }

    /// Boundary validation (only an enabled section is checked).
    pub fn validate(&self) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        let addr = self.listen_addr()?;
        if !addr.ip().is_loopback() {
            return Err(format!("[router] listen {:?} is not a loopback address; Tailscale Serve fronts the router on :443", self.listen));
        }
        if !(MIN_POLL_SECS..=MAX_POLL_SECS).contains(&self.poll_secs) {
            return Err(format!("[router] poll_secs {} is outside {MIN_POLL_SECS}..={MAX_POLL_SECS}", self.poll_secs));
        }
        if !(MIN_HEALTH_TIMEOUT_MS..=MAX_HEALTH_TIMEOUT_MS).contains(&self.health_timeout_ms) {
            return Err(format!(
                "[router] health_timeout_ms {} is outside {MIN_HEALTH_TIMEOUT_MS}..={MAX_HEALTH_TIMEOUT_MS}",
                self.health_timeout_ms
            ));
        }
        Ok(())
    }

    /// `listen` as a socket address.
    pub fn listen_addr(&self) -> Result<SocketAddr, String> {
        self.listen.parse().map_err(|e| format!("[router] listen {:?} is not host:port: {e}", self.listen))
    }
}

/// Load `[router]` from the user's `weave.toml` (missing file: disabled).
pub async fn load(home: &std::path::Path) -> Result<RouterConfig, String> {
    use clawft_platform::{Platform, config_loader};
    let platform = clawft_platform::NativePlatform::new();
    let path = crate::user_daemon::user_weave_toml(home);
    let v = config_loader::load_weave_toml_file(platform.fs(), &path)
        .await
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let cfg = RouterConfig::from_config(&v)?;
    cfg.validate()?;
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn absent_section_is_off_and_defaults_hold() {
        let c = RouterConfig::from_config(&json!({})).unwrap();
        assert!(!c.enabled);
        assert_eq!(c.listen, DEFAULT_LISTEN);
        assert_eq!(c.poll_secs, 5);
        c.validate().unwrap();
    }

    #[test]
    fn enabled_section_must_bind_loopback() {
        let c = RouterConfig::from_config(&json!({"router": {"enabled": true, "listen": "0.0.0.0:18000"}})).unwrap();
        let e = c.validate().unwrap_err();
        assert!(e.contains("loopback"), "{e}");
        let ok = RouterConfig::from_config(&json!({"router": {"enabled": true, "listen": "[::1]:18000"}})).unwrap();
        ok.validate().unwrap();
    }

    #[test]
    fn bounds_and_unknown_keys_are_refused() {
        let c = RouterConfig::from_config(&json!({"router": {"enabled": true, "poll_secs": 0}})).unwrap();
        assert!(c.validate().unwrap_err().contains("poll_secs"));
        let c = RouterConfig::from_config(&json!({"router": {"enabled": true, "health_timeout_ms": 50}})).unwrap();
        assert!(c.validate().unwrap_err().contains("health_timeout_ms"));
        let e = RouterConfig::from_config(&json!({"router": {"enabled": true, "port": 1}})).unwrap_err();
        assert!(e.contains("[router]"), "{e}");
    }
}
