//! ADR-057: substrate per-path read ACLs.
//!
//! Complements the weave capability-class gate (`Capability::Read`).
//! Every `substrate.read` / `.list` / `.subscribe` MUST also satisfy a
//! path ACL. Default posture: **deny** for private subtrees
//! (`sensor/**`, `health/sensor/**`, `actor/<id>/**`); **public** for
//! cluster/health/meta/chain.
//!
//! # Storage shape (logical)
//!
//! ```text
//! substrate/<mesh-id>/acl/<path-glob>
//!   { allow, deny, inherit, sealed_by, sealed_at }
//! ```
//!
//! This module holds the in-memory table + path-trie lookup. Persistence
//! of the ACL subtree into the live substrate state tree is the caller's
//! responsibility (daemon publish under the mesh bootstrap identity).
//!
//! # Lookup complexity
//!
//! Rules are stored in a segment trie with `*` (one segment) and `**`
//! (rest-of-path) wildcards. Evaluation walks the path once and
//! collects matching rules ordered most-specific → least-specific, so
//! cost is O(depth × branching), not O(rules).

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// ExoChain event kind for authenticated read denials (ADR-022 + ADR-057).
pub const READ_DENIED_EVENT: &str = "substrate.read.denied";

/// Literal identity string forms accepted in allow/deny lists.
pub const ID_PUBLIC: &str = "public";
/// Prefix for node identities: `node:<32-hex node id>`.
pub const ID_NODE_PREFIX: &str = "node:";
/// Prefix for actor identities: `actor:a-<hex>`.
pub const ID_ACTOR_PREFIX: &str = "actor:";
/// Prefix for capability-token scopes: `scope:<name>`.
pub const ID_SCOPE_PREFIX: &str = "scope:";

/// True for a bare node id: 32 lowercase hex chars (ADR-103 D11).
fn is_node_id_shape(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

// ── Caller identity ─────────────────────────────────────────────────

/// Resolved caller for an ACL check.
///
/// Built from the JSON-RPC `auth` / `actor_id` field. Anonymous local
/// UDS callers are [`CallerIdentity::anonymous`]; they only match paths
/// whose effective rule allows `public`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CallerIdentity {
    /// Original opaque string from the RPC envelope, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
    /// Node id without the `node:` prefix (e.g. `n-046780`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    /// Actor id without the `actor:` prefix (e.g. `a-deadbe`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_id: Option<String>,
    /// Capability scopes held by this caller (e.g. `admin`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<String>,
}

impl CallerIdentity {
    /// Anonymous / unauthenticated caller.
    pub fn anonymous() -> Self {
        Self::default()
    }

    /// Whether this is an anonymous caller (no node, actor, or scope).
    pub fn is_anonymous(&self) -> bool {
        self.node_id.is_none() && self.actor_id.is_none() && self.scopes.is_empty() && self.raw.is_none()
    }

    /// Whether the caller is authenticated enough to warrant a chain-log
    /// on denial (ADR-057 §What the read gate checks step 5).
    pub fn is_authenticated(&self) -> bool {
        !self.is_anonymous()
    }

    /// Construct a node identity (`node:n-…`).
    pub fn node(id: impl Into<String>) -> Self {
        let id = strip_prefix_owned(id.into(), ID_NODE_PREFIX);
        Self {
            raw: Some(format!("{ID_NODE_PREFIX}{id}")),
            node_id: Some(id),
            ..Self::default()
        }
    }

    /// Construct an actor identity (`actor:a-…`).
    pub fn actor(id: impl Into<String>) -> Self {
        let id = strip_prefix_owned(id.into(), ID_ACTOR_PREFIX);
        Self {
            raw: Some(format!("{ID_ACTOR_PREFIX}{id}")),
            actor_id: Some(id),
            ..Self::default()
        }
    }

    /// Add a capability scope (e.g. `"admin"`).
    pub fn with_scope(mut self, scope: impl Into<String>) -> Self {
        let s = strip_prefix_owned(scope.into(), ID_SCOPE_PREFIX);
        if !self.scopes.iter().any(|x| x == &s) {
            self.scopes.push(s);
        }
        self
    }

