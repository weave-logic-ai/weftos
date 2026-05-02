//! Kernel-side substrate service.
//!
//! Provides a minimal, daemon-hosted implementation of the four
//! substrate RPCs (`substrate.read`, `substrate.publish`,
//! `substrate.subscribe`, `substrate.notify`). Each path carries:
//!
//! - a most-recent `serde_json::Value` (the state),
//! - a monotonic tick that advances on every write,
//! - a declared [`crate::topic::SubscriberSink`] set for fan-out,
//! - a declared [`Sensitivity`] (Public / Workspace / Private /
//!   Capture) consulted by the policy gate.
//!
//! The service is intentionally in-memory. It replaces the ad-hoc
//! markdown + file-backed adapter hacks described in weftos-0.7 by
//! giving external clients a real pub/sub+read/write surface to the
//! kernel's substrate.
//!
//! # Egress gating
//!
//! All reads/subscribes pass through [`SubstrateService::egress_check`]
//! — the single seam where a future governance policy will gate
//! `Capture`-tier topics on per-caller capability grants. For M1.5
//! bring-up this is a log-but-allow stub so adapters can land before
//! the policy layer is live.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::topic::{SubscriberId, SubscriberSink};

/// Privacy sensitivity of a substrate path.
///
/// Mirrors `clawft_substrate::Sensitivity` intentionally so adapter
/// code can share the classification vocabulary. Kept local here to
/// avoid a kernel → substrate crate dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Sensitivity {
    /// Safe to display anywhere; no prompt required.
    #[default]
    Public,
    /// Scoped to the current workspace/project.
    Workspace,
    /// Personal data beyond the workspace.
    Private,
    /// Derived from ambient capture (camera/mic/screen).
    Capture,
}

impl Sensitivity {
    /// Whether this level requires an authenticated caller for
    /// read / subscribe operations. `Capture` (and higher, if ever
    /// added) always requires; the rest allow anonymous reads for
    /// bring-up.
    pub fn requires_caller_identity(self) -> bool {
        matches!(self, Sensitivity::Capture)
    }

    /// Short text label, for log lines and error messages.
    pub fn as_str(self) -> &'static str {
        match self {
            Sensitivity::Public => "public",
            Sensitivity::Workspace => "workspace",
            Sensitivity::Private => "private",
            Sensitivity::Capture => "capture",
        }
    }
}

/// One substrate path's state.
struct Entry {
    value: Option<Value>,
    tick: u64,
    sensitivity: Sensitivity,
    sinks: Vec<(SubscriberId, SubscriberSink)>,
}

impl Entry {
    fn new(sensitivity: Sensitivity) -> Self {
        Self {
            value: None,
            tick: 0,
            sensitivity,
            sinks: Vec::new(),
        }
    }
}

/// Snapshot of a substrate read.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubstrateReadSnapshot {
    /// Current value at the path; `None` if never written.
    pub value: Option<Value>,
    /// Monotonic tick for the path.
    pub tick: u64,
    /// Declared sensitivity.
    pub sensitivity: Sensitivity,
}

/// One child entry returned by [`SubstrateService::list`].
///
/// Only paths that have (or have ever had) a Replace value are
/// considered "present" for the purpose of `has_value`. `child_count`
/// is the number of descendants strictly below `path` that themselves
/// have a value — internal nodes without values do not inflate it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubstrateListEntry {
    /// Full substrate path of this child (no trailing slash).
    pub path: String,
    /// `true` if this exact path has been published.
    pub has_value: bool,
    /// Count of descendants below `path` that themselves carry a value.
    pub child_count: u32,
}

/// Snapshot returned by [`SubstrateService::list`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubstrateListSnapshot {
    /// Children enumerated under the requested prefix (sorted by path).
    pub children: Vec<SubstrateListEntry>,
    /// Global substrate tick at the moment the list was taken. Mirrors
    /// [`SubstrateReadSnapshot::tick`] semantics — monotonic.
    pub tick: u64,
}

/// Reason an egress check denied access.
#[derive(Debug, Clone)]
pub struct EgressDenied {
    /// Short failure reason.
    pub reason: String,
}

impl std::fmt::Display for EgressDenied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "egress denied: {}", self.reason)
    }
}

/// Reason the node-identity write gate rejected a publish.
///
/// The gate enforces that every substrate write lands under
/// `substrate/<node-id>/` — see
/// [`crate::node_registry::required_path_prefix`]. Either the path
/// lies outside the caller's namespace or the caller supplied no node
/// identity at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateDenied {
    /// The path does not sit under `substrate/<node-id>/`.
    WrongPrefix {
        /// Attempted path.
        path: String,
        /// Node that tried to write it.
        node_id: String,
    },
    /// The publish has no declared node identity.
    MissingNodeId {
        /// Attempted path.
        path: String,
    },
    /// The path is mesh-canonical (`substrate/_derived/...`) but the
    /// publishing node holds no [`crate::DerivedWriteGrant`] covering
    /// it. See `.planning/sensors/PIPELINE-PRIMITIVE-JOURNAL.md` §R3.6.
    MissingDerivedGrant {
        /// Attempted path.
        path: String,
        /// Node that tried to write it.
        node_id: String,
    },
}

impl std::fmt::Display for GateDenied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GateDenied::WrongPrefix { path, node_id } => write!(
                f,
                "node {node_id} may not publish to {path} (must sit under substrate/{node_id}/)"
            ),
            GateDenied::MissingNodeId { path } => write!(
                f,
                "publish to {path} rejected: no node_id declared (every write must be node-attributed)"
            ),
            GateDenied::MissingDerivedGrant { path, node_id } => write!(
                f,
                "node {node_id} may not publish to mesh-canonical path {path} \
                 (no DerivedWriteGrant for the topic; see R3.6)"
            ),
        }
    }
}

