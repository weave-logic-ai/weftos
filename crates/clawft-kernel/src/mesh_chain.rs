//! Chain replication protocol for mesh networking (K6.4).
//!
//! Defines request/response types for incremental chain synchronization
//! between mesh nodes. Chain events are exchanged as serialized JSON
//! (with optional RVF segment format for efficiency).

use serde::{Deserialize, Serialize};

/// Request to sync chain events from a peer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainSyncRequest {
    /// Chain identifier (0 = local chain).
    pub chain_id: u32,
    /// Request events after this sequence number.
    pub after_sequence: u64,
    /// Hash at the after_sequence position (for fork detection).
    pub after_hash: String,
    /// Maximum number of events to return.
    pub max_events: u32,
}

/// Response with chain events for sync.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainSyncResponse {
    /// Chain identifier.
    pub chain_id: u32,
    /// Serialized chain events (JSON).
    pub events: Vec<serde_json::Value>,
    /// Whether there are more events beyond this batch.
    pub has_more: bool,
    /// Tip sequence number on the responding node.
    pub tip_sequence: u64,
    /// Tip hash on the responding node.
    pub tip_hash: String,
}

/// Bridge event anchoring a remote chain's head in the local chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainBridgeEvent {
    /// Remote node ID.
    pub remote_node_id: String,
    /// Remote chain's head sequence.
    pub remote_head_sequence: u64,
    /// Remote chain's head hash.
    pub remote_head_hash: String,
    /// Timestamp of the bridge anchor.
    pub anchored_at: chrono::DateTime<chrono::Utc>,
}

/// Fork detection result.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChainForkStatus {
    /// Chains are in sync (same head).
    InSync,
    /// Local is behind -- need to pull events.
    LocalBehind {
        /// Local sequence number.
        local_seq: u64,
        /// Remote sequence number.
        remote_seq: u64,
    },
    /// Remote is behind -- need to push events.
    RemoteBehind {
        /// Local sequence number.
        local_seq: u64,
        /// Remote sequence number.
        remote_seq: u64,
    },
    /// Fork detected at the given sequence.
    Forked {
        /// Sequence where the fork occurred.
        fork_point: u64,
        /// Local hash at the fork point.
        local_hash: String,
        /// Remote hash at the fork point.
        remote_hash: String,
    },
}

/// Compare local and remote chain state to detect sync needs.
pub fn detect_chain_fork(
    local_seq: u64,
    local_hash: &str,
    remote_seq: u64,
    remote_hash: &str,
) -> ChainForkStatus {
    if local_seq == remote_seq && local_hash == remote_hash {
        ChainForkStatus::InSync
    } else if local_seq < remote_seq {
        ChainForkStatus::LocalBehind {
            local_seq,
            remote_seq,
        }
    } else if local_seq > remote_seq {
        ChainForkStatus::RemoteBehind {
            local_seq,
            remote_seq,
        }
    } else {
        // Same sequence but different hash -- fork!
        ChainForkStatus::Forked {
            fork_point: local_seq,
            local_hash: local_hash.to_string(),
            remote_hash: remote_hash.to_string(),
        }
    }
}

/// Compact state digest for sync stream initialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncStateDigest {
    /// Chain: highest sequence number.
    pub chain_seq: u64,
    /// Chain: head hash.
    pub chain_hash: String,
    /// Tree: Merkle root hash.
    pub tree_root_hash: String,
    /// Number of governance rules.
    pub governance_rule_count: u32,
    /// Node uptime in seconds.
    pub uptime_secs: u64,
}

// ── Backpressure: chain checkpoint catch-up ──────────────────────

/// Threshold for switching from event-by-event to checkpoint sync.
pub const CHECKPOINT_CATCHUP_THRESHOLD: u64 = 1000;

/// Determine sync strategy based on how far behind the peer is.
pub fn sync_strategy(local_seq: u64, remote_seq: u64) -> ChainSyncStrategy {
    let behind = local_seq.saturating_sub(remote_seq);
    if behind == 0 {
        ChainSyncStrategy::InSync
    } else if behind <= CHECKPOINT_CATCHUP_THRESHOLD {
        ChainSyncStrategy::EventByEvent { count: behind }
    } else {
        ChainSyncStrategy::CheckpointCatchup {
            behind,
            checkpoint_seq: remote_seq,
        }
    }
}

