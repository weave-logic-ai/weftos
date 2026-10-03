//! "Who has this artifact" over the mesh, with no central tracker (ADR-099
//! section 6, card mesh-placement-25).
//!
//! Two sources, both peer-to-peer:
//!
//! - **Advertised.** Every node merges what it can serve into its signed
//!   facts (`store.artifact.<prefix>` with the full content hash, see
//!   [`ArtifactExchange::held_capabilities`]); facts travel to all peers
//!   ([`crate::node_facts_exchange`]), so [`holders_from_facts`] answers from
//!   the local cache with no traffic. Locality comes from the peer's
//!   `net.lan` fact.
//! - **Asked.** [`ArtifactExchange::who_has`] sends `meta_request` to named
//!   peers and returns those that answer with a descriptor and the pieces
//!   they hold: the same message a fetch starts with.
//!
//! [`ArtifactExchange::find_holders`] combines them. Advertised facts are
//! hints (capped at `probed` provenance for remote nodes): a fetch still
//! verifies every piece and the whole content.

use std::collections::BTreeMap;
use std::sync::Arc;

use clawft_types::placement::AttrValue;

use crate::mesh_artifact::ArtifactExchange;
use crate::mesh_artifact_types::{ArtifactDescriptor, ArtifactKey, Bitfield};
use crate::mesh_swarm_fetch::PeerDialer;
use crate::mesh_swarm_picker::PeerCandidate;
use crate::node_facts::NodeFactsCache;
use crate::workload_pkg::codec::hex_encode;

/// Capability id carrying a node's LAN for locality.
pub const LAN_CAPABILITY: &str = "net.lan";

/// A peer that answered a `who_has` query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Holder {
    /// Peer node id.
    pub peer: String,
    /// The artifact's descriptor as the peer reports it.
    pub descriptor: ArtifactDescriptor,
    /// Pieces the peer holds.
    pub have: Bitfield,
}

/// Peers whose fresh facts advertise `content_hash` (`store.artifact.*`),
/// excluding `me`, with their LAN. Sorted by node id.
pub fn holders_from_facts(
    cache: &NodeFactsCache,
    content_hash: &[u8; 32],
    now: u64,
    me: &str,
) -> Vec<PeerCandidate> {
    let hex = hex_encode(content_hash);
    let prefix = format!("store.artifact.{}", &hex[..16]);
    cache
        .list(now)
        .into_iter()
        .filter(|c| c.node_id() != me)
        .filter(|c| {
            c.capabilities().iter().any(|cap| {
                cap.id.as_str() == prefix
                    && matches!(cap.attrs.get("content"), Some(AttrValue::Str(s)) if *s == hex)
            })
        })
        .map(|c| {
            let lan = c.capabilities().iter().find_map(|cap| {
                (cap.id.as_str() == LAN_CAPABILITY)
                    .then(|| match cap.attrs.get("lan_id") {
                        Some(AttrValue::Str(s)) => Some(s.clone()),
                        _ => None,
                    })
                    .flatten()
            });
            PeerCandidate {
                peer_id: c.node_id().to_string(),
                lan_id: lan,
            }
        })
        .collect()
}

impl ArtifactExchange {
    /// Ask `peers` whether they hold `key`. Peers that refuse, fail or are
    /// banned are left out. Sorted by peer id.
    pub async fn who_has(
        self: &Arc<Self>,
        dialer: Arc<dyn PeerDialer>,
        peers: &[PeerCandidate],
        key: ArtifactKey,
    ) -> Vec<Holder> {
        let mut set = tokio::task::JoinSet::new();
        for p in peers.iter().filter(|p| !self.is_banned(&p.peer_id)) {
            let (ex, dialer, peer) = (self.clone(), dialer.clone(), p.peer_id.clone());
            set.spawn(async move {
                let (mut s, d) = ex.open_session_for_lookup(dialer.as_ref(), &peer, key).await.ok()?;
                let _ = s.stream.close().await;
                Some(Holder {
                    peer,
                    descriptor: d,
                    have: s.has,
                })
            });
        }
        let mut out = BTreeMap::new();
        while let Some(r) = set.join_next().await {
            if let Ok(Some(h)) = r {
                out.insert(h.peer.clone(), h);
            }
        }
        out.into_values().collect()
    }

    /// Candidates for `key`: peers advertising it in their facts plus
    /// `neighbors` that answer a `who_has` query now.
    pub async fn find_holders(
        self: &Arc<Self>,
        dialer: Arc<dyn PeerDialer>,
        facts: &NodeFactsCache,
        neighbors: &[PeerCandidate],
        key: ArtifactKey,
        now: u64,
    ) -> Vec<PeerCandidate> {
        let mut out: BTreeMap<String, PeerCandidate> = BTreeMap::new();
        if let Some(d) = self.resolve(&key) {
            for c in holders_from_facts(facts, &d.content_hash, now, self.node_id()) {
                out.insert(c.peer_id.clone(), c);
            }
        } else if let ArtifactKey::Content(h) = key {
            for c in holders_from_facts(facts, &h, now, self.node_id()) {
                out.insert(c.peer_id.clone(), c);
            }
        }
        for h in self.who_has(dialer, neighbors, key).await {
            let known = neighbors.iter().find(|n| n.peer_id == h.peer);
            out.entry(h.peer.clone())
                .or_insert_with(|| known.cloned().unwrap_or_else(|| PeerCandidate::new(h.peer)));
        }
        out.into_values().collect()
    }
}
