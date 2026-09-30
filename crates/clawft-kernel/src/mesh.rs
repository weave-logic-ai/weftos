//! Mesh transport abstractions for WeftOS cluster networking (K6.1).
//!
//! Defines the transport-agnostic interface for node-to-node communication.
//! Actual transport backends (QUIC, WebSocket, TCP) implement these traits.

use std::net::SocketAddr;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Mesh networking errors.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum MeshError {
    /// A transport-level failure (connection reset, DNS, etc.).
    #[error("transport error: {0}")]
    Transport(String),

    /// Noise or WeftOS handshake failed.
    #[error("handshake failed: {0}")]
    Handshake(String),

    /// Attempted to send to a peer that is not connected.
    #[error("peer not connected: {0}")]
    PeerNotConnected(String),

    /// Message exceeds the maximum allowed size.
    #[error("message too large: {size} bytes (max {max})")]
    MessageTooLarge {
        /// Actual size of the message.
        size: usize,
        /// Configured maximum.
        max: usize,
    },

    /// The underlying connection was closed.
    #[error("connection closed")]
    ConnectionClosed,

    /// An operation exceeded its deadline.
    #[error("timeout: {0}")]
    Timeout(String),

    /// Encryption or decryption failure.
    #[error("encryption error: {0}")]
    Encryption(String),

    /// Local and remote governance genesis hashes differ.
    #[error("genesis mismatch: local vs remote cluster")]
    GenesisMismatch,

    /// Wraps a lower-level I/O error description.
    #[error("io error: {0}")]
    Io(String),
}

/// Maximum message size (16 MiB per D8).
pub const MAX_MESSAGE_SIZE: usize = 16 * 1024 * 1024;

/// A bidirectional byte stream over any transport.
///
/// # Cancel-safety (ADR-010 / WEFT-18)
///
/// `recv` **must** be cancel-safe: it is raced inside `tokio::select!` in
/// the mesh peer loop (`boot.rs` bidirectional accept path). If a future is
/// dropped mid-frame, the next `recv` on the same stream must resume cleanly
/// without length-prefix desync. Implementations that stage multi-step reads
/// (`read_exact` of length then body) **must** keep partial progress on `self`.
///
/// `send` is typically invoked only after a `select!` arm wins (not as a
/// racing future). Prefer making multi-write sends resilient to task drop, but
/// cancel-safety of `recv` is the load-bearing contract for mesh correctness.
#[async_trait]
pub trait MeshStream: Send + Sync + 'static {
    /// Send raw bytes over the stream.
    async fn send(&mut self, data: &[u8]) -> Result<(), MeshError>;

    /// Receive raw bytes from the stream.
    ///
    /// Cancel-safe: may be dropped while pending inside `tokio::select!`.
    async fn recv(&mut self) -> Result<Vec<u8>, MeshError>;

    /// Close the stream gracefully.
    async fn close(&mut self) -> Result<(), MeshError>;

    /// Get the remote address if available.
    fn remote_addr(&self) -> Option<SocketAddr>;
}

/// Listens for incoming mesh connections.
#[async_trait]
pub trait TransportListener: Send + Sync + 'static {
    /// Accept the next incoming connection.
    async fn accept(&mut self) -> Result<(Box<dyn MeshStream>, SocketAddr), MeshError>;

    /// Get the local address this listener is bound to.
    fn local_addr(&self) -> Result<SocketAddr, MeshError>;
}

/// Transport-agnostic mesh transport interface.
///
/// Implementations provide concrete connection semantics (QUIC, TCP,
/// WebSocket, etc.) while the mesh layer stays transport-neutral.
#[async_trait]
pub trait MeshTransport: Send + Sync + 'static {
    /// Human-readable transport name (e.g., "quic", "websocket", "tcp").
    fn name(&self) -> &str;

    /// Start listening for incoming connections on the given address.
    async fn listen(&self, addr: &str) -> Result<Box<dyn TransportListener>, MeshError>;

    /// Connect to a remote peer at the given address.
    async fn connect(&self, addr: &str) -> Result<Box<dyn MeshStream>, MeshError>;

    /// Check if this transport supports the given address scheme.
    fn supports(&self, addr: &str) -> bool;
}