    /// Parse a free-form RPC identity string.
    ///
    /// Accepts:
    /// - `None` / empty → anonymous
    /// - `public` → anonymous (explicit)
    /// - `node:n-…` or bare `n-…` → node
    /// - `actor:a-…` or bare `a-…` → actor
    /// - `scope:admin` or bare `admin` → scope only
    /// - anything else → opaque raw + treated as actor-id fallback
    pub fn parse(s: Option<&str>) -> Self {
        let Some(raw) = s.map(str::trim).filter(|s| !s.is_empty()) else {
            return Self::anonymous();
        };
        if raw == ID_PUBLIC {
            return Self::anonymous();
        }
        if let Some(rest) = raw.strip_prefix(ID_NODE_PREFIX) {
            return Self::node(rest);
        }
        if let Some(rest) = raw.strip_prefix(ID_ACTOR_PREFIX) {
            return Self::actor(rest);
        }
        if let Some(rest) = raw.strip_prefix(ID_SCOPE_PREFIX) {
            return Self {
                raw: Some(raw.to_string()),
                scopes: vec![rest.to_string()],
                ..Self::default()
            };
        }
        if raw == "admin" {
            return Self {
                raw: Some(raw.to_string()),
                scopes: vec!["admin".into()],
                ..Self::default()
            };
        }
        // Bare node / actor ids follow the JOURNALED-NODE convention.
        // Node ids are `hex(SHA-256(pubkey)[..16])` (ADR-103 D11); the
        // legacy `n-<hex>` form is still recognised for stored rules.
        if raw.starts_with("n-") || is_node_id_shape(raw) {
            return Self::node(raw);
        }
        if raw.starts_with("a-") {
            return Self::actor(raw);
        }
        // Opaque agent / session id — keep as raw so exact-string allow
        // rules still match, and treat as a soft actor-id for owner checks.
        Self {
            raw: Some(raw.to_string()),
            actor_id: Some(raw.to_string()),
            ..Self::default()
        }
    }

    /// Display form used in errors and chain payloads.
    pub fn display(&self) -> String {
        if let Some(ref n) = self.node_id {
            return format!("{ID_NODE_PREFIX}{n}");
        }
        if let Some(ref a) = self.actor_id {
            return format!("{ID_ACTOR_PREFIX}{a}");
        }
        if !self.scopes.is_empty() {
            return format!("{ID_SCOPE_PREFIX}{}", self.scopes.join(","));
        }
        if let Some(ref r) = self.raw {
            return r.clone();
        }
        "anonymous".into()
    }

    fn matches_identity_token(&self, token: &str) -> bool {
        if token == ID_PUBLIC {
            // `public` matches everyone, including anonymous.
            return true;
        }
        if let Some(rest) = token.strip_prefix(ID_NODE_PREFIX) {
            return self.node_id.as_deref() == Some(rest);
        }
        if let Some(rest) = token.strip_prefix(ID_ACTOR_PREFIX) {
            return self.actor_id.as_deref() == Some(rest);
        }
        if let Some(rest) = token.strip_prefix(ID_SCOPE_PREFIX) {
            return self.scopes.iter().any(|s| s == rest);
        }
        // Exact raw / bare match.
        self.raw.as_deref() == Some(token)
            || self.node_id.as_deref() == Some(token)
            || self.actor_id.as_deref() == Some(token)
    }
}

fn strip_prefix_owned(s: String, prefix: &str) -> String {
    s.strip_prefix(prefix).unwrap_or(&s).to_string()
}

// ── Rule + denial ───────────────────────────────────────────────────

/// One ACL entry for a path glob (ADR-057 data model).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AclRule {
    /// Identity strings that may read this path.
    #[serde(default)]
    pub allow: Vec<String>,
    /// Identity strings that are always rejected (wins over allow).
    #[serde(default)]
    pub deny: Vec<String>,
    /// When `true`, children without their own rule inherit this rule.
    /// When `false`, children fall through to less-specific rules or
    /// structural defaults.
    #[serde(default = "default_inherit_true")]
    pub inherit: bool,
    /// Who last wrote this rule (mesh bootstrap actor id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sealed_by: Option<String>,
    /// Unix millis when the rule was sealed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sealed_at: Option<u64>,
}

