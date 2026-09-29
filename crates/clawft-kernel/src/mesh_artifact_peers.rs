//! Peer-set and scheduling abstractions for artifact fetches
//! (mesh-placement-11). v1 uses one peer at a time with
//! [`SequentialScheduler`]; card 25 adds multi-source scheduling here.

use crate::mesh::MeshStream;
use crate::mesh_artifact::ExchangeError;
use crate::mesh_artifact_wire::{ArtifactDescriptor, ArtifactId, Bitfield};

/// A connection to one peer that may hold the artifact.
pub struct PeerLink {
    /// Peer node id (for audit).
    pub peer_id: String,
    /// Open stream to the peer's [`ArtifactExchange::serve`] loop.
    pub stream: Box<dyn MeshStream>,
    pub(crate) dead: bool,
}

impl PeerLink {
    /// Link to `peer_id` over `stream`.
    pub fn new(peer_id: impl Into<String>, stream: Box<dyn MeshStream>) -> Self {
        Self {
            peer_id: peer_id.into(),
            stream,
            dead: false,
        }
    }

    /// True after a transport or protocol failure on this link.
    pub fn is_dead(&self) -> bool {
        self.dead
    }
}

/// Candidate sources for a fetch. v1 uses them one at a time, in order.
#[derive(Default)]
pub struct PeerSet {
    pub(crate) links: Vec<PeerLink>,
}

impl PeerSet {
    /// Empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set with one peer.
    pub fn single(link: PeerLink) -> Self {
        Self { links: vec![link] }
    }

    /// Add a peer.
    pub fn push(&mut self, link: PeerLink) {
        self.links.push(link);
    }

    /// Number of peers.
    pub fn len(&self) -> usize {
        self.links.len()
    }

    /// True when there are no peers.
    pub fn is_empty(&self) -> bool {
        self.links.is_empty()
    }

    /// Mutable access, for callers that manage links themselves.
    pub fn links_mut(&mut self) -> &mut [PeerLink] {
        &mut self.links
    }
}

/// Chooses which pieces to request next from a peer.
pub trait PieceScheduler: Send {
    /// Up to `max` indexes that are in `need` (clear bits = still needed)
    /// and set in `peer_has`.
    fn next_batch(&mut self, need: &Bitfield, peer_has: &Bitfield, max: usize) -> Vec<u32>;
}

/// v1 scheduler: lowest missing index first.
#[derive(Debug, Default, Clone, Copy)]
pub struct SequentialScheduler;

impl PieceScheduler for SequentialScheduler {
    fn next_batch(&mut self, need: &Bitfield, peer_has: &Bitfield, max: usize) -> Vec<u32> {
        need.missing()
            .filter(|&i| peer_has.get(i))
            .take(max)
            .collect()
    }
}

/// What one fetch achieved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchOutcome {
    /// Artifact id.
    pub id: ArtifactId,
    /// Its descriptor.
    pub descriptor: ArtifactDescriptor,
    /// Pieces accepted during this fetch (excludes those already held).
    pub pieces_fetched: u32,
    /// Piece bytes accepted during this fetch.
    pub bytes_fetched: u64,
    /// Pieces rejected for a bad hash.
    pub pieces_rejected: u32,
    /// Peers that supplied at least one accepted piece.
    pub sources: Vec<String>,
}

/// Why a fetch failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FetchError {
    /// No peer could supply every piece.
    #[error("fetch incomplete: {missing} pieces missing after all peers ({last_error})")]
    Incomplete {
        /// Pieces still missing (descriptor-less fetches report 0).
        missing: u32,
        /// Last peer error seen.
        last_error: String,
    },
    /// Pieces assembled but the whole-content hash did not match.
    #[error("fetched content failed verification: {0}")]
    Verification(String),
    /// Local failure.
    #[error(transparent)]
    Local(#[from] ExchangeError),
}

/// Per-connection counters returned by [`ArtifactExchange::serve`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServeStats {
    /// Piece indexes sent, in order (repeats included).
    pub pieces_served: Vec<u32>,
    /// Piece bytes sent.
    pub bytes_served: u64,
}
