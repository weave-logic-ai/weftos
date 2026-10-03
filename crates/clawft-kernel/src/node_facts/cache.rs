//! TTL cache of verified node facts, held by
//! [`crate::cluster::ClusterMembership`] (ADR-099 section 2).
//!
//! Only verified facts enter. An entry is served while its facts are
//! within TTL; expired entries are invisible to readers and removed by
//! [`NodeFactsCache::evict_expired`]. A node's facts are replaced only by a
//! higher `seq` (or the same `seq` re-issued with identical content, which
//! refreshes nothing), so a replayed older block cannot roll a node back.
//! Deltas must be signed by the same key as the cached base, name its
//! `seq`, and carry a strictly increasing delta `seq`.

use dashmap::DashMap;

use clawft_types::placement::{
    Capability, FactsError, NodeFacts, NodeLoad, Provenance, TrustTier,
};

use crate::node_facts_advert::{
    NodeFactsAdvertError, SignedFactsDelta, SignedNodeFacts, verify_facts_delta, verify_node_facts,
};

/// Most nodes held.
pub const MAX_CACHED_NODES: usize = 4096;
/// Most mesh-derived `Discovered` entries held at once, whatever the total.
pub const MAX_DISCOVERED: usize = 512;

/// Where a node's tier came from. Only an operator-set tier is sticky: a
/// mesh-derived tier follows the current connection (admission) every frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TierSource {
    /// Derived from how the node's connection was admitted.
    Mesh,
    /// Set by the operator (or the local node itself).
    Operator,
}

/// One cached, verified facts block plus what the receiver knows about it.
#[derive(Debug, Clone, PartialEq)]
pub struct CachedNodeFacts {
    /// Current facts (base plus applied deltas).
    pub facts: NodeFacts,
    /// The signed base as received (re-shareable, re-verifiable).
    pub signed: SignedNodeFacts,
    /// Trust tier assigned by this node.
    pub trust_tier: TrustTier,
    /// Where the tier came from.
    pub tier_source: TierSource,
    /// Connection that supplied the facts (0 = none), for per-connection quotas.
    pub origin: u64,
    /// When this node accepted the base, unix seconds.
    pub received_at: u64,
    /// Last applied delta `seq` (0 = none).
    pub delta_seq: u64,
    /// Signing key of the node.
    pub public_key: [u8; 32],
}

impl CachedNodeFacts {
    /// Node id.
    pub fn node_id(&self) -> &str {
        self.facts.node_id()
    }
    /// Advertised capabilities.
    pub fn capabilities(&self) -> &[Capability] {
        self.facts.capabilities()
    }
    /// Receiver-assigned trust tier.
    pub fn trust_tier(&self) -> TrustTier {
        self.trust_tier
    }
    /// Busy count and free memory.
    pub fn load(&self) -> NodeLoad {
        self.facts.load()
    }
    /// Facts still within TTL at `now`.
    pub fn is_fresh(&self, now: u64) -> bool {
        self.facts.is_fresh(now)
    }
}

/// What an insert did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertOutcome {
    /// First facts for this node.
    Added,
    /// Replaced older facts.
    Replaced,
    /// Same `seq` and content as held; nothing changed.
    Unchanged,
}

/// Why the cache refused something.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CacheError {
    /// Signature, binding, structure or TTL failure.
    #[error(transparent)]
    Verify(#[from] NodeFactsAdvertError),
    /// Older (or conflicting same-`seq`) facts than those held.
    #[error("stale facts for {node_id}: seq {got} <= held {held}")]
    Stale {
        /// Node.
        node_id: String,
        /// Offered seq.
        got: u64,
        /// Held seq.
        held: u64,
    },
    /// No fresh base for a delta.
    #[error("no fresh facts cached for {0}")]
    UnknownNode(String),
    /// Delta signed by a different key than the base.
    #[error("delta key does not match the cached facts key")]
    KeyMismatch,
    /// Cache at [`MAX_CACHED_NODES`].
    #[error("node facts cache full")]
    Full,
}

/// Verified node facts with TTL.
#[derive(Debug, Default)]
pub struct NodeFactsCache {
    entries: DashMap<String, CachedNodeFacts>,
}

impl NodeFactsCache {
    /// Empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Verify and cache a signed facts block with a receiver-assigned tier.
    pub fn insert(
        &self,
        signed: SignedNodeFacts,
        trust_tier: TrustTier,
        now: u64,
    ) -> Result<InsertOutcome, CacheError> {
        self.insert_capped(signed, trust_tier, now, None, 0)
    }