/// WeftOS mesh handshake payload (sent inside Noise XX step 3).
///
/// Both sides exchange this after the encrypted channel is established
/// to verify governance compatibility and negotiate capabilities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeftHandshake {
    /// Node identity (hex-encoded public key hash).
    pub node_id: String,

    /// Governance genesis hash for cluster trust root.
    pub governance_genesis_hash: [u8; 32],

    /// Governance engine version string.
    pub governance_version: String,

    /// Bitmap of supported capabilities.
    pub capabilities: u32,

    /// Whether this node supports post-quantum KEM.
    pub kem_supported: bool,

    /// Chain head sequence number.
    pub chain_seq: u64,

    /// Supported sync stream types (FrameType discriminants).
    pub supported_sync_streams: Vec<u8>,
}

/// Information about a connected mesh peer.
#[derive(Debug, Clone)]
pub struct MeshPeer {
    /// Peer's node identity.
    pub node_id: String,

    /// Peer's handshake data.
    pub handshake: WeftHandshake,

    /// Peer's network address.
    pub address: SocketAddr,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mesh_error_display_transport() {
        let err = MeshError::Transport("reset".into());
        assert_eq!(err.to_string(), "transport error: reset");
    }

    #[test]
    fn mesh_error_display_handshake() {
        let err = MeshError::Handshake("bad key".into());
        assert_eq!(err.to_string(), "handshake failed: bad key");
    }

    #[test]
    fn mesh_error_display_peer_not_connected() {
        let err = MeshError::PeerNotConnected("node-42".into());
        assert_eq!(err.to_string(), "peer not connected: node-42");
    }

    #[test]
    fn mesh_error_display_message_too_large() {
        let err = MeshError::MessageTooLarge {
            size: 20_000_000,
            max: MAX_MESSAGE_SIZE,
        };
        let s = err.to_string();
        assert!(s.contains("20000000"));
        assert!(s.contains(&MAX_MESSAGE_SIZE.to_string()));
    }

    #[test]
    fn mesh_error_display_connection_closed() {
        let err = MeshError::ConnectionClosed;
        assert_eq!(err.to_string(), "connection closed");
    }

    #[test]
    fn mesh_error_display_timeout() {
        let err = MeshError::Timeout("connect".into());
        assert_eq!(err.to_string(), "timeout: connect");
    }

    #[test]
    fn mesh_error_display_encryption() {
        let err = MeshError::Encryption("decrypt failed".into());
        assert_eq!(err.to_string(), "encryption error: decrypt failed");
    }

    #[test]
    fn mesh_error_display_genesis_mismatch() {
        let err = MeshError::GenesisMismatch;
        assert!(err.to_string().contains("genesis mismatch"));
    }

    #[test]
    fn mesh_error_display_io() {
        let err = MeshError::Io("broken pipe".into());
        assert_eq!(err.to_string(), "io error: broken pipe");
    }

    #[test]
    fn max_message_size_is_16mib() {
        assert_eq!(MAX_MESSAGE_SIZE, 16 * 1024 * 1024);
    }

    #[test]
    fn weft_handshake_serde_roundtrip() {
        let hs = WeftHandshake {
            node_id: "abcdef1234567890".into(),
            governance_genesis_hash: [0xAA; 32],
            governance_version: "1.0.0".into(),
            capabilities: 0b1011,
            kem_supported: true,
            chain_seq: 42,
            supported_sync_streams: vec![0x01, 0x03, 0x07],
        };
        let json = serde_json::to_string(&hs).unwrap();
        let restored: WeftHandshake = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.node_id, hs.node_id);
        assert_eq!(restored.governance_genesis_hash, [0xAA; 32]);
        assert_eq!(restored.governance_version, "1.0.0");
        assert_eq!(restored.capabilities, 0b1011);
        assert!(restored.kem_supported);
        assert_eq!(restored.chain_seq, 42);
        assert_eq!(restored.supported_sync_streams, vec![0x01, 0x03, 0x07]);
    }

    #[test]
    fn mesh_peer_fields() {
        let hs = WeftHandshake {
            node_id: "peer-1".into(),
            governance_genesis_hash: [0; 32],
            governance_version: "0.1.0".into(),
            capabilities: 0,
            kem_supported: false,
            chain_seq: 0,
            supported_sync_streams: vec![],
        };
        let addr: SocketAddr = "127.0.0.1:9489".parse().unwrap();
        let peer = MeshPeer {
            node_id: "peer-1".into(),
            handshake: hs,
            address: addr,
        };
        assert_eq!(peer.node_id, "peer-1");
        assert_eq!(peer.address.port(), 9489);
    }
}
