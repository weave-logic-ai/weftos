//! Mesh connection accept loop and peer registration (K6.1).
//!
//! Provides the [`MeshConnectionPool`] for tracking active peer
//! connections, and the [`JoinRequest`] / [`JoinResponse`] types
//! for cluster join negotiation.
//!
//! # Status (WEFT-151 audit)
//!
//! **Orphan — library only.** No production callers outside this module
//! and re-exports. Runtime accept/connect lives **inline in**
//! `boot.rs` (transport listen loop) and peer state lives in
//! [`crate::mesh_runtime::MeshRuntime`] (`PeerConnection` DashMap),
//! not [`MeshConnectionPool`]. `FrameType::JoinRequest` /
//! `JoinResponse` exist in `mesh_framing` but never serialize these
//! structs. Unit tests only.
//!
//! **Disposition:** keep as the intended shared pool + join protocol;
//! consolidate with `MeshRuntime` peers rather than delete.
//! **Wiring schedule (0.8.x):** either (a) replace `MeshRuntime` peer
//! map with [`MeshConnectionPool`], or (b) wrap pool around runtime
//! peers; drive join negotiation with JoinRequest / JoinResponse
//! on first connect.

use dashmap::DashMap;
use serde::{Deserialize, Serialize};

use crate::mesh::MeshPeer;

/// Maximum number of concurrent peer connections (DoS mitigation).
const MAX_POOL_SIZE: usize = 256;

/// Connection pool managing active mesh peers.
///
/// Thread-safe via [`DashMap`]; can be shared across accept loops,
/// heartbeat tickers, and message dispatch tasks.
pub struct MeshConnectionPool {
    /// Active peer connections keyed by node_id.
    peers: DashMap<String, MeshPeerConnection>,
    /// Maximum number of concurrent peer connections.
    max_peers: usize,
}

/// An active connection to a mesh peer.
pub struct MeshPeerConnection {
    /// Peer information from handshake.
    pub peer: MeshPeer,
    /// Whether the connection is still alive.
    pub alive: bool,
}

impl MeshConnectionPool {
    /// Create an empty connection pool with the default max size.
    pub fn new() -> Self {
        Self {
            peers: DashMap::new(),
            max_peers: MAX_POOL_SIZE,
        }
    }

    /// Create a pool with a custom max peer limit.
    pub fn with_max_peers(max_peers: usize) -> Self {
        Self {
            peers: DashMap::new(),
            max_peers,
        }
    }

    /// Register a new peer connection after successful handshake.
    /// Returns `false` if the pool is at capacity and the peer was not added.
    pub fn register(&self, peer: MeshPeer) -> bool {
        // Allow re-registration of existing peers (update)
        if self.peers.contains_key(&peer.node_id) || self.peers.len() < self.max_peers {
            let node_id = peer.node_id.clone();
            self.peers
                .insert(node_id, MeshPeerConnection { peer, alive: true });
            true
        } else {
            false
        }
    }

    /// Remove a peer connection, returning it if it existed.
    pub fn remove(&self, node_id: &str) -> Option<MeshPeerConnection> {
        self.peers.remove(node_id).map(|(_, conn)| conn)
    }

    /// Get a reference to a peer connection.
    pub fn get(
        &self,
        node_id: &str,
    ) -> Option<dashmap::mapref::one::Ref<'_, String, MeshPeerConnection>> {
        self.peers.get(node_id)
    }

    /// List all connected peer node IDs.
    pub fn connected_peers(&self) -> Vec<String> {
        self.peers.iter().map(|r| r.key().clone()).collect()
    }

    /// Number of active connections.
    pub fn len(&self) -> usize {
        self.peers.len()
    }

    /// Whether the pool has no connections.
    pub fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }

    /// Get the maximum peer capacity.
    pub fn max_peers(&self) -> usize {
        self.max_peers
    }

    /// Get or create a connection to a node.
    /// Returns `true` if this was a reuse of an existing connection,
    /// `false` if a new connection was registered.
    pub fn get_or_insert(&self, node_id: &str, peer: MeshPeer) -> bool {
        if self.peers.contains_key(node_id) {
            // Reuse existing connection
            true
        } else {
            self.register(peer);
            false
        }
    }

    /// Pool statistics snapshot.
    pub fn stats(&self) -> PoolStats {
        PoolStats {
            active_connections: self.len(),
            ..Default::default()
        }
    }
}

