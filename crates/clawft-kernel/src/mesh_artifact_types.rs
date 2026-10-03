//! Shared types of the artifact piece protocol (mesh-placement-11):
//! ids, keys, the `have` bitfield, the piece-list descriptor, limits and
//! exchange configuration. See [`crate::mesh_artifact_wire`] for the codec.

use std::time::Duration;

use crate::mesh::MeshError;
use crate::workload_pkg::codec::{hex_decode_exact, hex_encode};
use crate::workload_pkg::verify::MAX_FILE_BYTES;

/// Default piece size (ADR-099 section 6 placeholder).
pub const DEFAULT_PIECE_SIZE: u64 = 16 * 1024 * 1024;
/// Largest piece size a node accepts from a peer by default: with at most
/// `max_sources` pieces buffered at once, memory is bounded by their product.
pub const DEFAULT_MAX_PIECE_SIZE: u64 = 16 * 1024 * 1024;
/// Largest artifact a node accepts by default (a caller that knows the size
/// from a signed manifest passes the exact size instead).
pub const DEFAULT_MAX_ARTIFACT_BYTES: u64 = 64 * 1024 * 1024 * 1024;
/// Smallest accepted piece size.
pub const MIN_PIECE_SIZE: u64 = 1024;
/// Largest accepted piece size.
pub const MAX_PIECE_SIZE: u64 = 1024 * 1024 * 1024;
/// Default block size for piece data on the wire.
pub const DEFAULT_BLOCK_SIZE: usize = 1024 * 1024;
/// Per-frame payload cap for 0x0B / 0x0C frames.
pub const MAX_ARTIFACT_FRAME: usize = 2 * 1024 * 1024;
/// Most piece indexes in one `request`.
pub const MAX_REQUEST_PIECES: usize = 1024;
/// Longest `reject` reason.
pub const MAX_REASON_BYTES: usize = 256;

const ROOT_DOMAIN: &[u8] = b"weftos.artifact.v1\0";

/// Wire-level failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WireError {
    /// Frame payload exceeds [`MAX_ARTIFACT_FRAME`].
    #[error("artifact frame too large: {size} bytes (max {max})")]
    FrameTooLarge {
        /// Payload size.
        size: usize,
        /// Cap.
        max: usize,
    },
    /// Structurally invalid frame or message.
    #[error("malformed artifact frame: {0}")]
    Malformed(String),
}

impl From<WireError> for MeshError {
    fn from(e: WireError) -> Self {
        match e {
            WireError::FrameTooLarge { size, max } => MeshError::MessageTooLarge { size, max },
            WireError::Malformed(m) => MeshError::Transport(m),
        }
    }
}

pub(crate) fn bad(msg: impl Into<String>) -> WireError {
    WireError::Malformed(msg.into())
}

/// Content-derived artifact id: root hash over the ordered piece list
/// (see [`ArtifactDescriptor::id`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ArtifactId(pub [u8; 32]);

impl ArtifactId {
    /// Parse 64 lower-case hex characters.
    pub fn from_hex(s: &str) -> Option<Self> {
        hex_decode_exact::<32>(s).map(Self)
    }
}

impl std::fmt::Display for ArtifactId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&hex_encode(&self.0))
    }
}

/// How a fetch names what it wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArtifactKey {
    /// By artifact id (piece-list root).
    Root(ArtifactId),
    /// By whole-content BLAKE3, as pinned in a signed manifest `FileRef`.
    Content([u8; 32]),
}

impl std::fmt::Display for ArtifactKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Root(id) => write!(f, "root:{id}"),
            Self::Content(h) => write!(f, "content:{}", hex_encode(h)),
        }
    }
}

/// One bit per piece: set when the piece is held and verified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bitfield {
    pub(crate) len: u32,
    pub(crate) bits: Vec<u8>,
}

impl Bitfield {
    /// All-clear bitfield for `len` pieces.
    pub fn new(len: u32) -> Self {
        Self {
            len,
            bits: vec![0; (len as usize).div_ceil(8)],
        }
    }

    /// Number of pieces covered.
    pub fn len(&self) -> u32 {
        self.len
    }

    /// True when it covers no pieces.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Whether piece `i` is set.
    pub fn get(&self, i: u32) -> bool {
        i < self.len && self.bits[(i / 8) as usize] & (0x80 >> (i % 8)) != 0
    }

    /// Set or clear piece `i` (out-of-range indexes are ignored).
    pub fn set(&mut self, i: u32, v: bool) {
        if i >= self.len {
            return;
        }
        let mask = 0x80 >> (i % 8);
        let byte = &mut self.bits[(i / 8) as usize];
        if v {
            *byte |= mask;
        } else {
            *byte &= !mask;
        }
    }

