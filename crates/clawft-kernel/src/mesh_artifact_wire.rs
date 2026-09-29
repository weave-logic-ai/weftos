//! Wire types for the swarm-ready artifact piece protocol
//! (ADR-099 section 6, mesh-placement-11).
//!
//! An artifact is identified by the root hash over its ordered piece list
//! ([`ArtifactDescriptor::id`]), not by the node it came from. Pieces are
//! fixed-size (64 MiB placeholder) and pinned by BLAKE3. Pieces travel in
//! blocks, because a piece is larger than one frame.
//!
//! Frame 0x0B ([`FrameType::ArtifactRequest`]) carries control messages
//! (`meta_request`, `announce`, `request`); frame 0x0C
//! ([`FrameType::ArtifactResponse`]) carries data (`meta`, `piece`,
//! `no_piece`, `reject`). Every frame is capped at [`MAX_ARTIFACT_FRAME`]
//! payload bytes; there is no ceiling on the artifact's total size.
//!
//! The encoding is a small, strict binary layout (big-endian integers,
//! 32-byte hashes, no trailing bytes) so piece data is never re-encoded.

use crate::mesh_artifact_types::bad;
pub use crate::mesh_artifact_types::{
    ArtifactDescriptor, ArtifactId, ArtifactKey, Bitfield, DEFAULT_BLOCK_SIZE, DEFAULT_PIECE_SIZE,
    MAX_ARTIFACT_FRAME, MAX_PIECE_SIZE, MAX_REASON_BYTES, MAX_REQUEST_PIECES, MIN_PIECE_SIZE,
    WireError,
};
use crate::mesh_framing::{FrameType, MeshFrame};

const TAG_META_REQUEST: u8 = 0x01;
const TAG_ANNOUNCE: u8 = 0x02;
const TAG_REQUEST: u8 = 0x03;
const TAG_META: u8 = 0x11;
const TAG_PIECE: u8 = 0x12;
const TAG_NO_PIECE: u8 = 0x13;
const TAG_REJECT: u8 = 0x14;

/// One artifact-protocol message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactMsg {
    /// 0x0B: ask a peer for an artifact's descriptor and its `have`.
    MetaRequest {
        /// What is wanted.
        key: ArtifactKey,
    },
    /// 0x0B: the pieces the sender holds, verified.
    Announce {
        /// Artifact.
        id: ArtifactId,
        /// Held pieces.
        have: Bitfield,
    },
    /// 0x0B: ask for pieces by index.
    Request {
        /// Artifact.
        id: ArtifactId,
        /// Piece indexes.
        pieces: Vec<u32>,
    },
    /// 0x0C: the descriptor.
    Meta {
        /// Piece list.
        descriptor: ArtifactDescriptor,
    },
    /// 0x0C: one block of piece data.
    Piece {
        /// Artifact.
        id: ArtifactId,
        /// Piece index.
        index: u32,
        /// Byte offset of this block within the piece.
        offset: u64,
        /// Block bytes.
        data: Vec<u8>,
    },
    /// 0x0C: the sender cannot supply this piece.
    NoPiece {
        /// Artifact.
        id: ArtifactId,
        /// Piece index.
        index: u32,
    },
    /// 0x0C: the request is refused (unknown, or not servable).
    Reject {
        /// What was asked for.
        key: ArtifactKey,
        /// Short reason.
        reason: String,
    },
}

impl ArtifactMsg {
    /// Frame type this message travels in.
    pub fn frame_type(&self) -> FrameType {
        match self {
            Self::MetaRequest { .. } | Self::Announce { .. } | Self::Request { .. } => {
                FrameType::ArtifactRequest
            }
            _ => FrameType::ArtifactResponse,
        }
    }

    /// Encode to a [`MeshFrame`], refusing payloads over the cap.
    pub fn to_frame(&self) -> Result<MeshFrame, WireError> {
        let mut w = Vec::new();
        match self {
            Self::MetaRequest { key } => {
                w.push(TAG_META_REQUEST);
                put_key(&mut w, key);
            }
            Self::Announce { id, have } => {
                w.push(TAG_ANNOUNCE);
                w.extend_from_slice(&id.0);
                w.extend_from_slice(&have.len.to_be_bytes());
                w.extend_from_slice(&have.bits);
            }
            Self::Request { id, pieces } => {
                if pieces.len() > MAX_REQUEST_PIECES {
                    return Err(bad("too many pieces in one request"));
                }
                w.push(TAG_REQUEST);
                w.extend_from_slice(&id.0);
                w.extend_from_slice(&(pieces.len() as u32).to_be_bytes());
                for p in pieces {
                    w.extend_from_slice(&p.to_be_bytes());
                }
            }
            Self::Meta { descriptor: d } => {
                w.push(TAG_META);
                w.extend_from_slice(&d.piece_size.to_be_bytes());
                w.extend_from_slice(&d.total_size.to_be_bytes());
                w.extend_from_slice(&d.content_hash);
                w.extend_from_slice(&(d.pieces.len() as u32).to_be_bytes());
                for p in &d.pieces {
                    w.extend_from_slice(p);
                }
            }
            Self::Piece {
                id,
                index,
                offset,
                data,
            } => {
                w.reserve(1 + 32 + 4 + 8 + 4 + data.len());
                w.push(TAG_PIECE);
                w.extend_from_slice(&id.0);
                w.extend_from_slice(&index.to_be_bytes());
                w.extend_from_slice(&offset.to_be_bytes());
                w.extend_from_slice(&(data.len() as u32).to_be_bytes());
                w.extend_from_slice(data);
            }
            Self::NoPiece { id, index } => {
                w.push(TAG_NO_PIECE);
                w.extend_from_slice(&id.0);
                w.extend_from_slice(&index.to_be_bytes());
            }
            Self::Reject { key, reason } => {
                w.push(TAG_REJECT);
                put_key(&mut w, key);
                let r = truncate_utf8(reason, MAX_REASON_BYTES);
                w.extend_from_slice(&(r.len() as u16).to_be_bytes());
                w.extend_from_slice(r.as_bytes());
            }
        }
        if w.len() > MAX_ARTIFACT_FRAME {
            return Err(WireError::FrameTooLarge {
                size: w.len(),
                max: MAX_ARTIFACT_FRAME,
            });
        }
        Ok(MeshFrame {
            frame_type: self.frame_type(),
            payload: w,
        })
    }

