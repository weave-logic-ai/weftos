//! `Node` — typed Object for a substrate node root.
//!
//! Shape: an object that carries a stable node identifier
//! (`node_id`, `peer_id`, or a `pubkey` field) AND at least one of the
//! per-node sub-trees (`health`, `sensor`, `meta`, `kernel`). The
//! daemon publishes node roots at `substrate/<node-id>/...` and a
//! consolidated `meta` snapshot lives at the root key itself.
//!
//! Priority: 15 — between specialised payload viewers (8-12) and the
//! Mesh structural classifier (20). When a value carries node-identity
//! and at least one node sub-section it should classify as Node, not
//! as the inner sub-tree.

use super::super::{ObjectType, ObjectTypeCapabilities, PropertyDecl, PropertyKind};
use serde_json::Value;

/// Identity-bearing fields. Presence of any one signals a Node root.
const NODE_ID_KEYS: &[&str] = &["node_id", "peer_id", "pubkey"];

/// Per-node sub-tree keys. The classifier wants at least one to avoid
/// promoting a bare `{pubkey}` chat envelope into a Node.
const NODE_SECTION_KEYS: &[&str] = &["health", "sensor", "meta", "kernel", "agent"];

/// Typed Object for a substrate node root.
pub struct Node;

impl ObjectType for Node {
    fn name() -> &'static str {
        "node"
    }

    fn display_name() -> &'static str {
        "Node"
    }

    fn matches(value: &Value) -> u32 {
        let Some(obj) = value.as_object() else {
            return 0;
        };
        let has_id = NODE_ID_KEYS
            .iter()
            .any(|k| obj.get(*k).and_then(Value::as_str).is_some());
        if !has_id {
            return 0;
        }
        let has_section = NODE_SECTION_KEYS
            .iter()
            .any(|k| obj.get(*k).map(Value::is_object).unwrap_or(false));
        if has_section { 15 } else { 0 }
    }

    fn properties() -> &'static [PropertyDecl] {
        &[
            PropertyDecl {
                name: "node_id",
                kind: PropertyKind::String,
                doc: "Stable BLAKE3-prefixed node identifier (n-<6-hex>).",
            },
            PropertyDecl {
                name: "pubkey",
                kind: PropertyKind::String,
                doc: "Ed25519 public key of the node's identity.",
            },
            PropertyDecl {
                name: "health",
                kind: PropertyKind::Object,
                doc: "Periodic health snapshot.",
            },
            PropertyDecl {
                name: "sensor",
                kind: PropertyKind::Object,
                doc: "Sensor sub-tree (mic, tof, camera, …).",
            },
            PropertyDecl {
                name: "meta",
                kind: PropertyKind::Object,
                doc: "Static node metadata (model, fw_version, role, …).",
            },
        ]
    }

    fn capabilities() -> ObjectTypeCapabilities {
        ObjectTypeCapabilities {
            // WEFT-276: read-only Action surface for a Node.
            applicable_actions: &[
                "node.copy_pubkey",
                "node.export_snapshot",
                "node.refresh_health",
            ],
            events_emitted: &[],
            default_viewer_priority_hint: Some(15),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn matches_node_id_plus_health() {
        let v = json!({
            "node_id": "n-bfc4cd",
            "health": { "rssi": -47, "free_heap": 184_000 },
        });
        assert_eq!(Node::matches(&v), 15);
    }

    #[test]
    fn matches_pubkey_plus_sensor() {
        let v = json!({
            "pubkey": "abcdef0123456789",
            "sensor": { "mic": {} },
        });
        assert_eq!(Node::matches(&v), 15);
    }

    #[test]
    fn rejects_id_without_section() {
        let v = json!({ "node_id": "n-bfc4cd" });
        assert_eq!(Node::matches(&v), 0);
    }

    #[test]
    fn rejects_section_without_id() {
        let v = json!({ "health": { "rssi": -47, "free_heap": 184_000 } });
        assert_eq!(Node::matches(&v), 0);
    }

    #[test]
    fn rejects_section_keys_with_non_object_values() {
        let v = json!({ "node_id": "n-bfc4cd", "health": "ok" });
        assert_eq!(Node::matches(&v), 0);
    }

    #[test]
    fn rejects_array() {
        assert_eq!(Node::matches(&json!([1, 2, 3])), 0);
    }

    #[test]
    fn rejects_null() {
        assert_eq!(Node::matches(&Value::Null), 0);
    }

    #[test]
    fn declares_actions() {
        let caps = Node::capabilities();
        assert!(caps.applicable_actions.contains(&"node.copy_pubkey"));
    }

    #[test]
    fn identity_metadata() {
        assert_eq!(Node::name(), "node");
        assert_eq!(Node::display_name(), "Node");
    }
}
