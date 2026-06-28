//! Healthcheck contract — typed builders + path helpers for the
//! `HealthReport` shape every node and sensor in the WeftOS mesh emits.
//!
//! Codifies `.planning/sensors/HEALTHCHECK-CONTRACT.md` so adapters can
//! produce contract-compliant payloads without re-implementing the
//! JSON shape (and re-discovering the sensor-vs-node optional-field
//! split) from scratch.
//!
//! ## What this module is
//!
//! - The two payload struct shapes ([`NodeHealth`], [`SensorHealth`]),
//!   each serialising to the JSON shape declared in the contract
//!   (§2.1 / §3.1).
//! - The shared status enum ([`Status`]) with the four canonical states
//!   plus the `Unknown` slot the contract §9 question 1 already
//!   reserves for pre-first-emit publishes.
//! - Path helpers ([`node_health_path`], [`sensor_health_path`]) so
//!   producers and consumers don't hand-build the path strings and
//!   accidentally diverge on `health/sensor` vs `sensors/<name>` (the
//!   contract is `health/sensor/<name>`).
//! - A small classifier helper ([`classify_value`]) for the matching
//!   rule from contract §5.1 — used by tests and (later) the
//!   `HealthReport` Object Type registration.
//!
//! ## What this module is not (yet)
//!
//! - Not the aggregator. Contract §7 specifies that `observed_rate_hz`
//!   / `since_ms` / `status` are computed by a daemon-side aggregator,
//!   not by the source adapter. That's a follow-up service (likely
//!   `clawft-service-health`).
//! - Not the projection. The merged read path (raw + derived) lives
//!   in `crate::projection` — this module produces the raw payloads
//!   that flow into it.
//! - Not the Object Type registration. The Explorer-side
//!   `HealthReport` Object Type (contract §5) lives one layer up
//!   (`clawft-ontology` / Explorer registry); this module only
//!   exposes a stable Rust shape it can consume.
//!
//! ## Why typed shapes (and not just `serde_json::Value`)
//!
//! Two reasons:
//!
//! 1. **Catch mistakes at compile time.** Contract §2.2 / §3.2 declare
//!    which fields are required vs optional. A struct with `Option<…>`
//!    on the right fields makes "I forgot `tick`" a `cargo check`
//!    failure rather than a runtime "Explorer renders an empty card."
//! 2. **Cheap migration to a stricter shape later.** When we split
//!    `HealthReport` into `NodeHealth` + `SensorHealth` Object Types
//!    (contract §9 question 5), this module already separates them —
//!    consumers that already use the typed shapes need no changes.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::delta::StateDelta;

/// The four-plus-one health-status states from contract §3.2 + §9.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Operating within nominal parameters.
    Healthy,
    /// Operating but with detected issues (errors, low signal, low heap).
    Degraded,
    /// Sensor-only: no recent emissions but not yet declared down. Not
    /// valid for node-level reports — node-level skips straight to
    /// [`Status::Down`] when `last_publish_ts` ages out (contract §4.1).
    Stale,
    /// Not publishing / not reachable.
    Down,
    /// Pre-first-emit / no signal yet to make a determination.
    /// Reserved by contract §9 question 1; emit it when the producer
    /// is still warming up.
    Unknown,
}

impl Status {
    /// Lowercase string form, matches the JSON serialisation.
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Healthy => "healthy",
            Status::Degraded => "degraded",
            Status::Stale => "stale",
            Status::Down => "down",
            Status::Unknown => "unknown",
        }
    }

    /// Whether this state is valid for node-level reports.
    /// Contract §4.1 only declares `healthy | degraded | down` for
    /// nodes; `stale` is sensor-only.
    pub fn is_valid_for_node(self) -> bool {
        !matches!(self, Status::Stale)
    }
}

/// Reboot reason enum from contract §2.2. Stored as a string in the
/// payload so unknown values from heterogeneous firmwares survive a
/// round-trip — but the typed enum exists for producers that know
/// exactly which one to emit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RebootReason {
    /// Cold boot / power applied.
    PowerOn,
    /// Firmware panic.
    Panic,
    /// Hardware watchdog reset.
    Watchdog,
    /// Software-initiated reset.
    SoftwareReset,
    /// Wake from deep-sleep.
    DeepSleepWake,
    /// Source did not record a reason.
    Unknown,
}

