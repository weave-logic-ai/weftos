//! Node identity registry.
//!
//! A **Node** is a physical thing in the mesh — an ESP32, the daemon
//! host, a Pi. It signs *emissions* (sensor data, heartbeats, anything
//! it merely reports). Sensing is not acting; nodes are deliberately
//! distinct from actors ([`crate::agent_registry::AgentRegistry`]),
//! which sign *Actions* (Foundry-style mutations).
//!
//! Each node holds an Ed25519 keypair. The node-id is the Ed25519
//! pubkey's short fingerprint: an `n-` prefix followed by the first
//! 6 hex chars of `BLAKE3(pubkey)`. Format committed in
//! `.planning/sensors/JOURNALED-NODE-ESP32.md` §2.2 — compact (8
//! chars), self-authenticating (recompute to verify), and prefixed
//! so a node-id never collides with a reserved word like `_derived`
//! or `meta`. A friendly human label lives as a property at
//! `substrate/<node-id>/meta/label`, not as the identity itself —
//! labels can collide, keys cannot.
//!
//! # Substrate write gate
//!
//! Under the node-identity contract every substrate write belongs to
//! exactly one node and must land under that node's namespace:
//!
//! ```text
//! substrate/<node-id>/...
//! ```
//!
//! This module provides the registry (node-id → pubkey) and the
//! canonical signing payload
//! ([`node_publish_payload`]). Enforcement of the prefix rule lives
//! on [`SubstrateService`][crate::substrate_service::SubstrateService];
//! signature verification lives at the RPC boundary in
//! `clawft-weave`. Both consult this registry.
//!
//! The registry is in-memory. Node keys are provisioned on the node
//! side (firmware-burned for the ESP32, persisted on disk for the
//! daemon) and registered at kernel boot. Restart-resets by design —
//! the on-disk provisioning is the source of truth.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};
use dashmap::DashMap;

/// Scope of a [`DerivedWriteGrant`]. Decides whether the registered
/// `topic` matches one literal subtree or all subtrees beneath it.
///
/// See `.planning/sensors/PIPELINE-PRIMITIVE-JOURNAL.md` §R3.6.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantScope {
    /// Grant covers exactly the path `substrate/_derived/<topic>` (no
    /// children). Useful for single-leaf canonical facts like
    /// `_derived/chain/head`.
    ExactTopic,
    /// Grant covers any path matching `substrate/_derived/<topic>/...`.
    /// Used by pipelines that publish a per-source subtree (whisper:
    /// `_derived/transcript/<source-node>/mic`).
    TopicPrefix,
}

/// Capability allowing a node to publish under
/// `substrate/_derived/<topic>` (or its subtree, depending on
/// [`GrantScope`]).
///
/// Mesh-canonical paths (`substrate/_derived/...`) are explicitly
/// outside the per-node prefix rule; without a grant the
/// substrate write gate refuses every publish to that subtree, even
/// from a daemon-class node. See
/// `.planning/sensors/PIPELINE-PRIMITIVE-JOURNAL.md` §R3.6 for the
/// design rationale.
///
/// **MVP scope:** grants are issued in-process by the daemon to
/// itself; there is no signature on the grant itself, no revocation
/// API, and no cross-node federation. Those land later (R3.6 calls
/// them out as deferred).
#[derive(Debug, Clone)]
pub struct DerivedWriteGrant {
    /// Node id permitted to write under the topic.
    pub grantee_node_id: String,
    /// Topic segment immediately under `_derived/`. For whisper this
    /// is `"transcript"`. Stored without the `_derived/` prefix so
    /// the registry holds the conceptual capability, not its path
    /// rendering.
    pub topic: String,
    /// Wall-clock issuance time in milliseconds since UNIX epoch.
    pub issued_at_ms: u64,
    /// Whether the topic is matched as exact-only or as a path
    /// prefix.
    pub scope: GrantScope,
}

