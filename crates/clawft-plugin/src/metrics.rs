//! Per-plugin WASM fuel / memory observability (WEFT-68).
//!
//! Fuel and memory limits are enforced by the WASM host
//! (`clawft-wasm-host::engine`). This module is the **observability surface**:
//! aggregate counters that plugin authors and operators can inspect to
//! tune resource limits empirically.
//!
//! # Metric shape (pinned)
//!
//! [`PluginMetrics`] serializes to a stable JSON object with these keys:
//!
//! | Field | Type | Meaning |
//! |-------|------|---------|
//! | `plugin_name` | string | Plugin name / id |
//! | `fuel_consumed_total` | u64 | Sum of fuel across all invocations |
//! | `memory_peak_bytes` | u64 | Max linear-memory size observed (bytes) |
//! | `invocation_count` | u64 | Number of recorded invocations |
//! | `last_fuel_consumed` | u64 | Fuel used by the most recent invocation |
//! | `last_memory_peak_bytes` | u64 | Memory peak of the most recent invocation |
//! | `last_duration_ms` | u64 | Wall-clock duration of the most recent invocation |
//! | `total_duration_ms` | u64 | Sum of wall-clock durations |
//!
//! Tests pin this shape so CLI / admin RPC consumers can depend on it.
//!
//! # Surfaces
//!
//! - **In-process registry** — [`PluginMetricsRegistry`] (thread-safe).
//! - **Persistence** — JSON under `~/.clawft/plugins/metrics/<name>.json`.
//! - **Chain event** — each [`PluginMetricsRegistry::record`] emits a
//!   structured tracing event on the `chain_event` target with kind
//!   [`EVENT_KIND_PLUGIN_WASM_INVOKE`] so the daemon bridge can forward
//!   it to ExoChain (same vocabulary as `clawft_core::chain_event!`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

/// Chain-event kind for a single WASM plugin invocation.
///
/// Mirrors the `EVENT_KIND_*` vocabulary in `clawft_core::chain_event`
/// so producers and consumers share a stable string without a compile
/// dependency from this crate on `clawft-core`.
pub const EVENT_KIND_PLUGIN_WASM_INVOKE: &str = "plugin.wasm.invoke";

/// Aggregate fuel / memory / invocation counters for one plugin.
///
/// This is the **pinned metric shape** for WEFT-68. Field names and
/// types must not change without a coordinated CLI / admin-RPC update.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginMetrics {
    /// Plugin name (or id) these counters apply to.
    pub plugin_name: String,
    /// Cumulative fuel units consumed across all recorded invocations.
    pub fuel_consumed_total: u64,
    /// Peak linear-memory size observed across invocations, in bytes.
    pub memory_peak_bytes: u64,
    /// Number of recorded invocations.
    pub invocation_count: u64,
    /// Fuel consumed by the most recent invocation.
    pub last_fuel_consumed: u64,
    /// Memory peak of the most recent invocation, in bytes.
    pub last_memory_peak_bytes: u64,
    /// Wall-clock duration of the most recent invocation, in ms.
    pub last_duration_ms: u64,
    /// Cumulative wall-clock duration across invocations, in ms.
    pub total_duration_ms: u64,
}

impl PluginMetrics {
    /// Create zeroed metrics for `plugin_name`.
    pub fn new(plugin_name: impl Into<String>) -> Self {
        Self {
            plugin_name: plugin_name.into(),
            fuel_consumed_total: 0,
            memory_peak_bytes: 0,
            invocation_count: 0,
            last_fuel_consumed: 0,
            last_memory_peak_bytes: 0,
            last_duration_ms: 0,
            total_duration_ms: 0,
        }
    }

    /// Apply one invocation sample onto these aggregates.
    pub fn record_invocation(
        &mut self,
        fuel_consumed: u64,
        memory_peak_bytes: u64,
        duration_ms: u64,
    ) {
        self.fuel_consumed_total = self.fuel_consumed_total.saturating_add(fuel_consumed);
        if memory_peak_bytes > self.memory_peak_bytes {
            self.memory_peak_bytes = memory_peak_bytes;
        }
        self.invocation_count = self.invocation_count.saturating_add(1);
        self.last_fuel_consumed = fuel_consumed;
        self.last_memory_peak_bytes = memory_peak_bytes;
        self.last_duration_ms = duration_ms;
        self.total_duration_ms = self.total_duration_ms.saturating_add(duration_ms);
    }

