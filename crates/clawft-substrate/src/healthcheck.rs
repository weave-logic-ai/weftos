//! Per-sensor healthcheck contract — `.planning/sensors/HEALTHCHECK-CONTRACT.md`.
//!
//! Implements the sensor-level `HealthReport` shape (§3 of the contract)
//! plus a small builder. Node-level reports (§2) require a node-id +
//! aggregator pipeline that is out of scope for the substrate crate;
//! they will land alongside the daemon-side health aggregator (§7 of the
//! contract).
//!
//! ## Why this lives in `clawft-substrate`
//!
//! The contract path is `substrate/<node-id>/health/sensor/<sensor-name>`,
//! which means every sensor adapter that publishes into the substrate is
//! the natural emitter. Until per-node namespacing lands (WEFT-418), we
//! emit at `substrate/meta/adapter/<adapter-id>/healthcheck` so the
//! shape can be wired and tested today; the path migrates cleanly when
//! node-id scoping arrives because the *value shape* stays the same.
//!
//! ## Status enum
//!
//! Sensor-level adds `"stale"` and `"unknown"` to the node-level set.
//! `"unknown"` covers the pre-first-emit window (per contract §9 open
//! question 1); we ship it now since open question 1's resolution was
//! "add `unknown` now."

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::delta::StateDelta;

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
        if configured_rate_hz > 0.0
            && observed_rate_hz < 0.5 * configured_rate_hz
        {
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
mod tests {
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