/// A registered node: node-id, public key, optional friendly label,
/// and when it was added to the registry.
#[derive(Debug, Clone)]
pub struct RegisteredNode {
    /// Stable identifier derived from the pubkey. Hex-encoded prefix
    /// of SHA-256(pubkey). See [`node_id_from_pubkey`].
    pub node_id: String,
    /// Ed25519 public key (32 bytes).
    pub pubkey: [u8; 32],
    /// Optional human-readable label, e.g. `"esp32-workbench"` or
    /// `"daemon-wsl"`. Authoritative label lives under
    /// `substrate/<node-id>/meta/label`; this field is a convenience
    /// copy so code paths that don't touch substrate can still render
    /// a friendly name.
    pub label: Option<String>,
    /// When the node was registered with the kernel.
    pub registered_at: DateTime<Utc>,
}

/// In-memory node-id → public-key map.
///
/// Cheap to clone — the inner map is wrapped in `Arc`/`DashMap`.
///
/// Also holds the [`DerivedWriteGrant`] table — the mesh-canonical
/// write-permissions seam. Both tables share the registry's clone
/// semantics so a service handed a `NodeRegistry` clone sees grant
/// updates the daemon issues mid-boot.
#[derive(Debug, Default, Clone)]
pub struct NodeRegistry {
    inner: Arc<DashMap<String, RegisteredNode>>,
    /// `(grantee_node_id, topic)` → grant. Per-pair so the same node
    /// holding two grants (e.g. `transcript` + `classify`) gets two
    /// distinct rows; that matches the R3.6 mandate that grants are
    /// path-bounded, not blanket.
    grants: Arc<DashMap<(String, String), DerivedWriteGrant>>,
}

/// Failure modes for [`NodeRegistry::issue_derived_grant`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DerivedGrantError {
    /// The topic is empty or contains a `/` — topics name a single
    /// segment immediately below `_derived/`. Multi-segment grants
    /// must use [`GrantScope::TopicPrefix`] over the *first* segment
    /// and rely on path matching for finer granularity.
    InvalidTopic {
        /// The offending topic string.
        topic: String,
    },
}

impl std::fmt::Display for DerivedGrantError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DerivedGrantError::InvalidTopic { topic } => write!(
                f,
                "invalid derived-write topic {topic:?}: must be one non-empty path segment"
            ),
        }
    }
}

impl std::error::Error for DerivedGrantError {}