    /// [`Self::insert`] for facts a *peer* sent. Every capability's
    /// provenance is capped at `max_provenance` in the cached copy: the
    /// receiver did not probe or measure remote data, so a peer's
    /// `measured` claim is never held as `measured`. The signed envelope is
    /// kept as received.
    ///
    /// `mesh_tier` is derived from the connection that sent the facts and is
    /// re-derived on every frame; a tier the operator set is kept. `origin`
    /// names the connection (see [`Self::discovered_from`]). When the cache
    /// is full, the oldest mesh-derived `Discovered` entry makes room before
    /// the insert is refused, so `Paired` and `Pinned` entries are never
    /// pushed out by `Discovered` ones.
    pub fn insert_remote(
        &self,
        signed: SignedNodeFacts,
        mesh_tier: TrustTier,
        max_provenance: Provenance,
        origin: u64,
        now: u64,
    ) -> Result<InsertOutcome, CacheError> {
        self.insert_capped(signed, mesh_tier, now, Some(max_provenance), origin)
    }

    fn insert_capped(
        &self,
        signed: SignedNodeFacts,
        trust_tier: TrustTier,
        now: u64,
        cap: Option<Provenance>,
        origin: u64,
    ) -> Result<InsertOutcome, CacheError> {
        let remote = cap.is_some();
        let mut facts = verify_node_facts(&signed, now)?;
        if let Some(cap) = cap {
            for c in &mut facts.capabilities {
                c.provenance = c.provenance.min(cap);
            }
        }
        let mut public_key = [0u8; 32];
        public_key.copy_from_slice(&signed.public_key);
        let node_id = facts.node_id.clone();
        let mut entry = CachedNodeFacts {
            facts,
            signed,
            trust_tier,
            tier_source: if remote { TierSource::Mesh } else { TierSource::Operator },
            origin,
            received_at: now,
            delta_seq: 0,
            public_key,
        };
        // Capacity first, for a node not held yet (soft: a racing insert of
        // another node may still land between this check and the write).
        if !self.entries.contains_key(&node_id) {
            // Unverified claims are capped as a class, so they cannot crowd
            // out the cache; the heaviest connection pays first.
            if remote
                && entry.tier_source == TierSource::Mesh
                && entry.trust_tier == TrustTier::Discovered
                && self.count_discovered() >= MAX_DISCOVERED
                && !self.evict_discovered_by_weight()
            {
                return Err(CacheError::Full);
            }
            if self.entries.len() >= MAX_CACHED_NODES {
                self.evict_expired(now);
                if self.entries.len() >= MAX_CACHED_NODES && !self.evict_discovered_by_weight() {
                    return Err(CacheError::Full);
                }
            }
        }
        // The held-versus-new decision and the write happen under the entry's
        // lock, so two concurrent inserts for one node cannot both pass the
        // staleness check and let the older `seq` land last.
        use dashmap::mapref::entry::Entry;
        match self.entries.entry(node_id.clone()) {
            Entry::Vacant(v) => {
                v.insert(entry);
                Ok(InsertOutcome::Added)
            }
            Entry::Occupied(mut o) => {
                // Expired entries do not block a node that restarted its seq.
                if o.get().is_fresh(now) {
                    let held = o.get_mut();
                    if remote && held.tier_source == TierSource::Operator {
                        entry.trust_tier = held.trust_tier;
                        entry.tier_source = TierSource::Operator;
                    }
                    if held.signed.payload == entry.signed.payload {
                        if remote && held.tier_source == TierSource::Mesh {
                            // Same facts, but the connection's tier is current.
                            held.trust_tier = trust_tier;
                            held.origin = origin;
                        }
                        return Ok(InsertOutcome::Unchanged);
                    }
                    if entry.facts.seq <= held.facts.seq {
                        return Err(CacheError::Stale {
                            node_id,
                            got: entry.facts.seq,
                            held: held.facts.seq,
                        });
                    }
                }
                o.insert(entry);
                Ok(InsertOutcome::Replaced)
            }
        }
    }

