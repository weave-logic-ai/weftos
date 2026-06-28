//! Artifact exchange protocol over mesh transport (K6-G1).
//!
//! Extends the mesh framing protocol with `ArtifactRequest` and
//! `ArtifactResponse` frame types, enabling nodes to request and serve
//! content-addressed artifacts by hash.

use serde::{Deserialize, Serialize};
#[cfg(feature = "exochain")]
use std::sync::Arc;

// ---------------------------------------------------------------------------
// ArtifactRequest / ArtifactResponse
// ---------------------------------------------------------------------------

/// Request an artifact by its BLAKE3 hash from a mesh peer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRequest {
    /// BLAKE3 hex hash of the requested artifact.
    pub hash: String,
    /// Requesting node's identifier.
    pub requester_node_id: String,
}

/// Response to an artifact request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactResponse {
    /// BLAKE3 hex hash of the artifact.
    pub hash: String,
    /// Whether the artifact was found.
    pub found: bool,
    /// Artifact data (empty if not found).
    pub data: Vec<u8>,
    /// Serving node's identifier.
    pub server_node_id: String,
}

// ---------------------------------------------------------------------------
// ArtifactAnnouncement (gossip)
// ---------------------------------------------------------------------------

/// Announcement that a node has a new artifact available.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactAnnouncement {
    /// BLAKE3 hex hash of the artifact.
    pub hash: String,
    /// Size in bytes.
    pub size: u64,
    /// Content type label.
    pub content_type: String,
    /// Node that has the artifact.
    pub node_id: String,
}

// ---------------------------------------------------------------------------
// ArtifactExchange
// ---------------------------------------------------------------------------

/// Handles artifact exchange between mesh peers.
///
/// In a real implementation this would integrate with `ArtifactStore` and
/// the mesh transport layer. Here we provide the protocol types and
/// a local artifact catalog for testing.
pub struct ArtifactExchange {
    /// Local node identifier.
    node_id: String,
    /// Known remote artifacts: hash -> list of node IDs that have it.
    catalog: dashmap::DashMap<String, Vec<String>>,
    /// Pending requests (for testing).
    pending_requests: dashmap::DashMap<String, ArtifactRequest>,
    /// Optional chain manager for exochain audit logging.
    #[cfg(feature = "exochain")]
    chain_manager: Option<Arc<crate::chain::ChainManager>>,
}

impl ArtifactExchange {
    /// Create a new artifact exchange for the given node.
    pub fn new(node_id: String) -> Self {
        Self {
            node_id,
            catalog: dashmap::DashMap::new(),
            pending_requests: dashmap::DashMap::new(),
            #[cfg(feature = "exochain")]
            chain_manager: None,
        }
    }

    /// Attach a chain manager for exochain audit logging.
    #[cfg(feature = "exochain")]
    pub fn set_chain_manager(&mut self, cm: Arc<crate::chain::ChainManager>) {
        self.chain_manager = Some(cm);
    }

    /// Record that a remote node has an artifact.
    pub fn register_remote(&self, hash: &str, remote_node_id: &str) {
        #[cfg(feature = "exochain")]
        if let Some(ref cm) = self.chain_manager {
            cm.append(
                "mesh_artifact",
                crate::chain::EVENT_KIND_MESH_ARTIFACT_STORE,
                Some(serde_json::json!({
                    "hash": hash,
                    "remote_node_id": remote_node_id,
                    "action": "register_remote",
                })),
            );
        }
        self.catalog
            .entry(hash.to_string())
            .or_default()
            .push(remote_node_id.to_string());
    }

    /// Process an artifact announcement from gossip.
    pub fn handle_announcement(&self, announcement: &ArtifactAnnouncement) {
        #[cfg(feature = "exochain")]
        if let Some(ref cm) = self.chain_manager {
            cm.append(
                "mesh_artifact",
                crate::chain::EVENT_KIND_MESH_PEER_ADD,
                Some(serde_json::json!({
                    "hash": &announcement.hash,
                    "node_id": &announcement.node_id,
                    "size": announcement.size,
                    "content_type": &announcement.content_type,
                    "action": "handle_announcement",
                })),
            );
        }
        self.register_remote(&announcement.hash, &announcement.node_id);
    }

    /// Create a request frame for a remote artifact.
    pub fn create_request(&self, hash: &str) -> ArtifactRequest {
        #[cfg(feature = "exochain")]
        if let Some(ref cm) = self.chain_manager {
            cm.append(
                "mesh_artifact",
                crate::chain::EVENT_KIND_MESH_ARTIFACT_FETCH,
                Some(serde_json::json!({
                    "hash": hash,
                    "requester_node_id": &self.node_id,
                    "action": "create_request",
                })),
            );
        }
        let req = ArtifactRequest {
            hash: hash.to_string(),
            requester_node_id: self.node_id.clone(),
        };
        self.pending_requests.insert(hash.to_string(), req.clone());
        req
    }

