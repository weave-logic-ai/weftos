//! Length-prefix framing for mesh wire protocol (K6.1).
//!
//! Each mesh frame is: `[4-byte big-endian length][1-byte message type][payload]`
//! Maximum frame size is [`MAX_MESSAGE_SIZE`] (16 MiB).

use crate::mesh::{MAX_MESSAGE_SIZE, MeshError, MeshStream};

/// Mesh message types for framing dispatch.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameType {
    /// WeftOS handshake payload.
    Handshake = 0x01,
    /// KernelMessage (IPC).
    IpcMessage = 0x02,
    /// Chain sync request/response.
    ChainSync = 0x03,
    /// Tree sync request/response.
    TreeSync = 0x04,
    /// Service advertisement.
    ServiceAdvert = 0x05,
    /// Process advertisement.
    ProcessAdvert = 0x06,
    /// Heartbeat ping/pong.
    Heartbeat = 0x07,
    /// Join request.
    JoinRequest = 0x08,
    /// Join response.
    JoinResponse = 0x09,
    /// Sync state digest.
    SyncDigest = 0x0A,
    /// Artifact request (K6-G1).
    ArtifactRequest = 0x0B,
    /// Artifact response (K6-G1).
    ArtifactResponse = 0x0C,
    /// Log aggregation (K6-G2).
    LogAggregation = 0x0D,
    /// Assessment sync (K6.6 -- cross-project assessment mesh).
    AssessmentSync = 0x0E,
}

impl FrameType {
    /// Parse a byte into a known frame type, returning `None` for
    /// unrecognised discriminants.
    pub fn from_byte(b: u8) -> Option<Self> {
        match b {
            0x01 => Some(Self::Handshake),
            0x02 => Some(Self::IpcMessage),
            0x03 => Some(Self::ChainSync),
            0x04 => Some(Self::TreeSync),
            0x05 => Some(Self::ServiceAdvert),
            0x06 => Some(Self::ProcessAdvert),
            0x07 => Some(Self::Heartbeat),
            0x08 => Some(Self::JoinRequest),
            0x09 => Some(Self::JoinResponse),
            0x0A => Some(Self::SyncDigest),
            0x0B => Some(Self::ArtifactRequest),
            0x0C => Some(Self::ArtifactResponse),
            0x0D => Some(Self::LogAggregation),
            0x0E => Some(Self::AssessmentSync),
            _ => None,
        }
    }
}

/// A framed mesh message.
#[derive(Debug, Clone)]
pub struct MeshFrame {
    /// Discriminant identifying the payload contents.
    pub frame_type: FrameType,
    /// Raw payload bytes (deserialized by the receiver).
    pub payload: Vec<u8>,
}

impl MeshFrame {
    /// Encode frame to wire format: `[4-byte len][1-byte type][payload]`.
    ///
    /// The 4-byte length covers the type byte **plus** the payload,
    /// so `len = 1 + payload.len()`.
    pub fn encode(&self) -> Result<Vec<u8>, MeshError> {
        let payload_len = self.payload.len();
        let total = 1 + payload_len; // type byte + payload
        if total > MAX_MESSAGE_SIZE {
            return Err(MeshError::MessageTooLarge {
                size: total,
                max: MAX_MESSAGE_SIZE,
            });
        }
        let mut buf = Vec::with_capacity(4 + total);
        buf.extend_from_slice(&(total as u32).to_be_bytes());
        buf.push(self.frame_type as u8);
        buf.extend_from_slice(&self.payload);
        Ok(buf)
    }

    /// Decode frame from wire bytes (after the 4-byte length prefix
    /// has already been consumed).
    pub fn decode(data: &[u8]) -> Result<Self, MeshError> {
        if data.is_empty() {
            return Err(MeshError::Transport("empty frame".into()));
        }
        let frame_type = FrameType::from_byte(data[0]).ok_or_else(|| {
            MeshError::Transport(format!("unknown frame type: 0x{:02x}", data[0]))
        })?;
        let payload = data[1..].to_vec();
        Ok(Self {
            frame_type,
            payload,
        })
    }
}

/// Read a single framed message from a mesh stream.
///
/// Expects the stream to yield the type byte + payload (the length
/// prefix is handled by the transport layer).
pub async fn read_frame(stream: &mut dyn MeshStream) -> Result<MeshFrame, MeshError> {
    let data = stream.recv().await?;
    if data.len() > MAX_MESSAGE_SIZE {
        return Err(MeshError::MessageTooLarge {
            size: data.len(),
            max: MAX_MESSAGE_SIZE,
        });
    }
    MeshFrame::decode(&data)
}