impl std::error::Error for GateDenied {}

/// Substrate RPC service.
#[derive(Clone)]
pub struct SubstrateService {
    inner: Arc<SubstrateInner>,
}

struct SubstrateInner {
    entries: DashMap<String, Entry>,
    global_tick: AtomicU64,
}

impl Default for SubstrateService {
    fn default() -> Self {
        Self::new()
    }
}

impl SubstrateService {
    /// Create a new, empty service.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(SubstrateInner {
                entries: DashMap::new(),
                global_tick: AtomicU64::new(0),
            }),
        }
    }

    /// Declare (or re-declare) the sensitivity level for a path.
    ///
    /// If the path already exists, the declaration is updated in
    /// place without clearing the value. Paths not declared default
    /// to [`Sensitivity::Workspace`] on first touch.
    pub fn declare(&self, path: &str, sensitivity: Sensitivity) {
        let mut entry = self
            .inner
            .entries
            .entry(path.to_string())
            .or_insert_with(|| Entry::new(sensitivity));
        entry.sensitivity = sensitivity;
    }

    /// Egress gate stub.
    ///
    /// For bring-up: log every `Capture` read/subscribe but allow
    /// it. A future governance commit wires this to the
    /// capability-grant layer. This is intentionally the *one* seam
    /// the policy will gate, so callers need not change.
    pub fn egress_check(
        &self,
        caller: Option<&str>,
        path: &str,
        op: &str,
    ) -> Result<(), EgressDenied> {
        let sensitivity = self
            .inner
            .entries
            .get(path)
            .map(|e| e.sensitivity)
            .unwrap_or(Sensitivity::Workspace);

        if sensitivity.requires_caller_identity() && caller.is_none() {
            return Err(EgressDenied {
                reason: format!(
                    "{op} on {path} (sensitivity={}) requires authenticated caller",
                    sensitivity.as_str()
                ),
            });
        }

        if sensitivity == Sensitivity::Capture {
            warn!(
                path,
                caller = ?caller,
                op,
                "substrate egress: capture-tier path accessed (allow-all stub)"
            );
        } else {
            debug!(path, caller = ?caller, op, sensitivity = sensitivity.as_str(), "substrate egress ok");
        }
        Ok(())
    }

    /// Read the current value + metadata at a path.
    pub fn read(&self, caller: Option<&str>, path: &str) -> Result<SubstrateReadSnapshot, EgressDenied> {
        self.egress_check(caller, path, "read")?;
        let snapshot = match self.inner.entries.get(path) {
            Some(e) => SubstrateReadSnapshot {
                value: e.value.clone(),
                tick: e.tick,
                sensitivity: e.sensitivity,
            },
            None => SubstrateReadSnapshot {
                value: None,
                tick: 0,
                sensitivity: Sensitivity::Workspace,
            },
        };
        Ok(snapshot)
    }

    /// Enumerate children of a prefix up to `depth` levels below it.
    ///
    /// Contract (ADR-Explorer Phase 1 §3.1):
    ///
    /// - `prefix` — substrate path; empty string or `"/"` means
    ///   "top-level". Trailing slashes are trimmed.
    /// - `depth = 0` — return only the prefix node itself, if it is
    ///   a value (no children enumerated).
    /// - `depth = 1` — one level below `prefix`. Default for the
    ///   Explorer lazy-tree walk.
    /// - `depth = N > 1` — up to N levels below `prefix` (flat list,
    ///   deepest-child first in sort order).
    ///
    /// Only paths with a published Replace value participate — pure
    /// internal nodes (no value of their own but with descendants that
    /// do) are still surfaced as entries with `has_value: false` when
    /// they sit on the path down to a value-bearing leaf; see
    /// `child_count` semantics on [`SubstrateListEntry`].
    ///
    /// Per-child egress: the prefix itself is `egress_check`'d once
    /// under op `"list"`. Each candidate descendant path is then also
    /// egress-checked so capture-tier path *names* don't leak to an
    /// anonymous caller — matching the spirit of `read`'s per-path gate.
    ///
    /// Not a hot path (the Explorer expands one prefix per user click);
    /// a full DashMap scan is fine for the foreseeable substrate size.
    pub fn list(
        &self,
        caller: Option<&str>,
        prefix: &str,
        depth: u32,
    ) -> Result<SubstrateListSnapshot, EgressDenied> {
        let norm_prefix = normalize_prefix(prefix);
        // Gate once on the prefix itself so a `list` on a capture-tier
        // node requires the same identity `read` would.
        self.egress_check(caller, &norm_prefix, "list")?;

        let global_tick = self.inner.global_tick.load(Ordering::Relaxed);

        // Collect all path + (has_value, sensitivity) pairs once. Cheap
        // relative to the Explorer click cadence; avoids holding the
        // DashMap iterator across per-entry work.
        let mut value_paths: Vec<(String, Sensitivity)> =
            Vec::with_capacity(self.inner.entries.len());
        for r in self.inner.entries.iter() {
            // Only paths that have been published participate. An
            // entry with `value == None` may still exist from a bare
            // `notify` (which creates the entry without a value) or
            // from `declare`; those must NOT surface in listings.
            if r.value.is_some() {
                value_paths.push((r.key().clone(), r.sensitivity));
            }
        }

        // Depth 0 — just the exact-match node.
        if depth == 0 {
            if let Some((_, sens)) = value_paths
                .iter()
                .find(|(p, _)| *p == norm_prefix)
            {
                // Respect capture-tier gate on the entry itself.
                if egress_permits(caller, *sens) {
                    let child_count = count_descendants(&value_paths, &norm_prefix, caller);
                    return Ok(SubstrateListSnapshot {
                        children: vec![SubstrateListEntry {
                            path: norm_prefix,
                            has_value: true,
                            child_count,
                        }],
                        tick: global_tick,
                    });
                }
            }
            return Ok(SubstrateListSnapshot {
                children: Vec::new(),
                tick: global_tick,
            });
        }

        // If the prefix itself is a leaf value (e.g. list of
        // `substrate/sensor/mic` when that path carries the mic
        // snapshot directly), return it as a single entry so the caller
        // can render the leaf even when they asked for its subtree.
        if norm_prefix.is_empty() {
            // Root list: proceed to group-by-next-segment below.
        } else if let Some((_, sens)) = value_paths.iter().find(|(p, _)| *p == norm_prefix)
            && is_leaf(&value_paths, &norm_prefix)
            && egress_permits(caller, *sens)
        {
            return Ok(SubstrateListSnapshot {
                children: vec![SubstrateListEntry {
                    path: norm_prefix,
                    has_value: true,
                    child_count: 0,
                }],
                tick: global_tick,
            });
        }

        // Group descendants by (prefix + next N segments for N in 1..=depth),
        // collecting one entry per unique child path. Using a BTreeMap keeps
        // results sorted (ASCII) without a post-sort pass.
        use std::collections::BTreeMap;
        let mut buckets: BTreeMap<String, bool> = BTreeMap::new();

        for (path, sens) in &value_paths {
            if !egress_permits(caller, *sens) {
                continue;
            }
            // Only consider strict descendants of the prefix (or, for
            // empty prefix, all paths).
            let Some(rest) = strict_descendant_rest(path, &norm_prefix) else {
                continue;
            };
            // Split the relative tail into segments.
            let tail_segs: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
            if tail_segs.is_empty() {
                continue;
            }
            // For each level from 1..=min(depth, tail_segs.len()) compute
            // the ancestor path at that level. Record whether that
            // exact ancestor carries a value.
            let max_level = (depth as usize).min(tail_segs.len());
            for level in 1..=max_level {
                let ancestor = join_with_prefix(&norm_prefix, &tail_segs[..level]);
                let is_exact_value_leaf = level == tail_segs.len();
                let entry = buckets.entry(ancestor).or_insert(false);
                if is_exact_value_leaf {
                    *entry = true;
                }
            }
        }

        let children: Vec<SubstrateListEntry> = buckets
            .into_iter()
            .map(|(path, has_value)| SubstrateListEntry {
                child_count: count_descendants(&value_paths, &path, caller),
                path,
                has_value,
            })
            .collect();

        Ok(SubstrateListSnapshot {
            children,
            tick: global_tick,
        })
    }

    /// Publish a Replace delta under the node-identity write gate.
    ///
    /// Every node-private substrate write belongs to exactly one
    /// **node**, and may only land under that node's namespace —
    /// `substrate/<node-id>/…`. This is the enforcement seam: callers
    /// provide the `node_id` they have already authenticated
    /// (signature verified at the RPC boundary, or trusted at
    /// in-process call sites where the daemon stamps its own id);
    /// `publish_gated` refuses to write anywhere else.
    ///
    /// Mesh-canonical writes (`substrate/_derived/...`) are
    /// **rejected** by this method — the legacy gate has no
    /// [`crate::NodeRegistry`] handle and cannot consult the
    /// `DerivedWriteGrant` table. Use
    /// [`Self::publish_gated_with_grants`] for that tier.
    ///
    /// Returns the new tick on success, or [`GateDenied`] when the
    /// path is outside the node's prefix or no node id was supplied.
    /// See [`crate::node_registry::path_belongs_to`] for the exact
    /// rule.
    ///
    /// Fans out to all external-stream subscribers registered via
    /// [`Self::subscribe`].
    pub fn publish_gated(
        &self,
        node_id: Option<&str>,
        path: &str,
        value: Value,
    ) -> Result<u64, GateDenied> {
        let node_id = node_id.ok_or_else(|| GateDenied::MissingNodeId {
            path: path.to_string(),
        })?;
        if !crate::node_registry::path_belongs_to(path, node_id) {
            return Err(GateDenied::WrongPrefix {
                path: path.to_string(),
                node_id: node_id.to_string(),
            });
        }
        Ok(self.publish_after_gate(node_id, path, value))
    }

    /// Tier-aware publish gate.
    ///
    /// Splits writes by path tier per
    /// `.planning/sensors/PIPELINE-PRIMITIVE-JOURNAL.md` §R3.3:
    ///
    /// - **Node-private** (`substrate/<node-id>/...`) — strict prefix
    ///   match, identical to [`Self::publish_gated`].
    /// - **Mesh-canonical** (`substrate/_derived/...`) — checks the
    ///   registry's [`crate::DerivedWriteGrant`] table.
    ///   `node_registry.has_derived_grant(node_id, path)` must return
    ///   `true`. Without a grant the write is rejected with
    ///   [`GateDenied::MissingDerivedGrant`].
    ///
    /// The dual-tier check is the load-bearing bit of R3 — it lets a
    /// daemon-class node publish whisper transcripts at
    /// `substrate/_derived/transcript/<source>/mic` while still
    /// honouring the per-node prefix rule for everything else.
    pub fn publish_gated_with_grants(
        &self,
        node_id: Option<&str>,
        path: &str,
        value: Value,
        node_registry: &crate::NodeRegistry,
    ) -> Result<u64, GateDenied> {
        let node_id = node_id.ok_or_else(|| GateDenied::MissingNodeId {
            path: path.to_string(),
        })?;
        // Tier detection. Mesh-canonical paths get the grant check;
        // anything else falls through to the legacy per-node-prefix
        // rule. Note that `_derived/` is the *only* reserved word
        // under `substrate/` — node-ids carry a leading `n-` exactly
        // so they cannot collide with this segment (see
        // `node_registry::node_id_from_pubkey`).
        if path.starts_with(crate::node_registry::MESH_CANONICAL_PREFIX) {
            if !node_registry.has_derived_grant(node_id, path) {
                return Err(GateDenied::MissingDerivedGrant {
                    path: path.to_string(),
                    node_id: node_id.to_string(),
                });
            }
            return Ok(self.publish_after_gate(node_id, path, value));
        }
        if !crate::node_registry::path_belongs_to(path, node_id) {
            return Err(GateDenied::WrongPrefix {
                path: path.to_string(),
                node_id: node_id.to_string(),
            });
        }
        Ok(self.publish_after_gate(node_id, path, value))
    }

    /// Shared post-gate write path. Allocates a new tick, writes the
    /// value, and fans out to all subscribers. Both
    /// [`Self::publish_gated`] and
    /// [`Self::publish_gated_with_grants`] flow through here so the
    /// fan-out behaviour stays identical across tiers.
    ///
    /// `node_id` is recorded as the caller on the fan-out line so
    /// subscribers can audit who wrote it. We deliberately keep the
    /// existing `caller` wire-field name — semantics evolve (from
    /// "actor" to "node") without breaking the log shape.
    fn publish_after_gate(&self, node_id: &str, path: &str, value: Value) -> u64 {
        let new_tick = self.inner.global_tick.fetch_add(1, Ordering::Relaxed) + 1;
        let line = build_update_line(path, Some(&value), new_tick, "publish", Some(node_id));
        let mut entry = self
            .inner
            .entries
            .entry(path.to_string())
            .or_insert_with(|| Entry::new(Sensitivity::Workspace));
        entry.value = Some(value);
        entry.tick = new_tick;
        fanout(&mut entry.sinks, &line);
        new_tick
    }

    /// Publish a Replace delta at a path. Returns the new tick.
    ///
    /// Fans out to all external-stream subscribers registered via
    /// [`Self::subscribe`]. For bring-up, the owner-check is trusted
    /// — the daemon layer should gate on its own agent role policy.
    ///
    /// **Legacy path.** This is the pre-node-identity surface. New
    /// callers should use [`Self::publish_gated`] so the per-node
    /// prefix invariant holds. Phased migration lets existing in-
    /// process publishers keep working until they're each updated.
    pub fn publish(&self, caller: Option<&str>, path: &str, value: Value) -> u64 {
        let new_tick = self.inner.global_tick.fetch_add(1, Ordering::Relaxed) + 1;
        let line = build_update_line(path, Some(&value), new_tick, "publish", caller);

        let mut entry = self
            .inner
            .entries
            .entry(path.to_string())
            .or_insert_with(|| Entry::new(Sensitivity::Workspace));
        entry.value = Some(value);
        entry.tick = new_tick;
        fanout(&mut entry.sinks, &line);
        new_tick
    }

    /// Emit a notify-only signal (no payload change) on a path.
    /// Returns the current tick (unchanged if no write has happened).
    pub fn notify(&self, caller: Option<&str>, path: &str) -> u64 {
        let new_tick = self.inner.global_tick.fetch_add(1, Ordering::Relaxed) + 1;
        let line = build_update_line(path, None, new_tick, "notify", caller);

        let mut entry = self
            .inner
            .entries
            .entry(path.to_string())
            .or_insert_with(|| Entry::new(Sensitivity::Workspace));
        entry.tick = new_tick;
        fanout(&mut entry.sinks, &line);
        new_tick
    }

    /// Subscribe an external streaming sink to updates on a path.
    ///
    /// Returns the subscriber id and a channel receiver. The caller
    /// pipes the JSON lines the sink receives into their socket.
    pub fn subscribe(
        &self,
        caller: Option<&str>,
        path: &str,
    ) -> Result<(SubscriberId, mpsc::Receiver<Vec<u8>>), EgressDenied> {
        self.egress_check(caller, path, "subscribe")?;
        let (tx, rx) = mpsc::channel::<Vec<u8>>(256);
        let mut entry = self
            .inner
            .entries
            .entry(path.to_string())
            .or_insert_with(|| Entry::new(Sensitivity::Workspace));
        let id = SubscriberId::next();
        entry.sinks.push((id, SubscriberSink::ExternalStream(tx)));
        Ok((id, rx))
    }

    /// Remove a specific subscription. Safe if the id is unknown.
    pub fn unsubscribe(&self, path: &str, id: SubscriberId) {
        if let Some(mut entry) = self.inner.entries.get_mut(path) {
            entry.sinks.retain(|(existing, _)| *existing != id);
        }
    }

    /// Number of declared paths (for metrics / tests).
    pub fn path_count(&self) -> usize {
        self.inner.entries.len()
    }
}

