//! `Sensor` — typed Object for a sensor leaf or sub-tree under
//! `substrate/<node>/sensor/<kind>` (mic, tof, camera, …).
//!
//! Shape: an object with a `kind` field whose value names a known
//! sensor family (`mic`, `tof`, `camera`, `imu`, `temp`, `range`,
//! `audio`, `video`, `pcm_chunk`), OR an object that carries a paired
//! `raw`/`summary` split (the management-surface contract for sensors
//! that publish both layers — see WEFT-269).
//!
//! Priority: 8 — below specialised payload viewers (10) so an audio
//! meter or PCM-chunk viewer still wins on the inner leaf, but above
//! the JSON fallback (1) so a bare `{kind:"mic", raw:..., summary:...}`
//! envelope renders with the SensorViewer chrome.

use super::super::{ObjectType, ObjectTypeCapabilities, PropertyDecl, PropertyKind};
use serde_json::Value;

/// Known sensor kind discriminators. Adding to this list is forward-
/// compatible — new kinds just start matching once they appear.
pub const SENSOR_KINDS: &[&str] = &[
    "mic",
    "tof",
    "camera",
    "imu",
    "temp",
    "range",
    "audio",
    "video",
    "pcm_chunk",
];

/// Typed Object for a sensor envelope (raw + summary).
pub struct Sensor;

impl ObjectType for Sensor {
    fn name() -> &'static str {
        "sensor"
    }

    fn display_name() -> &'static str {
        "Sensor"
    }

    fn matches(value: &Value) -> u32 {
        let Some(obj) = value.as_object() else {
            return 0;
        };
        // Strong signal: an explicit `kind:` we recognise as a sensor.
        if let Some(k) = obj.get("kind").and_then(Value::as_str)
            && SENSOR_KINDS.contains(&k)
        {
            return 8;
        }
        // Soft signal: paired raw + summary fields. Any object with
        // both keys is a sensor envelope by management-surface
        // convention, regardless of payload shape.
        if obj.contains_key("raw") && obj.contains_key("summary") {
            return 8;
        }
        0
    }

    fn properties() -> &'static [PropertyDecl] {
        &[
            PropertyDecl {
                name: "kind",
                kind: PropertyKind::String,
                doc: "Sensor family discriminator (mic, tof, camera, …).",
            },
            PropertyDecl {
                name: "raw",
                kind: PropertyKind::Unknown,
                doc: "Raw sensor payload at native cadence.",
            },
            PropertyDecl {
                name: "summary",
                kind: PropertyKind::Unknown,
                doc: "Down-sampled / aggregated summary view.",
            },
        ]
    }

    fn capabilities() -> ObjectTypeCapabilities {
        ObjectTypeCapabilities {
            // WEFT-276: read-only Action surface for a Sensor.
            applicable_actions: &[
                "sensor.toggle_summary",
                "sensor.snapshot",
                "sensor.copy_path",
            ],
            events_emitted: &[],
            default_viewer_priority_hint: Some(8),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn matches_known_kind_mic() {
        let v = json!({ "kind": "mic", "rms_db": -41.2 });
        assert_eq!(Sensor::matches(&v), 8);
    }

    #[test]
    fn matches_paired_raw_summary() {
        let v = json!({ "raw": { "frame": 1 }, "summary": { "rms_db": -40.0 } });
        assert_eq!(Sensor::matches(&v), 8);
    }

    #[test]
    fn rejects_unknown_kind() {
        let v = json!({ "kind": "biscuit" });
        assert_eq!(Sensor::matches(&v), 0);
    }

    #[test]
    fn rejects_only_raw() {
        let v = json!({ "raw": { "frame": 1 } });
        assert_eq!(Sensor::matches(&v), 0);
    }

    #[test]
    fn rejects_array() {
        assert_eq!(Sensor::matches(&json!([1, 2, 3])), 0);
    }

    #[test]
    fn rejects_null() {
        assert_eq!(Sensor::matches(&Value::Null), 0);
    }

    #[test]
    fn declares_actions() {
        let caps = Sensor::capabilities();
        assert!(caps.applicable_actions.contains(&"sensor.snapshot"));
    }

    #[test]
    fn identity_metadata() {
        assert_eq!(Sensor::name(), "sensor");
        assert_eq!(Sensor::display_name(), "Sensor");
    }
}