impl RebootReason {
    /// Stable string form matching the contract §2.2 enum table.
    pub fn as_str(self) -> &'static str {
        match self {
            RebootReason::PowerOn => "power-on",
            RebootReason::Panic => "panic",
            RebootReason::Watchdog => "watchdog",
            RebootReason::SoftwareReset => "software-reset",
            RebootReason::DeepSleepWake => "deep-sleep-wake",
            RebootReason::Unknown => "unknown",
        }
    }
}

/// Node-level health payload (contract §2.1).
///
/// Build with [`NodeHealth::new`] to populate just the required fields,
/// then chain the optional setters. Serialise via `serde_json::to_value`
/// for emission as a [`crate::delta::StateDelta::Replace`] payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeHealth {
    /// Status of the node. See [`Status::is_valid_for_node`] — `stale`
    /// is sensor-only.
    pub status: Status,
    /// Seconds since last boot.
    pub uptime_s: u64,
    /// Semver-ish firmware identifier — the Explorer flags mismatched
    /// nodes by string comparison.
    pub firmware_version: String,
    /// Wall-clock millis of this node's last publish on any path.
    pub last_publish_ts: u64,
    /// Producer's monotonic tick counter.
    pub tick: u64,

    /// WiFi signal in negative dBm. Omit for wired nodes (e.g. the
    /// daemon-host).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rssi_dbm: Option<i64>,
    /// Smallest-free-block heap in bytes. Omit for nodes without
    /// exposed heap stats (e.g. a Linux daemon-host).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub free_heap_bytes: Option<u64>,
    /// String form of [`RebootReason`]. Stored as `String` so unknown
    /// values from external firmwares survive round-trip.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reboot_reason: Option<String>,
    /// Monotonic counter persisted across boots.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub boot_count: Option<u64>,
}

impl NodeHealth {
    /// Build a node-level report with just the required fields. Use the
    /// `with_*` setters to add optional fields.
    pub fn new(
        status: Status,
        uptime_s: u64,
        firmware_version: impl Into<String>,
        last_publish_ts: u64,
        tick: u64,
    ) -> Self {
        debug_assert!(
            status.is_valid_for_node(),
            "Status::Stale is not valid for node-level reports (contract §4.1)"
        );
        Self {
            status,
            uptime_s,
            firmware_version: firmware_version.into(),
            last_publish_ts,
            tick,
            rssi_dbm: None,
            free_heap_bytes: None,
            reboot_reason: None,
            boot_count: None,
        }
    }

    /// Add WiFi signal strength.
    pub fn with_rssi_dbm(mut self, rssi: i64) -> Self {
        self.rssi_dbm = Some(rssi);
        self
    }

    /// Add free-heap reading.
    pub fn with_free_heap_bytes(mut self, bytes: u64) -> Self {
        self.free_heap_bytes = Some(bytes);
        self
    }

    /// Add a structured reboot reason.
    pub fn with_reboot_reason(mut self, reason: RebootReason) -> Self {
        self.reboot_reason = Some(reason.as_str().into());
        self
    }

    /// Add an opaque reboot-reason string (for sources that supply
    /// values not covered by [`RebootReason`]).
    pub fn with_reboot_reason_raw(mut self, raw: impl Into<String>) -> Self {
        self.reboot_reason = Some(raw.into());
        self
    }

    /// Add the persisted boot counter.
    pub fn with_boot_count(mut self, count: u64) -> Self {
        self.boot_count = Some(count);
        self
    }

    /// Serialise to a `serde_json::Value` ready to drop into a
    /// `StateDelta::Replace { value: … }`.
    pub fn into_value(self) -> serde_json::Value {
        serde_json::to_value(self).expect("NodeHealth serialises infallibly")
    }
}

