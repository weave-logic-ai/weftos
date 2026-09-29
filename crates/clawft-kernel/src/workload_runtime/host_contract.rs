//! Host contract common to every cog runtime (COG-001 section 4):
//! `COG_CSI_BIND`, `COG_SENSOR_URL`, a per-instance `COGNITUM_COG_TOKEN`
//! and `COGNITUM_COG_DATA_DIR`.
//!
//! The token is a secret: it is handed to the instance's environment only,
//! and anything written to the chain carries [`HostContract::token_hash`].

use std::net::SocketAddr;

use clawft_types::secret::SecretString;
use rand::RngCore;

use super::types::RuntimeError;

/// Env var: UDP bind address for the ESP32 CSI feed.
pub const ENV_CSI_BIND: &str = "COG_CSI_BIND";
/// Env var: HTTP sensor fallback (`host:port`).
pub const ENV_SENSOR_URL: &str = "COG_SENSOR_URL";
/// Env var: per-instance token accepted by that instance's ingest bridge.
pub const ENV_TOKEN: &str = "COGNITUM_COG_TOKEN";
/// Env var: writable data directory.
pub const ENV_DATA_DIR: &str = "COGNITUM_COG_DATA_DIR";
/// Default CSI feed port (ADR-069).
pub const DEFAULT_CSI_PORT: u16 = 5006;

/// Host contract for one instance.
#[derive(Debug, Clone)]
pub struct HostContract {
    /// UDP bind the cog listens on for the sensor feed.
    pub csi_bind: SocketAddr,
    /// HTTP sensor fallback address (`host:port`), if any.
    pub sensor_url: Option<SocketAddr>,
    /// Per-instance token.
    pub token: SecretString,
    /// The node's ingest bridge as the instance's network sees it. A cog
    /// always posts to its own `127.0.0.1:80`; container adapters relay
    /// that port here ([`super::container_relay`]). `None` means the cog's
    /// loopback already reaches the bridge (native, `network=host`).
    pub ingest_upstream: Option<SocketAddr>,
}

impl HostContract {
    /// Contract binding the feed on `csi_bind`, with a fresh random token.
    pub fn new(csi_bind: SocketAddr) -> Self {
        Self {
            csi_bind,
            sensor_url: None,
            token: Self::fresh_token(),
            ingest_upstream: None,
        }
    }

    /// Default: feed on `0.0.0.0:5006`.
    pub fn default_feed() -> Self {
        Self::new(SocketAddr::from(([0, 0, 0, 0], DEFAULT_CSI_PORT)))
    }

    /// Builder: HTTP sensor fallback.
    pub fn with_sensor(mut self, addr: SocketAddr) -> Self {
        self.sensor_url = Some(addr);
        self
    }

    /// Builder: ingest bridge address as seen from inside the instance.
    pub fn with_ingest_upstream(mut self, addr: SocketAddr) -> Self {
        self.ingest_upstream = Some(addr);
        self
    }

    /// 32 random bytes, lower-case hex.
    pub fn fresh_token() -> SecretString {
        let mut b = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut b);
        SecretString::new(crate::workload_pkg::codec::hex_encode(&b))
    }

    /// Short BLAKE3 fingerprint of the token, safe for chain payloads.
    pub fn token_hash(&self) -> String {
        blake3::hash(self.token.expose().as_bytes()).to_hex()[..16].to_string()
    }

    /// Validate before use.
    pub fn validate(&self) -> Result<(), RuntimeError> {
        if self.csi_bind.port() == 0 {
            return Err(RuntimeError::InvalidConfig("csi_bind needs a port".into()));
        }
        let t = self.token.expose();
        if t.len() < 32 || t.len() > 256 || !t.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(RuntimeError::InvalidConfig(
                "cog token must be 32..256 printable characters".into(),
            ));
        }
        Ok(())
    }

    /// Environment for the instance. `data_dir` is where the runtime mounts
    /// or creates the instance's writable directory.
    pub fn env(&self, data_dir: &str) -> Vec<(String, String)> {
        let mut env = vec![
            (ENV_CSI_BIND.to_string(), self.csi_bind.to_string()),
            (ENV_TOKEN.to_string(), self.token.expose().to_string()),
            (ENV_DATA_DIR.to_string(), data_dir.to_string()),
        ];
        if let Some(s) = self.sensor_url {
            env.push((ENV_SENSOR_URL.to_string(), s.to_string()));
        }
        env
    }

    /// Chain-safe summary (token as hash only).
    pub fn audit(&self) -> serde_json::Value {
        serde_json::json!({
            "csi_bind": self.csi_bind.to_string(),
            "sensor_url": self.sensor_url.map(|s| s.to_string()),
            "token_hash": self.token_hash(),
            "ingest_upstream": self.ingest_upstream.map(|s| s.to_string()),
        })
    }
}
