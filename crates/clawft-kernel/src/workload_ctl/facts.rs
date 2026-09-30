//! Card 03's verified, TTL-cached node facts as card 04's
//! [`PlacementFacts`] (ADR-099 sections 2-3).
//!
//! [`CachedNodeFacts`] carries the signed facts, the receiver-assigned
//! trust tier and live state (deltas). Liveness is not part of the facts:
//! it comes from `ClusterMembership` peer state (heartbeats) or, for a node
//! the control plane talks to directly, from the last signed round trip.
//! [`LiveNodeFacts`] pairs the two, so `place()` consumes exactly what the
//! cache verified.

use clawft_types::placement::engine::{self, Liveness, PlacementFacts};
use clawft_types::placement::{Capability, TrustTier as FactsTier};

use crate::cluster::{ClusterMembership, NodeState};
use crate::node_facts::{CachedNodeFacts, NodeFactsCache};
use crate::workload_governance::NodeTrustTier;

/// Cached facts plus liveness: what the placement engine reads.
#[derive(Debug, Clone)]
pub struct LiveNodeFacts {
    /// Verified facts (base plus deltas) with the receiver-assigned tier.
    pub cached: CachedNodeFacts,
    /// Liveness from membership or the last direct contact.
    pub liveness: Liveness,
}

/// Receiver-assigned facts tier as the engine's tier.
pub fn engine_tier(t: FactsTier) -> engine::TrustTier {
    match t {
        FactsTier::Discovered => engine::TrustTier::Discovered,
        FactsTier::Paired => engine::TrustTier::Paired,
        FactsTier::Pinned => engine::TrustTier::Pinned,
    }
}

/// Receiver-assigned facts tier as the governance effect's tier.
pub fn governance_tier(t: FactsTier) -> NodeTrustTier {
    match t {
        FactsTier::Discovered => NodeTrustTier::Discovered,
        FactsTier::Paired => NodeTrustTier::Paired,
        FactsTier::Pinned => NodeTrustTier::Pinned,
    }
}

/// Membership peer state as liveness. Only `Active` is `Alive`.
pub fn liveness_of(state: &NodeState) -> Liveness {
    match state {
        NodeState::Active => Liveness::Alive,
        NodeState::Suspect => Liveness::Suspect,
        NodeState::Unreachable | NodeState::Leaving | NodeState::Left => Liveness::Dead,
        _ => Liveness::Unknown,
    }
}

impl PlacementFacts for LiveNodeFacts {
    fn node_id(&self) -> &str {
        self.cached.node_id()
    }

    fn capabilities(&self) -> &[Capability] {
        self.cached.capabilities()
    }

    fn liveness(&self) -> Liveness {
        self.liveness
    }

    fn trust_tier(&self) -> engine::TrustTier {
        engine_tier(self.cached.trust_tier())
    }

    fn facts_expire_at_ms(&self) -> Option<u64> {
        Some(self.cached.facts.expires_at().saturating_mul(1000))
    }

    /// Share of capabilities busy or reserved; unknown for an empty block.
    fn load(&self) -> Option<f64> {
        let l = self.cached.load();
        (l.total > 0).then(|| l.busy as f64 / l.total as f64)
    }
}

/// Every fresh cached node with its liveness. `local` is always alive;
/// otherwise `membership` peer state wins, then `contact` (the control
/// plane's last direct round trip), else [`Liveness::Unknown`].
pub fn placement_view(
    cache: &NodeFactsCache,
    now_secs: u64,
    local: Option<&str>,
    membership: Option<&ClusterMembership>,
    contact: &dyn Fn(&str) -> Option<Liveness>,
) -> Vec<LiveNodeFacts> {
    cache
        .list(now_secs)
        .into_iter()
        .map(|cached| {
            let id = cached.node_id().to_string();
            let liveness = if local == Some(id.as_str()) {
                Liveness::Alive
            } else if let Some(p) = membership.and_then(|m| m.get_peer(&id)) {
                liveness_of(&p.state)
            } else {
                contact(&id).unwrap_or(Liveness::Unknown)
            };
            LiveNodeFacts { cached, liveness }
        })
        .collect()
}