    /// Create a response frame (as server).
    pub fn create_response(&self, hash: &str, found: bool, data: Vec<u8>) -> ArtifactResponse {
        ArtifactResponse {
            hash: hash.to_string(),
            found,
            data,
            server_node_id: self.node_id.clone(),
        }
    }

    /// Verify that received artifact data matches the expected hash.
    #[cfg(feature = "ecc")]
    pub fn verify_artifact(hash: &str, data: &[u8]) -> bool {
        let actual = blake3::hash(data).to_hex().to_string();
        actual == hash
    }

    /// Find which nodes have a given artifact.
    pub fn find_providers(&self, hash: &str) -> Vec<String> {
        self.catalog
            .get(hash)
            .map(|v| v.value().clone())
            .unwrap_or_default()
    }

    /// Check if any remote node has the artifact.
    pub fn is_available_remotely(&self, hash: &str) -> bool {
        self.catalog.contains_key(hash)
    }

    /// Create an announcement for a locally stored artifact.
    pub fn create_announcement(
        &self,
        hash: &str,
        size: u64,
        content_type: &str,
    ) -> ArtifactAnnouncement {
        ArtifactAnnouncement {
            hash: hash.to_string(),
            size,
            content_type: content_type.to_string(),
            node_id: self.node_id.clone(),
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_request() {
        let exchange = ArtifactExchange::new("node-1".into());
        let req = exchange.create_request("abc123");
        assert_eq!(req.hash, "abc123");
        assert_eq!(req.requester_node_id, "node-1");
    }

    #[test]
    fn create_response_found() {
        let exchange = ArtifactExchange::new("node-2".into());
        let resp = exchange.create_response("abc123", true, vec![1, 2, 3]);
        assert!(resp.found);
        assert_eq!(resp.data, vec![1, 2, 3]);
        assert_eq!(resp.server_node_id, "node-2");
    }

    #[test]
    fn create_response_not_found() {
        let exchange = ArtifactExchange::new("node-2".into());
        let resp = exchange.create_response("missing", false, vec![]);
        assert!(!resp.found);
        assert!(resp.data.is_empty());
    }

    #[test]
    fn register_and_find_providers() {
        let exchange = ArtifactExchange::new("node-1".into());
        exchange.register_remote("hash1", "node-2");
        exchange.register_remote("hash1", "node-3");

        let providers = exchange.find_providers("hash1");
        assert_eq!(providers.len(), 2);
        assert!(providers.contains(&"node-2".to_string()));
        assert!(providers.contains(&"node-3".to_string()));
    }

    #[test]
    fn handle_announcement() {
        let exchange = ArtifactExchange::new("node-1".into());
        let ann = ArtifactAnnouncement {
            hash: "new_hash".into(),
            size: 1024,
            content_type: "wasm-module".into(),
            node_id: "node-3".into(),
        };
        exchange.handle_announcement(&ann);
        assert!(exchange.is_available_remotely("new_hash"));
        assert_eq!(exchange.find_providers("new_hash"), vec!["node-3"]);
    }

    #[test]
    fn not_available_remotely() {
        let exchange = ArtifactExchange::new("node-1".into());
        assert!(!exchange.is_available_remotely("unknown"));
        assert!(exchange.find_providers("unknown").is_empty());
    }

    #[test]
    fn create_announcement() {
        let exchange = ArtifactExchange::new("node-1".into());
        let ann = exchange.create_announcement("hash_x", 2048, "generic");
        assert_eq!(ann.hash, "hash_x");
        assert_eq!(ann.size, 2048);
        assert_eq!(ann.node_id, "node-1");
    }

    #[cfg(feature = "ecc")]
    #[test]
    fn verify_artifact_correct() {
        let data = b"content for verification";
        let hash = blake3::hash(data).to_hex().to_string();
        assert!(ArtifactExchange::verify_artifact(&hash, data));
    }

    #[cfg(feature = "ecc")]
    #[test]
    fn verify_artifact_tampered() {
        let data = b"original";
        let hash = blake3::hash(data).to_hex().to_string();
        assert!(!ArtifactExchange::verify_artifact(&hash, b"tampered"));
    }

    #[test]
    fn artifact_request_serialization() {
        let req = ArtifactRequest {
            hash: "abc".into(),
            requester_node_id: "n1".into(),
        };
        let json = serde_json::to_string(&req).unwrap();
        let deser: ArtifactRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.hash, "abc");
    }

    #[test]
    fn artifact_response_serialization() {
        let resp = ArtifactResponse {
            hash: "abc".into(),
            found: true,
            data: vec![1, 2],
            server_node_id: "n2".into(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let deser: ArtifactResponse = serde_json::from_str(&json).unwrap();
        assert!(deser.found);
        assert_eq!(deser.data, vec![1, 2]);
    }
}
