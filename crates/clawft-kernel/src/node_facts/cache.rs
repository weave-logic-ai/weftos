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

use clawft_types::placement::{Capability, FactsError, NodeFacts, NodeLoad, TrustTier};

use crate::node_facts_advert::{
    NodeFactsAdvertError, SignedFactsDelta, SignedNodeFacts, verify_facts_delta, verify_node_facts,
};

/// Most nodes held.
pub const MAX_CACHED_NODES: usize = 4096;

/// One cached, verified facts block plus what the receiver knows about it.
#[derive(Debug, Clone, PartialEq)]
pub struct CachedNodeFacts {
    /// Current facts (base plus applied deltas).
    pub facts: NodeFacts,
    /// The signed base as received (re-shareable, re-verifiable).
    pub signed: SignedNodeFacts,
    /// Trust tier assigned by this node.
    pub trust_tier: TrustTier,
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
        let facts = verify_node_facts(&signed, now)?;
        let mut public_key = [0u8; 32];
        public_key.copy_from_slice(&signed.public_key);
        let node_id = facts.node_id.clone();
        let entry = CachedNodeFacts {
            facts,
            signed,
            trust_tier,
            received_at: now,
            delta_seq: 0,
            public_key,
        };
        // Expired entries do not block a node that restarted its seq.
        if let Some(held) = self.entries.get(&node_id).filter(|e| e.is_fresh(now)) {
            if held.signed.payload == entry.signed.payload {
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
        if !self.entries.contains_key(&node_id) && self.entries.len() >= MAX_CACHED_NODES {
            self.evict_expired(now);
            if self.entries.len() >= MAX_CACHED_NODES {
                return Err(CacheError::Full);
            }
        }
        Ok(match self.entries.insert(node_id, entry) {
            Some(_) => InsertOutcome::Replaced,
            None => InsertOutcome::Added,
        })
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