/// Connection pool statistics.
#[derive(Debug, Clone, Default)]
pub struct PoolStats {
    /// Number of currently active connections.
    pub active_connections: usize,
    /// Total connections established since pool creation.
    pub total_connects: u64,
    /// Total connection reuses since pool creation.
    pub total_reuses: u64,
    /// Total evictions since pool creation.
    pub total_evictions: u64,
}

impl Default for MeshConnectionPool {
    fn default() -> Self {
        Self::new()
    }
}

/// Request to join a mesh cluster.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JoinRequest {
    /// Node ID of the requesting node.
    pub node_id: String,
    /// Governance genesis hash for cluster verification.
    pub governance_genesis_hash: [u8; 32],
    /// Platform type string (e.g., "cloud-native", "edge").
    pub platform: String,
    /// Supported transport protocols (e.g., ["quic", "tcp"]).
    pub transports: Vec<String>,
    /// Chain head sequence.
    pub chain_seq: u64,
    /// Tree Merkle root hash.
    pub tree_root_hash: [u8; 32],
}

/// Response to a join request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JoinResponse {
    /// Whether the join was accepted.
    pub accepted: bool,
    /// Reason for rejection (if not accepted).
    pub reason: Option<String>,
    /// List of known peers (if accepted).
    pub peer_list: Vec<PeerInfo>,
    /// Number of governance rules in this cluster.
    pub governance_rule_count: u32,
}