impl NodeRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a node. The node-id is derived deterministically from
    /// the pubkey — re-registering the same key updates the label but
    /// keeps the id stable. Returns the (possibly-updated) entry.
    pub fn register(&self, pubkey: [u8; 32], label: Option<String>) -> RegisteredNode {
        let node_id = node_id_from_pubkey(&pubkey);
        let entry = RegisteredNode {
            node_id: node_id.clone(),
            pubkey,
            label,
            registered_at: Utc::now(),
        };
        self.inner.insert(node_id, entry.clone());
        entry
    }

    /// Look up a node by its id.
    pub fn get(&self, node_id: &str) -> Option<RegisteredNode> {
        self.inner.get(node_id).map(|e| e.clone())
    }

    /// Whether `node_id` is known.
    pub fn contains(&self, node_id: &str) -> bool {
        self.inner.contains_key(node_id)
    }

    /// Number of registered nodes.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// List all registered nodes.
    pub fn list(&self) -> Vec<RegisteredNode> {
        self.inner.iter().map(|e| e.value().clone()).collect()
    }

    /// Issue a [`DerivedWriteGrant`] permitting `grantee_node_id` to
    /// publish under the mesh-canonical topic.
    ///
    /// **MVP authority model.** Per R3.6 the daemon issues to itself
    /// in-process; there's no signature check at this seam. Future
    /// federated grants will add an "issuer signature" arm that
    /// `clawft-weave`'s RPC boundary will check before this call.
    /// Today, callers are trusted because they had to be inside the
    /// daemon process to reach the registry.
    ///
    /// Idempotent: re-issuing a `(grantee, topic)` pair overwrites
    /// `issued_at_ms` and `scope`. Returns the freshly-stored grant.
    pub fn issue_derived_grant(
        &self,
        grantee_node_id: impl Into<String>,
        topic: impl Into<String>,
        scope: GrantScope,
    ) -> Result<DerivedWriteGrant, DerivedGrantError> {
        let grantee_node_id = grantee_node_id.into();
        let topic = topic.into();
        if topic.is_empty() || topic.contains('/') {
            return Err(DerivedGrantError::InvalidTopic { topic });
        }
        let issued_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let grant = DerivedWriteGrant {
            grantee_node_id: grantee_node_id.clone(),
            topic: topic.clone(),
            issued_at_ms,
            scope,
        };
        self.grants.insert((grantee_node_id, topic), grant.clone());
        Ok(grant)
    }

    /// Whether `grantee_node_id` holds a grant covering `path`.
    ///
    /// `path` must look like `substrate/_derived/<topic>/...` (or
    /// `substrate/_derived/<topic>` for [`GrantScope::ExactTopic`]).
    /// Anything else returns `false`. The check is the second half
    /// of the mesh-canonical write gate — the first half is the
    /// `substrate/_derived/` tier detection that lives on
    /// [`crate::SubstrateService::publish_gated_with_grants`].
    pub fn has_derived_grant(&self, grantee_node_id: &str, path: &str) -> bool {
        // Strip the mesh-canonical tier prefix; if the path doesn't
        // belong to the tier, no grant matches.
        let Some(tail) = path.strip_prefix(MESH_CANONICAL_PREFIX) else {
            return false;
        };
        if tail.is_empty() {
            return false;
        }
        // First segment of the tail is the topic; the rest (if any)
        // is the grantee-controlled subtree.
        let (topic, rest) = match tail.split_once('/') {
            Some((t, r)) => (t, Some(r)),
            None => (tail, None),
        };
        if topic.is_empty() {
            return false;
        }
        let Some(entry) = self
            .grants
            .get(&(grantee_node_id.to_string(), topic.to_string()))
        else {
            return false;
        };
        match entry.scope {
            // Exact: the path must be the bare topic, no subtree.
            GrantScope::ExactTopic => rest.is_none(),
            // Prefix: the path must be the topic followed by at least
            // one non-empty subtree segment. A bare topic write
            // `substrate/_derived/transcript` is rejected — that path
            // belongs to the topic root, not to any pipeline-owned
            // attribution subtree, and would collide if two
            // pipelines ever shared a topic.
            GrantScope::TopicPrefix => rest.is_some_and(|r| !r.is_empty()),
        }
    }

    /// Snapshot of all currently-issued grants. Test helper / future
    /// admin-RPC seam; not intended for hot-path use.
    pub fn list_derived_grants(&self) -> Vec<DerivedWriteGrant> {
        self.grants.iter().map(|e| e.value().clone()).collect()
    }
}

/// Required path prefix for a mesh-canonical write. Trailing slash
/// included so a `strip_prefix` cleanly leaves the remaining `<topic>`
/// segment without a leading separator. See R3.0.
pub const MESH_CANONICAL_PREFIX: &str = "substrate/_derived/";

/// Derive a node-id from an Ed25519 public key.
///
/// Layout: `"n-"` + the first 6 hex chars (3 bytes) of
/// `BLAKE3(pubkey)`. Total length 8 chars. Examples:
/// `n-3a7f9c`, `n-001abc`. The `n-` prefix is load-bearing — it
/// keeps node-ids in their own namespace so a substrate path
/// segment can never collide with a reserved word like `_derived`
/// or `meta`. 24 bits of collision resistance is intentionally
/// modest at MVP; longer hex (and the same `n-` prefix) is a
/// format-compatible upgrade once mesh size demands it.
///
/// Format committed in `.planning/sensors/JOURNALED-NODE-ESP32.md`
/// §2.2.
pub fn node_id_from_pubkey(pubkey: &[u8; 32]) -> String {
    let h = blake3::hash(pubkey);
    let bytes = h.as_bytes();
    // 3 bytes -> 6 hex chars.
    format!(
        "n-{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2]
    )
}

