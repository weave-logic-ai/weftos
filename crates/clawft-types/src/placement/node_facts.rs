//! Node facts: the signed body a node advertises about itself (ADR-099
//! section 2, card mesh-placement-03).
//!
//! [`NodeFacts`] is a list of wave-1 [`Capability`] records (arch, OS,
//! runtimes, accelerators, memory, storage, feeds, measured `perf.*`) plus
//! identity, issue time, TTL and a sequence number. Probing lives in
//! `clawft_kernel::node_facts`; signing and the TTL cache live beside
//! `SignedCapabilityAdvertisement` in the kernel. This module is pure data
//! and validation.
//!
//! A node's live state (a capability going `busy`, free memory changing) is
//! a small [`FactsDelta`] applied on top of the signed base, so the base is
//! re-signed only when the hardware or runtime picture changes.
//!
//! Trust is **not** self-asserted: facts carrying `trust.*` capabilities are
//! rejected. The receiver assigns a [`TrustTier`] (pairing, pinning) when it
//! caches the facts.

use serde::{Deserialize, Serialize};

use super::capability::{AttrValue, Capability, CapabilityId, CapabilityState};
use super::memory::{MEM_SYSTEM, MEM_UNIFIED};

/// Wire version of [`NodeFacts`].
pub const NODE_FACTS_VERSION: u32 = 1;
/// Longest accepted TTL (one day).
pub const MAX_FACTS_TTL_SECS: u64 = 86_400;
/// Shortest accepted TTL.
pub const MIN_FACTS_TTL_SECS: u64 = 10;
/// Most capabilities in one facts block.
pub const MAX_FACTS_CAPABILITIES: usize = 512;
/// Most probe notes in one facts block.
pub const MAX_PROBE_NOTES: usize = 64;
/// Longest probe note field, in bytes.
pub const MAX_NOTE_LEN: usize = 512;
/// Longest node id, in bytes.
pub const MAX_NODE_ID_LEN: usize = 128;
/// Most state changes in one delta.
pub const MAX_DELTA_CHANGES: usize = 64;

/// Why facts or a delta were refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FactsError {
    /// Structural problem (bounds, ids, version).
    #[error("invalid node facts: {0}")]
    Invalid(String),
    /// Facts are past `issued_at + ttl_secs`.
    #[error("node facts expired at {expired_at} (now {now})")]
    Expired {
        /// Expiry, unix seconds.
        expired_at: u64,
        /// Clock used.
        now: u64,
    },
    /// Facts claim an issue time too far in the future.
    #[error("node facts issued in the future ({issued_at} > {now} + skew)")]
    FromFuture {
        /// Claimed issue time.
        issued_at: u64,
        /// Clock used.
        now: u64,
    },
    /// Delta does not fit the cached base.
    #[error("delta rejected: {0}")]
    Delta(String),
}

/// Trust tier the **receiver** assigns to a node (never self-asserted).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustTier {
    /// Seen on the mesh, not paired.
    Discovered,
    /// Paired through the operator pairing window.
    Paired,
    /// Operator-pinned (and the local node itself).
    Pinned,
}

impl TrustTier {
    /// Vocabulary id (`trust.tier.paired`).
    pub fn capability_id(self) -> &'static str {
        match self {
            TrustTier::Discovered => "trust.tier.discovered",
            TrustTier::Paired => "trust.tier.paired",
            TrustTier::Pinned => "trust.tier.pinned",
        }
    }
}

/// A human-readable note from a probe: what was checked, how, and what it
/// could not see. Carried so provenance is explainable, never used to match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeNote {
    /// Probe or capability the note is about (`accel.npu.ane`, `docker`).
    pub probe: String,
    /// The note.
    pub note: String,
}

impl ProbeNote {
    /// Build a note, truncating both fields to [`MAX_NOTE_LEN`].
    pub fn new(probe: impl Into<String>, note: impl Into<String>) -> Self {
        Self {
            probe: clip(probe.into()),
            note: clip(note.into()),
        }
    }
}

fn clip(mut s: String) -> String {
    if s.len() > MAX_NOTE_LEN {
        let mut end = MAX_NOTE_LEN;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
    }
    s
}

/// Load summary derived from capability states.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeLoad {
    /// Capabilities currently `busy` or `reserved`.
    pub busy: usize,
    /// Capabilities in total.
    pub total: usize,
    /// Free bytes in the host (or unified) pool, if advertised.
    pub mem_free: Option<u64>,
}

/// What a node says about itself. Signed as a whole by the node key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeFacts {
    /// Wire version ([`NODE_FACTS_VERSION`]).
    pub version: u32,
    /// Advertising node id.
    pub node_id: String,
    /// Issue time, unix seconds.
    pub issued_at: u64,
    /// Time to live from `issued_at`, seconds.
    pub ttl_secs: u64,
    /// Monotonic per node; a cache refuses a lower `seq` than it holds.
    pub seq: u64,
    /// Advertised capabilities, each with its own provenance.
    pub capabilities: Vec<Capability>,
    /// Probe notes (how each fact was obtained, what was not visible).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<ProbeNote>,
}

impl NodeFacts {
    /// New facts at `seq` with no capabilities.
    pub fn new(node_id: impl Into<String>, issued_at: u64, ttl_secs: u64, seq: u64) -> Self {
        Self {
            version: NODE_FACTS_VERSION,
            node_id: node_id.into(),
            issued_at,
            ttl_secs,
            seq,
            capabilities: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// Advertising node id.
    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    /// Advertised capabilities.
    pub fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }

    /// Capabilities with this exact id.
    pub fn find<'a>(&'a self, id: &'a str) -> impl Iterator<Item = &'a Capability> + 'a {
        self.capabilities
            .iter()
            .filter(move |c| c.id.as_str() == id)
    }

