//! Seed adapter configuration, operator pins and response helpers.

use std::time::Duration;

use serde_json::Value;

use super::types::RuntimeError;

/// Adapter id.
pub const SEED_ID: &str = "remote.api.cognitum-seed";
/// Store registry name accepted for pins.
pub const SEED_REGISTRY: &str = "cognitum";
/// Seed concurrency cap (firmware 0.24.2).
pub const SEED_CONCURRENCY_CAP: usize = 3;
/// Default wall-clock cap for a console run.
pub const CONSOLE_TIMEOUT: Duration = Duration::from_secs(30);
/// Timeout for ordinary API calls.
pub const API_TIMEOUT: Duration = Duration::from_secs(20);
/// Log lines fetched as stop evidence.
pub const LOG_LINES: usize = 20;

/// An operator pin for a store cog.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SeedPin {
    /// Cog id.
    pub id: String,
    /// Exact store version.
    pub version: String,
    /// Expected store SHA-256, when pinned that tightly.
    #[serde(default)]
    pub sha256: Option<String>,
    /// Console commands allowed for this cog (default `["--once"]`).
    #[serde(default = "default_console")]
    pub console_commands: Vec<String>,
}

fn default_console() -> Vec<String> {
    vec!["--once".into()]
}

impl SeedPin {
    /// Pin `id` at `version`.
    pub fn new(id: &str, version: &str) -> Self {
        Self {
            id: id.into(),
            version: version.into(),
            sha256: None,
            console_commands: default_console(),
        }
    }
}

/// Seed adapter configuration.
#[derive(Debug, Clone)]
pub struct SeedConfig {
    /// Operator-assigned WeftOS node id for this Seed.
    pub node_id: String,
    /// Operator pins (governance config).
    pub pins: Vec<SeedPin>,
    /// Concurrency cap (defaults to [`SEED_CONCURRENCY_CAP`]).
    pub concurrency_cap: usize,
}

/// One entry of `GET /api/v1/apps`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledCog {
    /// Cog id.
    pub id: String,
    /// Version.
    pub version: String,
    /// Running now.
    pub running: bool,
}

pub(super) fn backend(path: &str, status: u16, body: &Value) -> RuntimeError {
    let err = body.get("error").and_then(Value::as_str).unwrap_or("");
    RuntimeError::Backend(format!(
        "seed {path}: HTTP {status} {}",
        err.chars().take(200).collect::<String>()
    ))
}

pub(super) fn lines(v: Option<&Value>) -> String {
    match v {
        Some(Value::Array(a)) => a
            .iter()
            .map(|l| {
                l.as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| l.to_string())
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}