/// Compose the canonical byte payload a node must sign as the
/// proof-of-possession for `node.register`.
///
/// Layout:
/// `b"node.register\0" || pubkey || b"\0" || ts_le || b"\0" || label`.
///
/// The label is included in the payload so a registration cannot be
/// silently relabelled by anyone other than the keyholder. Empty
/// label is accepted (and signs as a zero-length suffix).
pub fn node_register_payload(pubkey: &[u8; 32], ts: u64, label: &str) -> Vec<u8> {
    let mut buf = Vec::with_capacity(14 + 32 + 1 + 8 + 1 + label.len());
    buf.extend_from_slice(b"node.register\0");
    buf.extend_from_slice(pubkey);
    buf.push(0);
    buf.extend_from_slice(&ts.to_le_bytes());
    buf.push(0);
    buf.extend_from_slice(label.as_bytes());
    buf
}

/// Compose the canonical byte payload a node must sign for a
/// `substrate.publish`.
///
/// Layout:
/// `b"substrate.publish.node\0" || path || b"\0" || message || b"\0" || ts_le || b"\0" || node_id`.
///
/// Separate from [`crate::agent_registry::publish_payload`] so an
/// actor signature over an ipc.publish payload cannot be replayed as
/// a node signature over a substrate.publish — the domain-separation
/// prefix differs.
pub fn node_publish_payload(path: &str, message: &str, ts: u64, node_id: &str) -> Vec<u8> {
    let mut buf = Vec::with_capacity(
        22 + path.len() + 1 + message.len() + 1 + 8 + 1 + node_id.len(),
    );
    buf.extend_from_slice(b"substrate.publish.node\0");
    buf.extend_from_slice(path.as_bytes());
    buf.push(0);
    buf.extend_from_slice(message.as_bytes());
    buf.push(0);
    buf.extend_from_slice(&ts.to_le_bytes());
    buf.push(0);
    buf.extend_from_slice(node_id.as_bytes());
    buf
}

/// Required path prefix for any substrate write originated by `node_id`.
///
/// Returns `substrate/<node-id>/` (with the trailing slash so a
/// `starts_with` check correctly rejects e.g.
/// `substrate/<node-id-prefix-collision>/…`).
pub fn required_path_prefix(node_id: &str) -> String {
    format!("substrate/{node_id}/")
}

