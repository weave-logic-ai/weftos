//! `Mesh` — the root Object Type. One Mesh instance per WeftOS mesh
//! network.
//!
//! From `.planning/ontology/ADOPTION.md` §5: "A single WeftOS mesh
//! network IS one Object instance — a Mesh. Every other Object lives
//! inside that Mesh's namespace."
//!
//! ## Matching heuristic
//!
//! Mesh identity is fuzzy at the shape layer. There is no single
//! canonical Mesh path in substrate today, so we infer Mesh-ness from
//! the presence of the *top-level substrate sections* that the daemon
//! populates: `kernel`, `cluster`, `chain`, `sensor`, `network`,
//! `agent`. When enough of those sections appear as nested objects at
//! the top level, we classify the value as a Mesh root.
//!
//! Threshold: at least 3 known section keys. Priority: 20 — higher
//! than the specialized types (10) because Mesh is structural, not
//! payload-shaped; when it matches it wins decisively over any single
//! leaf viewer. This is intentional: the Explorer's top-level snapshot
//! should display as a Mesh, not as a mis-classified leaf.

use super::super::{ObjectType, ObjectTypeCapabilities, PropertyDecl, PropertyKind};
use serde_json::Value;

/// Known top-level substrate sections the daemon populates. A value
/// having ≥ 3 of these as object-valued keys is classified as a Mesh.
const MESH_SECTION_KEYS: &[&str] = &[
    "kernel",
    "cluster",
    "chain",
    "sensor",
    "network",
    "agent",
];

/// Minimum number of known section keys before a value qualifies as a
/// Mesh. Chosen at 3 so a stub value with just `{ kernel, cluster }`
/// doesn't false-positive during early-boot snapshots.
const MESH_SECTION_THRESHOLD: usize = 3;

/// Root Object Type for a WeftOS mesh network.
pub struct Mesh;

impl ObjectType for Mesh {
    fn name() -> &'static str {
        "mesh"
    }

    fn display_name() -> &'static str {
        "Mesh"
    }

    fn matches(value: &Value) -> u32 {
        let Some(obj) = value.as_object() else {
            return 0;
        };
        let hits = MESH_SECTION_KEYS
            .iter()
            .filter(|k| obj.get(**k).map(Value::is_object).unwrap_or(false))
            .count();
        if hits >= MESH_SECTION_THRESHOLD { 20 } else { 0 }
    }

    fn properties() -> &'static [PropertyDecl] {
        &[
            PropertyDecl {
                name: "mesh_id",
                kind: PropertyKind::String,
                doc: "Stable identifier for this mesh network.",
            },
            PropertyDecl {
                name: "kernel",
                kind: PropertyKind::Object,
                doc: "Kernel status + runtime metadata section.",
            },
            PropertyDecl {
                name: "cluster",
                kind: PropertyKind::Object,
                doc: "Cluster / peer membership section.",
            },
            PropertyDecl {
                name: "chain",
                kind: PropertyKind::Object,
                doc: "ExoChain event log + status section.",
            },
            PropertyDecl {
                name: "sensor",
                kind: PropertyKind::Object,
                doc: "Sensor roots (mic, tof, camera, …).",
            },
        ]
    }

    fn capabilities() -> ObjectTypeCapabilities {
        ObjectTypeCapabilities {
            // WEFT-276: read-only Action surface for the Mesh root.
            // Surfaces in the Explorer detail pane as a passive list
            // until the Action pipeline (T08-33+) lands.
            applicable_actions: &[
                "mesh.export_snapshot",
                "mesh.list_nodes",
                "mesh.refresh",
            ],
            events_emitted: &[],
            default_viewer_priority_hint: Some(20),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn matches_full_mesh_shape() {
        let v = json!({
            "kernel": { "uptime_s": 42 },
            "cluster": { "peers": [] },
            "chain": { "sequence": 1 },
            "sensor": { "mic": {} },
            "network": { "state": "connected" },
        });
        assert_eq!(Mesh::matches(&v), 20);
    }

    #[test]
    fn matches_minimum_threshold() {
        let v = json!({
            "kernel": {},
            "cluster": {},
            "chain": {},
        });
        assert_eq!(Mesh::matches(&v), 20);
    }

    #[test]
    fn rejects_below_threshold() {
        // Two sections is not enough — could be a partial snapshot or a
        // coincidence.
        let v = json!({
            "kernel": {},
            "cluster": {},
        });
        assert_eq!(Mesh::matches(&v), 0);
    }

    #[test]
    fn rejects_section_keys_with_non_object_values() {
        // Scalars under the section keys don't count — a real Mesh has
        // nested objects.
        let v = json!({
            "kernel": "booting",
            "cluster": 3,
            "chain": true,
        });
        assert_eq!(Mesh::matches(&v), 0);
    }

    #[test]
    fn rejects_unrelated_keys() {
        let v = json!({
            "foo": {},
            "bar": {},
            "baz": {},
        });
        assert_eq!(Mesh::matches(&v), 0);
    }

    #[test]
    fn rejects_array() {
        let v = json!([{ "kernel": {} }, { "cluster": {} }, { "chain": {} }]);
        assert_eq!(Mesh::matches(&v), 0);
    }

    #[test]
    fn rejects_null() {
        assert_eq!(Mesh::matches(&Value::Null), 0);
    }

    #[test]
    fn rejects_empty_object() {
        assert_eq!(Mesh::matches(&json!({})), 0);
    }

    #[test]
    fn declares_expected_properties() {
        let props = Mesh::properties();
        let names: Vec<&str> = props.iter().map(|p| p.name).collect();
        assert!(names.contains(&"mesh_id"));
        assert!(names.contains(&"kernel"));
        assert!(names.contains(&"cluster"));
        assert!(names.contains(&"chain"));
        assert!(names.contains(&"sensor"));
    }

    #[test]
    fn identity_metadata() {
        assert_eq!(Mesh::name(), "mesh");
        assert_eq!(Mesh::display_name(), "Mesh");
    }
}