/// Sensor-level health payload (contract §3.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SensorHealth {
    /// Sensor status. All five [`Status`] variants are valid here.
    pub status: Status,
    /// Wall-clock millis of this sensor's last successful payload
    /// publish (the data emission, not this health record).
    pub last_emit_ts: u64,
    /// Nominal emission rate the sensor is configured for.
    pub configured_rate_hz: f64,
    /// Rolling-window measured rate. Source emits its best estimate;
    /// the daemon-side aggregator may overwrite via the derived path
    /// (contract §7).
    pub observed_rate_hz: f64,
    /// Monotonic counter of errors since boot.
    pub error_count: u64,
    /// Producer's monotonic tick counter.
    pub tick: u64,

    /// Millis the sensor has been in its current `status` state.
    /// Optional — the aggregator computes this in the derived path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since_ms: Option<u64>,
    /// Short human-readable last-error message. `None` (serialised as
    /// absence) when there's nothing to report; deliberately not
    /// `Some("")` so consumers can `if let Some(err)` cleanly.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// Free-text diagnostic.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

impl SensorHealth {
    /// Build a sensor-level report with the required fields.
    pub fn new(
        status: Status,
        last_emit_ts: u64,
        configured_rate_hz: f64,
        observed_rate_hz: f64,
        error_count: u64,
        tick: u64,
    ) -> Self {
        Self {
            status,
            last_emit_ts,
            configured_rate_hz,
            observed_rate_hz,
            error_count,
            tick,
            since_ms: None,
            last_error: None,
            notes: None,
        }
    }

    /// Add the time-in-current-state reading.
    pub fn with_since_ms(mut self, ms: u64) -> Self {
        self.since_ms = Some(ms);
        self
    }

    /// Add the most-recent error message.
    pub fn with_last_error(mut self, err: impl Into<String>) -> Self {
        self.last_error = Some(err.into());
        self
    }

    /// Add a free-text diagnostic note.
    pub fn with_notes(mut self, notes: impl Into<String>) -> Self {
        self.notes = Some(notes.into());
        self
    }

    /// Serialise to a `serde_json::Value` ready to drop into a
    /// `StateDelta::Replace { value: … }`.
    pub fn into_value(self) -> serde_json::Value {
        serde_json::to_value(self).expect("SensorHealth serialises infallibly")
    }
}

/// Build the canonical node-level health path for `node_id`.
///
/// `substrate/<node-id>/health` per contract §1.
pub fn node_health_path(node_id: &str) -> String {
    format!("substrate/{node_id}/health")
}

/// Build the canonical sensor-level health path for the named sensor
/// hosted by `node_id`.
///
/// `substrate/<node-id>/health/sensor/<sensor-name>` per contract §1.
pub fn sensor_health_path(node_id: &str, sensor_name: &str) -> String {
    format!("substrate/{node_id}/health/sensor/{sensor_name}")
}

/// Build the raw-counters path the source itself writes (contract §7).
pub fn node_health_raw_path(node_id: &str) -> String {
    format!("substrate/{node_id}/health/raw")
}

/// Build the raw-counters path the source itself writes for a sensor
/// (contract §7).
pub fn sensor_health_raw_path(node_id: &str, sensor_name: &str) -> String {
    format!("substrate/{node_id}/health/sensor/{sensor_name}/raw")
}

/// Build the derived-rollup path the daemon-side aggregator writes
/// (contract §7).
pub fn node_health_derived_path(daemon_id: &str, source_node_id: &str) -> String {
    format!("substrate/{daemon_id}/derived/health/{source_node_id}")
}

/// Build the derived-rollup path for a sensor (contract §7).
pub fn sensor_health_derived_path(
    daemon_id: &str,
    source_node_id: &str,
    sensor_name: &str,
) -> String {
    format!("substrate/{daemon_id}/derived/health/{source_node_id}/sensor/{sensor_name}")
}

/// Granularity inferred from the shape of a health value. Result of
/// [`classify_value`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthGranularity {
    /// Has at least `uptime_s`.
    Node,
    /// Has at least `last_emit_ts` or `observed_rate_hz`.
    Sensor,
}

