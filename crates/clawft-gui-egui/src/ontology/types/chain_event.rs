//! `ChainEvent` — typed Object for an ExoChain event stream.
//!
//! Matches an **array** of event records, where each record carries at
//! least `seq` (u64), `ts` (u64 or ISO-8601 string), and `kind`
//! (string). Payload is the optional nested object with the
//! event-specific detail.
//!
//! The substrate publish site for per-event rows is `substrate/chain/*`;
//! the Explorer view of the chain-tail naturally materialises as an
//! array, which is the shape this type claims.
//!
//! Priority: 10. Array shapes are specific enough that this is a clean
//! classification when it hits.

use super::super::{ObjectType, PropertyDecl, PropertyKind};
use serde_json::Value;

/// Minimum number of records to scan when classifying. An empty array
/// is ambiguous — could be any stream — so we require at least one
/// well-formed row before claiming the shape.
const MIN_SAMPLE_ROWS: usize = 1;

/// Typed Object for a stream (array) of ExoChain event rows.
pub struct ChainEvent;

impl ObjectType for ChainEvent {
    fn name() -> &'static str {
        "chain_event"
    }

    fn display_name() -> &'static str {
        "Chain Event Stream"
    }

    fn matches(value: &Value) -> u32 {
        let Some(arr) = value.as_array() else {
            return 0;
        };
        if arr.len() < MIN_SAMPLE_ROWS {
            return 0;
        }
        // Every row must be an object with seq + ts + kind in the
        // expected shape. Strict: one bad row declines the match, so
        // noisy mixed arrays fall through to the JSON fallback.
        for row in arr {
            if !row_looks_like_chain_event(row) {
                return 0;
            }
        }
        10
    }

    fn properties() -> &'static [PropertyDecl] {
        &[
            PropertyDecl {
                name: "seq",
                kind: PropertyKind::U64,
                doc: "Monotonic chain sequence number.",
            },
            PropertyDecl {
                name: "ts",
                kind: PropertyKind::U64,
                doc: "Event timestamp (unix millis or ISO-8601 string).",
            },
            PropertyDecl {
                name: "kind",
                kind: PropertyKind::String,
                doc: "Event kind discriminator (e.g. 'agent.spawn').",
            },
            PropertyDecl {
                name: "payload",
                kind: PropertyKind::Object,
                doc: "Optional event-specific payload object.",
            },
        ]
    }
}

/// Check that a single array element has the mandatory ChainEvent row
/// fields: `seq` (u64), `ts` (u64 or string), `kind` (string).
fn row_looks_like_chain_event(row: &Value) -> bool {
    let Some(obj) = row.as_object() else {
        return false;
    };
    let has_seq = obj.get("seq").and_then(Value::as_u64).is_some();
    // `ts` is lenient: either u64 (unix epoch) or string (ISO-8601).
    // Both are seen in substrate: the daemon publishes ISO strings,
    // kernel-internal events use u64 epoch ms.
    let has_ts = match obj.get("ts") {
        Some(Value::Number(n)) => n.as_u64().is_some(),
        Some(Value::String(_)) => true,
        _ => false,
    };
    let has_kind = obj.get("kind").and_then(Value::as_str).is_some();
    has_seq && has_ts && has_kind
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn matches_numeric_timestamp_stream() {
        let v = json!([
            { "seq": 1_u64, "ts": 1_700_000_000_u64, "kind": "agent.spawn" },
            { "seq": 2_u64, "ts": 1_700_000_001_u64, "kind": "agent.stop",
              "payload": { "pid": 7 } },
        ]);
        assert_eq!(ChainEvent::matches(&v), 10);
    }

    #[test]
    fn matches_string_timestamp_stream() {
        // Daemon-side ChainEventInfo uses ISO-8601 `timestamp` — this
        // test locks in that the type accepts string `ts` too.
        let v = json!([
            { "seq": 1_u64, "ts": "2026-04-23T00:00:00Z", "kind": "kernel.boot" },
        ]);
        assert_eq!(ChainEvent::matches(&v), 10);
    }

    #[test]
    fn matches_single_row() {
        let v = json!([{ "seq": 42_u64, "ts": 0_u64, "kind": "x" }]);
        assert_eq!(ChainEvent::matches(&v), 10);
    }

    #[test]
    fn rejects_empty_array() {
        assert_eq!(ChainEvent::matches(&json!([])), 0);
    }

    #[test]
    fn rejects_row_missing_seq() {
        let v = json!([{ "ts": 1_u64, "kind": "x" }]);
        assert_eq!(ChainEvent::matches(&v), 0);
    }

    #[test]
    fn rejects_row_missing_kind() {
        let v = json!([{ "seq": 1_u64, "ts": 1_u64 }]);
        assert_eq!(ChainEvent::matches(&v), 0);
    }

    #[test]
    fn rejects_row_with_negative_seq() {
        // Negative ints are not u64; this guards against misreading a
        // signed stream as an unsigned chain-sequence.
        let v = json!([{ "seq": -1, "ts": 1_u64, "kind": "x" }]);
        assert_eq!(ChainEvent::matches(&v), 0);
    }

    #[test]
    fn rejects_mixed_row_shapes() {
        let v = json!([
            { "seq": 1_u64, "ts": 1_u64, "kind": "x" },
            { "hello": "world" },
        ]);
        assert_eq!(ChainEvent::matches(&v), 0);
    }

    #[test]
    fn rejects_non_array_value() {
        let v = json!({ "seq": 1_u64, "ts": 1_u64, "kind": "x" });
        assert_eq!(ChainEvent::matches(&v), 0);
    }

    #[test]
    fn rejects_null() {
        assert_eq!(ChainEvent::matches(&Value::Null), 0);
    }

    #[test]
    fn declares_expected_properties() {
        let props = ChainEvent::properties();
        let names: Vec<&str> = props.iter().map(|p| p.name).collect();
        assert_eq!(names, vec!["seq", "ts", "kind", "payload"]);
    }

    #[test]
    fn identity_metadata() {
        assert_eq!(ChainEvent::name(), "chain_event");
        assert_eq!(ChainEvent::display_name(), "Chain Event Stream");
    }
}
