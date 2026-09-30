//! Operator-assigned trust for `workload-host` targets (ADR-099 section 3).
//!
//! The operator's peer list is authoritative on every sync: a listed peer
//! gets the listed tier (raised or lowered), and a known remote target
//! that is no longer listed is demoted to `discovered`, which governance
//! never places on. Demoted targets stay known so their placed instances
//! can still be stopped and unloaded. Addresses in `keep` (this node's own
//! in-process host, Seeds) are not touched.

use clawft_types::placement::TrustTier as FactsTier;

use super::plane::{PlacementControlPlane, PlaneError};

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
        known
    }

    /// Make the known targets match the operator's `peers` (address, tier).
    /// Returns one message per peer that could not be reached (it is
    /// retried on the next sync).
    pub async fn apply_operator_peers(
        &self,
        peers: &[(String, FactsTier)],
        keep: &[&str],
    ) -> Vec<(String, PlaneError)> {
        for t in self.targets() {
            let listed = peers
                .iter()
                .find(|(a, _)| *a == t.addr)
                .map(|(_, tier)| *tier);
            let want = match listed {
                Some(tier) => tier,
                None if keep.contains(&t.addr.as_str()) => continue,
                None => FactsTier::Discovered,
            };
            if t.tier != want {
                tracing::info!(node = %t.node_id, addr = %t.addr, from = ?t.tier, to = ?want,
                    "workload peer tier changed by operator policy");
                self.set_tier(&t.node_id, want);
            }
        }
        let known: Vec<String> = self.targets().into_iter().map(|t| t.addr).collect();
        let mut failed = Vec::new();
        for (addr, tier) in peers.iter().filter(|(a, _)| !known.contains(a)) {
            if let Err(e) = self.add_target(addr, *tier).await {
                failed.push((addr.clone(), e));
            }
        }
        failed
    }
}