/// Implementation of contract §5.1 `matches(value)`.
///
/// Returns `Some((priority, granularity))` if `value` matches the
/// `HealthReport` Object Type, else `None`. Priority `8` matches the
/// contract recommendation. The granularity helps consumers route the
/// right viewer (Node vs Sensor sub-card) without re-parsing.
///
/// Match clause (contract §5.1):
/// 1. `status` is a string in the declared enum, AND
/// 2. at least ONE of `uptime_s` (u64), `last_emit_ts` (u64),
///    `observed_rate_hz` (f64) is present.
pub fn classify_value(value: &serde_json::Value) -> Option<(u32, HealthGranularity)> {
    let obj = value.as_object()?;
    let status = obj.get("status")?.as_str()?;
    if !matches!(
        status,
        "healthy" | "degraded" | "stale" | "down" | "unknown"
    ) {
        return None;
    }
    if obj
        .get("uptime_s")
        .and_then(serde_json::Value::as_u64)
        .is_some()
    {
        return Some((8, HealthGranularity::Node));
    }
    if obj
        .get("last_emit_ts")
        .and_then(serde_json::Value::as_u64)
        .is_some()
        || obj
            .get("observed_rate_hz")
            .and_then(serde_json::Value::as_f64)
            .is_some()
    {
        return Some((8, HealthGranularity::Sensor));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ---- Status -----------------------------------------------------

    #[test]
    fn status_serialises_lowercase() {
        let s = serde_json::to_string(&Status::Healthy).unwrap();
        assert_eq!(s, "\"healthy\"");
        let s = serde_json::to_string(&Status::Stale).unwrap();
        assert_eq!(s, "\"stale\"");
    }

    #[test]
    fn status_node_validity_excludes_stale() {
        assert!(Status::Healthy.is_valid_for_node());
        assert!(Status::Degraded.is_valid_for_node());
        assert!(Status::Down.is_valid_for_node());
        assert!(Status::Unknown.is_valid_for_node());
        assert!(!Status::Stale.is_valid_for_node());
    }

    // ---- RebootReason -----------------------------------------------

    #[test]
    fn reboot_reason_strings_match_contract() {
        assert_eq!(RebootReason::PowerOn.as_str(), "power-on");
        assert_eq!(RebootReason::Panic.as_str(), "panic");
        assert_eq!(RebootReason::Watchdog.as_str(), "watchdog");
        assert_eq!(RebootReason::SoftwareReset.as_str(), "software-reset");
        assert_eq!(RebootReason::DeepSleepWake.as_str(), "deep-sleep-wake");
        assert_eq!(RebootReason::Unknown.as_str(), "unknown");
    }

    // ---- Path helpers -----------------------------------------------

    #[test]
    fn path_helpers_match_contract_section_1() {
        assert_eq!(node_health_path("esp32-7a"), "substrate/esp32-7a/health");
        assert_eq!(
            sensor_health_path("esp32-7a", "mic"),
            "substrate/esp32-7a/health/sensor/mic"
        );
    }

    #[test]
    fn raw_and_derived_path_helpers_match_contract_section_7() {
        assert_eq!(
            node_health_raw_path("esp32-7a"),
            "substrate/esp32-7a/health/raw"
        );
        assert_eq!(
            sensor_health_raw_path("esp32-7a", "mic"),
            "substrate/esp32-7a/health/sensor/mic/raw"
        );
        assert_eq!(
            node_health_derived_path("daemon-host", "esp32-7a"),
            "substrate/daemon-host/derived/health/esp32-7a"
        );
        assert_eq!(
            sensor_health_derived_path("daemon-host", "esp32-7a", "mic"),
            "substrate/daemon-host/derived/health/esp32-7a/sensor/mic"
        );
    }

    // ---- NodeHealth shape -------------------------------------------

    #[test]
    fn node_health_minimal_emits_required_fields_only() {
        let h = NodeHealth::new(
            Status::Healthy,
            84210,
            "0.7.0-phase2",
            1714000000000,
            168420,
        );
        let v = h.into_value();
        // Required fields present.
        assert_eq!(v["status"], "healthy");
        assert_eq!(v["uptime_s"], 84210);
        assert_eq!(v["firmware_version"], "0.7.0-phase2");
        assert_eq!(v["last_publish_ts"], 1714000000000_u64);
        assert_eq!(v["tick"], 168420);
        // Optional fields skipped — not present as `null`.
        assert!(v.get("rssi_dbm").is_none());
        assert!(v.get("free_heap_bytes").is_none());
        assert!(v.get("reboot_reason").is_none());
        assert!(v.get("boot_count").is_none());
    }

    #[test]
    fn node_health_with_all_optionals_matches_contract_example() {
        let h = NodeHealth::new(
            Status::Healthy,
            84210,
            "0.7.0-phase2",
            1714000000000,
            168420,
        )
        .with_rssi_dbm(-56)
        .with_free_heap_bytes(148320)
        .with_reboot_reason(RebootReason::PowerOn)
        .with_boot_count(17);
        let v = h.into_value();
        // Mirror contract §2.1 exemplar.
        let expected = json!({
            "status": "healthy",
            "uptime_s": 84210,
            "firmware_version": "0.7.0-phase2",
            "rssi_dbm": -56,
            "free_heap_bytes": 148320,
            "last_publish_ts": 1714000000000_u64,
            "reboot_reason": "power-on",
            "boot_count": 17,
            "tick": 168420
        });
        assert_eq!(v, expected);
    }

    #[test]
    fn node_health_round_trips_through_serde() {
        let h = NodeHealth::new(Status::Down, 0, "0.0.0", 0, 0)
            .with_reboot_reason(RebootReason::Watchdog);
        let s = serde_json::to_string(&h).unwrap();
        let back: NodeHealth = serde_json::from_str(&s).unwrap();
        assert_eq!(back.status, Status::Down);
        assert_eq!(back.reboot_reason.as_deref(), Some("watchdog"));
    }

    #[test]
    #[cfg_attr(debug_assertions, should_panic(expected = "Stale"))]
    fn node_health_rejects_stale_status_in_debug() {
        // Contract §4.1: `stale` is sensor-only; node-level shouldn't
        // emit it. Only enforced in debug builds (debug_assertions) so
        // a release build that did receive a deserialised `stale` from
        // an out-of-spec firmware doesn't crash the daemon.
        let _ = NodeHealth::new(Status::Stale, 0, "x", 0, 0);
    }

    // ---- SensorHealth shape -----------------------------------------

    #[test]
    fn sensor_health_minimal_emits_required_fields_only() {
        let h = SensorHealth::new(Status::Healthy, 1714000000000, 2.0, 1.98, 0, 168420);
        let v = h.into_value();
        assert_eq!(v["status"], "healthy");
        assert_eq!(v["last_emit_ts"], 1714000000000_u64);
        assert_eq!(v["configured_rate_hz"], 2.0);
        assert_eq!(v["observed_rate_hz"], 1.98);
        assert_eq!(v["error_count"], 0);
        assert_eq!(v["tick"], 168420);
        assert!(v.get("since_ms").is_none());
        assert!(v.get("last_error").is_none());
        assert!(v.get("notes").is_none());
    }

    #[test]
    fn sensor_health_with_all_optionals_matches_contract_example() {
        let h = SensorHealth::new(Status::Healthy, 1714000000000, 2.0, 1.98, 0, 168420)
            .with_since_ms(84210);
        let v = h.into_value();
        // Contract §3.1 exemplar uses null for last_error/notes.
        // We skip-on-None instead — the absence is semantically equal
        // for `Object Type matches(value)` and lighter on the wire.
        let expected = json!({
            "status": "healthy",
            "last_emit_ts": 1714000000000_u64,
            "configured_rate_hz": 2.0,
            "observed_rate_hz": 1.98,
            "error_count": 0,
            "since_ms": 84210,
            "tick": 168420
        });
        assert_eq!(v, expected);
    }

    #[test]
    fn sensor_health_with_error_renders_string() {
        let h = SensorHealth::new(Status::Degraded, 0, 100.0, 99.0, 1, 1)
            .with_last_error("I2S DMA underrun")
            .with_notes("WARN: rolling 1m");
        let v = h.into_value();
        assert_eq!(v["last_error"], "I2S DMA underrun");
        assert_eq!(v["notes"], "WARN: rolling 1m");
    }

    // ---- Classifier (contract §5.1) ---------------------------------

    #[test]
    fn classifier_matches_node_via_uptime() {
        let v = json!({"status": "healthy", "uptime_s": 1});
        assert_eq!(classify_value(&v), Some((8, HealthGranularity::Node)));
    }

    #[test]
    fn classifier_matches_sensor_via_last_emit_ts() {
        let v = json!({"status": "stale", "last_emit_ts": 100_u64});
        assert_eq!(classify_value(&v), Some((8, HealthGranularity::Sensor)));
    }

    #[test]
    fn classifier_matches_sensor_via_observed_rate_hz() {
        let v = json!({"status": "down", "observed_rate_hz": 0.0});
        assert_eq!(classify_value(&v), Some((8, HealthGranularity::Sensor)));
    }

    #[test]
    fn classifier_rejects_random_status_blob() {
        // §5.1: rejects `{ status: "ok" }`-shape blobs from unrelated
        // code that just happens to use the word "status".
        let v = json!({"status": "ok"});
        assert_eq!(classify_value(&v), None);
        let v = json!({"status": "healthy"}); // Right enum, no required fields.
        assert_eq!(classify_value(&v), None);
    }

    #[test]
    fn classifier_rejects_unknown_status_string() {
        let v = json!({"status": "fubar", "uptime_s": 1});
        assert_eq!(classify_value(&v), None);
    }

    #[test]
    fn classifier_accepts_unknown_status_for_pre_first_emit() {
        // Contract §9 question 1: `unknown` reserved for pre-first-emit.
        let v = json!({"status": "unknown", "uptime_s": 0});
        assert_eq!(classify_value(&v), Some((8, HealthGranularity::Node)));
    }

    // ---- End-to-end shape contract assertion ------------------------

    #[test]
    fn round_trip_emit_then_classify_node() {
        let payload = NodeHealth::new(Status::Degraded, 120, "0.7.0", 1_700_000_000_000, 42)
            .with_rssi_dbm(-72)
            .into_value();
        // The producer-emitted shape must classify as a node-level
        // HealthReport — otherwise the Object Type registry won't pick
        // the right viewer.
        assert_eq!(classify_value(&payload), Some((8, HealthGranularity::Node)));
    }

    #[test]
    fn round_trip_emit_then_classify_sensor() {
        let payload =
            SensorHealth::new(Status::Healthy, 1_700_000_000_000, 2.0, 1.95, 0, 7).into_value();
        assert_eq!(
            classify_value(&payload),
            Some((8, HealthGranularity::Sensor))
        );
    }
}

// ── M7b-1 (WEFT-415/417/432) per-adapter healthcheck shim ──────────────────────
//
// Per-adapter wire format used by snapshot.rs and the mic adapter to publish
// health snapshots at `substrate/meta/adapter/<id>/healthcheck`. Predates the
// full HEALTHCHECK-CONTRACT.md impl above (WEFT-437); both layers stay because
// per-adapter snapshot (M7b-1) and daemon-side aggregator (M7b-4) are different
// concerns at the same module path.

/// Sensor-level health status.
///
/// Stable kebab-case strings on the wire — Explorer / tray match by
/// string, not by Rust type, so renames here are wire-breaking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SensorStatus {
    /// Emitting at configured rate, no errors.
    Healthy,
    /// Errors observed in the recent window OR configured but
    /// half-rate-or-better.
    Degraded,
    /// `observed_rate_hz < 0.5 * configured_rate_hz` sustained.
    Stale,
    /// `observed_rate_hz == 0` for ≥10 s.
    Down,
    /// Pre-first-emit — sensor was just registered, no data yet.
    Unknown,
}