fn default_inherit_true() -> bool {
    true
}

impl AclRule {
    /// Public-readable rule.
    pub fn public() -> Self {
        Self {
            allow: vec![ID_PUBLIC.into()],
            deny: Vec::new(),
            inherit: true,
            sealed_by: None,
            sealed_at: None,
        }
    }

    /// Allow only the listed identities (no public).
    pub fn allow_only(idents: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            allow: idents.into_iter().map(Into::into).collect(),
            deny: Vec::new(),
            inherit: true,
            sealed_by: None,
            sealed_at: None,
        }
    }

    /// Builder: set inherit flag.
    pub fn with_inherit(mut self, inherit: bool) -> Self {
        self.inherit = inherit;
        self
    }

    /// Builder: seal metadata.
    pub fn sealed(mut self, by: impl Into<String>, at: u64) -> Self {
        self.sealed_by = Some(by.into());
        self.sealed_at = Some(at);
        self
    }

    fn deny_matches(&self, caller: &CallerIdentity) -> bool {
        self.deny
            .iter()
            .any(|t| caller.matches_identity_token(t))
    }

    fn allow_matches(&self, caller: &CallerIdentity) -> bool {
        self.allow
            .iter()
            .any(|t| caller.matches_identity_token(t))
    }
}

/// Typed denial distinct from path-not-found (ADR-057 step 4).
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
pub struct AclDenied {
    /// Requested substrate path.
    pub path: String,
    /// Display form of the caller.
    pub caller: String,
    /// Operation (`read` / `list` / `subscribe`).
    pub op: String,
    /// Short machine-readable reason.
    pub reason: String,
}

impl AclDenied {
    /// Wire-error prefix so clients can distinguish `acl_denied` from
    /// other unauthorized / not-found failures.
    pub const CODE: &'static str = "acl_denied";

    /// Format as a daemon error string: `acl_denied: …`.
    pub fn wire_message(&self) -> String {
        format!(
            "{}: {} on {} for {} ({})",
            Self::CODE,
            self.op,
            self.path,
            self.caller,
            self.reason
        )
    }

    /// Payload for a `substrate.read.denied` ExoChain event.
    pub fn chain_payload(&self) -> serde_json::Value {
        serde_json::json!({
            "path": self.path,
            "caller": self.caller,
            "op": self.op,
            "reason": self.reason,
            "code": Self::CODE,
        })
    }
}

impl fmt::Display for AclDenied {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.wire_message())
    }
}

// ── Path trie ───────────────────────────────────────────────────────

#[derive(Debug, Default)]
struct TrieNode {
    /// Exact-segment children.
    children: BTreeMap<String, TrieNode>,
    /// Single-segment wildcard child (`*`).
    star: Option<Box<TrieNode>>,
    /// Rest-of-path wildcard (`**`) — terminal; rule lives here.
    globstar: Option<Box<TrieNode>>,
    /// Rule terminating at this node (exact path / end of pattern).
    rule: Option<AclRule>,
    /// Original glob string (for diagnostics / specificity).
    glob: Option<String>,
}

/// Per-path ACL table with path-trie lookup (ADR-057).
#[derive(Debug, Default)]
pub struct AclTable {
    root: TrieNode,
    /// Number of rules currently sealed in the table.
    rule_count: usize,
    /// Mesh id used when seeding boot defaults (if any).
    mesh_id: Option<String>,
}

impl AclTable {
    /// Empty table — structural defaults still apply on check.
    pub fn new() -> Self {
        Self::default()
    }

