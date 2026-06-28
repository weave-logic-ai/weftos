//! Universal cross-references between forest structures.
//!
//! This module is compiled only when the `ecc` feature is enabled.
//! It provides [`UniversalNodeId`] (BLAKE3-hashed identity for any node),
//! [`CrossRef`] (a typed directed edge between two nodes), and
//! [`CrossRefStore`] (a concurrent forward/reverse index).

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use dashmap::DashMap;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// StructureTag
// ---------------------------------------------------------------------------

/// Identifies which forest structure a node belongs to.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StructureTag {
    /// ExoChain ledger (0x01).
    ExoChain,
    /// Resource tree (0x02).
    ResourceTree,
    /// Causal graph (0x03).
    CausalGraph,
    /// HNSW vector index (0x04).
    HnswIndex,
    /// Domain-specific extension (0x10+).
    Custom(u8),
}

impl StructureTag {
    /// Returns the canonical byte discriminant.
    pub fn as_u8(&self) -> u8 {
        match self {
            Self::ExoChain => 0x01,
            Self::ResourceTree => 0x02,
            Self::CausalGraph => 0x03,
            Self::HnswIndex => 0x04,
            Self::Custom(v) => *v,
        }
    }
}

impl fmt::Display for StructureTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ExoChain => write!(f, "ExoChain"),
            Self::ResourceTree => write!(f, "ResourceTree"),
            Self::CausalGraph => write!(f, "CausalGraph"),
            Self::HnswIndex => write!(f, "HnswIndex"),
            Self::Custom(v) => write!(f, "Custom(0x{v:02x})"),
        }
    }
}

// ---------------------------------------------------------------------------
// UniversalNodeId
// ---------------------------------------------------------------------------

/// A 32-byte BLAKE3 hash that uniquely identifies any node across all
/// forest structures.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UniversalNodeId(pub [u8; 32]);

impl UniversalNodeId {
    /// Derive a deterministic identity by hashing the concatenation of all
    /// constituent fields via BLAKE3.
    pub fn new(
        structure_tag: &StructureTag,
        context_id: &[u8],
        hlc_timestamp: u64,
        content_hash: &[u8],
        parent_id: &[u8],
    ) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&[structure_tag.as_u8()]);
        hasher.update(context_id);
        hasher.update(&hlc_timestamp.to_le_bytes());
        hasher.update(content_hash);
        hasher.update(parent_id);
        Self(*hasher.finalize().as_bytes())
    }

    /// The all-zeros sentinel ID.
    pub fn zero() -> Self {
        Self([0u8; 32])
    }

    /// Construct from a raw 32-byte array.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Borrow the inner bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for UniversalNodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in &self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for UniversalNodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "UniversalNodeId({self})")
    }
}

// ---------------------------------------------------------------------------
// CrossRefType
// ---------------------------------------------------------------------------

/// The semantic relationship carried by a [`CrossRef`].
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CrossRefType {
    /// Source was triggered by target (0x01).
    TriggeredBy,
    /// Source is evidence for target (0x02).
    EvidenceFor,
    /// Source elaborates on target (0x03).
    Elaborates,
    /// Source is the emotional cause of target (0x04).
    EmotionCause,
    /// Source provides goal motivation for target (0x05).
    GoalMotivation,
    /// Scene boundary marker (0x06).
    SceneBoundary,
    /// Memory encoding link (0x09).
    MemoryEncoded,
    /// Theory-of-mind inference (0x0A).
    TomInference,
    /// Domain-specific extension.
    Custom(u8),
}

impl fmt::Display for CrossRefType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TriggeredBy => write!(f, "TriggeredBy"),
            Self::EvidenceFor => write!(f, "EvidenceFor"),
            Self::Elaborates => write!(f, "Elaborates"),
            Self::EmotionCause => write!(f, "EmotionCause"),
            Self::GoalMotivation => write!(f, "GoalMotivation"),
            Self::SceneBoundary => write!(f, "SceneBoundary"),
            Self::MemoryEncoded => write!(f, "MemoryEncoded"),
            Self::TomInference => write!(f, "TomInference"),
            Self::Custom(v) => write!(f, "Custom(0x{v:02x})"),
        }
    }
}

// ---------------------------------------------------------------------------
// CrossRef
// ---------------------------------------------------------------------------