impl SensorStatus {
    /// Stable lower-case form for serialization.
    pub fn as_str(self) -> &'static str {
        match self {
            SensorStatus::Healthy => "healthy",
            SensorStatus::Degraded => "degraded",
            SensorStatus::Stale => "stale",
            SensorStatus::Down => "down",
            SensorStatus::Unknown => "unknown",
        }
    }
}

/// Sensor-level HealthReport — see HEALTHCHECK-CONTRACT.md §3.
///
/// All required fields per §3.2 are non-optional. Optional fields use
/// `Option<T>`; serialization skips them when `None` to keep the wire
/// JSON compact.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SensorHealthReport {
    /// Sensor status — see [`SensorStatus`].
    pub status: SensorStatus,
    /// Wall-clock millis of the last successful payload publish.
    pub last_emit_ts: u64,
    /// Nominal emission rate the sensor is configured for.
    pub configured_rate_hz: f64,
    /// Rolling-window measured rate. Computed by the producer (or by an
    /// aggregator; see contract §7).
    pub observed_rate_hz: f64,
    /// Monotonic counter of errors since boot.
    pub error_count: u64,
    /// Producer's monotonic tick counter.
    pub tick: u64,
    /// Millis the sensor has been in its current `status` state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since_ms: Option<u64>,
    /// Short last-error message; `None` when no error has been seen.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// Free-text diagnostic; `None` when nothing to add.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