/// Write a single framed message to a mesh stream.
pub async fn write_frame(stream: &mut dyn MeshStream, frame: &MeshFrame) -> Result<(), MeshError> {
    let encoded = frame.encode()?;
    stream.send(&encoded).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_type_from_byte_all_variants() {
        let expected = [
            (0x01, FrameType::Handshake),
            (0x02, FrameType::IpcMessage),
            (0x03, FrameType::ChainSync),
            (0x04, FrameType::TreeSync),
            (0x05, FrameType::ServiceAdvert),
            (0x06, FrameType::ProcessAdvert),
            (0x07, FrameType::Heartbeat),
            (0x08, FrameType::JoinRequest),
            (0x09, FrameType::JoinResponse),
            (0x0A, FrameType::SyncDigest),
            (0x0B, FrameType::ArtifactRequest),
            (0x0C, FrameType::ArtifactResponse),
            (0x0D, FrameType::LogAggregation),
            (0x0E, FrameType::AssessmentSync),
        ];
        for (byte, variant) in expected {
            assert_eq!(FrameType::from_byte(byte), Some(variant));
        }
    }

    #[test]
    fn frame_type_from_byte_unknown() {
        assert!(FrameType::from_byte(0x00).is_none());
        assert!(FrameType::from_byte(0x0F).is_none());
        assert!(FrameType::from_byte(0xFF).is_none());
    }

    #[test]
    fn encode_decode_roundtrip() {
        let frame = MeshFrame {
            frame_type: FrameType::Heartbeat,
            payload: vec![1, 2, 3, 4],
        };
        let encoded = frame.encode().unwrap();

        // First 4 bytes are the length prefix: 1 (type) + 4 (payload) = 5
        let len = u32::from_be_bytes([encoded[0], encoded[1], encoded[2], encoded[3]]);
        assert_eq!(len, 5);

        // Decode the data portion (after length prefix)
        let decoded = MeshFrame::decode(&encoded[4..]).unwrap();
        assert_eq!(decoded.frame_type, FrameType::Heartbeat);
        assert_eq!(decoded.payload, vec![1, 2, 3, 4]);
    }

    #[test]
    fn encode_decode_empty_payload() {
        let frame = MeshFrame {
            frame_type: FrameType::Handshake,
            payload: vec![],
        };
        let encoded = frame.encode().unwrap();
        let decoded = MeshFrame::decode(&encoded[4..]).unwrap();
        assert_eq!(decoded.frame_type, FrameType::Handshake);
        assert!(decoded.payload.is_empty());
    }

    #[test]
    fn decode_empty_data_fails() {
        let result = MeshFrame::decode(&[]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("empty frame"));
    }

    #[test]
    fn decode_unknown_type_fails() {
        let result = MeshFrame::decode(&[0xFF, 0x01, 0x02]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("unknown frame type: 0xff"));
    }

    #[test]
    fn encode_oversized_frame_fails() {
        let frame = MeshFrame {
            frame_type: FrameType::IpcMessage,
            payload: vec![0u8; MAX_MESSAGE_SIZE], // payload alone = MAX, +1 for type byte
        };
        let result = frame.encode();
        assert!(result.is_err());
        match result.unwrap_err() {
            MeshError::MessageTooLarge { size, max } => {
                assert_eq!(size, MAX_MESSAGE_SIZE + 1);
                assert_eq!(max, MAX_MESSAGE_SIZE);
            }
            other => panic!("expected MessageTooLarge, got: {other:?}"),
        }
    }

    #[test]
    fn all_frame_types_encode_decode() {
        let types = [
            FrameType::Handshake,
            FrameType::IpcMessage,
            FrameType::ChainSync,
            FrameType::TreeSync,
            FrameType::ServiceAdvert,
            FrameType::ProcessAdvert,
            FrameType::Heartbeat,
            FrameType::JoinRequest,
            FrameType::JoinResponse,
            FrameType::SyncDigest,
            FrameType::ArtifactRequest,
            FrameType::ArtifactResponse,
            FrameType::LogAggregation,
            FrameType::AssessmentSync,
        ];
        for ft in types {
            let frame = MeshFrame {
                frame_type: ft,
                payload: vec![0xCA, 0xFE],
            };
            let encoded = frame.encode().unwrap();
            let decoded = MeshFrame::decode(&encoded[4..]).unwrap();
            assert_eq!(decoded.frame_type, ft);
            assert_eq!(decoded.payload, vec![0xCA, 0xFE]);
        }
    }
}
