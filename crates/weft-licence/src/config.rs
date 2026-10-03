//! Service configuration (`/etc/weft-licence/config.toml`).

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::SvcError;

/// The same build-time clock floor the COG-011 bridge uses: before this unix
/// time the Seed's clock (no RTC) is treated as not set. 2026-09-21 UTC.
/// ASSUMPTION: the COG-011 text is not in this repository, so the value is
/// copied from the ADR-106 wording ("same floor as the bridge") and must be
/// kept equal to the bridge's constant when COG-011 lands in tree.
pub const CLOCK_FLOOR: u64 = 1_790_000_000;

/// Load limits (ADR-106 section 6, 7). Every field is configurable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    /// Checkouts or transfers in flight at once.
    pub in_flight: u32,
    /// Steward requests per minute (charged after the signature verifies).
    pub requests_per_min: u32,
    /// Unsigned callers (identity, refused requests) per minute, all together.
    pub unsigned_per_min: u32,
    /// Largest artifact, bytes.
    pub max_artifact_bytes: u64,
    /// Transfer rate to the steward, bytes per second.
    pub rate_bytes_per_sec: u64,
    /// Byte transfers per (cog, version, arch) per steward key per 24 h.
    pub serves_per_day: u32,
    /// Artifact cache size, bytes (entries under an active grant are kept).
    pub cache_bytes: u64,
    /// Most grants in one renewal or listing batch.
    pub renew_batch: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            in_flight: 1,
            requests_per_min: 10,
            unsigned_per_min: 30,
            max_artifact_bytes: 64 * 1024 * 1024,
            rate_bytes_per_sec: 4 * 1024 * 1024,
            serves_per_day: 3,
            cache_bytes: 256 * 1024 * 1024,
            renew_batch: 256,
        }
    }
}

/// The service configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// State directory: key, binding, slots, cache. Mode 0700.
    pub state_dir: PathBuf,
    /// Addresses to listen on: the USB link-local and tailnet interfaces.
    /// `0.0.0.0` and `::` are refused.
    pub listen: Vec<SocketAddr>,
    /// The Seed's device id (must match the binding).
    pub device_id: String,
    /// Pinned operator public keys, 64 hex each, for bindings, the licence
    /// file and serve overrides. `init --operator-key` adds to the state dir.
    pub operator_pubkeys: Vec<String>,
    /// Operator-signed licence file.
    pub licence_file: PathBuf,
    /// Cognitum registry (`app-registry.json`) URL. https only.
    pub registry_url: String,
    /// Lab only: allow a local path or http registry (tests, never the Seed).
    pub allow_insecure_registry: bool,
    /// Grant lifetime, seconds (default 72 h, at most 7 days).
    pub grant_ttl_secs: u64,
    /// Clock floor, unix seconds; see [`CLOCK_FLOOR`].
    pub clock_floor: u64,
    /// Signed-request replay window, seconds each side.
    pub request_window_secs: u64,
    /// Limits.
    pub limits: Limits,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            state_dir: PathBuf::from("/var/lib/weft-licence"),
            listen: Vec::new(),
            device_id: String::new(),
            operator_pubkeys: Vec::new(),
            licence_file: PathBuf::from("/var/lib/weft-licence/licence.json"),
            registry_url: String::new(),
            allow_insecure_registry: false,
            grant_ttl_secs: 72 * 3600,
            clock_floor: CLOCK_FLOOR,
            request_window_secs: 120,
            limits: Limits::default(),
        }
    }
}

impl Config {
    /// Parse a TOML file and validate it.
    pub fn load(path: &Path) -> Result<Self, SvcError> {
        let text = std::fs::read_to_string(path).map_err(|e| SvcError::Config(e.to_string()))?;
        let c: Config = toml::from_str(&text).map_err(|e| SvcError::Config(e.to_string()))?;
        c.validate()?;
        Ok(c)
    }

    /// Refuse a configuration that could expose the listener or break a rule.
    pub fn validate(&self) -> Result<(), SvcError> {
        let bad = |m: &str| Err(SvcError::Config(m.to_string()));
        if self.listen.iter().any(|a| a.ip().is_unspecified()) {
            return bad("listen: 0.0.0.0 and :: are refused; name the USB and tailnet addresses");
        }
        if self.grant_ttl_secs == 0 || self.grant_ttl_secs > weft_licence_wire::MAX_GRANT_TTL_SECS {
            return bad("grant_ttl_secs must be between 1 s and 7 days");
        }
        if self.limits.in_flight == 0 || self.limits.requests_per_min == 0 {
            return bad("limits: in_flight and requests_per_min must be above 0");
        }
        if self.limits.renew_batch == 0 || self.limits.renew_batch > 256 {
            return bad("limits.renew_batch must be 1 to 256");
        }
        Ok(())
    }
}