    /// Number of set pieces.
    pub fn count(&self) -> u32 {
        (0..self.len).filter(|&i| self.get(i)).count() as u32
    }

    /// True when every piece is set.
    pub fn is_complete(&self) -> bool {
        self.count() == self.len
    }

    /// Indexes of pieces not set.
    pub fn missing(&self) -> impl Iterator<Item = u32> + '_ {
        (0..self.len).filter(|&i| !self.get(i))
    }
}

/// The piece list that defines an artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactDescriptor {
    /// Fixed piece size (the last piece may be shorter).
    pub piece_size: u64,
    /// Total content length in bytes.
    pub total_size: u64,
    /// BLAKE3 of the whole content, checked at the end of a fetch.
    pub content_hash: [u8; 32],
    /// BLAKE3 of each piece, in order.
    pub pieces: Vec<[u8; 32]>,
}

impl ArtifactDescriptor {
    /// Root hash over `(piece_size, total_size, content_hash, piece
    /// hashes)`. The content hash is bound in so that one id names exactly
    /// one claim about what the pieces assemble to: a peer cannot reuse an
    /// honest id with a false content hash.
    pub fn id(&self) -> ArtifactId {
        let mut h = blake3::Hasher::new();
        h.update(ROOT_DOMAIN);
        h.update(&self.piece_size.to_be_bytes());
        h.update(&self.total_size.to_be_bytes());
        h.update(&self.content_hash);
        for p in &self.pieces {
            h.update(p);
        }
        ArtifactId(*h.finalize().as_bytes())
    }

    /// Number of pieces.
    pub fn piece_count(&self) -> u32 {
        self.pieces.len() as u32
    }

    /// Length of piece `i` in bytes.
    pub fn piece_len(&self, i: u32) -> u64 {
        let start = u64::from(i) * self.piece_size;
        self.total_size.saturating_sub(start).min(self.piece_size)
    }

    /// Whole-content hash as hex (the `ArtifactStore` key of the content).
    pub fn content_hex(&self) -> String {
        hex_encode(&self.content_hash)
    }

    /// Check the piece size and that the piece count matches the size.
    pub fn validate(&self) -> Result<(), WireError> {
        if !(MIN_PIECE_SIZE..=MAX_PIECE_SIZE).contains(&self.piece_size) {
            return Err(bad(format!("piece size {} out of range", self.piece_size)));
        }
        let expected = self.total_size.div_ceil(self.piece_size);
        if expected != self.pieces.len() as u64 || expected > u64::from(u32::MAX) {
            return Err(bad(format!(
                "{} pieces listed for {} bytes",
                self.pieces.len(),
                self.total_size
            )));
        }
        Ok(())
    }
}

/// Room for the `piece` header inside one frame.
const PIECE_HEADER_BYTES: usize = 1 + 32 + 4 + 8 + 4;

/// Tunables for one node's exchange.
#[derive(Debug, Clone)]
pub struct ExchangeConfig {
    /// Piece size for artifacts seeded here.
    pub piece_size: u64,
    /// Bytes of piece data per `piece` frame.
    pub block_size: usize,
    /// Pieces requested per round trip.
    pub request_window: usize,
    /// Bad copies of one piece accepted from one peer before giving up
    /// on that piece from that peer.
    pub max_piece_retries: u32,
    /// Completed artifacts up to this size are also stored whole under
    /// their content hash, so package verification can read them.
    pub materialize_limit: u64,
    /// Longest wait for one frame from a peer.
    pub recv_timeout: Duration,
    /// Longest a serve session waits for the fetcher's next request
    /// before closing the stream.
    pub serve_idle_timeout: Duration,
    /// Cap on bytes sent to peers per second (`None` = unlimited).
    pub upload_bytes_per_sec: Option<u64>,
    /// Cap on bytes received from peers per second (`None` = unlimited).
    pub download_bytes_per_sec: Option<u64>,
    /// Most peers a swarm fetch pulls from at once.
    pub max_sources: usize,
    /// Corrupt pieces from one peer before it is banned (1 = first).
    pub ban_after_corrupt: u32,
    /// Largest piece size accepted in a peer's descriptor. A descriptor
    /// above it is refused before any piece is requested.
    pub max_piece_size: u64,
    /// Largest total size accepted in a peer's descriptor (an exact size
    /// from a signed manifest, when the caller has one, is checked as well).
    pub max_artifact_bytes: u64,
}