fn fanout(sinks: &mut Vec<(SubscriberId, SubscriberSink)>, line: &[u8]) {
    let mut dead = Vec::new();
    for (id, sink) in sinks.iter() {
        match sink {
            SubscriberSink::ExternalStream(tx) => {
                if tx.try_send(line.to_vec()).is_err() {
                    // closed or full — prune closed ones
                    if tx.is_closed() {
                        dead.push(*id);
                    }
                }
            }
            SubscriberSink::PidInbox(_) => {
                // PID delivery isn't wired for substrate today.
                // The kernel a2a router handles in-process fanout.
            }
        }
    }
    if !dead.is_empty() {
        sinks.retain(|(id, _)| !dead.contains(id));
    }
}

/// Normalize a user-supplied prefix: drop leading/trailing slashes.
/// An empty-or-`"/"` prefix becomes `""`, meaning "list from the root".
fn normalize_prefix(prefix: &str) -> String {
    let trimmed = prefix.trim_matches('/');
    trimmed.to_string()
}

/// Return the relative remainder after `prefix/` inside `path`, if
/// `path` is a strict descendant of `prefix`. Handles the root case
/// (empty prefix means "every path is a descendant").
fn strict_descendant_rest<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    if prefix.is_empty() {
        // Root prefix: every non-empty path is a descendant; rest is
        // the whole path.
        if path.is_empty() {
            return None;
        }
        return Some(path);
    }
    if path == prefix {
        return None; // same node, not a strict descendant
    }
    let with_sep = path.strip_prefix(prefix)?;
    let rest = with_sep.strip_prefix('/')?;
    if rest.is_empty() {
        None
    } else {
        Some(rest)
    }
}

