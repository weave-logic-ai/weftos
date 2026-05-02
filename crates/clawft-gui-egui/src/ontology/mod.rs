//! Ontology — typed Object-Type layer above the substrate.
//!
//! This module is the runtime counterpart to `.planning/ontology/ADOPTION.md`.
//! Substrate remains the permissive core (path-keyed KV + pub/sub). The
//! ontology promotes well-known substrate shapes into typed Object Types
//! that carry properties, capability metadata, and eventually governance
//! slots.
//!
//! The dispatch pattern deliberately mirrors
//! [`crate::explorer::viewers`]: each `ObjectType` implements
//! `matches(value) -> u32` / `name()` / `display_name()` /
//! `properties()` and participates in a priority-cascade inference pass.
//! Viewers and Object Types coexist — the Explorer consults both.
//!
//! See [`ObjectType`] for the trait, [`types`] for concrete
//! implementations, and [`infer`] for the dispatch entry point.

use serde_json::Value;

pub mod types;

/// A single typed field declaration on an Object Type.
///
/// `PropertyDecl`s describe the *expected* shape of an Object — they
/// are informational for the MVP and will harden into validation hooks
/// as Action Types land.
#[derive(Debug, Clone, Copy)]
pub struct PropertyDecl {
    /// Property key (matches the substrate JSON field name).
    pub name: &'static str,
    /// Declared value kind. `Unknown` is legitimate for open-shape
    /// fields whose runtime type varies.
    pub kind: PropertyKind,
    /// Short human-readable doc string. Used by tooling + the Explorer
    /// property panel (future).
    pub doc: &'static str,
}

/// Kind tags for typed property fields. Arrays nest via a static
/// reference so the whole declaration stays `const`-compatible.
#[derive(Debug, Clone, Copy)]
pub enum PropertyKind {
    /// UTF-8 string.
    String,
    /// Boolean.
    Bool,
    /// Signed 64-bit integer.
    I64,
    /// Unsigned 64-bit integer.
    U64,
    /// 64-bit float.
    F64,
    /// Homogeneous array. Element kind is borrowed so declarations can
    /// stay `static`.
    Array(&'static PropertyKind),
    /// Nested object (shape left open at this layer).
    Object,
    /// Untyped / open-shape value.
    Unknown,
}

/// Capability metadata for an Object Type.
///
/// MVP: all three fields are empty. The shape is reserved so Action
/// Types, Function inputs, and viewer-priority hints can be wired
/// through without touching every concrete type declaration later.
#[derive(Debug, Clone, Copy, Default)]
pub struct ObjectTypeCapabilities {
    /// Action names that accept this Object Type as input. Empty in MVP.
    pub applicable_actions: &'static [&'static str],
    /// Event names this Object Type emits. Empty in MVP.
    pub events_emitted: &'static [&'static str],
    /// Optional suggestion to the viewer registry for the preferred
    /// renderer's priority ceiling. `None` in MVP.
    pub default_viewer_priority_hint: Option<u32>,
}

/// Trait defining a typed Object shape above the substrate.
///
/// All methods are associated functions — there is no instance state.
/// An Object Type is a *classifier* over substrate values, not a
/// container for them.
pub trait ObjectType {
    /// Stable identifier (snake_case). Used as the wire / memory key —
    /// e.g. "mesh", "audio_stream", "chain_event".
    fn name() -> &'static str;

    /// Human-readable display label for badges and UI surfaces.
    fn display_name() -> &'static str;

    /// Shape-match a substrate value. Return a priority > 0 if this
    /// Object Type applies; 0 otherwise. Higher priority wins during
    /// registry dispatch.
    fn matches(value: &Value) -> u32;

    /// Declared properties (typed fields) on this Object Type.
    fn properties() -> &'static [PropertyDecl];

    /// Capability metadata. Default impl returns an empty capability
    /// set — override only when a type has concrete Actions/events.
    fn capabilities() -> ObjectTypeCapabilities {
        ObjectTypeCapabilities::default()
    }
}

/// Result of [`infer`] — the winning Object Type's identity.
///
/// Intentionally minimal: name + display label. Callers that need
/// properties/capabilities can look them up through the type's own
/// associated functions once the name is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InferredType {
    /// Stable snake_case name — matches `ObjectType::name()`.
    pub name: &'static str,
    /// Human-readable label — matches `ObjectType::display_name()`.
    pub display: &'static str,
}