impl Default for ExchangeConfig {
    fn default() -> Self {
        Self {
            piece_size: DEFAULT_PIECE_SIZE,
            block_size: DEFAULT_BLOCK_SIZE,
            request_window: 4,
            max_piece_retries: 3,
            materialize_limit: MAX_FILE_BYTES,
            recv_timeout: Duration::from_secs(30),
            serve_idle_timeout: Duration::from_secs(120),
            upload_bytes_per_sec: None,
            download_bytes_per_sec: None,
            max_sources: 4,
            ban_after_corrupt: 1,
            max_piece_size: DEFAULT_MAX_PIECE_SIZE,
            max_artifact_bytes: DEFAULT_MAX_ARTIFACT_BYTES,
        }
    }
}

impl ExchangeConfig {
    /// Check sizes against the protocol limits.
    pub fn validate(&self) -> Result<(), ExchangeError> {
        if !(MIN_PIECE_SIZE..=MAX_PIECE_SIZE).contains(&self.piece_size) {
            return Err(ExchangeError::Config("piece_size out of range".into()));
        }
        if self.block_size == 0 || self.block_size + PIECE_HEADER_BYTES > MAX_ARTIFACT_FRAME {
            return Err(ExchangeError::Config(
                "block_size must fit one artifact frame".into(),
            ));
        }
        if self.request_window == 0 || self.max_piece_retries == 0 {
            return Err(ExchangeError::Config(
                "request_window and max_piece_retries must be > 0".into(),
            ));
        }
        if self.recv_timeout.is_zero() || self.serve_idle_timeout.is_zero() {
            return Err(ExchangeError::Config("timeouts must be > 0".into()));
        }
        if self.upload_bytes_per_sec == Some(0) || self.download_bytes_per_sec == Some(0) {
            return Err(ExchangeError::Config("bandwidth caps must be > 0".into()));
        }
        if self.piece_size > self.max_piece_size || self.max_piece_size > MAX_PIECE_SIZE {
            return Err(ExchangeError::Config(
                "piece_size must not exceed max_piece_size (and max_piece_size the protocol limit)"
                    .into(),
            ));
        }
        if self.max_artifact_bytes == 0 {
            return Err(ExchangeError::Config("max_artifact_bytes must be > 0".into()));
        }
        if self.max_sources == 0 || self.ban_after_corrupt == 0 {
            return Err(ExchangeError::Config(
                "max_sources and ban_after_corrupt must be > 0".into(),
            ));
        }
        Ok(())
    }
}

/// Local exchange failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExchangeError {
    /// Invalid configuration.
    #[error("artifact exchange config: {0}")]
    Config(String),
    /// Wire-level failure.
    #[error(transparent)]
    Wire(#[from] WireError),
    /// Storage failure.
    #[error("artifact storage: {0}")]
    Store(String),
    /// Reading the source content failed.
    #[error("artifact source io: {0}")]
    Io(String),
    /// Content did not match what a manifest or descriptor pins.
    #[error("artifact content mismatch: {0}")]
    Mismatch(String),
    /// Unknown artifact.
    #[error("unknown artifact {0}")]
    Unknown(String),
    /// The peer is banned (it served a corrupt piece).
    #[error("peer {0} is banned")]
    Banned(String),
    /// The package, signer or content hash is revoked.
    #[error("revoked: {0}")]
    Revoked(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_binds_piece_list_sizes_and_content_hash() {
        let d = ArtifactDescriptor {
            piece_size: 1024,
            total_size: 1500,
            content_hash: [0; 32],
            pieces: vec![[1; 32], [2; 32]],
        };
        let mut swapped = d.clone();
        swapped.pieces.swap(0, 1);
        let mut shorter = d.clone();
        shorter.total_size = 1400;
        let mut other_claim = d.clone();
        other_claim.content_hash = [9; 32];
        assert_ne!(d.id(), swapped.id());
        assert_ne!(d.id(), shorter.id());
        assert_ne!(d.id(), other_claim.id(), "id binds the content hash");
        assert_eq!(d.piece_len(0), 1024);
        assert_eq!(d.piece_len(1), 476);
        assert_eq!(ArtifactId::from_hex(&d.id().to_string()), Some(d.id()));
    }

    #[test]
    fn bitfield_tracks_missing_pieces() {
        let mut b = Bitfield::new(10);
        for i in [0, 3, 9] {
            b.set(i, true);
        }
        assert_eq!(b.count(), 3);
        assert!(!b.is_complete());
        assert_eq!(b.missing().collect::<Vec<_>>(), vec![1, 2, 4, 5, 6, 7, 8]);
        b.set(42, true); // out of range: ignored
        assert!(!b.get(42));
    }
}