/// Strategy for catching up a behind peer.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChainSyncStrategy {
    /// Peer is fully in sync.
    InSync,
    /// Peer is slightly behind; send events one by one.
    EventByEvent { count: u64 },
    /// Peer is far behind; send a checkpoint snapshot then replay.
    CheckpointCatchup { behind: u64, checkpoint_seq: u64 },
}

// ── Sync frames with RVF wire segments ──────────────────────────

/// Sync frame header (wraps RVF segment payloads).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncFrame {
    /// Which sync stream this frame belongs to.
    pub stream_type: SyncStreamType,
    /// Frame sequence number within the stream.
    pub frame_seq: u64,
    /// Payload type discriminator.
    pub payload_type: SyncPayloadType,
}

/// Discriminator for QUIC sync streams.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum SyncStreamType {
    Control = 0,
    Chain = 1,
    Tree = 2,
    Causal = 3,
    Hnsw = 4,
    CrossRef = 5,
    Impulse = 6,
    Ipc = 7,
}

/// Payload type within a sync frame.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum SyncPayloadType {
    StateDigest = 0x00,
    ChainEvents = 0x01,
    TreeDiff = 0x02,
    CausalDelta = 0x03,
    HnswBatch = 0x04,
    CrossRefBatch = 0x05,
    ImpulseFlood = 0x06,
    Ack = 0x0F,
}