    /// Seed the boot-time defaults for a mesh (ADR-057 §Default ACL).
    ///
    /// Concrete mesh-scoped public rules are inserted; node/actor private
    /// subtrees are enforced via structural defaults so every node that
    /// later joins inherits deny-by-default without re-seeding.
    pub fn with_boot_defaults(mesh_id: impl Into<String>) -> Self {
        let mesh_id = mesh_id.into();
        let mut t = Self::new();
        t.mesh_id = Some(mesh_id.clone());
        let sealed_by = "boot";
        let sealed_at = 0u64;

        let public = AclRule::public().sealed(sealed_by, sealed_at);

        // Mesh-scoped public surfaces.
        t.insert(
            format!("substrate/{mesh_id}/cluster/nodes/**"),
            public.clone(),
        );
        t.insert(
            format!("substrate/{mesh_id}/cluster/health/**"),
            public.clone(),
        );
        t.insert(format!("substrate/{mesh_id}/chain/**"), public.clone());

        // Template patterns for every node-id / actor-id under substrate/.
        // `*` captures the owner segment; structural defaults fill in
        // owner-specific allow lists when no explicit rule matches.
        t.insert("substrate/*/meta", public.clone());
        t.insert("substrate/*/health", public);

        // Private sensor subtrees — owner node + scope:admin only.
        // Owner is expanded at check time via structural defaults; the
        // explicit rule marks the subtree as non-public (allow list
        // without `public`) and inherit=true so children share it.
        t.insert(
            "substrate/*/sensor/**",
            AclRule::allow_only(["scope:admin"]).sealed(sealed_by, sealed_at),
        );
        t.insert(
            "substrate/*/health/sensor/**",
            AclRule::allow_only(["scope:admin"]).sealed(sealed_by, sealed_at),
        );
        // Actor-private state.
        t.insert(
            "substrate/a-*/**",
            AclRule::allow_only(["scope:admin"]).sealed(sealed_by, sealed_at),
        );

        t
    }

    /// Number of explicit rules.
    pub fn len(&self) -> usize {
        self.rule_count
    }

    /// Whether the table has no explicit rules.
    pub fn is_empty(&self) -> bool {
        self.rule_count == 0
    }

    /// Mesh id used for boot defaults, if seeded.
    pub fn mesh_id(&self) -> Option<&str> {
        self.mesh_id.as_deref()
    }

    /// Insert or replace a rule for a path glob.
    ///
    /// Globs support trailing `**` (subtree) and single-segment `*`.
    /// No regex.
    pub fn insert(&mut self, glob: impl Into<String>, rule: AclRule) {
        let glob = glob.into();
        let segs = split_path(&glob);
        let mut node = &mut self.root;
        for seg in &segs {
            if *seg == "**" {
                if node.globstar.is_none() {
                    node.globstar = Some(Box::new(TrieNode::default()));
                }
                node = node.globstar.as_mut().unwrap();
                // `**` consumes the rest; stop.
                break;
            } else if *seg == "*" {
                if node.star.is_none() {
                    node.star = Some(Box::new(TrieNode::default()));
                }
                node = node.star.as_mut().unwrap();
            } else {
                node = node
                    .children
                    .entry((*seg).to_string())
                    .or_default();
            }
        }
        let had = node.rule.is_some();
        node.rule = Some(rule);
        node.glob = Some(glob);
        if !had {
            self.rule_count += 1;
        }
    }

    /// Ensure an exact path is publicly readable (ADR-057
    /// `publish_public` helper — write side also publishes the value;
    /// this only seals the allow rule).
    pub fn publish_public(&mut self, path: &str) {
        let path = normalize_path(path);
        self.insert(
            path,
            AclRule {
                allow: vec![ID_PUBLIC.into()],
                deny: Vec::new(),
                inherit: false,
                sealed_by: Some("publish_public".into()),
                sealed_at: None,
            },
        );
    }

    /// Logical storage key for a rule under the mesh ACL subtree.
    pub fn storage_path(mesh_id: &str, glob: &str) -> String {
        // Encode the glob so `/` doesn't create nested keys beyond the
        // acl/ root: replace `/` with `~` for the final segment.
        let encoded = glob.replace('/', "~");
        format!("substrate/{mesh_id}/acl/{encoded}")
    }