    /// Serialize to the canonical JSON object (pretty optional).
    pub fn to_json_value(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("PluginMetrics is always serializable")
    }
}

/// In-process, thread-safe registry of per-plugin aggregate metrics.
///
/// Also the default process-wide store used by the WASM host and the
/// `weft plugins inspect` CLI (via disk persistence).
#[derive(Debug, Default)]
pub struct PluginMetricsRegistry {
    inner: Mutex<HashMap<String, PluginMetrics>>,
}

impl PluginMetricsRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// Record one invocation for `plugin_name` and return the updated snapshot.
    ///
    /// Emits a `chain_event`-target tracing record with kind
    /// [`EVENT_KIND_PLUGIN_WASM_INVOKE`] carrying the per-invocation
    /// counters so the daemon bridge can forward to ExoChain.
    pub fn record(
        &self,
        plugin_name: &str,
        fuel_consumed: u64,
        memory_peak_bytes: u64,
        duration_ms: u64,
    ) -> PluginMetrics {
        let snapshot = {
            let mut map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            let entry = map
                .entry(plugin_name.to_string())
                .or_insert_with(|| PluginMetrics::new(plugin_name));
            entry.record_invocation(fuel_consumed, memory_peak_bytes, duration_ms);
            entry.clone()
        };

        // Observability surface: structured chain-event vocabulary without
        // a compile dependency on clawft-core. Field names match the
        // PluginMetrics shape so consumers can correlate.
        tracing::info!(
            target: "chain_event",
            source = "plugin",
            kind = EVENT_KIND_PLUGIN_WASM_INVOKE,
            plugin_name = %snapshot.plugin_name,
            fuel_consumed = fuel_consumed,
            memory_peak_bytes = memory_peak_bytes,
            duration_ms = duration_ms,
            fuel_consumed_total = snapshot.fuel_consumed_total,
            memory_peak_bytes_agg = snapshot.memory_peak_bytes,
            invocation_count = snapshot.invocation_count,
            "chain"
        );

        snapshot
    }

    /// Look up aggregate metrics for a plugin.
    pub fn get(&self, plugin_name: &str) -> Option<PluginMetrics> {
        let map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        map.get(plugin_name).cloned()
    }

    /// Return metrics for `plugin_name`, or a zeroed snapshot if never recorded.
    pub fn get_or_zero(&self, plugin_name: &str) -> PluginMetrics {
        self.get(plugin_name)
            .unwrap_or_else(|| PluginMetrics::new(plugin_name))
    }

    /// List all tracked plugins (sorted by name).
    pub fn list(&self) -> Vec<PluginMetrics> {
        let map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut out: Vec<PluginMetrics> = map.values().cloned().collect();
        out.sort_by(|a, b| a.plugin_name.cmp(&b.plugin_name));
        out
    }

    /// Replace/insert a full metrics snapshot (used when loading from disk).
    pub fn upsert(&self, metrics: PluginMetrics) {
        let mut map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        map.insert(metrics.plugin_name.clone(), metrics);
    }

    /// Clear all in-memory entries (tests).
    pub fn clear(&self) {
        let mut map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        map.clear();
    }

    /// Number of plugins with recorded metrics.
    pub fn len(&self) -> usize {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Process-wide metrics registry (lazy singleton).
///
/// Used by the WASM host so counters survive across invocations in one
/// process. The CLI primarily reads the on-disk snapshot.
pub fn global_plugin_metrics() -> &'static PluginMetricsRegistry {
    static REGISTRY: OnceLock<PluginMetricsRegistry> = OnceLock::new();
    REGISTRY.get_or_init(PluginMetricsRegistry::new)
}

/// Default directory for persisted per-plugin metrics JSON files.
///
/// Resolves to `$WEFTOS_PLUGIN_METRICS_DIR` when set, else
/// `~/.clawft/plugins/metrics/`. Returns `None` if the home directory
/// cannot be determined, or when running inside a cargo test binary with no
/// override, so tests never write the real home (persistence is skipped).
pub fn default_metrics_dir() -> Option<PathBuf> {
    if let Ok(d) = std::env::var("WEFTOS_PLUGIN_METRICS_DIR")
        && !d.is_empty()
    {
        return Some(PathBuf::from(d));
    }
    if running_in_test_binary() {
        return None;
    }
    dirs_home().map(|h| h.join(".clawft").join("plugins").join("metrics"))
}