/// Minimal peer info shared during join.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    /// Peer node identifier.
    pub node_id: String,
    /// Peer network address.
    pub address: String,
    /// Peer platform type string.
    pub platform: String,
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use super::*;
    use crate::mesh::WeftHandshake;

    fn make_handshake(node_id: &str) -> WeftHandshake {
        WeftHandshake {
            node_id: node_id.into(),
            governance_genesis_hash: [0; 32],
            governance_version: "1.0.0".into(),
            capabilities: 0,
            kem_supported: false,
            chain_seq: 0,
            supported_sync_streams: vec![],
        }
    }

    fn make_peer(node_id: &str) -> MeshPeer {
        MeshPeer {
            node_id: node_id.into(),
            handshake: make_handshake(node_id),
            address: "127.0.0.1:9489".parse::<SocketAddr>().unwrap(),
        }
    }

    #[test]
    fn pool_register_and_get() {
        let pool = MeshConnectionPool::new();
        assert!(pool.is_empty());

        assert!(pool.register(make_peer("node-1")));
        assert_eq!(pool.len(), 1);
        assert!(!pool.is_empty());

        let conn = pool.get("node-1").unwrap();
        assert_eq!(conn.peer.node_id, "node-1");
        assert!(conn.alive);
    }

    #[test]
    fn pool_remove() {
        let pool = MeshConnectionPool::new();
        pool.register(make_peer("node-1"));
        pool.register(make_peer("node-2"));
        assert_eq!(pool.len(), 2);

        let removed = pool.remove("node-1");
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().peer.node_id, "node-1");
        assert_eq!(pool.len(), 1);

        // Removing again returns None
        assert!(pool.remove("node-1").is_none());
    }

    #[test]
    fn pool_connected_peers() {
        let pool = MeshConnectionPool::new();
        pool.register(make_peer("alpha"));
        pool.register(make_peer("beta"));
        pool.register(make_peer("gamma"));

        let mut peers = pool.connected_peers();
        peers.sort();
        assert_eq!(peers, vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn pool_default() {
        let pool = MeshConnectionPool::default();
        assert!(pool.is_empty());
    }

    #[test]
    fn pool_overwrite_existing_peer() {
        let pool = MeshConnectionPool::new();
        assert!(pool.register(make_peer("node-1")));

        // Re-registering with the same node_id overwrites
        let mut peer2 = make_peer("node-1");
        peer2.address = "10.0.0.1:9489".parse().unwrap();
        assert!(pool.register(peer2));

        assert_eq!(pool.len(), 1);
        let conn = pool.get("node-1").unwrap();
        assert_eq!(conn.peer.address.to_string(), "10.0.0.1:9489");
    }

    #[test]
    fn pool_capacity_limit() {
        let pool = MeshConnectionPool::with_max_peers(2);
        assert!(pool.register(make_peer("node-1")));
        assert!(pool.register(make_peer("node-2")));
        // Pool is full, third peer rejected
        assert!(!pool.register(make_peer("node-3")));
        assert_eq!(pool.len(), 2);
        // But re-registering existing peer still works
        assert!(pool.register(make_peer("node-1")));
        assert_eq!(pool.max_peers(), 2);
    }

    #[test]
    fn join_request_serde_roundtrip() {
        let req = JoinRequest {
            node_id: "new-node".into(),
            governance_genesis_hash: [0xBB; 32],
            platform: "cloud-native".into(),
            transports: vec!["quic".into(), "tcp".into()],
            chain_seq: 100,
            tree_root_hash: [0xCC; 32],
        };
        let json = serde_json::to_string(&req).unwrap();
        let restored: JoinRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.node_id, "new-node");
        assert_eq!(restored.governance_genesis_hash, [0xBB; 32]);
        assert_eq!(restored.transports, vec!["quic", "tcp"]);
        assert_eq!(restored.chain_seq, 100);
        assert_eq!(restored.tree_root_hash, [0xCC; 32]);
    }

    #[test]
    fn join_response_accepted_serde() {
        let resp = JoinResponse {
            accepted: true,
            reason: None,
            peer_list: vec![
                PeerInfo {
                    node_id: "peer-a".into(),
                    address: "10.0.0.1:9489".into(),
                    platform: "cloud-native".into(),
                },
                PeerInfo {
                    node_id: "peer-b".into(),
                    address: "10.0.0.2:9489".into(),
                    platform: "edge".into(),
                },
            ],
            governance_rule_count: 15,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let restored: JoinResponse = serde_json::from_str(&json).unwrap();
        assert!(restored.accepted);
        assert!(restored.reason.is_none());
        assert_eq!(restored.peer_list.len(), 2);
        assert_eq!(restored.governance_rule_count, 15);
    }

    #[test]
    fn join_response_rejected_serde() {
        let resp = JoinResponse {
            accepted: false,
            reason: Some("genesis mismatch".into()),
            peer_list: vec![],
            governance_rule_count: 0,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let restored: JoinResponse = serde_json::from_str(&json).unwrap();
        assert!(!restored.accepted);
        assert_eq!(restored.reason.as_deref(), Some("genesis mismatch"));
        assert!(restored.peer_list.is_empty());
    }

    #[test]
    fn connection_pool_reuses_existing() {
        let pool = MeshConnectionPool::new();
        let peer1 = make_peer("node-1");
        // First insert: new connection
        assert!(!pool.get_or_insert("node-1", peer1));
        assert_eq!(pool.len(), 1);

        // Second insert with same node_id: reuse
        let peer1_again = make_peer("node-1");
        assert!(pool.get_or_insert("node-1", peer1_again));
        assert_eq!(pool.len(), 1);

        // Different node: new connection
        let peer2 = make_peer("node-2");
        assert!(!pool.get_or_insert("node-2", peer2));
        assert_eq!(pool.len(), 2);
    }

    #[test]
    fn pool_stats_reports_active() {
        let pool = MeshConnectionPool::new();
        let stats = pool.stats();
        assert_eq!(stats.active_connections, 0);

        pool.register(make_peer("node-1"));
        pool.register(make_peer("node-2"));
        let stats = pool.stats();
        assert_eq!(stats.active_connections, 2);
        assert_eq!(stats.total_connects, 0); // counters start at default
    }

    #[test]
    fn peer_info_serde() {
        let info = PeerInfo {
            node_id: "node-x".into(),
            address: "192.168.1.1:9489".into(),
            platform: "wasi".into(),
        };
        let json = serde_json::to_string(&info).unwrap();
        let restored: PeerInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.node_id, "node-x");
        assert_eq!(restored.address, "192.168.1.1:9489");
        assert_eq!(restored.platform, "wasi");
    }
}