    /// Check whether `caller` may perform `op` on `path`.
    ///
    /// Order (ADR-057):
    /// 1. Collect matching rules, most-specific first.
    /// 2. Deny if any deny token matches.
    /// 3. Allow if any allow token matches.
    /// 4. If rule has `inherit: false` and no match → structural default.
    /// 5. If rule has `inherit: true` and no match → try next rule.
    /// 6. No rules → structural default.
    pub fn check(
        &self,
        path: &str,
        caller: &CallerIdentity,
        op: &str,
    ) -> Result<(), AclDenied> {
        let path = normalize_path(path);
        let segs = split_path(&path);
        let mut matches: Vec<MatchRef<'_>> = Vec::new();
        collect_matches(&self.root, &segs, 0, 0, &mut matches);
        // Most specific first: higher score first; stable by insertion order.
        matches.sort_by(|a, b| b.score.cmp(&a.score));

        for m in &matches {
            let rule = m.rule;
            if rule.deny_matches(caller) {
                return Err(deny(&path, caller, op, "explicit deny rule"));
            }
            if rule.allow_matches(caller) {
                // Owner-augmented allow: rules that only list scope:admin
                // still need the structural owner check when the path is
                // private — handled below only when allow doesn't match.
                // If allow matched (incl. public or scope:admin), pass.
                return Ok(());
            }
            // Expand owner identity against structural private paths even
            // when the table rule only listed scope:admin — the owner
            // node/actor is always implicitly allowed on its private
            // subtree (ADR-057 default table).
            if structural_owner_allows(&path, caller) {
                return Ok(());
            }
            if !rule.inherit {
                // Child must declare its own rule or fall to default.
                break;
            }
            // inherit: try next less-specific rule.
        }

        if structural_allows(&path, caller) {
            return Ok(());
        }

        Err(deny(
            &path,
            caller,
            op,
            structural_reason(&path),
        ))
    }

    /// Convenience: check with an optional raw identity string.
    pub fn check_raw(
        &self,
        path: &str,
        caller: Option<&str>,
        op: &str,
    ) -> Result<(), AclDenied> {
        self.check(path, &CallerIdentity::parse(caller), op)
    }
}

struct MatchRef<'a> {
    rule: &'a AclRule,
    score: u32,
}

fn collect_matches<'a>(
    node: &'a TrieNode,
    segs: &[&str],
    idx: usize,
    depth_score: u32,
    out: &mut Vec<MatchRef<'a>>,
) {
    if idx == segs.len() {
        if let Some(ref rule) = node.rule {
            out.push(MatchRef {
                rule,
                score: depth_score.saturating_mul(10) + 5, // exact end
            });
        }
        return;
    }

    // `**` at this node matches the rest of the path.
    if let Some(ref gs) = node.globstar
        && let Some(ref rule) = gs.rule
    {
        out.push(MatchRef {
            rule,
            // Prefer longer prefix before **.
            score: depth_score.saturating_mul(10) + 1,
        });
    }

    let seg = segs[idx];

    // Exact child.
    if let Some(child) = node.children.get(seg) {
        collect_matches(child, segs, idx + 1, depth_score + 3, out);
    }

    // Single-segment wildcard.
    if let Some(ref star) = node.star {
        collect_matches(star, segs, idx + 1, depth_score + 1, out);
    }
}

fn deny(path: &str, caller: &CallerIdentity, op: &str, reason: &str) -> AclDenied {
    AclDenied {
        path: path.to_string(),
        caller: caller.display(),
        op: op.to_string(),
        reason: reason.to_string(),
    }
}

// ── Structural defaults ─────────────────────────────────────────────

/// Owner of a private path may always read it.
fn structural_owner_allows(path: &str, caller: &CallerIdentity) -> bool {
    let segs: Vec<&str> = split_path(path);
    if segs.len() < 2 || segs[0] != "substrate" {
        return false;
    }
    let owner = segs[1];
    let rest = &segs[2..];

    if is_sensor_private(rest) {
        return caller.node_id.as_deref() == Some(owner);
    }
    if owner.starts_with("a-") && !rest.is_empty() {
        return caller.actor_id.as_deref() == Some(owner);
    }
    false
}

fn is_sensor_private(rest: &[&str]) -> bool {
    matches!(rest.first(), Some(&"sensor"))
        || (rest.len() >= 2 && rest[0] == "health" && rest[1] == "sensor")
}