/// True when this process is a cargo test binary (`target/*/deps/<name>-<hash>`).
fn running_in_test_binary() -> bool {
    cfg!(test)
        || std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().and_then(|d| d.file_name()).map(|n| n == "deps"))
            .unwrap_or(false)
}

fn dirs_home() -> Option<PathBuf> {
    // Avoid a hard `dirs` dependency in clawft-plugin: use std env vars
    // that match dirs::home_dir on the platforms we ship.
    if let Ok(h) = std::env::var("HOME")
        && !h.is_empty()
    {
        return Some(PathBuf::from(h));
    }
    if let Ok(h) = std::env::var("USERPROFILE")
        && !h.is_empty()
    {
        return Some(PathBuf::from(h));
    }
    None
}

/// Path to the metrics JSON file for `plugin_name`.
pub fn metrics_file_path(dir: &Path, plugin_name: &str) -> PathBuf {
    // Sanitize: allow alphanumerics, dash, underscore, dot.
    let safe: String = plugin_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    dir.join(format!("{safe}.json"))
}

/// Persist a metrics snapshot to `dir/<plugin>.json`.
pub fn save_metrics(dir: &Path, metrics: &PluginMetrics) -> Result<(), std::io::Error> {
    std::fs::create_dir_all(dir)?;
    let path = metrics_file_path(dir, &metrics.plugin_name);
    let json = serde_json::to_string_pretty(metrics)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, json)
}

/// Load metrics for `plugin_name` from disk, if present.
pub fn load_metrics(dir: &Path, plugin_name: &str) -> Result<Option<PluginMetrics>, std::io::Error> {
    let path = metrics_file_path(dir, plugin_name);
    if !path.exists() {
        return Ok(None);
    }
    let contents = std::fs::read_to_string(path)?;
    let metrics: PluginMetrics = serde_json::from_str(&contents)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    Ok(Some(metrics))
}

/// Record an invocation into the global registry **and** persist to the
/// default metrics directory when home is resolvable.
///
/// This is the convenience entry point used by the WASM host after each
/// `execute_tool` call.
pub fn record_plugin_invocation(
    plugin_name: &str,
    fuel_consumed: u64,
    memory_peak_bytes: u64,
    duration_ms: u64,
) -> PluginMetrics {
    let snapshot =
        global_plugin_metrics().record(plugin_name, fuel_consumed, memory_peak_bytes, duration_ms);
    if let Some(dir) = default_metrics_dir()
        && let Err(e) = save_metrics(&dir, &snapshot)
    {
        tracing::warn!(
            plugin = plugin_name,
            error = %e,
            "failed to persist plugin metrics"
        );
    }
    snapshot
}

