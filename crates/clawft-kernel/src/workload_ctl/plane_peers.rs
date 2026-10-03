//! Operator-assigned trust for `workload-host` targets (ADR-099 section 3).
//!
//! The operator's peer list is authoritative on every sync: a listed peer
//! gets the listed tier (raised or lowered), and a known remote target
//! that is no longer listed is demoted to `discovered`, which governance
//! never places on. Demoted targets stay known so their placed instances
//! can still be stopped and unloaded. Addresses in `keep` (this node's own
//! in-process host, Seeds) are not touched.
//!
//! Trust is bound to a node key, not to an address. A listed peer with a
//! `key` gets its tier only for that key: a known target at the address
//! with another key is demoted, and the pinned key is learned by a
//! `describe` that refuses any other signer. A listed peer without a key
//! gives its tier to the key first seen at the address; a different key
//! answering there later is never learned under it (`refresh` requires
//! the stored key).

use clawft_types::placement::TrustTier as FactsTier;

use super::plane::{PlacementControlPlane, PlaneError, TargetInfo};
use crate::workload_pkg::codec::hex_decode_exact;

/// One operator-listed `workload-host`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperatorPeer {
    /// Where it listens.
    pub addr: String,
    /// Operator-assigned tier.
    pub tier: FactsTier,
    /// The node key the tier is for; `None` trusts the key first seen at
    /// `addr`.
    pub key: Option<[u8; 32]>,
}

impl OperatorPeer {
    /// A peer trusted on first use.
    pub fn new(addr: impl Into<String>, tier: FactsTier) -> Self {
        Self {
            addr: addr.into(),
            tier,
            key: None,
        }
    }

    /// Only the node with `key` gets the tier.
    pub fn with_key(mut self, key: [u8; 32]) -> Self {
        self.key = Some(key);
        self
    }

    /// Whether this entry covers the known target `t` (`known` is every
    /// known target): a pinned entry covers its key at its address; an
    /// unpinned one only the first key learned at its address.
    fn covers(&self, t: &TargetInfo, known: &[TargetInfo]) -> bool {
        if self.addr != t.addr {
            return false;
        }
        match self.key {
            Some(k) => hex_decode_exact::<32>(&t.public_key) == Some(k),
            None => known
                .iter()
                .filter(|o| o.addr == self.addr)
                .min_by_key(|o| (o.learned_ms, o.node_id.clone()))
                .is_some_and(|first| first.node_id == t.node_id),
        }
    }
}

impl PlacementControlPlane {
    /// Set the operator-assigned tier of a known target (targets map and
    /// facts cache). False if the node is not known.
    pub fn set_tier(&self, node_id: &str, tier: FactsTier) -> bool {
        let known = self
            .targets
            .write()
            .ok()
            .and_then(|mut t| t.get_mut(node_id).map(|e| e.tier = tier))
            .is_some();
        self.facts.set_trust_tier(node_id, tier);
        // A Seed is addressed in-process: governance reads the tier held
        // beside its adapter.
        let seed = self
            .seeds
            .write()
            .ok()
            .and_then(|mut s| {
                s.get_mut(node_id).map(|e| {
                    e.1 = tier;
                    // The host's own gate calls read this live.
                    e.0.set_node_tier(super::facts::governance_tier(tier));
                })
            })
            .is_some();
        self.persist();
        known || seed
    }

    /// Make the known targets match the operator's `peers`. Returns one
    /// error per peer that could not be learned (it is retried on the next
    /// sync).
    pub async fn apply_operator_peers(
        &self,
        peers: &[OperatorPeer],
        keep: &[&str],
    ) -> Vec<(String, PlaneError)> {
        let known = self.targets();
        for t in &known {
            let want = match peers.iter().find(|p| p.covers(t, &known)) {
                Some(p) => p.tier,
                None if keep.contains(&t.addr.as_str()) => continue,
                None => FactsTier::Discovered,
            };
            if t.tier != want {
                tracing::info!(node = %t.node_id, addr = %t.addr, from = ?t.tier, to = ?want,
                    "workload peer tier changed by operator policy");
                self.set_tier(&t.node_id, want);
            }
        }
        let mut failed = Vec::new();
        for p in peers {
            // Satisfied by the first key learned at the address (unpinned)
            // or by the pinned key; otherwise learn it (a pinned key only).
            if known.iter().any(|t| p.covers(t, &known)) {
                continue;
            }
            if let Err(e) = self.add_target_expecting(&p.addr, p.tier, p.key).await {
                failed.push((p.addr.clone(), e));
            }
        }
        failed
    }
}
