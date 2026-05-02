//! `AudioStream` — typed Object for live audio level snapshots.
//!
//! Shape parallels the INMP441 MEMS mic publish at
//! `substrate/sensor/mic`: `{ rms_db, peak_db, available, sample_rate,
//! tick }`. This overlap with `AudioMeterViewer`'s shape is
//! intentional — the Object Type and the Viewer both key off the same
//! `rms_db` + `peak_db` pair, but live at different layers. The viewer
//! paints; the type declares.
//!
//! Priority: 10 (matches viewer parity).

use super::super::{ObjectType, PropertyDecl, PropertyKind};
use serde_json::Value;

/// Typed Object for live audio level snapshots (dBFS RMS + peak).
pub struct AudioStream;

impl ObjectType for AudioStream {
    fn name() -> &'static str {
        "audio_stream"
    }

    fn display_name() -> &'static str {
        "Audio Stream"
    }

    fn matches(value: &Value) -> u32 {
        let Some(obj) = value.as_object() else {
            return 0;
        };
        let has_rms = obj.get("rms_db").and_then(Value::as_f64).is_some();
        let has_peak = obj.get("peak_db").and_then(Value::as_f64).is_some();
        if has_rms && has_peak { 10 } else { 0 }
    }

    fn properties() -> &'static [PropertyDecl] {
        &[
            PropertyDecl {
                name: "rms_db",
                kind: PropertyKind::F64,
                doc: "Root-mean-square level in dBFS.",
            },
            PropertyDecl {
                name: "peak_db",
                kind: PropertyKind::F64,
                doc: "Peak level in dBFS for the most recent window.",
            },
            PropertyDecl {
                name: "available",
                kind: PropertyKind::Bool,
                doc: "Whether the capture device is currently delivering audio.",
            },
            PropertyDecl {
                name: "sample_rate",
                kind: PropertyKind::I64,
                doc: "Sample rate in Hz.",
            },
            PropertyDecl {
                name: "tick",
                kind: PropertyKind::U64,
                doc: "Monotonic publisher tick counter.",
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn matches_full_shape() {
        let v = json!({
            "rms_db": -41.2,
            "peak_db": -17.1,
            "available": true,
            "sample_rate": 16000,
            "tick": 214_u64,
        });
        assert_eq!(AudioStream::matches(&v), 10);
    }

    #[test]
    fn matches_minimal_pair() {
        let v = json!({ "rms_db": -50.0, "peak_db": -30.0 });
        assert_eq!(AudioStream::matches(&v), 10);
    }

    #[test]
    fn rejects_only_rms() {
        let v = json!({ "rms_db": -41.2 });
        assert_eq!(AudioStream::matches(&v), 0);
    }

    #[test]
    fn rejects_only_peak() {
        let v = json!({ "peak_db": -17.1 });
        assert_eq!(AudioStream::matches(&v), 0);
    }

    #[test]
    fn rejects_string_encoded_values() {
        // String "-41.2" is not an f64 — strict typing guards against
        // misclassifying chat transcripts that happen to share keys.
        let v = json!({ "rms_db": "-41.2", "peak_db": "-17.1" });
        assert_eq!(AudioStream::matches(&v), 0);
    }

    #[test]
    fn rejects_null_peak() {
        let v = json!({ "rms_db": -41.2, "peak_db": null });
        assert_eq!(AudioStream::matches(&v), 0);
    }

    #[test]
    fn rejects_empty_object() {
        assert_eq!(AudioStream::matches(&json!({})), 0);
    }

    #[test]
    fn rejects_null() {
        assert_eq!(AudioStream::matches(&Value::Null), 0);
    }

    #[test]
    fn rejects_array() {
        let v = json!([-41.2, -17.1]);
        assert_eq!(AudioStream::matches(&v), 0);
    }

    #[test]
    fn declares_expected_properties() {
        let props = AudioStream::properties();
        let names: Vec<&str> = props.iter().map(|p| p.name).collect();
        assert_eq!(
            names,
            vec!["rms_db", "peak_db", "available", "sample_rate", "tick"]
        );
    }

    #[test]
    fn identity_metadata() {
        assert_eq!(AudioStream::name(), "audio_stream");
        assert_eq!(AudioStream::display_name(), "Audio Stream");
    }
}