impl SensorHealthReport {
    /// Build an `Unknown`-status pre-first-emit report. Useful as the
    /// initial value emitted when an adapter starts up.
    pub fn unknown(configured_rate_hz: f64) -> Self {
        Self {
            status: SensorStatus::Unknown,
            last_emit_ts: 0,
            configured_rate_hz,
            observed_rate_hz: 0.0,
            error_count: 0,
            tick: 0,
            since_ms: None,
            last_error: None,
            notes: Some("pre-first-emit".into()),
        }
    }

    /// Build a `Healthy`-status report with the given observed rate.
    pub fn healthy(
        last_emit_ts: u64,
        configured_rate_hz: f64,
        observed_rate_hz: f64,
        tick: u64,
    ) -> Self {
        Self {
            status: SensorStatus::Healthy,
            last_emit_ts,
            configured_rate_hz,
            observed_rate_hz,
            error_count: 0,
            tick,
            since_ms: None,
            last_error: None,
            notes: None,
        }
    }

    /// Apply the contract's status-transition rules to derive a status
    /// from `observed_rate_hz`, `configured_rate_hz`, and recent error
    /// activity. Used by adapters that don't run their own state
    /// machine (most preview adapters).
    ///
    /// - `errors_in_window > 0` → `Degraded`.
    /// - `observed_rate_hz == 0` → `Down`.
    /// - `observed_rate_hz < 0.5 * configured_rate_hz` → `Stale`.
    /// - otherwise → `Healthy`.
    ///
    /// `Unknown` is returned only by [`SensorHealthReport::unknown`];
    /// the derived path always picks one of the four numeric states.
    pub fn derive_status(
        observed_rate_hz: f64,
        configured_rate_hz: f64,
        errors_in_window: u64,
    ) -> SensorStatus {
        if errors_in_window > 0 {
            return SensorStatus::Degraded;
        }
        if observed_rate_hz == 0.0 {
            return SensorStatus::Down;
        }
        if configured_rate_hz > 0.0 && observed_rate_hz < 0.5 * configured_rate_hz {
            return SensorStatus::Stale;
        }
        SensorStatus::Healthy
    }