/// A directed, typed cross-reference between two universal nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossRef {
    /// The originating node.
    pub source: UniversalNodeId,
    /// Which structure the source lives in.
    pub source_structure: StructureTag,
    /// The destination node.
    pub target: UniversalNodeId,
    /// Which structure the target lives in.
    pub target_structure: StructureTag,
    /// Semantic relationship type.
    pub ref_type: CrossRefType,
    /// HLC timestamp at creation.
    pub created_at: u64,
    /// ExoChain sequence number for provenance.
    pub chain_seq: u64,
}

// ---------------------------------------------------------------------------
// CrossRefStore
// ---------------------------------------------------------------------------

/// Concurrent forward/reverse index of [`CrossRef`] edges.
pub struct CrossRefStore {
    forward: DashMap<UniversalNodeId, Vec<CrossRef>>,
    reverse: DashMap<UniversalNodeId, Vec<CrossRef>>,
    count: AtomicU64,
}

impl CrossRefStore {
    /// Create an empty store.
    pub fn new() -> Self {
        Self {
            forward: DashMap::new(),
            reverse: DashMap::new(),
            count: AtomicU64::new(0),
        }
    }

    /// Insert a cross-reference, indexing it in both directions.
    pub fn insert(&self, crossref: CrossRef) {
        self.forward
            .entry(crossref.source.clone())
            .or_default()
            .push(crossref.clone());
        self.reverse
            .entry(crossref.target.clone())
            .or_default()
            .push(crossref);
        self.count.fetch_add(1, Ordering::Relaxed);
    }

    /// All cross-refs originating from `id`.
    pub fn get_forward(&self, id: &UniversalNodeId) -> Vec<CrossRef> {
        self.forward
            .get(id)
            .map(|v| v.value().clone())
            .unwrap_or_default()
    }

    /// All cross-refs pointing *to* `id`.
    pub fn get_reverse(&self, id: &UniversalNodeId) -> Vec<CrossRef> {
        self.reverse
            .get(id)
            .map(|v| v.value().clone())
            .unwrap_or_default()
    }

    /// All cross-refs where `id` appears as source **or** target.
    pub fn get_all(&self, id: &UniversalNodeId) -> Vec<CrossRef> {
        let mut out = self.get_forward(id);
        out.extend(self.get_reverse(id));
        out
    }

    /// Total number of cross-refs inserted.
    pub fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    /// Filter cross-refs involving `id` (either direction) by relationship type.
    pub fn by_type(&self, id: &UniversalNodeId, ref_type: &CrossRefType) -> Vec<CrossRef> {
        self.get_all(id)
            .into_iter()
            .filter(|cr| &cr.ref_type == ref_type)
            .collect()
    }

    /// Remove all entries (useful for calibration resets).
    pub fn clear(&self) {
        self.forward.clear();
        self.reverse.clear();
        self.count.store(0, Ordering::Relaxed);
    }
}

impl Default for CrossRefStore {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_id(tag: &StructureTag, ctx: &[u8], ts: u64) -> UniversalNodeId {
        UniversalNodeId::new(tag, ctx, ts, b"hash", b"parent")
    }

    fn sample_crossref(src: UniversalNodeId, tgt: UniversalNodeId, rt: CrossRefType) -> CrossRef {
        CrossRef {
            source: src,
            source_structure: StructureTag::ExoChain,
            target: tgt,
            target_structure: StructureTag::ResourceTree,
            ref_type: rt,
            created_at: 1000,
            chain_seq: 42,
        }
    }

    #[test]
    fn universal_node_id_creation() {
        let id = make_id(&StructureTag::ExoChain, b"ctx", 1);
        assert_ne!(id.as_bytes(), &[0u8; 32]);
    }

    #[test]
    fn universal_node_id_deterministic() {
        let a = make_id(&StructureTag::ExoChain, b"ctx", 1);
        let b = make_id(&StructureTag::ExoChain, b"ctx", 1);
        assert_eq!(a, b);

        let c = make_id(&StructureTag::ExoChain, b"ctx", 2);
        assert_ne!(a, c);
    }