fn structural_allows(path: &str, caller: &CallerIdentity) -> bool {
    // scope:admin always passes structural defaults.
    if caller.scopes.iter().any(|s| s == "admin") {
        return true;
    }

    let segs: Vec<&str> = split_path(path);
    if segs.is_empty() {
        return true;
    }

    // Non-substrate paths: bring-up / test surface — allow.
    if segs[0] != "substrate" {
        return true;
    }
    if segs.len() == 1 {
        // Listing the substrate root — allow (names only; per-child
        // checks still apply on expansion).
        return true;
    }

    let owner = segs[1];
    let rest = &segs[2..];

    // Public by design.
    if rest.is_empty() {
        // substrate/<id> itself — allow (metadata root).
        return true;
    }
    if rest == ["meta"] || rest == ["health"] {
        return true;
    }
    if matches!(rest.first(), Some(&"cluster" | &"chain" | &"acl")) {
        return true;
    }

    // Private sensor / per-sensor health.
    if is_sensor_private(rest) {
        return caller.node_id.as_deref() == Some(owner);
    }

    // Actor-private subtrees (owner segment looks like an actor id).
    if owner.starts_with("a-") {
        return caller.actor_id.as_deref() == Some(owner);
    }

    // Everything else under substrate/<node>/… (control, services,
    // legacy test paths, etc.) — allow for backward compatibility with
    // pre-ACL callers. Sensor/actor privacy is the hard boundary.
    true
}

fn structural_reason(path: &str) -> &'static str {
    let segs: Vec<&str> = split_path(path);
    if segs.len() >= 3 && segs[0] == "substrate" {
        let rest = &segs[2..];
        if is_sensor_private(rest) {
            return "sensor subtree is deny-by-default";
        }
        if segs[1].starts_with("a-") {
            return "actor subtree is deny-by-default";
        }
    }
    "no matching allow rule"
}

// ── Path helpers ────────────────────────────────────────────────────

fn normalize_path(path: &str) -> String {
    let t = path.trim().trim_matches('/');
    t.to_string()
}

fn split_path(path: &str) -> Vec<&str> {
    path.split('/')
        .filter(|s| !s.is_empty())
        .collect()
}

// ── publish_public value helper ─────────────────────────────────────

/// Result of the no_std-friendly `publish_public` planning helper.
///
/// Firmware / edge clients call this to obtain the ACL rule that must
/// accompany a public publish; the actual substrate write is done by
/// the signed publish path (ADR-025).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishPublicPlan {
    /// Exact path being published.
    pub path: String,
    /// Rule to seal on that path (`allow: ["public"]`, inherit=false).
    pub rule: AclRule,
    /// Logical ACL storage key under the mesh subtree (when mesh_id known).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acl_storage_path: Option<String>,
}