    /// Render to a JSON value matching the contract shape.
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

/// Build the sensor-level healthcheck topic path for `adapter_id`.
///
/// Pre-WEFT-418 path: `substrate/meta/adapter/<adapter-id>/healthcheck`.
/// Post-WEFT-418 (node-id scoping): callers will swap to
/// `substrate/<node-id>/health/sensor/<adapter-id>` without changing the
/// emitted value shape.
pub fn healthcheck_topic_path(adapter_id: &str) -> String {
    format!("substrate/meta/adapter/{adapter_id}/healthcheck")
}

/// Build a `Replace` delta for a sensor health report.
pub fn build_report_delta(adapter_id: &str, report: &SensorHealthReport) -> StateDelta {
    StateDelta::Replace {
        path: healthcheck_topic_path(adapter_id),
        value: report.to_value(),
    }
}

#[cfg(test)]
mod sensor_shim_tests {
    use super::*;

    #[test]
    fn status_strings_are_lowercase() {
        assert_eq!(SensorStatus::Healthy.as_str(), "healthy");
        assert_eq!(SensorStatus::Degraded.as_str(), "degraded");
        assert_eq!(SensorStatus::Stale.as_str(), "stale");
        assert_eq!(SensorStatus::Down.as_str(), "down");
        assert_eq!(SensorStatus::Unknown.as_str(), "unknown");
    }

    #[test]
    fn unknown_report_marks_pre_first_emit() {
        let r = SensorHealthReport::unknown(2.0);
        assert_eq!(r.status, SensorStatus::Unknown);
        assert_eq!(r.last_emit_ts, 0);
        assert_eq!(r.configured_rate_hz, 2.0);
        assert_eq!(r.error_count, 0);
        assert_eq!(r.notes.as_deref(), Some("pre-first-emit"));
    }