    #[test]
    fn universal_node_id_display_hex() {
        let id = UniversalNodeId::zero();
        let s = format!("{id}");
        assert_eq!(s.len(), 64);
        assert!(s.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(s, "0".repeat(64));
    }

    #[test]
    fn universal_node_id_zero() {
        let z = UniversalNodeId::zero();
        assert_eq!(z.as_bytes(), &[0u8; 32]);
        assert_eq!(z, UniversalNodeId::from_bytes([0u8; 32]));
    }

    #[test]
    fn structure_tag_as_u8() {
        assert_eq!(StructureTag::ExoChain.as_u8(), 0x01);
        assert_eq!(StructureTag::ResourceTree.as_u8(), 0x02);
        assert_eq!(StructureTag::CausalGraph.as_u8(), 0x03);
        assert_eq!(StructureTag::HnswIndex.as_u8(), 0x04);
        assert_eq!(StructureTag::Custom(0x10).as_u8(), 0x10);
    }

    #[test]
    fn structure_tag_display() {
        assert_eq!(StructureTag::ExoChain.to_string(), "ExoChain");
        assert_eq!(StructureTag::ResourceTree.to_string(), "ResourceTree");
        assert_eq!(StructureTag::Custom(0x10).to_string(), "Custom(0x10)");
    }

    #[test]
    fn crossref_type_display() {
        assert_eq!(CrossRefType::TriggeredBy.to_string(), "TriggeredBy");
        assert_eq!(CrossRefType::EvidenceFor.to_string(), "EvidenceFor");
        assert_eq!(CrossRefType::TomInference.to_string(), "TomInference");
        assert_eq!(CrossRefType::Custom(0xff).to_string(), "Custom(0xff)");
    }

    #[test]
    fn crossref_store_insert_and_get_forward() {
        let store = CrossRefStore::new();
        let src = make_id(&StructureTag::ExoChain, b"a", 1);
        let tgt = make_id(&StructureTag::ResourceTree, b"b", 2);
        store.insert(sample_crossref(src.clone(), tgt, CrossRefType::TriggeredBy));

        let fwd = store.get_forward(&src);
        assert_eq!(fwd.len(), 1);
        assert_eq!(fwd[0].ref_type, CrossRefType::TriggeredBy);
    }

    #[test]
    fn crossref_store_insert_and_get_reverse() {
        let store = CrossRefStore::new();
        let src = make_id(&StructureTag::ExoChain, b"a", 1);
        let tgt = make_id(&StructureTag::ResourceTree, b"b", 2);
        store.insert(sample_crossref(src, tgt.clone(), CrossRefType::EvidenceFor));

        let rev = store.get_reverse(&tgt);
        assert_eq!(rev.len(), 1);
        assert_eq!(rev[0].ref_type, CrossRefType::EvidenceFor);
    }

    #[test]
    fn crossref_store_get_all_both_directions() {
        let store = CrossRefStore::new();
        let a = make_id(&StructureTag::ExoChain, b"a", 1);
        let b = make_id(&StructureTag::ResourceTree, b"b", 2);
        let c = make_id(&StructureTag::CausalGraph, b"c", 3);

        // a -> b
        store.insert(sample_crossref(
            a.clone(),
            b.clone(),
            CrossRefType::TriggeredBy,
        ));
        // c -> a
        store.insert(sample_crossref(c, a.clone(), CrossRefType::Elaborates));

        let all = store.get_all(&a);
        assert_eq!(all.len(), 2);
        assert_eq!(store.count(), 2);
    }

    #[test]
    fn crossref_store_by_type() {
        let store = CrossRefStore::new();
        let a = make_id(&StructureTag::ExoChain, b"a", 1);
        let b = make_id(&StructureTag::ResourceTree, b"b", 2);
        let c = make_id(&StructureTag::CausalGraph, b"c", 3);

        store.insert(sample_crossref(a.clone(), b, CrossRefType::TriggeredBy));
        store.insert(sample_crossref(a.clone(), c, CrossRefType::EvidenceFor));

        let triggered = store.by_type(&a, &CrossRefType::TriggeredBy);
        assert_eq!(triggered.len(), 1);

        let evidence = store.by_type(&a, &CrossRefType::EvidenceFor);
        assert_eq!(evidence.len(), 1);

        let none = store.by_type(&a, &CrossRefType::SceneBoundary);
        assert!(none.is_empty());
    }

    #[test]
    fn crossref_store_clear() {
        let store = CrossRefStore::new();
        let a = make_id(&StructureTag::ExoChain, b"a", 1);
        let b = make_id(&StructureTag::ResourceTree, b"b", 2);
        store.insert(sample_crossref(a.clone(), b, CrossRefType::TriggeredBy));
        assert_eq!(store.count(), 1);

        store.clear();
        assert_eq!(store.count(), 0);
        assert!(store.get_forward(&a).is_empty());
    }
}