/// Plan a `publish_public` write (ADR-057 §Opt-in publishing helpers).
///
/// Returns the ACL rule that opts a path out of the default-private
/// posture. Does not perform I/O.
pub fn plan_publish_public(path: &str, mesh_id: Option<&str>) -> PublishPublicPlan {
    let path = normalize_path(path);
    let rule = AclRule {
        allow: vec![ID_PUBLIC.into()],
        deny: Vec::new(),
        inherit: false,
        sealed_by: Some("publish_public".into()),
        sealed_at: None,
    };
    let acl_storage_path = mesh_id.map(|m| AclTable::storage_path(m, &path));
    PublishPublicPlan {
        path,
        rule,
        acl_storage_path,
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn mesh_table() -> AclTable {
        AclTable::with_boot_defaults("mesh-1")
    }

    #[test]
    fn public_meta_allows_anonymous() {
        let t = mesh_table();
        t.check("substrate/n-abc123/meta", &CallerIdentity::anonymous(), "read")
            .unwrap();
    }

    #[test]
    fn public_health_allows_anonymous() {
        let t = mesh_table();
        t.check(
            "substrate/n-abc123/health",
            &CallerIdentity::anonymous(),
            "read",
        )
        .unwrap();
    }

    #[test]
    fn public_cluster_and_chain_allow_anonymous() {
        let t = mesh_table();
        t.check(
            "substrate/mesh-1/cluster/nodes/n-abc",
            &CallerIdentity::anonymous(),
            "read",
        )
        .unwrap();
        t.check(
            "substrate/mesh-1/cluster/health/summary",
            &CallerIdentity::anonymous(),
            "list",
        )
        .unwrap();
        t.check(
            "substrate/mesh-1/chain/head",
            &CallerIdentity::anonymous(),
            "subscribe",
        )
        .unwrap();
    }

    #[test]
    fn sensor_denies_anonymous() {
        let t = mesh_table();
        let err = t
            .check(
                "substrate/n-abc123/sensor/mic/pcm_chunk",
                &CallerIdentity::anonymous(),
                "read",
            )
            .unwrap_err();
        assert_eq!(err.op, "read");
        assert!(err.wire_message().starts_with("acl_denied:"));
        assert!(err.reason.contains("sensor") || err.reason.contains("allow"));
    }

    #[test]
    fn sensor_denies_unrelated_actor() {
        let t = mesh_table();
        let err = t
            .check(
                "substrate/n-abc123/sensor/mic/pcm_chunk",
                &CallerIdentity::actor("a-watch1"),
                "subscribe",
            )
            .unwrap_err();
        assert_eq!(err.caller, "actor:a-watch1");
        assert!(err.wire_message().contains(AclDenied::CODE));
    }

    #[test]
    fn sensor_allows_owning_node() {
        let t = mesh_table();
        t.check(
            "substrate/n-abc123/sensor/mic/pcm_chunk",
            &CallerIdentity::node("n-abc123"),
            "read",
        )
        .unwrap();
    }

    #[test]
    fn sensor_allows_scope_admin() {
        let t = mesh_table();
        t.check(
            "substrate/n-abc123/sensor/imu/samples",
            &CallerIdentity::anonymous().with_scope("admin"),
            "read",
        )
        .unwrap();
    }

    #[test]
    fn health_sensor_private_but_node_health_public() {
        let t = mesh_table();
        // Aggregate health is public.
        t.check(
            "substrate/n-abc123/health",
            &CallerIdentity::anonymous(),
            "read",
        )
        .unwrap();
        // Per-sensor health is private.
        let err = t
            .check(
                "substrate/n-abc123/health/sensor/mic",
                &CallerIdentity::anonymous(),
                "read",
            )
            .unwrap_err();
        assert!(err.wire_message().starts_with("acl_denied:"));
    }

    #[test]
    fn actor_subtree_deny_by_default() {
        let t = mesh_table();
        let err = t
            .check(
                "substrate/a-deadbe/private/state",
                &CallerIdentity::actor("a-other"),
                "read",
            )
            .unwrap_err();
        assert!(err.reason.contains("actor") || err.reason.contains("allow"));

        // Owner actor can read.
        t.check(
            "substrate/a-deadbe/private/state",
            &CallerIdentity::actor("a-deadbe"),
            "read",
        )
        .unwrap();
    }

    #[test]
    fn explicit_deny_wins_over_allow() {
        let mut t = AclTable::new();
        t.insert(
            "substrate/n-1/sensor/mic/**",
            AclRule {
                allow: vec!["public".into(), "node:n-1".into()],
                deny: vec!["node:n-evil".into()],
                inherit: true,
                sealed_by: None,
                sealed_at: None,
            },
        );
        // Owner still ok.
        t.check(
            "substrate/n-1/sensor/mic/pcm",
            &CallerIdentity::node("n-1"),
            "read",
        )
        .unwrap();
        // Denied node loses even if somehow also in allow via public.
        let err = t
            .check(
                "substrate/n-1/sensor/mic/pcm",
                &CallerIdentity::node("n-evil"),
                "read",
            )
            .unwrap_err();
        assert!(err.reason.contains("deny"));
    }

    #[test]
    fn child_overrides_parent_inherit_false() {
        let mut t = AclTable::new();
        // Parent allows public for whole sensor tree.
        t.insert(
            "substrate/n-1/sensor/**",
            AclRule::public().with_inherit(true),
        );
        // Leaf locks down (no inherit, no public).
        t.insert(
            "substrate/n-1/sensor/mic/pcm_chunk",
            AclRule::allow_only(["node:n-1", "scope:admin"]).with_inherit(false),
        );

        // Sibling under parent still public.
        t.check(
            "substrate/n-1/sensor/mic/rms",
            &CallerIdentity::anonymous(),
            "read",
        )
        .unwrap();
        // Locked leaf denies anonymous.
        t.check(
            "substrate/n-1/sensor/mic/pcm_chunk",
            &CallerIdentity::anonymous(),
            "read",
        )
        .unwrap_err();
        // Owner allowed.
        t.check(
            "substrate/n-1/sensor/mic/pcm_chunk",
            &CallerIdentity::node("n-1"),
            "read",
        )
        .unwrap();
    }

    #[test]
    fn deny_leaf_under_allow_tree() {
        let mut t = AclTable::new();
        t.insert("substrate/n-1/sensor/**", AclRule::public());
        t.insert(
            "substrate/n-1/sensor/mic/pcm_chunk",
            AclRule {
                allow: vec!["public".into()],
                deny: vec!["actor:a-watch".into()],
                inherit: false,
                sealed_by: None,
                sealed_at: None,
            },
        );
        t.check(
            "substrate/n-1/sensor/mic/pcm_chunk",
            &CallerIdentity::actor("a-watch"),
            "subscribe",
        )
        .unwrap_err();
        t.check(
            "substrate/n-1/sensor/mic/pcm_chunk",
            &CallerIdentity::anonymous(),
            "read",
        )
        .unwrap();
    }

    #[test]
    fn publish_public_opts_sensor_path_out() {
        let mut t = mesh_table();
        let path = "substrate/n-abc123/sensor/mic/rms";
        t.check(path, &CallerIdentity::anonymous(), "read")
            .unwrap_err();
        t.publish_public(path);
        t.check(path, &CallerIdentity::anonymous(), "read")
            .unwrap();
    }

    #[test]
    fn plan_publish_public_shape() {
        let plan = plan_publish_public("substrate/n-1/sensor/x", Some("mesh-1"));
        assert_eq!(plan.rule.allow, vec!["public"]);
        assert!(!plan.rule.inherit);
        assert!(plan.acl_storage_path.unwrap().contains("acl/"));
    }

    #[test]
    fn acl_denied_distinct_from_not_found_shape() {
        let t = mesh_table();
        let err = t
            .check_raw(
                "substrate/n-abc/sensor/mic/pcm_chunk",
                Some("a-watch1"),
                "read",
            )
            .unwrap_err();
        // Clients must be able to detect acl_denied vs path missing.
        assert!(err.wire_message().starts_with("acl_denied:"));
        assert!(!err.wire_message().contains("not found"));
        let payload = err.chain_payload();
        assert_eq!(payload["code"], "acl_denied");
        assert_eq!(payload["op"], "read");
    }

    #[test]
    fn parse_identity_forms() {
        assert!(CallerIdentity::parse(None).is_anonymous());
        assert_eq!(
            CallerIdentity::parse(Some("node:n-01")).node_id.as_deref(),
            Some("n-01")
        );
        assert_eq!(
            CallerIdentity::parse(Some("n-01")).node_id.as_deref(),
            Some("n-01")
        );
        assert_eq!(
            CallerIdentity::parse(Some("actor:a-02")).actor_id.as_deref(),
            Some("a-02")
        );
        assert!(
            CallerIdentity::parse(Some("scope:admin"))
                .scopes
                .iter()
                .any(|s| s == "admin")
        );
        assert!(CallerIdentity::parse(Some("admin")).scopes.iter().any(|s| s == "admin"));
    }

    #[test]
    fn integration_subscriber_cannot_see_mic_but_can_see_meta() {
        // ADR-057 MUST-HAVE: subscriber Actor with no allow rule cannot
        // see sensor/mic/pcm_chunk even though it can see meta.
        let t = mesh_table();
        let watch = CallerIdentity::actor("a-watch1");
        t.check("substrate/n-esp32/meta", &watch, "read").unwrap();
        t.check(
            "substrate/n-esp32/sensor/mic/pcm_chunk",
            &watch,
            "subscribe",
        )
        .unwrap_err();
    }

    #[test]
    fn storage_path_encodes_glob() {
        let p = AclTable::storage_path("mesh-1", "substrate/*/sensor/**");
        assert_eq!(p, "substrate/mesh-1/acl/substrate~*~sensor~**");
    }

    #[test]
    fn legacy_test_paths_remain_readable() {
        // Backward compat: non-sensor, non-actor paths stay open so
        // existing substrate RPC tests keep working.
        let t = mesh_table();
        t.check("substrate/test/ping", &CallerIdentity::anonymous(), "read")
            .unwrap();
    }
}