    /// Encode to stream bytes: `[4-byte len][type][payload]`.
    pub fn to_wire(&self) -> Result<Vec<u8>, WireError> {
        self.to_frame()?
            .encode()
            .map_err(|e| bad(format!("frame encode: {e}")))
    }

    /// Decode stream bytes produced by [`Self::to_wire`]. The declared
    /// length is checked against the cap before anything is parsed.
    pub fn from_wire(raw: &[u8]) -> Result<Self, WireError> {
        if raw.len() < 5 {
            return Err(bad("short frame"));
        }
        let declared = u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]) as usize;
        let payload_len = declared.saturating_sub(1).max(raw.len().saturating_sub(5));
        if payload_len > MAX_ARTIFACT_FRAME {
            return Err(WireError::FrameTooLarge {
                size: payload_len,
                max: MAX_ARTIFACT_FRAME,
            });
        }
        if declared != raw.len() - 4 {
            return Err(bad("length prefix does not match frame"));
        }
        let frame = MeshFrame::decode(&raw[4..]).map_err(|e| bad(e.to_string()))?;
        Self::from_frame(&frame)
    }

    /// Decode a [`MeshFrame`] of type 0x0B or 0x0C.
    pub fn from_frame(frame: &MeshFrame) -> Result<Self, WireError> {
        if frame.payload.len() > MAX_ARTIFACT_FRAME {
            return Err(WireError::FrameTooLarge {
                size: frame.payload.len(),
                max: MAX_ARTIFACT_FRAME,
            });
        }
        let mut r = Reader(&frame.payload);
        let tag = r.u8()?;
        let msg = match tag {
            TAG_META_REQUEST => Self::MetaRequest { key: r.key()? },
            TAG_ANNOUNCE => {
                let id = ArtifactId(r.hash()?);
                let len = r.u32()?;
                let bits = r.take((len as usize).div_ceil(8))?.to_vec();
                Self::Announce {
                    id,
                    have: Bitfield { len, bits },
                }
            }
            TAG_REQUEST => {
                let id = ArtifactId(r.hash()?);
                let n = r.u32()? as usize;
                if n > MAX_REQUEST_PIECES {
                    return Err(bad("too many pieces in one request"));
                }
                let pieces = (0..n).map(|_| r.u32()).collect::<Result<_, _>>()?;
                Self::Request { id, pieces }
            }
            TAG_META => {
                let piece_size = r.u64()?;
                let total_size = r.u64()?;
                let content_hash = r.hash()?;
                let n = r.u32()? as usize;
                if n.saturating_mul(32) > r.0.len() {
                    return Err(bad("piece list longer than frame"));
                }
                let pieces = (0..n).map(|_| r.hash()).collect::<Result<_, _>>()?;
                let descriptor = ArtifactDescriptor {
                    piece_size,
                    total_size,
                    content_hash,
                    pieces,
                };
                descriptor.validate()?;
                Self::Meta { descriptor }
            }
            TAG_PIECE => {
                let id = ArtifactId(r.hash()?);
                let index = r.u32()?;
                let offset = r.u64()?;
                let n = r.u32()? as usize;
                let data = r.take(n)?.to_vec();
                Self::Piece {
                    id,
                    index,
                    offset,
                    data,
                }
            }
            TAG_NO_PIECE => Self::NoPiece {
                id: ArtifactId(r.hash()?),
                index: r.u32()?,
            },
            TAG_REJECT => {
                let key = r.key()?;
                let n = r.u16()? as usize;
                if n > MAX_REASON_BYTES {
                    return Err(bad("reject reason too long"));
                }
                let reason = String::from_utf8(r.take(n)?.to_vec())
                    .map_err(|_| bad("reject reason not utf-8"))?;
                Self::Reject { key, reason }
            }
            other => return Err(bad(format!("unknown artifact message tag 0x{other:02x}"))),
        };
        if !r.0.is_empty() {
            return Err(bad("trailing bytes after artifact message"));
        }
        if msg.frame_type() != frame.frame_type {
            return Err(bad("artifact message in the wrong frame type"));
        }
        Ok(msg)
    }
}