/// Join `prefix` + segment slice into a substrate path. Root-aware:
/// empty prefix yields just the joined segments.
fn join_with_prefix(prefix: &str, segs: &[&str]) -> String {
    if prefix.is_empty() {
        segs.join("/")
    } else {
        let mut out = String::with_capacity(prefix.len() + 1 + segs.iter().map(|s| s.len() + 1).sum::<usize>());
        out.push_str(prefix);
        for s in segs {
            out.push('/');
            out.push_str(s);
        }
        out
    }
}

/// Whether a path is a leaf in the current substrate: no other
/// value-carrying path is a strict descendant of it.
fn is_leaf(value_paths: &[(String, Sensitivity)], path: &str) -> bool {
    !value_paths
        .iter()
        .any(|(p, _)| strict_descendant_rest(p, path).is_some())
}

/// Count all descendants of `path` that carry a value and are visible
/// to `caller` under the egress rules.
fn count_descendants(
    value_paths: &[(String, Sensitivity)],
    path: &str,
    caller: Option<&str>,
) -> u32 {
    let mut n: u32 = 0;
    for (p, sens) in value_paths {
        if !egress_permits(caller, *sens) {
            continue;
        }
        if strict_descendant_rest(p, path).is_some() {
            n = n.saturating_add(1);
        }
    }
    n
}