/// QUIC stream priority per SyncStreamType (lower = higher priority).
pub fn stream_priority(stream_type: SyncStreamType) -> u8 {
    match stream_type {
        SyncStreamType::Control => 0, // highest
        SyncStreamType::Chain => 1,
        SyncStreamType::Tree => 2,
        SyncStreamType::Ipc => 3,
        SyncStreamType::Causal => 4,
        SyncStreamType::CrossRef => 4,
        SyncStreamType::Hnsw => 5,
        SyncStreamType::Impulse => 6, // lowest
    }
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_sync_request_serde_roundtrip() {
        let req = ChainSyncRequest {
            chain_id: 1,
            after_sequence: 42,
            after_hash: "abc123".into(),
            max_events: 100,
        };
        let json = serde_json::to_string(&req).unwrap();
        let decoded: ChainSyncRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.chain_id, 1);
        assert_eq!(decoded.after_sequence, 42);
        assert_eq!(decoded.after_hash, "abc123");
        assert_eq!(decoded.max_events, 100);
    }

    #[test]
    fn chain_sync_response_serde_roundtrip() {
        let resp = ChainSyncResponse {
            chain_id: 0,
            events: vec![serde_json::json!({"type": "write", "key": "a"})],
            has_more: true,
            tip_sequence: 99,
            tip_hash: "tip_hash_val".into(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let decoded: ChainSyncResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.chain_id, 0);
        assert_eq!(decoded.events.len(), 1);
        assert!(decoded.has_more);
        assert_eq!(decoded.tip_sequence, 99);
        assert_eq!(decoded.tip_hash, "tip_hash_val");
    }

    #[test]
    fn chain_bridge_event_serde_roundtrip() {
        let evt = ChainBridgeEvent {
            remote_node_id: "node-42".into(),
            remote_head_sequence: 10,
            remote_head_hash: "deadbeef".into(),
            anchored_at: chrono::Utc::now(),
        };
        let json = serde_json::to_string(&evt).unwrap();
        let decoded: ChainBridgeEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.remote_node_id, "node-42");
        assert_eq!(decoded.remote_head_sequence, 10);
        assert_eq!(decoded.remote_head_hash, "deadbeef");
    }

    #[test]
    fn detect_chain_fork_in_sync() {
        let status = detect_chain_fork(5, "hash_a", 5, "hash_a");
        assert_eq!(status, ChainForkStatus::InSync);
    }

    #[test]
    fn detect_chain_fork_local_behind() {
        let status = detect_chain_fork(3, "hash_a", 7, "hash_b");
        assert_eq!(
            status,
            ChainForkStatus::LocalBehind {
                local_seq: 3,
                remote_seq: 7,
            }
        );
    }

    #[test]
    fn detect_chain_fork_remote_behind() {
        let status = detect_chain_fork(10, "hash_a", 5, "hash_b");
        assert_eq!(
            status,
            ChainForkStatus::RemoteBehind {
                local_seq: 10,
                remote_seq: 5,
            }
        );
    }

    #[test]
    fn detect_chain_fork_forked() {
        let status = detect_chain_fork(8, "hash_local", 8, "hash_remote");
        assert_eq!(
            status,
            ChainForkStatus::Forked {
                fork_point: 8,
                local_hash: "hash_local".into(),
                remote_hash: "hash_remote".into(),
            }
        );
    }

    // ── Backpressure sync strategy tests ────────────────────────────

    #[test]
    fn sync_strategy_in_sync() {
        assert_eq!(sync_strategy(100, 100), ChainSyncStrategy::InSync);
    }

    #[test]
    fn sync_strategy_event_by_event() {
        let strategy = sync_strategy(1100, 500);
        assert_eq!(strategy, ChainSyncStrategy::EventByEvent { count: 600 });
    }

    #[test]
    fn sync_strategy_checkpoint_catchup() {
        let strategy = sync_strategy(5000, 100);
        assert_eq!(
            strategy,
            ChainSyncStrategy::CheckpointCatchup {
                behind: 4900,
                checkpoint_seq: 100,
            }
        );
    }

    #[test]
    fn sync_strategy_boundary_at_threshold() {
        // Exactly at threshold: event-by-event
        let strategy = sync_strategy(1000, 0);
        assert_eq!(strategy, ChainSyncStrategy::EventByEvent { count: 1000 });
        // One past threshold: checkpoint
        let strategy = sync_strategy(1001, 0);
        assert_eq!(
            strategy,
            ChainSyncStrategy::CheckpointCatchup {
                behind: 1001,
                checkpoint_seq: 0,
            }
        );
    }

    #[test]
    fn sync_strategy_remote_ahead() {
        // remote_seq > local_seq: saturating_sub yields 0 = InSync
        assert_eq!(sync_strategy(50, 100), ChainSyncStrategy::InSync);
    }

    // ── SyncFrame / SyncStreamType serde tests ───────────────────────

    #[test]
    fn sync_frame_serde_roundtrip() {
        let frame = SyncFrame {
            stream_type: SyncStreamType::Chain,
            frame_seq: 42,
            payload_type: SyncPayloadType::ChainEvents,
        };
        let json = serde_json::to_string(&frame).unwrap();
        let restored: SyncFrame = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.stream_type, SyncStreamType::Chain);
        assert_eq!(restored.frame_seq, 42);
        assert_eq!(restored.payload_type, SyncPayloadType::ChainEvents);
    }

    #[test]
    fn sync_stream_type_serde_roundtrip() {
        let types = [
            SyncStreamType::Control,
            SyncStreamType::Chain,
            SyncStreamType::Tree,
            SyncStreamType::Causal,
            SyncStreamType::Hnsw,
            SyncStreamType::CrossRef,
            SyncStreamType::Impulse,
            SyncStreamType::Ipc,
        ];
        for t in types {
            let json = serde_json::to_string(&t).unwrap();
            let restored: SyncStreamType = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, t);
        }
    }

    #[test]
    fn stream_priority_ordering() {
        // Control has highest priority (lowest number)
        assert!(stream_priority(SyncStreamType::Control) < stream_priority(SyncStreamType::Chain));
        assert!(stream_priority(SyncStreamType::Chain) < stream_priority(SyncStreamType::Tree));
        assert!(stream_priority(SyncStreamType::Tree) < stream_priority(SyncStreamType::Ipc));
        assert!(stream_priority(SyncStreamType::Ipc) < stream_priority(SyncStreamType::Hnsw));
        assert!(stream_priority(SyncStreamType::Hnsw) < stream_priority(SyncStreamType::Impulse));
        // Causal and CrossRef share priority
        assert_eq!(
            stream_priority(SyncStreamType::Causal),
            stream_priority(SyncStreamType::CrossRef)
        );
    }

    #[test]
    fn sync_state_digest_serde_roundtrip() {
        let digest = SyncStateDigest {
            chain_seq: 100,
            chain_hash: "head_hash".into(),
            tree_root_hash: "merkle_root".into(),
            governance_rule_count: 5,
            uptime_secs: 3600,
        };
        let json = serde_json::to_string(&digest).unwrap();
        let decoded: SyncStateDigest = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.chain_seq, 100);
        assert_eq!(decoded.chain_hash, "head_hash");
        assert_eq!(decoded.tree_root_hash, "merkle_root");
        assert_eq!(decoded.governance_rule_count, 5);
        assert_eq!(decoded.uptime_secs, 3600);
    }
}
