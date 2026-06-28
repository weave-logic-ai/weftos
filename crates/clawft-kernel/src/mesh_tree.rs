//! Resource tree synchronization for mesh networking (K6.4).
//!
//! Defines types for comparing and synchronizing resource tree state
//! between mesh nodes using Merkle root hash comparison and incremental
//! diff transfer.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Request to sync tree state from a peer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeSyncRequest {
    /// Local Merkle root hash.
    pub local_root_hash: String,
    /// Number of nodes in local tree.
    pub local_node_count: usize,
    /// Whether to request a full snapshot (vs. incremental diff).
    pub full_snapshot: bool,
}

/// Response with tree state for sync.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeSyncResponse {
    /// Remote Merkle root hash.
    pub remote_root_hash: String,
    /// Whether trees are already in sync.
    pub in_sync: bool,
    /// Changed/new nodes (if not in_sync).
    pub diff_nodes: Vec<TreeNodeDiff>,
    /// Deleted node paths (if any).
    pub deleted_paths: Vec<String>,
}

/// A diff entry for a single tree node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeNodeDiff {
    /// Resource path.
    pub path: String,
    /// Resource kind (e.g., "namespace", "service", "agent").
    pub kind: String,
    /// Node hash after the change.
    pub hash: String,
    /// Metadata key-value pairs.
    pub metadata: HashMap<String, String>,
    /// Chain sequence that caused this change.
    pub chain_seq: Option<u64>,
    /// Type of diff.
    pub diff_type: TreeDiffType,
}

/// Type of tree diff entry.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TreeDiffType {
    /// Node was added.
    Added,
    /// Node metadata was modified.
    Modified,
    /// Node was removed.
    Removed,
}

/// Merkle proof for a specific tree node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MerkleProof {
    /// Path of the node being proven.
    pub path: String,
    /// Hash of the node.
    pub node_hash: String,
    /// Sibling hashes along the path from node to root.
    pub sibling_hashes: Vec<String>,
    /// Expected root hash.
    pub root_hash: String,
}

impl MerkleProof {
    /// Verify that the proof chain leads to the expected root.
    ///
    /// **Stub**: This currently performs structural validation only
    /// (non-empty fields). Full cryptographic verification -- recomputing
    /// hashes from leaf to root -- requires the tree's hash function and
    /// will be implemented when the exo-resource-tree crate exposes a
    /// `verify_proof(proof, root)` API.
    pub fn verify(&self) -> bool {
        !self.path.is_empty() && !self.node_hash.is_empty() && !self.root_hash.is_empty()
    }
}

/// Compare two tree roots to determine sync action.
pub fn compare_tree_roots(local_hash: &str, remote_hash: &str) -> TreeSyncAction {
    if local_hash == remote_hash {
        TreeSyncAction::InSync
    } else if local_hash.is_empty() {
        TreeSyncAction::FullPull
    } else if remote_hash.is_empty() {
        TreeSyncAction::FullPush
    } else {
        TreeSyncAction::IncrementalSync
    }
}

/// Sync action based on tree root comparison.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeSyncAction {
    /// Trees are identical.
    InSync,
    /// Local tree is empty -- pull full snapshot from remote.
    FullPull,
    /// Remote tree is empty -- push full snapshot to remote.
    FullPush,
    /// Both have content but differ -- do incremental Merkle diff.
    IncrementalSync,
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_sync_request_serde_roundtrip() {
        let req = TreeSyncRequest {
            local_root_hash: "abc123".into(),
            local_node_count: 42,
            full_snapshot: false,
        };
        let json = serde_json::to_string(&req).unwrap();
        let decoded: TreeSyncRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.local_root_hash, "abc123");
        assert_eq!(decoded.local_node_count, 42);
        assert!(!decoded.full_snapshot);
    }

    #[test]
    fn tree_sync_response_serde_roundtrip() {
        let resp = TreeSyncResponse {
            remote_root_hash: "root_hash".into(),
            in_sync: false,
            diff_nodes: vec![TreeNodeDiff {
                path: "/ns/svc".into(),
                kind: "service".into(),
                hash: "node_hash".into(),
                metadata: HashMap::from([("version".into(), "1".into())]),
                chain_seq: Some(5),
                diff_type: TreeDiffType::Added,
            }],
            deleted_paths: vec!["/ns/old".into()],
        };
        let json = serde_json::to_string(&resp).unwrap();
        let decoded: TreeSyncResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.remote_root_hash, "root_hash");
        assert!(!decoded.in_sync);
        assert_eq!(decoded.diff_nodes.len(), 1);
        assert_eq!(decoded.diff_nodes[0].path, "/ns/svc");
        assert_eq!(decoded.diff_nodes[0].diff_type, TreeDiffType::Added);
        assert_eq!(decoded.deleted_paths, vec!["/ns/old"]);
    }

    #[test]
    fn tree_node_diff_serde_roundtrip() {
        let diff = TreeNodeDiff {
            path: "/agents/bot-1".into(),
            kind: "agent".into(),
            hash: "diff_hash".into(),
            metadata: HashMap::new(),
            chain_seq: None,
            diff_type: TreeDiffType::Modified,
        };
        let json = serde_json::to_string(&diff).unwrap();
        let decoded: TreeNodeDiff = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.path, "/agents/bot-1");
        assert_eq!(decoded.kind, "agent");
        assert_eq!(decoded.diff_type, TreeDiffType::Modified);
        assert!(decoded.chain_seq.is_none());
    }

    #[test]
    fn merkle_proof_verify_valid() {
        let proof = MerkleProof {
            path: "/ns/svc".into(),
            node_hash: "node_abc".into(),
            sibling_hashes: vec!["sib1".into(), "sib2".into()],
            root_hash: "root_xyz".into(),
        };
        assert!(proof.verify());
    }

    #[test]
    fn merkle_proof_verify_empty_fields() {
        let empty_path = MerkleProof {
            path: "".into(),
            node_hash: "abc".into(),
            sibling_hashes: vec![],
            root_hash: "root".into(),
        };
        assert!(!empty_path.verify());

        let empty_node = MerkleProof {
            path: "/x".into(),
            node_hash: "".into(),
            sibling_hashes: vec![],
            root_hash: "root".into(),
        };
        assert!(!empty_node.verify());

        let empty_root = MerkleProof {
            path: "/x".into(),
            node_hash: "abc".into(),
            sibling_hashes: vec![],
            root_hash: "".into(),
        };
        assert!(!empty_root.verify());
    }

    #[test]
    fn compare_tree_roots_in_sync() {
        assert_eq!(
            compare_tree_roots("hash_a", "hash_a"),
            TreeSyncAction::InSync
        );
    }

    #[test]
    fn compare_tree_roots_full_pull() {
        assert_eq!(
            compare_tree_roots("", "hash_remote"),
            TreeSyncAction::FullPull
        );
    }

    #[test]
    fn compare_tree_roots_full_push() {
        assert_eq!(
            compare_tree_roots("hash_local", ""),
            TreeSyncAction::FullPush
        );
    }

    #[test]
    fn compare_tree_roots_incremental() {
        assert_eq!(
            compare_tree_roots("hash_a", "hash_b"),
            TreeSyncAction::IncrementalSync
        );
    }

    #[test]
    fn tree_diff_type_variants() {
        // Ensure all variants serialize/deserialize correctly.
        for variant in [
            TreeDiffType::Added,
            TreeDiffType::Modified,
            TreeDiffType::Removed,
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            let decoded: TreeDiffType = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded, variant);
        }
    }
}