/// Mirror of [`SubstrateService::egress_check`]'s accept/deny logic for
/// a path whose sensitivity we already know. Used by `list` so we can
/// skip capture-tier children for anonymous callers without taking a
/// second DashMap lookup per entry.
fn egress_permits(caller: Option<&str>, sensitivity: Sensitivity) -> bool {
    !(sensitivity.requires_caller_identity() && caller.is_none())
}

fn build_update_line(
    path: &str,
    value: Option<&Value>,
    tick: u64,
    kind: &str,
    caller: Option<&str>,
) -> Vec<u8> {
    let body = serde_json::json!({
        "path": path,
        "tick": tick,
        "kind": kind,
        "value": value,
        "actor_id": caller,
    });
    let mut bytes = serde_json::to_vec(&body).unwrap_or_else(|_| b"{}".to_vec());
    bytes.push(b'\n');
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_then_read_roundtrip() {
        let svc = SubstrateService::new();
        let t = svc.publish(None, "substrate/test/ping", serde_json::json!(42));
        let snap = svc.read(None, "substrate/test/ping").unwrap();
        assert_eq!(snap.value, Some(serde_json::json!(42)));
        assert_eq!(snap.tick, t);
    }

    #[test]
    fn read_unknown_path_returns_empty() {
        let svc = SubstrateService::new();
        let snap = svc.read(None, "nope").unwrap();
        assert!(snap.value.is_none());
        assert_eq!(snap.tick, 0);
    }

    #[tokio::test]
    async fn subscribe_receives_publish_and_notify() {
        let svc = SubstrateService::new();
        let (_id, mut rx) = svc.subscribe(None, "substrate/test/ping").unwrap();
        svc.publish(None, "substrate/test/ping", serde_json::json!("hi"));
        svc.notify(None, "substrate/test/ping");

        let line1 = tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv())
            .await
            .unwrap()
            .unwrap();
        let v1: serde_json::Value = serde_json::from_slice(&line1[..line1.len() - 1]).unwrap();
        assert_eq!(v1["kind"], "publish");

        let line2 = tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv())
            .await
            .unwrap()
            .unwrap();
        let v2: serde_json::Value = serde_json::from_slice(&line2[..line2.len() - 1]).unwrap();
        assert_eq!(v2["kind"], "notify");
        assert!(v2["value"].is_null());
    }

    #[test]
    fn egress_check_denies_capture_anonymous() {
        let svc = SubstrateService::new();
        svc.declare("substrate/mic/frames", Sensitivity::Capture);
        let err = svc.read(None, "substrate/mic/frames").unwrap_err();
        assert!(err.reason.contains("requires authenticated"));
    }

    #[test]
    fn egress_check_allows_capture_with_identity() {
        let svc = SubstrateService::new();
        svc.declare("substrate/mic/frames", Sensitivity::Capture);
        assert!(svc.read(Some("aid-1"), "substrate/mic/frames").is_ok());
    }

    #[test]
    fn tick_monotonic_across_ops() {
        let svc = SubstrateService::new();
        let a = svc.publish(None, "p", serde_json::json!(1));
        let b = svc.notify(None, "p");
        let c = svc.publish(None, "p", serde_json::json!(2));
        assert!(b > a);
        assert!(c > b);
    }

    // ── list() ──────────────────────────────────────────────────────

    fn seed_sensor_substrate(svc: &SubstrateService) {
        svc.publish(None, "substrate/sensor/mic", serde_json::json!({"rms_db": -20}));
        svc.publish(None, "substrate/sensor/tof", serde_json::json!({"frame": 1}));
        svc.publish(
            None,
            "substrate/sensor/mic/history",
            serde_json::json!([1, 2, 3]),
        );
        svc.publish(None, "substrate/kernel/status", serde_json::json!({"state": "running"}));
    }

    #[test]
    fn list_empty_store_returns_empty_children() {
        let svc = SubstrateService::new();
        let snap = svc.list(None, "substrate", 1).unwrap();
        assert!(snap.children.is_empty());
    }

    #[test]
    fn list_prefix_with_no_children_returns_empty() {
        let svc = SubstrateService::new();
        seed_sensor_substrate(&svc);
        let snap = svc.list(None, "substrate/nope", 1).unwrap();
        assert!(snap.children.is_empty());
    }

    #[test]
    fn list_top_level_substrate() {
        let svc = SubstrateService::new();
        seed_sensor_substrate(&svc);
        let snap = svc.list(None, "substrate", 1).unwrap();
        // Expect `substrate/kernel` and `substrate/sensor` (both internal).
        let paths: Vec<&str> = snap.children.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(paths, vec!["substrate/kernel", "substrate/sensor"]);
        // Neither is itself a published value — internal nodes.
        assert!(snap.children.iter().all(|c| !c.has_value));
        // Child counts reflect descendants-with-values.
        let sensor = snap.children.iter().find(|c| c.path == "substrate/sensor").unwrap();
        // substrate/sensor/mic, substrate/sensor/mic/history, substrate/sensor/tof
        assert_eq!(sensor.child_count, 3);
    }

    #[test]
    fn list_depth_one_under_sensor_groups_children() {
        let svc = SubstrateService::new();
        seed_sensor_substrate(&svc);
        let snap = svc.list(None, "substrate/sensor", 1).unwrap();
        let mic = snap
            .children
            .iter()
            .find(|c| c.path == "substrate/sensor/mic")
            .expect("mic child present");
        let tof = snap
            .children
            .iter()
            .find(|c| c.path == "substrate/sensor/tof")
            .expect("tof child present");
        assert!(mic.has_value, "mic has a direct value");
        assert!(tof.has_value, "tof has a direct value");
        // mic has one descendant (history); tof has none.
        assert_eq!(mic.child_count, 1);
        assert_eq!(tof.child_count, 0);
    }

    #[test]
    fn list_depth_two_returns_flat_descendants() {
        let svc = SubstrateService::new();
        seed_sensor_substrate(&svc);
        let snap = svc.list(None, "substrate/sensor", 2).unwrap();
        let paths: Vec<&str> = snap.children.iter().map(|c| c.path.as_str()).collect();
        // depth=2 reveals mic/history as a sibling entry in the flat list.
        assert!(paths.contains(&"substrate/sensor/mic"));
        assert!(paths.contains(&"substrate/sensor/mic/history"));
        assert!(paths.contains(&"substrate/sensor/tof"));
    }

    #[test]
    fn list_depth_zero_returns_just_the_prefix_node() {
        let svc = SubstrateService::new();
        seed_sensor_substrate(&svc);
        // A value-carrying prefix: returns a single entry for itself.
        let snap = svc.list(None, "substrate/sensor/mic", 0).unwrap();
        assert_eq!(snap.children.len(), 1);
        assert_eq!(snap.children[0].path, "substrate/sensor/mic");
        assert!(snap.children[0].has_value);
        assert_eq!(snap.children[0].child_count, 1);

        // A non-value prefix at depth 0: no entry.
        let snap = svc.list(None, "substrate/sensor", 0).unwrap();
        assert!(snap.children.is_empty());
    }

    #[test]
    fn list_leaf_prefix_returns_itself() {
        let svc = SubstrateService::new();
        svc.publish(None, "substrate/x", serde_json::json!(1));
        let snap = svc.list(None, "substrate/x", 1).unwrap();
        assert_eq!(snap.children.len(), 1);
        assert_eq!(snap.children[0].path, "substrate/x");
        assert!(snap.children[0].has_value);
        assert_eq!(snap.children[0].child_count, 0);
    }

    #[test]
    fn list_empty_prefix_lists_from_root() {
        let svc = SubstrateService::new();
        seed_sensor_substrate(&svc);
        let snap = svc.list(None, "", 1).unwrap();
        let paths: Vec<&str> = snap.children.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(paths, vec!["substrate"]);
        // root prefix, slash-only — equivalent.
        let snap2 = svc.list(None, "/", 1).unwrap();
        assert_eq!(snap.children, snap2.children);
    }

    #[test]
    fn list_trailing_slash_is_normalized() {
        let svc = SubstrateService::new();
        seed_sensor_substrate(&svc);
        let a = svc.list(None, "substrate/sensor", 1).unwrap();
        let b = svc.list(None, "substrate/sensor/", 1).unwrap();
        assert_eq!(a.children, b.children);
    }

    #[test]
    fn list_skips_paths_without_value_from_notify_only() {
        let svc = SubstrateService::new();
        // `notify` creates an entry without `value`. It must NOT
        // appear in list output.
        svc.notify(None, "substrate/ghost/path");
        let snap = svc.list(None, "substrate", 1).unwrap();
        assert!(snap.children.is_empty(), "ghost entry leaked: {:?}", snap.children);
    }

    #[test]
    fn list_tick_reflects_global_tick() {
        let svc = SubstrateService::new();
        svc.publish(None, "substrate/a", serde_json::json!(1));
        svc.publish(None, "substrate/b", serde_json::json!(2));
        let snap = svc.list(None, "substrate", 1).unwrap();
        assert!(snap.tick >= 2, "tick should advance with writes");
    }

    #[test]
    fn list_egress_denies_anonymous_on_capture_prefix() {
        let svc = SubstrateService::new();
        svc.declare("substrate/mic/capture", Sensitivity::Capture);
        svc.publish(Some("aid-1"), "substrate/mic/capture", serde_json::json!({"pcm": []}));
        let err = svc.list(None, "substrate/mic/capture", 1).unwrap_err();
        assert!(err.reason.contains("requires authenticated"));
    }

    #[test]
    fn list_hides_capture_children_from_anonymous() {
        let svc = SubstrateService::new();
        // Parent is workspace — safe to list.
        svc.publish(None, "substrate/mic/public", serde_json::json!({"ok": true}));
        // Capture-tier child.
        svc.declare("substrate/mic/capture", Sensitivity::Capture);
        svc.publish(Some("aid"), "substrate/mic/capture", serde_json::json!({"pcm": []}));

        let snap = svc.list(None, "substrate/mic", 1).unwrap();
        let paths: Vec<&str> = snap.children.iter().map(|c| c.path.as_str()).collect();
        assert!(paths.contains(&"substrate/mic/public"));
        assert!(
            !paths.contains(&"substrate/mic/capture"),
            "capture-tier child leaked to anonymous caller: {paths:?}"
        );

        // With identity, the capture child is visible.
        let snap = svc.list(Some("aid"), "substrate/mic", 1).unwrap();
        let paths: Vec<&str> = snap.children.iter().map(|c| c.path.as_str()).collect();
        assert!(paths.contains(&"substrate/mic/capture"));
    }

    #[test]
    fn list_sort_order_is_stable_ascii() {
        let svc = SubstrateService::new();
        svc.publish(None, "substrate/z", serde_json::json!(1));
        svc.publish(None, "substrate/a", serde_json::json!(2));
        svc.publish(None, "substrate/m", serde_json::json!(3));
        let snap = svc.list(None, "substrate", 1).unwrap();
        let paths: Vec<&str> = snap.children.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(paths, vec!["substrate/a", "substrate/m", "substrate/z"]);
    }

    // ── node-identity write gate ───────────────────────────────

    #[test]
    fn publish_gated_accepts_path_under_node_prefix() {
        let svc = SubstrateService::new();
        let tick = svc
            .publish_gated(Some("n1"), "substrate/n1/sensor/mic", serde_json::json!(42))
            .expect("valid prefix accepted");
        assert_eq!(tick, 1);
        // And the value is readable back.
        let snap = svc.read(None, "substrate/n1/sensor/mic").unwrap();
        assert_eq!(snap.value, Some(serde_json::json!(42)));
    }

    #[test]
    fn publish_gated_rejects_cross_node_write() {
        let svc = SubstrateService::new();
        let err = svc
            .publish_gated(
                Some("n1"),
                "substrate/n2/sensor/mic",
                serde_json::json!(0),
            )
            .expect_err("cross-node write must be rejected");
        match err {
            GateDenied::WrongPrefix { path, node_id } => {
                assert_eq!(path, "substrate/n2/sensor/mic");
                assert_eq!(node_id, "n1");
            }
            other => panic!("expected WrongPrefix, got {other:?}"),
        }
    }

    #[test]
    fn publish_gated_rejects_top_level_write() {
        let svc = SubstrateService::new();
        let err = svc
            .publish_gated(Some("n1"), "substrate/sensor/mic", serde_json::json!(0))
            .expect_err("top-level write must be rejected");
        assert!(matches!(err, GateDenied::WrongPrefix { .. }));
    }

    #[test]
    fn publish_gated_rejects_missing_node_id() {
        let svc = SubstrateService::new();
        let err = svc
            .publish_gated(None, "substrate/n1/x", serde_json::json!(0))
            .expect_err("unsigned writes must be rejected");
        match err {
            GateDenied::MissingNodeId { path } => {
                assert_eq!(path, "substrate/n1/x");
            }
            other => panic!("expected MissingNodeId, got {other:?}"),
        }
    }

    #[test]
    fn publish_gated_rejects_prefix_collision() {
        // `n1` must not be able to write into `n11`'s namespace just
        // because the string starts with `substrate/n1`. The trailing
        // slash in `required_path_prefix` blocks it.
        let svc = SubstrateService::new();
        let err = svc
            .publish_gated(Some("n1"), "substrate/n11/x", serde_json::json!(0))
            .expect_err("sibling-prefix collision must be rejected");
        assert!(matches!(err, GateDenied::WrongPrefix { .. }));
    }

    #[test]
    fn publish_gated_increments_tick_like_publish() {
        let svc = SubstrateService::new();
        let t1 = svc
            .publish_gated(Some("n1"), "substrate/n1/a", serde_json::json!(1))
            .unwrap();
        let t2 = svc
            .publish_gated(Some("n1"), "substrate/n1/b", serde_json::json!(2))
            .unwrap();
        assert!(t2 > t1);
    }

    #[test]
    fn publish_gated_fans_out_to_subscribers() {
        let svc = SubstrateService::new();
        let (_id, mut rx) = svc
            .subscribe(Some("aid"), "substrate/n1/sensor/mic")
            .unwrap();
        svc.publish_gated(
            Some("n1"),
            "substrate/n1/sensor/mic",
            serde_json::json!({"rms_db": -40}),
        )
        .unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let got = rt.block_on(async move { rx.recv().await });
        assert!(got.is_some(), "subscriber did not receive fanout");
    }

    // ── publish_gated_with_grants (R3.3 tier-aware gate) ───────────

    #[test]
    fn gate_accepts_derived_write_with_grant() {
        let svc = SubstrateService::new();
        let registry = crate::NodeRegistry::new();
        registry
            .issue_derived_grant(
                "n-daemon",
                "transcript",
                crate::GrantScope::TopicPrefix,
            )
            .unwrap();
        let tick = svc
            .publish_gated_with_grants(
                Some("n-daemon"),
                "substrate/_derived/transcript/n-foo/mic",
                serde_json::json!({"text": "hi"}),
                &registry,
            )
            .expect("grant in place — write must succeed");
        assert_eq!(tick, 1);
        // Read back through the egress gate to prove the value
        // landed.
        let snap = svc
            .read(None, "substrate/_derived/transcript/n-foo/mic")
            .unwrap();
        assert_eq!(snap.value, Some(serde_json::json!({"text": "hi"})));
    }

    #[test]
    fn gate_rejects_derived_write_without_grant() {
        let svc = SubstrateService::new();
        let registry = crate::NodeRegistry::new();
        // No grant issued.
        let err = svc
            .publish_gated_with_grants(
                Some("n-daemon"),
                "substrate/_derived/transcript/n-foo/mic",
                serde_json::json!({"text": "nope"}),
                &registry,
            )
            .expect_err("missing grant must reject");
        match err {
            GateDenied::MissingDerivedGrant { path, node_id } => {
                assert_eq!(path, "substrate/_derived/transcript/n-foo/mic");
                assert_eq!(node_id, "n-daemon");
            }
            other => panic!("expected MissingDerivedGrant, got {other:?}"),
        }
    }

    #[test]
    fn gate_rejects_derived_for_unrelated_topic() {
        let svc = SubstrateService::new();
        let registry = crate::NodeRegistry::new();
        // Grant only for `transcript`; daemon attempts a `classify`
        // write. R3.6: grants are bounded — one pipeline's grant
        // does NOT bleed into another's namespace.
        registry
            .issue_derived_grant(
                "n-daemon",
                "transcript",
                crate::GrantScope::TopicPrefix,
            )
            .unwrap();
        let err = svc
            .publish_gated_with_grants(
                Some("n-daemon"),
                "substrate/_derived/classify/n-foo/mic",
                serde_json::json!({"label": "speech"}),
                &registry,
            )
            .expect_err("grant scoped to other topic must not apply");
        assert!(matches!(err, GateDenied::MissingDerivedGrant { .. }));
    }

    #[test]
    fn gate_accepts_node_prefix_write_unchanged() {
        // Regression: tier detection must not break the node-private
        // happy path. With no grant in the registry, a write under
        // the node's own prefix still succeeds via the fall-through
        // branch.
        let svc = SubstrateService::new();
        let registry = crate::NodeRegistry::new();
        let tick = svc
            .publish_gated_with_grants(
                Some("n-daemon"),
                "substrate/n-daemon/sensor/mic",
                serde_json::json!(42),
                &registry,
            )
            .expect("node-private path still accepted");
        assert_eq!(tick, 1);
    }

    #[test]
    fn gate_rejects_cross_node_write_unchanged() {
        // Regression: the per-node prefix rule still rejects writes
        // into another node's subtree in the new gate.
        let svc = SubstrateService::new();
        let registry = crate::NodeRegistry::new();
        let err = svc
            .publish_gated_with_grants(
                Some("n-a"),
                "substrate/n-b/sensor/mic",
                serde_json::json!(0),
                &registry,
            )
            .expect_err("cross-node write must be rejected");
        match err {
            GateDenied::WrongPrefix { path, node_id } => {
                assert_eq!(path, "substrate/n-b/sensor/mic");
                assert_eq!(node_id, "n-a");
            }
            other => panic!("expected WrongPrefix, got {other:?}"),
        }
    }

    #[test]
    fn gate_rejects_top_level_flat_write_unchanged() {
        // Regression: a path with no node-id segment (the old "flat"
        // shape) still gets rejected as WrongPrefix even with a grant
        // in the registry — it's neither node-private nor
        // mesh-canonical.
        let svc = SubstrateService::new();
        let registry = crate::NodeRegistry::new();
        registry
            .issue_derived_grant(
                "n-daemon",
                "transcript",
                crate::GrantScope::TopicPrefix,
            )
            .unwrap();
        let err = svc
            .publish_gated_with_grants(
                Some("n-daemon"),
                "substrate/sensor/mic",
                serde_json::json!(0),
                &registry,
            )
            .expect_err("flat path must be rejected");
        assert!(matches!(err, GateDenied::WrongPrefix { .. }));
    }

    #[test]
    fn legacy_publish_gated_still_rejects_derived_paths() {
        // The legacy `publish_gated` (no registry) must reject
        // `_derived/` writes outright — it has no way to consult
        // the grant table. This guards against accidental
        // regressions in callers that haven't migrated.
        let svc = SubstrateService::new();
        let err = svc
            .publish_gated(
                Some("n-daemon"),
                "substrate/_derived/transcript/n-foo/mic",
                serde_json::json!({"text": "x"}),
            )
            .expect_err("legacy gate must reject derived writes");
        assert!(matches!(err, GateDenied::WrongPrefix { .. }));
    }
}