/// Whether `path` is a legal write target for `node_id`. Convenience
/// wrapper over [`required_path_prefix`].
pub fn path_belongs_to(path: &str, node_id: &str) -> bool {
    path.starts_with(&required_path_prefix(node_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_id_is_stable_per_pubkey() {
        let pk = [7u8; 32];
        let a = node_id_from_pubkey(&pk);
        let b = node_id_from_pubkey(&pk);
        assert_eq!(a, b);
        assert_eq!(a.len(), 8, "n- + 6 hex chars");
        assert!(a.starts_with("n-"));
        assert!(a[2..].chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn node_id_differs_per_pubkey() {
        let a = node_id_from_pubkey(&[1u8; 32]);
        let b = node_id_from_pubkey(&[2u8; 32]);
        assert_ne!(a, b);
    }

    #[test]
    fn node_id_format_starts_with_namespace_prefix() {
        // The "n-" prefix is load-bearing: it keeps node-ids
        // disjoint from reserved path segments like "_derived",
        // "meta", "sensor", etc. Without it, a hex string starting
        // with valid characters could in principle collide.
        let id = node_id_from_pubkey(&[42u8; 32]);
        assert!(id.starts_with("n-"));
    }

    #[test]
    fn register_and_lookup_round_trips() {
        let reg = NodeRegistry::new();
        let entry = reg.register([7u8; 32], Some("esp32-workbench".into()));
        assert_eq!(reg.len(), 1);
        let fetched = reg.get(&entry.node_id).unwrap();
        assert_eq!(fetched.pubkey, [7u8; 32]);
        assert_eq!(fetched.label.as_deref(), Some("esp32-workbench"));
    }

    #[test]
    fn reregister_same_key_updates_label_keeps_id() {
        let reg = NodeRegistry::new();
        let a = reg.register([9u8; 32], Some("old".into()));
        let b = reg.register([9u8; 32], Some("new".into()));
        assert_eq!(a.node_id, b.node_id);
        assert_eq!(reg.len(), 1);
        assert_eq!(reg.get(&a.node_id).unwrap().label.as_deref(), Some("new"));
    }

    #[test]
    fn contains_reports_membership() {
        let reg = NodeRegistry::new();
        let e = reg.register([3u8; 32], None);
        assert!(reg.contains(&e.node_id));
        assert!(!reg.contains("deadbeef"));
    }

    #[test]
    fn list_returns_every_entry() {
        let reg = NodeRegistry::new();
        reg.register([1u8; 32], None);
        reg.register([2u8; 32], None);
        reg.register([3u8; 32], None);
        let all = reg.list();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn publish_payload_is_deterministic_per_inputs() {
        let p1 = node_publish_payload("substrate/n1/sensor/mic", "{}", 42, "n1");
        let p2 = node_publish_payload("substrate/n1/sensor/mic", "{}", 42, "n1");
        assert_eq!(p1, p2);
    }

    #[test]
    fn publish_payload_changes_with_ts() {
        let p1 = node_publish_payload("substrate/n1/x", "v", 1, "n1");
        let p2 = node_publish_payload("substrate/n1/x", "v", 2, "n1");
        assert_ne!(p1, p2);
    }

    #[test]
    fn publish_payload_domain_separated_from_actor() {
        // The actor-level `publish_payload` uses `b"ipc.publish\0"`
        // as its prefix. Ours uses `b"substrate.publish.node\0"`.
        // The two byte streams must not collide on any input.
        let node_p = node_publish_payload("t", "v", 1, "n1");
        let actor_p = crate::agent_registry::publish_payload("t", "v", 1, "n1");
        assert_ne!(node_p, actor_p);
    }

    #[test]
    fn required_prefix_includes_trailing_slash() {
        assert_eq!(required_path_prefix("n1"), "substrate/n1/");
    }

    #[test]
    fn path_belongs_to_accepts_exact_subtree() {
        assert!(path_belongs_to("substrate/n1/sensor/mic", "n1"));
        assert!(path_belongs_to("substrate/n1/meta/label", "n1"));
    }

    #[test]
    fn path_belongs_to_rejects_cross_node() {
        assert!(!path_belongs_to("substrate/n2/sensor/mic", "n1"));
    }

    #[test]
    fn path_belongs_to_rejects_prefix_collision() {
        // A node named `n1` must not be able to write under `n11`'s
        // namespace just because `starts_with("substrate/n1")` is true.
        // The trailing-slash guard blocks this.
        assert!(!path_belongs_to("substrate/n11/anything", "n1"));
    }

    #[test]
    fn path_belongs_to_rejects_top_level() {
        assert!(!path_belongs_to("substrate/sensor/mic", "n1"));
        assert!(!path_belongs_to("", "n1"));
        assert!(!path_belongs_to("something-else", "n1"));
    }

    // ── DerivedWriteGrant ──────────────────────────────────────────

    #[test]
    fn issue_grant_round_trips() {
        let reg = NodeRegistry::new();
        let g = reg
            .issue_derived_grant("n-daemon", "transcript", GrantScope::TopicPrefix)
            .expect("valid grant accepted");
        assert_eq!(g.grantee_node_id, "n-daemon");
        assert_eq!(g.topic, "transcript");
        assert_eq!(g.scope, GrantScope::TopicPrefix);
        // Issuance time is recorded; we can't pin the exact value but
        // it must be non-zero on any sane host clock.
        assert!(g.issued_at_ms > 0);
        assert_eq!(reg.list_derived_grants().len(), 1);
    }

    #[test]
    fn issue_grant_rejects_invalid_topic() {
        let reg = NodeRegistry::new();
        assert!(matches!(
            reg.issue_derived_grant("n-d", "", GrantScope::ExactTopic),
            Err(DerivedGrantError::InvalidTopic { .. })
        ));
        assert!(matches!(
            reg.issue_derived_grant("n-d", "transcript/mic", GrantScope::TopicPrefix),
            Err(DerivedGrantError::InvalidTopic { .. })
        ));
    }

    #[test]
    fn has_grant_topic_prefix_matches_subtree() {
        let reg = NodeRegistry::new();
        reg.issue_derived_grant("n-daemon", "transcript", GrantScope::TopicPrefix)
            .unwrap();
        assert!(reg.has_derived_grant(
            "n-daemon",
            "substrate/_derived/transcript/n-foo/mic"
        ));
        assert!(reg.has_derived_grant(
            "n-daemon",
            "substrate/_derived/transcript/n-bar/cam"
        ));
        // Bare-topic write under a TopicPrefix grant is rejected;
        // pipelines own attribution subtrees, not the topic root.
        assert!(!reg.has_derived_grant("n-daemon", "substrate/_derived/transcript"));
    }

    #[test]
    fn has_grant_exact_topic_rejects_subtree() {
        let reg = NodeRegistry::new();
        reg.issue_derived_grant("n-leader", "chain", GrantScope::ExactTopic)
            .unwrap();
        // ExactTopic covers exactly `substrate/_derived/chain`, not
        // its subtree. (Real chain head is a single-leaf canonical
        // value — see R3.1.)
        assert!(reg.has_derived_grant("n-leader", "substrate/_derived/chain"));
        assert!(!reg.has_derived_grant("n-leader", "substrate/_derived/chain/head"));
    }

    #[test]
    fn has_grant_returns_false_when_grant_absent() {
        let reg = NodeRegistry::new();
        // No grants issued.
        assert!(!reg.has_derived_grant(
            "n-daemon",
            "substrate/_derived/transcript/n-foo/mic"
        ));
    }

    #[test]
    fn has_grant_rejects_other_grantee() {
        let reg = NodeRegistry::new();
        reg.issue_derived_grant("n-daemon", "transcript", GrantScope::TopicPrefix)
            .unwrap();
        assert!(!reg.has_derived_grant(
            "n-other",
            "substrate/_derived/transcript/n-foo/mic"
        ));
    }

    #[test]
    fn has_grant_rejects_unrelated_topic() {
        let reg = NodeRegistry::new();
        reg.issue_derived_grant("n-daemon", "transcript", GrantScope::TopicPrefix)
            .unwrap();
        // Same grantee, different topic.
        assert!(!reg.has_derived_grant(
            "n-daemon",
            "substrate/_derived/classify/n-foo/mic"
        ));
    }

    #[test]
    fn has_grant_rejects_non_mesh_canonical_path() {
        let reg = NodeRegistry::new();
        reg.issue_derived_grant("n-daemon", "transcript", GrantScope::TopicPrefix)
            .unwrap();
        // Even with a grant, paths that don't sit under
        // `substrate/_derived/` belong to the node-private tier and
        // must not be matched by this check.
        assert!(!reg.has_derived_grant("n-daemon", "substrate/n-daemon/foo"));
        assert!(!reg.has_derived_grant("n-daemon", "substrate/_derived/"));
        assert!(!reg.has_derived_grant("n-daemon", ""));
    }

    #[test]
    fn issue_grant_is_idempotent_per_pair() {
        let reg = NodeRegistry::new();
        let _ = reg
            .issue_derived_grant("n-daemon", "transcript", GrantScope::TopicPrefix)
            .unwrap();
        // Second issue of same (grantee, topic) overwrites in place,
        // doesn't grow the table.
        let _ = reg
            .issue_derived_grant("n-daemon", "transcript", GrantScope::ExactTopic)
            .unwrap();
        assert_eq!(reg.list_derived_grants().len(), 1);
        // After the overwrite the path with subtree no longer matches
        // (scope flipped to ExactTopic).
        assert!(!reg.has_derived_grant(
            "n-daemon",
            "substrate/_derived/transcript/n-foo/mic"
        ));
    }
}
