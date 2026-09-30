//! Read-only inputs the placement engine consumes: per-node facts (through
//! the [`PlacementFacts`] trait) and the cluster state.
//!
//! The engine never defines node facts itself. Card mesh-placement-03 owns
//! the signed `NodeFacts` record; it implements [`PlacementFacts`] so the
//! engine stays independent of how facts are probed, signed or gossiped.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::super::capability::Capability;
use super::super::memory::MemoryDemand;
use super::spec::WorkloadSpec;

/// Node trust tier (ADR-099 section 8.3), ordered least to most trusted.
///
/// Wire names match the `trust.tier.{discovered,paired,pinned}` vocabulary
/// ids and the kernel governance `NodeTrustTier`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustTier {
    /// Seen on the mesh, not paired. Runs nothing by default.
    Discovered,
    /// Operator-paired. Runs operator-signed workloads.
    Paired,
    /// Pinned. May run workloads that carry secrets.
    Pinned,
}

impl TrustTier {
    /// Wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            TrustTier::Discovered => "discovered",
            TrustTier::Paired => "paired",
            TrustTier::Pinned => "pinned",
        }
    }
}

/// Node liveness as the caller's heartbeat tracker sees it. Only
/// [`Liveness::Alive`] passes the liveness constraint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Liveness {
    /// Heartbeats current.
    Alive,
    /// Heartbeats late; not a placement target.
    Suspect,
    /// Declared dead.
    Dead,
    /// Never heard from.
    Unknown,
}

impl Liveness {
    /// Wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Liveness::Alive => "alive",
            Liveness::Suspect => "suspect",
            Liveness::Dead => "dead",
            Liveness::Unknown => "unknown",
        }
    }
}

/// What the engine needs to know about one node. Read-only.
///
/// Measured performance is not a separate method: it is carried as
/// `perf.*` capabilities with `measured` provenance (see
/// [`super::super::perf`]).
pub trait PlacementFacts {
    /// Stable node id.
    fn node_id(&self) -> &str;
    /// Advertised capabilities (open vocabulary).
    fn capabilities(&self) -> &[Capability];
    /// Liveness from the heartbeat tracker.
    fn liveness(&self) -> Liveness;
    /// Trust tier assigned by the operator.
    fn trust_tier(&self) -> TrustTier;
    /// When the advertised facts expire (ms since the Unix epoch), or
    /// `None` if they carry no TTL.
    fn facts_expire_at_ms(&self) -> Option<u64>;
    /// Current load in `[0.0, 1.0]`, if known. Out-of-range values are
    /// clamped; `None` is scored as fully loaded (conservative).
    fn load(&self) -> Option<f64> {
        None
    }
}

/// Which workload an instance belongs to: the spec's `(kind, name)`.
///
/// ADR-099 section 1 keys an instance by `(manifest, config, node)`, so a
/// workload has at most one instance per node; an instance on a node that
/// carries the same identity as the request is the request's own instance
/// (a re-placement), not a neighbour.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorkloadRef {
    /// Workload kind.
    pub kind: String,
    /// Workload name.
    pub name: String,
}

impl WorkloadRef {
    /// The identity of `spec`.
    pub fn of(spec: &WorkloadSpec) -> Self {
        Self {
            kind: spec.kind.clone(),
            name: spec.name.clone(),
        }
    }

    /// True if this is `spec`'s identity.
    pub fn is(&self, spec: &WorkloadSpec) -> bool {
        // Identity equality only; the engine never branches on the kind.
        *self == Self::of(spec)
    }
}

/// An instance already placed (or being placed) somewhere in the cluster.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceRecord {
    /// Node it runs on.
    pub node_id: String,
    /// The workload it is an instance of. `None` for an instance the
    /// control plane cannot attribute; such an instance is always treated
    /// as a neighbour (never skipped as the request's own).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workload: Option<WorkloadRef>,
    /// Its co-residency labels (for example `role:planner`).
    #[serde(default)]
    pub labels: Vec<String>,
    /// Labels it refuses to share a node with.
    #[serde(default)]
    pub excludes: Vec<String>,
    /// Memory reserved for it that the node's advertised free memory does
    /// not reflect yet (a pending placement). Zero once facts catch up.
    #[serde(default)]
    pub pending: MemoryDemand,
}

/// Cluster-wide state the engine reads. Built by the control plane.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClusterState {
    /// Current time (ms since the Unix epoch), for fact TTLs. The engine
    /// never reads a clock.
    pub now_ms: u64,
    /// Revoked node ids.
    #[serde(default)]
    pub revoked_nodes: BTreeSet<String>,
    /// Revoked package ids, signer keys and artifact hashes.
    #[serde(default)]
    pub revoked_refs: BTreeSet<String>,
    /// Instances already placed.
    #[serde(default)]
    pub instances: Vec<InstanceRecord>,
}

impl ClusterState {
    /// Instances on `node_id`, excluding `spec`'s own instance there.
    pub fn neighbours_on<'a>(
        &'a self,
        node_id: &'a str,
        spec: &'a WorkloadSpec,
    ) -> impl Iterator<Item = &'a InstanceRecord> {
        self.instances_on(node_id)
            .filter(move |i| !i.workload.as_ref().is_some_and(|w| w.is(spec)))
    }

    /// Instances on `node_id`.
    pub fn instances_on<'a>(
        &'a self,
        node_id: &'a str,
    ) -> impl Iterator<Item = &'a InstanceRecord> {
        self.instances.iter().filter(move |i| i.node_id == node_id)
    }
}