    /// Unix second at which these facts stop being usable.
    pub fn expires_at(&self) -> u64 {
        self.issued_at.saturating_add(self.ttl_secs)
    }

    /// True while `now` is before [`Self::expires_at`].
    pub fn is_fresh(&self, now: u64) -> bool {
        now < self.expires_at()
    }

    /// Busy count and free memory, derived from capability states.
    pub fn load(&self) -> NodeLoad {
        let busy = self
            .capabilities
            .iter()
            .filter(|c| matches!(c.state, CapabilityState::Busy | CapabilityState::Reserved))
            .count();
        let mem = self
            .find(MEM_UNIFIED)
            .next()
            .or_else(|| self.find(MEM_SYSTEM).next());
        NodeLoad {
            busy,
            total: self.capabilities.len(),
            mem_free: mem.and_then(|c| c.attrs.get("free")).and_then(|v| match v {
                AttrValue::Int(i) if *i >= 0 => Some(*i as u64),
                _ => None,
            }),
        }
    }

    /// Structural validation (bounds, version, no self-asserted trust).
    pub fn validate(&self) -> Result<(), FactsError> {
        let bad = |s: &str| Err(FactsError::Invalid(s.to_string()));
        if self.version != NODE_FACTS_VERSION {
            return bad("unsupported version");
        }
        if self.node_id.is_empty() || self.node_id.len() > MAX_NODE_ID_LEN {
            return bad("node_id must be 1..=128 bytes");
        }
        if !self
            .node_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
        {
            return bad("node_id has characters outside [A-Za-z0-9-_.:]");
        }
        if !(MIN_FACTS_TTL_SECS..=MAX_FACTS_TTL_SECS).contains(&self.ttl_secs) {
            return bad("ttl_secs out of range");
        }
        if self.capabilities.len() > MAX_FACTS_CAPABILITIES {
            return bad("too many capabilities");
        }
        if self.notes.len() > MAX_PROBE_NOTES {
            return bad("too many probe notes");
        }
        if self
            .notes
            .iter()
            .any(|n| n.probe.len() > MAX_NOTE_LEN || n.note.len() > MAX_NOTE_LEN)
        {
            return bad("probe note too long");
        }
        for cap in &self.capabilities {
            if cap.id.family() == "trust" {
                return bad("trust.* is assigned by the receiver, not self-asserted");
            }
            cap.validate()
                .map_err(|e| FactsError::Invalid(e.to_string()))?;
        }
        Ok(())
    }

    /// Validate and check the time window: not expired, not issued more
    /// than `max_skew_secs` in the future.
    pub fn check_window(&self, now: u64, max_skew_secs: u64) -> Result<(), FactsError> {
        self.validate()?;
        if self.issued_at > now.saturating_add(max_skew_secs) {
            return Err(FactsError::FromFuture {
                issued_at: self.issued_at,
                now,
            });
        }
        if !self.is_fresh(now) {
            return Err(FactsError::Expired {
                expired_at: self.expires_at(),
                now,
            });
        }
        Ok(())
    }

    /// Apply a live-state delta. The delta must name this node and this
    /// base `seq`; each change must point at a capability with the same
    /// id. All-or-nothing: on error nothing changes.
    pub fn apply_delta(&mut self, d: &FactsDelta) -> Result<(), FactsError> {
        let bad = |s: String| Err(FactsError::Delta(s));
        if d.node_id != self.node_id {
            return bad(format!(
                "delta for {} applied to {}",
                d.node_id, self.node_id
            ));
        }
        if d.base_seq != self.seq {
            return bad(format!(
                "delta base_seq {} != facts seq {}",
                d.base_seq, self.seq
            ));
        }
        if d.changes.len() > MAX_DELTA_CHANGES {
            return bad("too many changes".into());
        }
        for ch in &d.changes {
            match self.capabilities.get(ch.index as usize) {
                Some(cap) if cap.id == ch.id => {}
                _ => return bad(format!("no capability {} at index {}", ch.id, ch.index)),
            }
        }
        for ch in &d.changes {
            self.capabilities[ch.index as usize].state = ch.state;
        }
        if let Some(free) = d.mem_free {
            let free = AttrValue::Int(i64::try_from(free).unwrap_or(i64::MAX));
            for cap in self
                .capabilities
                .iter_mut()
                .filter(|c| matches!(c.id.as_str(), MEM_UNIFIED | MEM_SYSTEM))
            {
                cap.attrs.insert("free".into(), free.clone());
            }
        }
        Ok(())
    }
}

/// One capability's new live state inside a [`FactsDelta`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateChange {
    /// Index into the base `capabilities`.
    pub index: u32,
    /// Id expected at that index (guards against a stale index).
    pub id: CapabilityId,
    /// New state.
    pub state: CapabilityState,
}

/// Small busy/free update on top of signed base facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FactsDelta {
    /// Node the delta is about.
    pub node_id: String,
    /// `seq` of the base facts it applies to.
    pub base_seq: u64,
    /// Delta sequence; strictly increasing per base.
    pub seq: u64,
    /// Issue time, unix seconds.
    pub issued_at: u64,
    /// Capability state changes.
    #[serde(default)]
    pub changes: Vec<StateChange>,
    /// New free bytes for the host/unified pool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mem_free: Option<u64>,
}