    fn count_discovered(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.tier_source == TierSource::Mesh && e.trust_tier == TrustTier::Discovered)
            .count()
    }

    /// Drop one mesh-derived `Discovered` entry, chosen by weight: the oldest
    /// entry of the connection that holds the most of them, so a connection
    /// that fills its quota loses its own entries before anyone else's, and a
    /// peer with one entry outlives an attacker with several. (A connection
    /// that reconnects gets a fresh id and a fresh quota; connections with
    /// equally many entries lose the oldest first. There is no remote address
    /// to charge: see the swarm doc.) False if there is none to drop.
    fn evict_discovered_by_weight(&self) -> bool {
        let mut by_origin: std::collections::HashMap<u64, (usize, u64, String)> = Default::default();
        for e in self
            .entries
            .iter()
            .filter(|e| e.tier_source == TierSource::Mesh && e.trust_tier == TrustTier::Discovered)
        {
            // Entries with no connection (origin 0) each count as their own group.
            let group = if e.origin == 0 { u64::MAX - (by_origin.len() as u64) } else { e.origin };
            let slot = by_origin
                .entry(group)
                .or_insert((0, u64::MAX, String::new()));
            slot.0 += 1;
            if (e.received_at, e.key().clone()) < (slot.1, slot.2.clone()) || slot.2.is_empty() {
                slot.1 = e.received_at;
                slot.2 = e.key().clone();
            }
        }
        let victim = by_origin
            .values()
            .max_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)))
            .map(|v| v.2.clone());
        match victim {
            Some(k) => self.entries.remove(&k).is_some(),
            None => false,
        }
    }

    /// Fresh mesh-derived `Discovered` entries that connection `origin` supplied.
    pub fn discovered_from(&self, origin: u64, now: u64) -> usize {
        if origin == 0 {
            return 0;
        }
        self.entries
            .iter()
            .filter(|e| {
                e.origin == origin
                    && e.tier_source == TierSource::Mesh
                    && e.trust_tier == TrustTier::Discovered
                    && e.is_fresh(now)
            })
            .count()
    }

    /// Lower a mesh-derived tier to `Discovered` (the node's admission was
    /// revoked or its verified connection is gone). An operator-set tier is
    /// left alone. True if the tier changed.
    pub fn demote_mesh_tier(&self, node_id: &str) -> bool {
        match self.entries.get_mut(node_id) {
            Some(mut e)
                if e.tier_source == TierSource::Mesh && e.trust_tier != TrustTier::Discovered =>
            {
                e.trust_tier = TrustTier::Discovered;
                true
            }
            _ => false,
        }
    }

    /// Verify a signed delta and apply it to the cached base.
    pub fn apply_delta(&self, signed: &SignedFactsDelta, now: u64) -> Result<(), CacheError> {
        let (delta, pk) = verify_facts_delta(signed, now)?;
        let mut entry = self
            .entries
            .get_mut(&delta.node_id)
            .filter(|e| e.is_fresh(now))
            .ok_or_else(|| CacheError::UnknownNode(delta.node_id.clone()))?;
        if entry.public_key != pk {
            return Err(CacheError::KeyMismatch);
        }
        if delta.seq <= entry.delta_seq {
            return Err(NodeFactsAdvertError::Facts(FactsError::Delta(format!(
                "delta seq {} <= applied {}",
                delta.seq, entry.delta_seq
            )))
            .into());
        }
        entry
            .facts
            .apply_delta(&delta)
            .map_err(NodeFactsAdvertError::Facts)?;
        entry.delta_seq = delta.seq;
        Ok(())
    }

    /// Re-assign the receiver-side trust tier of a held node (an operator
    /// changed it; the signed facts are unchanged). False if not held.
    pub fn set_trust_tier(&self, node_id: &str, trust_tier: TrustTier) -> bool {
        match self.entries.get_mut(node_id) {
            Some(mut e) => {
                e.trust_tier = trust_tier;
                e.tier_source = TierSource::Operator;
                true
            }
            None => false,
        }
    }

    /// Fresh facts for one node.
    pub fn get(&self, node_id: &str, now: u64) -> Option<CachedNodeFacts> {
        self.entries
            .get(node_id)
            .filter(|e| e.is_fresh(now))
            .map(|e| e.clone())
    }

    /// All fresh facts, sorted by node id.
    pub fn list(&self, now: u64) -> Vec<CachedNodeFacts> {
        let mut v: Vec<CachedNodeFacts> = self
            .entries
            .iter()
            .filter(|e| e.is_fresh(now))
            .map(|e| e.clone())
            .collect();
        v.sort_by(|a, b| a.node_id().cmp(b.node_id()));
        v
    }

    /// Drop expired entries; returns how many were removed.
    pub fn evict_expired(&self, now: u64) -> usize {
        let before = self.entries.len();
        self.entries.retain(|_, e| e.is_fresh(now));
        before - self.entries.len()
    }

    /// Entries held (fresh or not yet evicted).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True if nothing is held.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
#[path = "cache_tests.rs"]
mod tests;