/// Shape-infer the highest-priority registered Object Type for `value`.
///
/// Returns `None` when no type claims the value. Unknown shapes fall
/// through to the JSON viewer at the viewer registry; the Explorer
/// simply skips the badge.
///
/// New Object Types register themselves at the marker comment below so
/// future tracks (Workshop, ⊃μBus modules, etc.) can splice in without
/// re-opening this function.
pub fn infer(value: &Value) -> Option<InferredType> {
    // Dispatch order follows priority convention: specialized types
    // first, generic envelopes last. Within the same priority tier the
    // source ordering here is the tie-breaker.
    //
    // [[OBJECT_TYPES_REGISTRATIONS_INSERT]]
    if types::health_report::HealthReport::matches(value) > 0 {
        return Some(InferredType {
            name: types::health_report::HealthReport::name(),
            display: types::health_report::HealthReport::display_name(),
        });
    }
    if types::audio_stream::AudioStream::matches(value) > 0 {
        return Some(InferredType {
            name: types::audio_stream::AudioStream::name(),
            display: types::audio_stream::AudioStream::display_name(),
        });
    }
    if types::chain_event::ChainEvent::matches(value) > 0 {
        return Some(InferredType {
            name: types::chain_event::ChainEvent::name(),
            display: types::chain_event::ChainEvent::display_name(),
        });
    }
    // Node before Mesh: a value carrying node-identity + sub-section
    // is more specifically a Node than a structural Mesh root.
    if types::node::Node::matches(value) > 0 {
        return Some(InferredType {
            name: types::node::Node::name(),
            display: types::node::Node::display_name(),
        });
    }
    if types::sensor::Sensor::matches(value) > 0 {
        return Some(InferredType {
            name: types::sensor::Sensor::name(),
            display: types::sensor::Sensor::display_name(),
        });
    }
    if types::mesh::Mesh::matches(value) > 0 {
        return Some(InferredType {
            name: types::mesh::Mesh::name(),
            display: types::mesh::Mesh::display_name(),
        });
    }
    None
}

#[cfg(test)]
mod integration {
    use super::*;
    use serde_json::json;

    #[test]
    fn infers_audio_stream_from_rms_peak_pair() {
        let v = json!({
            "rms_db": -41.2,
            "peak_db": -17.1,
            "available": true,
            "sample_rate": 16000,
            "tick": 214_u64,
        });
        let inferred = infer(&v).expect("audio stream should infer");
        assert_eq!(inferred.name, "audio_stream");
        assert_eq!(inferred.display, "Audio Stream");
    }

    #[test]
    fn infers_chain_event_stream_from_array() {
        let v = json!([
            { "seq": 1_u64, "ts": 1_700_000_000_u64, "kind": "agent.spawn" },
            { "seq": 2_u64, "ts": 1_700_000_001_u64, "kind": "agent.stop" },
        ]);
        let inferred = infer(&v).expect("chain event should infer");
        assert_eq!(inferred.name, "chain_event");
    }

    #[test]
    fn infers_mesh_from_top_level_sections() {
        let v = json!({
            "kernel": {},
            "cluster": {},
            "chain": {},
            "sensor": {},
            "network": {},
        });
        let inferred = infer(&v).expect("mesh should infer");
        assert_eq!(inferred.name, "mesh");
    }

    #[test]
    fn returns_none_for_unknown_shape() {
        let v = json!({ "arbitrary": "blob", "count": 42 });
        assert!(infer(&v).is_none());
    }

    #[test]
    fn returns_none_for_null() {
        assert!(infer(&Value::Null).is_none());
    }

    #[test]
    fn audio_stream_wins_over_mesh_on_overlap() {
        // A value that looks mesh-shaped AND has rms/peak should prefer
        // the more specific audio_stream classification. Practically
        // unlikely, but the dispatch order makes the guarantee explicit.
        let v = json!({
            "rms_db": -50.0,
            "peak_db": -30.0,
            "kernel": {},
            "cluster": {},
            "chain": {},
            "sensor": {},
        });
        let inferred = infer(&v).expect("should infer something");
        assert_eq!(inferred.name, "audio_stream");
    }
}