// ---------------------------------------------------------------------------
// Tests — pin the metric shape (WEFT-68 AC)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// Canonical field set for PluginMetrics JSON. Changing this list is a
    /// breaking change for `weft plugins inspect` and admin RPC consumers.
    const PINNED_METRIC_KEYS: &[&str] = &[
        "plugin_name",
        "fuel_consumed_total",
        "memory_peak_bytes",
        "invocation_count",
        "last_fuel_consumed",
        "last_memory_peak_bytes",
        "last_duration_ms",
        "total_duration_ms",
    ];

    #[test]
    fn metric_shape_is_pinned() {
        let m = PluginMetrics {
            plugin_name: "demo".into(),
            fuel_consumed_total: 1_000,
            memory_peak_bytes: 65_536,
            invocation_count: 2,
            last_fuel_consumed: 400,
            last_memory_peak_bytes: 65_536,
            last_duration_ms: 12,
            total_duration_ms: 30,
        };
        let value = m.to_json_value();
        let obj = value.as_object().expect("metrics serialize to object");
        let keys: BTreeSet<&str> = obj.keys().map(|s| s.as_str()).collect();
        let expected: BTreeSet<&str> = PINNED_METRIC_KEYS.iter().copied().collect();
        assert_eq!(
            keys, expected,
            "PluginMetrics JSON keys drifted from WEFT-68 pinned shape"
        );

        // Type pins
        assert!(obj["plugin_name"].is_string());
        assert!(obj["fuel_consumed_total"].is_u64() || obj["fuel_consumed_total"].is_i64());
        assert!(obj["memory_peak_bytes"].is_u64() || obj["memory_peak_bytes"].is_i64());
        assert!(obj["invocation_count"].is_u64() || obj["invocation_count"].is_i64());
        assert!(obj["last_fuel_consumed"].is_u64() || obj["last_fuel_consumed"].is_i64());
        assert!(obj["last_memory_peak_bytes"].is_u64() || obj["last_memory_peak_bytes"].is_i64());
        assert!(obj["last_duration_ms"].is_u64() || obj["last_duration_ms"].is_i64());
        assert!(obj["total_duration_ms"].is_u64() || obj["total_duration_ms"].is_i64());
    }

    #[test]
    fn metric_shape_round_trips() {
        let m = PluginMetrics {
            plugin_name: "roundtrip".into(),
            fuel_consumed_total: 42,
            memory_peak_bytes: 1024,
            invocation_count: 3,
            last_fuel_consumed: 10,
            last_memory_peak_bytes: 512,
            last_duration_ms: 5,
            total_duration_ms: 15,
        };
        let json = serde_json::to_string(&m).unwrap();
        let back: PluginMetrics = serde_json::from_str(&json).unwrap();
        assert_eq!(m, back);
    }

    #[test]
    fn record_aggregates_fuel_memory_invocations() {
        let reg = PluginMetricsRegistry::new();
        reg.record("alpha", 100, 2048, 10);
        reg.record("alpha", 250, 4096, 20);
        reg.record("alpha", 50, 1024, 5);

        let m = reg.get("alpha").unwrap();
        assert_eq!(m.fuel_consumed_total, 400);
        assert_eq!(m.memory_peak_bytes, 4096);
        assert_eq!(m.invocation_count, 3);
        assert_eq!(m.last_fuel_consumed, 50);
        assert_eq!(m.last_memory_peak_bytes, 1024);
        assert_eq!(m.last_duration_ms, 5);
        assert_eq!(m.total_duration_ms, 35);
    }

    #[test]
    fn record_is_per_plugin() {
        let reg = PluginMetricsRegistry::new();
        reg.record("a", 10, 100, 1);
        reg.record("b", 20, 200, 2);
        assert_eq!(reg.get("a").unwrap().fuel_consumed_total, 10);
        assert_eq!(reg.get("b").unwrap().fuel_consumed_total, 20);
        assert_eq!(reg.len(), 2);
    }

    #[test]
    fn get_or_zero_for_unknown() {
        let reg = PluginMetricsRegistry::new();
        let m = reg.get_or_zero("never-seen");
        assert_eq!(m.plugin_name, "never-seen");
        assert_eq!(m.invocation_count, 0);
        assert_eq!(m.fuel_consumed_total, 0);
        assert_eq!(m.memory_peak_bytes, 0);
    }

    #[test]
    fn event_kind_constant_is_stable() {
        assert_eq!(EVENT_KIND_PLUGIN_WASM_INVOKE, "plugin.wasm.invoke");
    }

    #[test]
    fn persist_and_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let m = PluginMetrics {
            plugin_name: "persisted".into(),
            fuel_consumed_total: 999,
            memory_peak_bytes: 8192,
            invocation_count: 7,
            last_fuel_consumed: 111,
            last_memory_peak_bytes: 4096,
            last_duration_ms: 33,
            total_duration_ms: 200,
        };
        save_metrics(dir.path(), &m).unwrap();
        let loaded = load_metrics(dir.path(), "persisted").unwrap().unwrap();
        assert_eq!(loaded, m);
    }

    #[test]
    fn metrics_file_path_sanitizes_name() {
        let dir = Path::new("/tmp/metrics");
        let p = metrics_file_path(dir, "weftos.plugin.foo/bar");
        assert_eq!(p, PathBuf::from("/tmp/metrics/weftos.plugin.foo_bar.json"));
    }

    #[test]
    fn load_missing_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_metrics(dir.path(), "nope").unwrap().is_none());
    }

    #[test]
    fn tests_never_resolve_the_real_home_metrics_dir() {
        // A test binary with no override must not persist into ~/.clawft.
        if std::env::var("WEFTOS_PLUGIN_METRICS_DIR").is_err() {
            assert!(default_metrics_dir().is_none());
        }
    }
}