    #[test]
    fn derive_status_healthy_at_full_rate() {
        assert_eq!(
            SensorHealthReport::derive_status(2.0, 2.0, 0),
            SensorStatus::Healthy
        );
    }

    #[test]
    fn derive_status_stale_below_half_rate() {
        assert_eq!(
            SensorHealthReport::derive_status(0.4, 2.0, 0),
            SensorStatus::Stale
        );
    }

    #[test]
    fn derive_status_down_at_zero_rate() {
        assert_eq!(
            SensorHealthReport::derive_status(0.0, 2.0, 0),
            SensorStatus::Down
        );
    }

    #[test]
    fn derive_status_degraded_when_errors_present() {
        // Even at full rate, recent errors flip to Degraded.
        assert_eq!(
            SensorHealthReport::derive_status(2.0, 2.0, 1),
            SensorStatus::Degraded
        );
    }

    #[test]
    fn report_serializes_with_required_fields_only_when_optional_missing() {
        let r = SensorHealthReport::healthy(1714000000000, 2.0, 1.98, 42);
        let v = r.to_value();
        // Required fields present.
        assert_eq!(v["status"], "healthy");
        assert_eq!(v["last_emit_ts"], 1714000000000_u64);
        assert_eq!(v["configured_rate_hz"], 2.0);
        assert_eq!(v["observed_rate_hz"], 1.98);
        assert_eq!(v["error_count"], 0);
        assert_eq!(v["tick"], 42);
        // Optional fields skipped when None.
        assert!(v.get("since_ms").is_none());
        assert!(v.get("last_error").is_none());
        assert!(v.get("notes").is_none());
    }

    #[test]
    fn report_includes_optional_fields_when_present() {
        let mut r = SensorHealthReport::healthy(1, 2.0, 1.5, 1);
        r.status = SensorStatus::Degraded;
        r.error_count = 3;
        r.last_error = Some("I2S DMA underrun".into());
        r.notes = Some("WARN: clock drift detected".into());
        r.since_ms = Some(4200);
        let v = r.to_value();
        assert_eq!(v["status"], "degraded");
        assert_eq!(v["error_count"], 3);
        assert_eq!(v["last_error"], "I2S DMA underrun");
        assert_eq!(v["notes"], "WARN: clock drift detected");
        assert_eq!(v["since_ms"], 4200);
    }

    #[test]
    fn topic_path_uses_meta_prefix_pre_node_id_migration() {
        assert_eq!(
            healthcheck_topic_path("mic"),
            "substrate/meta/adapter/mic/healthcheck"
        );
    }

    #[test]
    fn build_report_delta_emits_replace_at_topic_path() {
        let r = SensorHealthReport::healthy(1, 2.0, 2.0, 1);
        let d = build_report_delta("mic", &r);
        match d {
            StateDelta::Replace { path, value } => {
                assert_eq!(path, "substrate/meta/adapter/mic/healthcheck");
                assert_eq!(value["status"], "healthy");
            }
            other => panic!("expected Replace, got {other:?}"),
        }
    }

    #[test]
    fn round_trip_via_serde_preserves_required_fields() {
        let r = SensorHealthReport::healthy(1714000000000, 2.0, 1.98, 168420);
        let json_str = serde_json::to_string(&r).unwrap();
        let r2: SensorHealthReport = serde_json::from_str(&json_str).unwrap();
        assert_eq!(r, r2);
    }

    #[test]
    fn json_value_is_object_with_no_dangling_arrays() {
        // Sanity: the report must be a flat object — Explorer's
        // HealthViewer (per contract §5) renders fields as a table; a
        // top-level array would break that.
        let v = SensorHealthReport::unknown(1.0).to_value();
        assert!(v.is_object(), "report must serialize as object: {v:?}");
    }

    #[test]
    fn derive_status_treats_zero_configured_as_healthy_when_observed_positive() {
        // Edge case: a sensor with configured_rate_hz = 0 (event-driven,
        // no nominal cadence). Avoid divide-by-zero via the > 0 guard
        // and report Healthy whenever observed > 0 with no errors.
        assert_eq!(
            SensorHealthReport::derive_status(0.5, 0.0, 0),
            SensorStatus::Healthy
        );
    }
}
