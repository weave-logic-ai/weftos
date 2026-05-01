//! `HealthReport` — typed Object for periodic node-health snapshots
//! published at `substrate/<node>/health`.
//!
//! Shape (see `.planning/sensors/EXPLORER-MANAGEMENT-SURFACE.md`
//! affordances #1, #2): an object with at least one of the canonical
//! health-scalar fields (`rssi`, `free_heap`, `uptime_s`, `cpu_pct`,
//! `temp_c`, `tick`) plus typically a `node_id` or `kind:"health"`
//! discriminator.
//!
//! Priority: 12 — slightly above the leaf viewer parity tier (10) so a
//! health snapshot wins decisively over the generic JSON badge but
//! still loses to Mesh's structural classifier (20). The intent is
//! that under `substrate/<node>/health` we always render as
//! HealthReport even when the snapshot only has two scalar fields.

use super::super::{ObjectType, ObjectTypeCapabilities, PropertyDecl, PropertyKind};
use serde_json::Value;

/// Canonical health scalar fields. Presence of any one of these on an
/// object root is the lowest-bar match. The viewer side filters to
/// scalars present at paint time, so adding to this list is forward-
/// compatible.
const HEALTH_SCALAR_KEYS: &[&str] = &[
    "rssi",
    "free_heap",
    "uptime_s",
    "cpu_pct",
    "temp_c",
    "tick",
];

/// Minimum number of scalar matches before classifying as HealthReport.
/// Two prevents a single stray `tick` field on an unrelated payload
/// from collapsing into HealthReport.
const HEALTH_SCALAR_THRESHOLD: usize = 2;

/// Typed Object for periodic node-health snapshots.
pub struct HealthReport;

impl ObjectType for HealthReport {
    fn name() -> &'static str {
        "health_report"
    }

    fn display_name() -> &'static str {
        "Health Report"
    }

    fn matches(value: &Value) -> u32 {
        let Some(obj) = value.as_object() else {
            return 0;
        };
        // Strong discriminator: explicit `kind: "health"` always wins.
        if obj.get("kind").and_then(Value::as_str) == Some("health") {
            return 12;
        }
        let hits = HEALTH_SCALAR_KEYS
            .iter()
            .filter(|k| obj.get(**k).map(Value::is_number).unwrap_or(false))
            .count();
        if hits >= HEALTH_SCALAR_THRESHOLD { 12 } else { 0 }
    }

    fn properties() -> &'static [PropertyDecl] {
        &[
            PropertyDecl {
                name: "rssi",
                kind: PropertyKind::I64,
                doc: "Wi-Fi RSSI in dBm (typically -90 … -30).",
            },
            PropertyDecl {
                name: "free_heap",
                kind: PropertyKind::U64,
                doc: "Free heap bytes at sample time.",
            },
            PropertyDecl {
                name: "uptime_s",
                kind: PropertyKind::U64,
                doc: "Seconds since the publishing node booted.",
            },
            PropertyDecl {
                name: "cpu_pct",
                kind: PropertyKind::F64,
                doc: "Recent CPU utilisation as a percentage 0..100.",
            },
            PropertyDecl {
                name: "temp_c",
                kind: PropertyKind::F64,
                doc: "Board temperature in degrees Celsius.",
            },
            PropertyDecl {
                name: "tick",
                kind: PropertyKind::U64,
                doc: "Monotonic publisher tick counter.",
            },
        ]
    }

    fn capabilities() -> ObjectTypeCapabilities {
        ObjectTypeCapabilities {
            // WEFT-276: read-only Action surface for a HealthReport.
            // The full pipeline lands later (T08-33+); for now we
            // declare so the Explorer can render a passive list.
            applicable_actions: &["health.refresh", "health.copy_snapshot"],
            events_emitted: &[],
            default_viewer_priority_hint: Some(12),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn matches_full_health_shape() {
        let v = json!({
            "rssi": -47,
            "free_heap": 184_000_u64,
            "uptime_s": 12_345_u64,
            "tick": 42_u64,
        });
        assert_eq!(HealthReport::matches(&v), 12);
    }

    #[test]
    fn matches_kind_discriminator_alone() {
        let v = json!({ "kind": "health" });
        assert_eq!(HealthReport::matches(&v), 12);
    }

    #[test]
    fn matches_minimum_two_scalars() {
        let v = json!({ "rssi": -60, "free_heap": 32_000_u64 });
        assert_eq!(HealthReport::matches(&v), 12);
    }

    #[test]
    fn rejects_single_scalar_below_threshold() {
        let v = json!({ "tick": 1_u64 });
        assert_eq!(HealthReport::matches(&v), 0);
    }

    #[test]
    fn rejects_string_encoded_scalars() {
        // Strict typing — strings don't pass `is_number`.
        let v = json!({ "rssi": "-47", "free_heap": "184000" });
        assert_eq!(HealthReport::matches(&v), 0);
    }

    #[test]
    fn rejects_array() {
        let v = json!([1, 2, 3]);
        assert_eq!(HealthReport::matches(&v), 0);
    }

    #[test]
    fn rejects_null() {
        assert_eq!(HealthReport::matches(&Value::Null), 0);
    }

    #[test]
    fn rejects_unrelated_object() {
        let v = json!({ "name": "foo", "value": 7 });
        assert_eq!(HealthReport::matches(&v), 0);
    }

    #[test]
    fn declares_expected_properties() {
        let props = HealthReport::properties();
        let names: Vec<&str> = props.iter().map(|p| p.name).collect();
        assert!(names.contains(&"rssi"));
        assert!(names.contains(&"free_heap"));
        assert!(names.contains(&"uptime_s"));
        assert!(names.contains(&"tick"));
    }

    #[test]
    fn identity_metadata() {
        assert_eq!(HealthReport::name(), "health_report");
        assert_eq!(HealthReport::display_name(), "Health Report");
    }

    #[test]
    fn declares_actions_for_health_report() {
        // WEFT-276 acceptance: capability surface is non-empty for the
        // typed Action lookup.
        let caps = HealthReport::capabilities();
        assert!(caps.applicable_actions.contains(&"health.refresh"));
    }
}