fn put_key(w: &mut Vec<u8>, key: &ArtifactKey) {
    match key {
        ArtifactKey::Root(id) => {
            w.push(0);
            w.extend_from_slice(&id.0);
        }
        ArtifactKey::Content(h) => {
            w.push(1);
            w.extend_from_slice(h);
        }
    }
}

fn truncate_utf8(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], WireError> {
        if n > self.0.len() {
            return Err(bad("truncated artifact message"));
        }
        let (head, tail) = self.0.split_at(n);
        self.0 = tail;
        Ok(head)
    }
    fn u8(&mut self) -> Result<u8, WireError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, WireError> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Result<u32, WireError> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn u64(&mut self) -> Result<u64, WireError> {
        let mut a = [0u8; 8];
        a.copy_from_slice(self.take(8)?);
        Ok(u64::from_be_bytes(a))
    }
    fn hash(&mut self) -> Result<[u8; 32], WireError> {
        let mut a = [0u8; 32];
        a.copy_from_slice(self.take(32)?);
        Ok(a)
    }
    fn key(&mut self) -> Result<ArtifactKey, WireError> {
        match self.u8()? {
            0 => Ok(ArtifactKey::Root(ArtifactId(self.hash()?))),
            1 => Ok(ArtifactKey::Content(self.hash()?)),
            k => Err(bad(format!("unknown key kind {k}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id() -> ArtifactId {
        ArtifactId([7; 32])
    }

    fn roundtrip(m: ArtifactMsg) {
        let raw = m.to_wire().unwrap();
        assert_eq!(ArtifactMsg::from_wire(&raw).unwrap(), m);
    }

    #[test]
    fn every_message_roundtrips_in_its_frame_type() {
        let mut have = Bitfield::new(11);
        have.set(0, true);
        have.set(10, true);
        let d = ArtifactDescriptor {
            piece_size: 1024,
            total_size: 2048 + 5,
            content_hash: [1; 32],
            pieces: vec![[2; 32], [3; 32], [4; 32]],
        };
        roundtrip(ArtifactMsg::MetaRequest {
            key: ArtifactKey::Content([9; 32]),
        });
        roundtrip(ArtifactMsg::Announce { id: id(), have });
        roundtrip(ArtifactMsg::Request {
            id: id(),
            pieces: vec![0, 3, 9],
        });
        roundtrip(ArtifactMsg::Meta { descriptor: d });
        roundtrip(ArtifactMsg::Piece {
            id: id(),
            index: 2,
            offset: 4096,
            data: vec![5; 100],
        });
        roundtrip(ArtifactMsg::NoPiece { id: id(), index: 1 });
        roundtrip(ArtifactMsg::Reject {
            key: ArtifactKey::Root(id()),
            reason: "not servable".into(),
        });
        let req = ArtifactMsg::Request {
            id: id(),
            pieces: vec![],
        };
        assert_eq!(req.frame_type().as_byte(), 0x0B);
        let piece = ArtifactMsg::NoPiece { id: id(), index: 0 };
        assert_eq!(piece.frame_type().as_byte(), 0x0C);
    }

    #[test]
    fn oversize_frames_are_refused_both_ways() {
        let big = ArtifactMsg::Piece {
            id: id(),
            index: 0,
            offset: 0,
            data: vec![0; MAX_ARTIFACT_FRAME],
        };
        assert!(matches!(
            big.to_wire(),
            Err(WireError::FrameTooLarge { .. })
        ));
        // A peer that ignores the cap: the length prefix alone is refused.
        let mut raw = vec![];
        raw.extend_from_slice(&((MAX_ARTIFACT_FRAME + 2) as u32).to_be_bytes());
        raw.push(0x0C);
        raw.extend_from_slice(&[0; 64]);
        assert!(matches!(
            ArtifactMsg::from_wire(&raw),
            Err(WireError::FrameTooLarge { .. })
        ));
    }

    #[test]
    fn malformed_frames_are_refused() {
        let mut raw = ArtifactMsg::NoPiece { id: id(), index: 1 }
            .to_wire()
            .unwrap();
        // Wrong frame type for the tag.
        raw[4] = 0x0B;
        assert!(ArtifactMsg::from_wire(&raw).is_err());
        // Trailing bytes.
        let mut raw = ArtifactMsg::NoPiece { id: id(), index: 1 }
            .to_wire()
            .unwrap();
        raw.push(0);
        assert!(ArtifactMsg::from_wire(&raw).is_err());
        // Descriptor whose piece count does not match its size.
        let d = ArtifactDescriptor {
            piece_size: 1024,
            total_size: 10_000,
            content_hash: [0; 32],
            pieces: vec![[0; 32]],
        };
        let raw = ArtifactMsg::Meta { descriptor: d }.to_wire().unwrap();
        assert!(ArtifactMsg::from_wire(&raw).is_err());
    }
}
