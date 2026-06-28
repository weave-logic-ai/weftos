//! The core resource tree data structure.

use std::collections::HashMap;

use sha3::{
    Shake256,
    digest::{ExtendableOutput, Update, XofReader},
};

/// Compute 256-bit (32-byte) SHAKE-256 hash.
/// Inlined from rvf-crypto to remove the external path dependency.
fn shake256_256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Shake256::default();
    hasher.update(data);
    let mut reader = hasher.finalize_xof();
    let mut output = [0u8; 32];
    reader.read(&mut output);
    output
}

use crate::error::{TreeError, TreeResult};
use crate::model::{ResourceId, ResourceKind, ResourceNode};
use crate::scoring::NodeScoring;

/// A hierarchical resource tree backed by a flat map.
///
/// The tree always contains a root node at "/". All resource paths must
/// start with "/" and be inserted under an existing parent.
#[derive(Debug)]
pub struct ResourceTree {
    nodes: HashMap<ResourceId, ResourceNode>,
}

impl ResourceTree {
    /// Create an empty tree with only the root node "/".
    pub fn new() -> Self {
        let mut nodes = HashMap::new();
        let root = ResourceNode::new(ResourceId::root(), ResourceKind::Namespace, None);
        nodes.insert(ResourceId::root(), root);
        Self { nodes }
    }

    /// Insert a new resource node under the given parent.
    pub fn insert(
        &mut self,
        id: ResourceId,
        kind: ResourceKind,
        parent_id: ResourceId,
    ) -> TreeResult<()> {
        // Validate path starts with /
        if !id.0.starts_with('/') {
            return Err(TreeError::InvalidPath {
                reason: format!("path must start with '/': {id}"),
            });
        }

        // Check for duplicates
        if self.nodes.contains_key(&id) {
            return Err(TreeError::AlreadyExists { id });
        }

        // Verify parent exists
        if !self.nodes.contains_key(&parent_id) {
            return Err(TreeError::ParentNotFound { parent_id });
        }

        // Create the node
        let node = ResourceNode::new(id.clone(), kind, Some(parent_id.clone()));
        self.nodes.insert(id.clone(), node);

        // Add child reference to parent
        if let Some(parent) = self.nodes.get_mut(&parent_id) {
            parent.children.push(id);
        }

        Ok(())
    }

    /// Remove a leaf node (must have no children).
    pub fn remove(&mut self, id: ResourceId) -> TreeResult<ResourceNode> {
        if id.is_root() {
            return Err(TreeError::InvalidPath {
                reason: "cannot remove root node".to_string(),
            });
        }

        // Check the node exists and is a leaf
        let child_count = self
            .nodes
            .get(&id)
            .ok_or_else(|| TreeError::NotFound { id: id.clone() })?
            .children
            .len();

        if child_count > 0 {
            return Err(TreeError::NotEmpty { id, child_count });
        }

        // Remove from parent's children list
        let node = self.nodes.remove(&id).unwrap();
        if let Some(ref parent_id) = node.parent
            && let Some(parent) = self.nodes.get_mut(parent_id)
        {
            parent.children.retain(|c| c != &id);
        }

        Ok(node)
    }

    /// Get an immutable reference to a node.
    pub fn get(&self, id: &ResourceId) -> Option<&ResourceNode> {
        self.nodes.get(id)
    }

    /// Get a mutable reference to a node.
    pub fn get_mut(&mut self, id: &ResourceId) -> Option<&mut ResourceNode> {
        self.nodes.get_mut(id)
    }

    /// Return all direct children of a node.
    pub fn children(&self, id: &ResourceId) -> TreeResult<Vec<&ResourceNode>> {
        let node = self
            .nodes
            .get(id)
            .ok_or_else(|| TreeError::NotFound { id: id.clone() })?;

        Ok(node
            .children
            .iter()
            .filter_map(|child_id| self.nodes.get(child_id))
            .collect())
    }

    /// Walk up from a node to the root, returning the ancestor IDs (excluding the node itself).
    pub fn ancestors(&self, id: &ResourceId) -> Vec<ResourceId> {
        let mut result = Vec::new();
        let mut current = self.nodes.get(id).and_then(|n| n.parent.clone());
        while let Some(pid) = current {
            result.push(pid.clone());
            current = self.nodes.get(&pid).and_then(|n| n.parent.clone());
        }
        result
    }

    /// Recompute the Merkle hash for a single node.
    ///
    /// Hash = SHAKE-256(sorted child hashes || scoring bytes || sorted metadata kv).
    pub fn recompute_merkle(&mut self, id: &ResourceId) {
        // Gather child hashes (sorted for determinism) + scoring bytes
        let (child_hashes, scoring_bytes): (Vec<[u8; 32]>, [u8; 24]) = {
            let node = match self.nodes.get(id) {
                Some(n) => n,
                None => return,
            };
            let mut hashes: Vec<[u8; 32]> = node
                .children
                .iter()
                .filter_map(|cid| self.nodes.get(cid).map(|c| c.merkle_hash))
                .collect();
            hashes.sort();
            (hashes, node.scoring.to_hash_bytes())
        };

        // Gather sorted metadata
        let meta_bytes: Vec<u8> = {
            let node = match self.nodes.get(id) {
                Some(n) => n,
                None => return,
            };
            let mut keys: Vec<&String> = node.metadata.keys().collect();
            keys.sort();
            let mut buf = Vec::new();
            for key in keys {
                buf.extend_from_slice(key.as_bytes());
                if let Some(val) = node.metadata.get(key) {
                    buf.extend_from_slice(val.to_string().as_bytes());
                }
            }
            buf
        };

        // Compute SHAKE-256 hash: child_hashes || scoring || metadata
        let mut buf = Vec::with_capacity(child_hashes.len() * 32 + 24 + meta_bytes.len());
        for h in &child_hashes {
            buf.extend_from_slice(h);
        }
        buf.extend_from_slice(&scoring_bytes);
        buf.extend_from_slice(&meta_bytes);
        let result = shake256_256(&buf);

        // Store
        if let Some(node) = self.nodes.get_mut(id) {
            node.merkle_hash = result;
        }
    }

    /// Bottom-up Merkle recomputation for the entire tree.
    ///
    /// Uses a topological sort (leaves first) to ensure children are
    /// hashed before their parents. For non-leaf nodes, the parent's
    /// scoring is aggregated from its children via reward-weighted mean
    /// before hashing.
    pub fn recompute_all(&mut self) {
        // Collect all IDs with their depth (distance from root)
        let mut id_depth: Vec<(ResourceId, usize)> = self
            .nodes
            .keys()
            .map(|id| {
                let depth = self.ancestors(id).len();
                (id.clone(), depth)
            })
            .collect();

        // Sort deepest first (leaves before parents)
        id_depth.sort_by(|a, b| b.1.cmp(&a.1));

        // Recompute in order: aggregate children's scoring, then hash
        for (id, _) in &id_depth {
            // Aggregate children's scoring into parent for non-leaf nodes
            let child_scorings: Vec<NodeScoring> = {
                let node = match self.nodes.get(id) {
                    Some(n) => n,
                    None => continue,
                };
                if node.children.is_empty() {
                    Vec::new()
                } else {
                    node.children
                        .iter()
                        .filter_map(|cid| self.nodes.get(cid).map(|c| c.scoring))
                        .collect()
                }
            };
            if !child_scorings.is_empty() {
                let refs: Vec<&NodeScoring> = child_scorings.iter().collect();
                let aggregated = NodeScoring::aggregate(&refs);
                if let Some(node) = self.nodes.get_mut(id) {
                    node.scoring = aggregated;
                }
            }

            self.recompute_merkle(id);
        }
    }

    /// Set the scoring vector for a node and recompute its Merkle hash.
    ///
    /// Returns the old scoring, or `None` if the node was not found.
    pub fn update_scoring(&mut self, id: &ResourceId, scoring: NodeScoring) -> Option<NodeScoring> {
        let old = {
            let node = self.nodes.get_mut(id)?;
            let old = node.scoring;
            node.scoring = scoring;
            node.updated_at = chrono::Utc::now();
            old
        };
        self.recompute_merkle(id);
        Some(old)
    }

    /// EMA-blend an observation into a node's scoring and recompute its hash.
    ///
    /// Returns `true` if the node was found and updated.
    pub fn blend_scoring(
        &mut self,
        id: &ResourceId,
        observation: &NodeScoring,
        alpha: f32,
    ) -> bool {
        let found = if let Some(node) = self.nodes.get_mut(id) {
            node.scoring.blend(observation, alpha);
            node.updated_at = chrono::Utc::now();
            true
        } else {
            false
        };
        if found {
            self.recompute_merkle(id);
        }
        found
    }

    /// Return the Merkle hash of the root node.
    pub fn root_hash(&self) -> [u8; 32] {
        self.nodes
            .get(&ResourceId::root())
            .map(|n| n.merkle_hash)
            .unwrap_or([0u8; 32])
    }

    /// Number of nodes in the tree (including root).
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the tree contains only the root.
    pub fn is_empty(&self) -> bool {
        self.nodes.len() <= 1
    }

    /// Iterate over all nodes.
    pub fn iter(&self) -> impl Iterator<Item = (&ResourceId, &ResourceNode)> {
        self.nodes.iter()
    }

    /// Serialize the entire tree to a JSON-compatible structure.
    pub(crate) fn to_serializable(&self) -> Vec<&ResourceNode> {
        self.nodes.values().collect()
    }

    /// Rebuild from deserialized nodes.
    pub(crate) fn from_nodes(nodes: Vec<ResourceNode>) -> Self {
        let map: HashMap<ResourceId, ResourceNode> =
            nodes.into_iter().map(|n| (n.id.clone(), n)).collect();
        Self { nodes: map }
    }
}

impl Default for ResourceTree {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_tree_has_root() {
        let tree = ResourceTree::new();
        assert_eq!(tree.len(), 1);
        assert!(tree.get(&ResourceId::root()).is_some());
    }

    #[test]
    fn insert_and_get() {
        let mut tree = ResourceTree::new();
        tree.insert(
            ResourceId::new("/kernel"),
            ResourceKind::Namespace,
            ResourceId::root(),
        )
        .unwrap();

        assert_eq!(tree.len(), 2);
        let node = tree.get(&ResourceId::new("/kernel")).unwrap();
        assert_eq!(node.kind, ResourceKind::Namespace);
        assert_eq!(node.parent, Some(ResourceId::root()));
    }

    #[test]
    fn insert_duplicate_fails() {
        let mut tree = ResourceTree::new();
        tree.insert(
            ResourceId::new("/kernel"),
            ResourceKind::Namespace,
            ResourceId::root(),
        )
        .unwrap();

        let err = tree
            .insert(
                ResourceId::new("/kernel"),
                ResourceKind::Namespace,
                ResourceId::root(),
            )
            .unwrap_err();
        assert!(matches!(err, TreeError::AlreadyExists { .. }));
    }

    #[test]
    fn insert_missing_parent_fails() {
        let mut tree = ResourceTree::new();
        let err = tree
            .insert(
                ResourceId::new("/kernel/services"),
                ResourceKind::Namespace,
                ResourceId::new("/kernel"),
            )
            .unwrap_err();
        assert!(matches!(err, TreeError::ParentNotFound { .. }));
    }

    #[test]
    fn insert_invalid_path_fails() {
        let mut tree = ResourceTree::new();
        let err = tree
            .insert(
                ResourceId::new("no-slash"),
                ResourceKind::Service,
                ResourceId::root(),
            )
            .unwrap_err();
        assert!(matches!(err, TreeError::InvalidPath { .. }));
    }

    #[test]
    fn remove_leaf() {
        let mut tree = ResourceTree::new();
        tree.insert(
            ResourceId::new("/kernel"),
            ResourceKind::Namespace,
            ResourceId::root(),
        )
        .unwrap();

        let removed = tree.remove(ResourceId::new("/kernel")).unwrap();
        assert_eq!(removed.id, ResourceId::new("/kernel"));
        assert_eq!(tree.len(), 1);

        // Parent should no longer list it as child
        let root = tree.get(&ResourceId::root()).unwrap();
        assert!(root.children.is_empty());
    }

    #[test]
    fn remove_non_leaf_fails() {
        let mut tree = ResourceTree::new();
        tree.insert(
            ResourceId::new("/kernel"),
            ResourceKind::Namespace,
            ResourceId::root(),
        )
        .unwrap();
        tree.insert(
            ResourceId::new("/kernel/services"),
            ResourceKind::Namespace,
            ResourceId::new("/kernel"),
        )
        .unwrap();

        let err = tree.remove(ResourceId::new("/kernel")).unwrap_err();
        assert!(matches!(err, TreeError::NotEmpty { child_count: 1, .. }));
    }

    #[test]
    fn remove_root_fails() {
        let mut tree = ResourceTree::new();
        let err = tree.remove(ResourceId::root()).unwrap_err();
        assert!(matches!(err, TreeError::InvalidPath { .. }));
    }

    #[test]
    fn remove_nonexistent_fails() {
        let mut tree = ResourceTree::new();
        let err = tree.remove(ResourceId::new("/nonexistent")).unwrap_err();
        assert!(matches!(err, TreeError::NotFound { .. }));
    }

    #[test]
    fn children_query() {
        let mut tree = ResourceTree::new();
        tree.insert(
            ResourceId::new("/a"),
            ResourceKind::Namespace,
            ResourceId::root(),
        )
        .unwrap();
        tree.insert(
            ResourceId::new("/b"),
            ResourceKind::Namespace,
            ResourceId::root(),
        )
        .unwrap();

        let kids = tree.children(&ResourceId::root()).unwrap();
        assert_eq!(kids.len(), 2);
    }

    #[test]
    fn ancestors_walk() {
        let mut tree = ResourceTree::new();
        tree.insert(
            ResourceId::new("/kernel"),
            ResourceKind::Namespace,
            ResourceId::root(),
        )
        .unwrap();
        tree.insert(
            ResourceId::new("/kernel/services"),
            ResourceKind::Namespace,
            ResourceId::new("/kernel"),
        )
        .unwrap();
        tree.insert(
            ResourceId::new("/kernel/services/cron"),
            ResourceKind::Service,
            ResourceId::new("/kernel/services"),
        )
        .unwrap();

        let anc = tree.ancestors(&ResourceId::new("/kernel/services/cron"));
        assert_eq!(anc.len(), 3);
        assert_eq!(anc[0], ResourceId::new("/kernel/services"));
        assert_eq!(anc[1], ResourceId::new("/kernel"));
        assert_eq!(anc[2], ResourceId::root());
    }

    #[test]
    fn merkle_recompute_single() {
        let mut tree = ResourceTree::new();
        tree.insert(
            ResourceId::new("/a"),
            ResourceKind::Namespace,
            ResourceId::root(),
        )
        .unwrap();

        // Add metadata to /a
        tree.get_mut(&ResourceId::new("/a"))
            .unwrap()
            .metadata
            .insert("key".to_string(), serde_json::json!("value"));

        tree.recompute_merkle(&ResourceId::new("/a"));

        let hash = tree.get(&ResourceId::new("/a")).unwrap().merkle_hash;
        assert_ne!(hash, [0u8; 32]);
    }

    #[test]
    fn merkle_recompute_all_deterministic() {
        let mut tree = ResourceTree::new();
        tree.insert(
            ResourceId::new("/a"),
            ResourceKind::Namespace,
            ResourceId::root(),
        )
        .unwrap();
        tree.insert(
            ResourceId::new("/b"),
            ResourceKind::Namespace,
            ResourceId::root(),
        )
        .unwrap();

        tree.recompute_all();
        let hash1 = tree.root_hash();

        // Recompute again -- same result
        tree.recompute_all();
        let hash2 = tree.root_hash();

        assert_eq!(hash1, hash2);
        assert_ne!(hash1, [0u8; 32]);
    }

    #[test]
    fn merkle_changes_on_mutation() {
        let mut tree = ResourceTree::new();
        tree.insert(
            ResourceId::new("/a"),
            ResourceKind::Namespace,
            ResourceId::root(),
        )
        .unwrap();

        tree.recompute_all();
        let hash_before = tree.root_hash();

        // Add a child
        tree.insert(
            ResourceId::new("/b"),
            ResourceKind::Service,
            ResourceId::root(),
        )
        .unwrap();

        tree.recompute_all();
        let hash_after = tree.root_hash();

        assert_ne!(hash_before, hash_after);
    }

    #[test]
    fn is_empty_semantics() {
        let tree = ResourceTree::new();
        assert!(tree.is_empty()); // only root

        let mut tree2 = ResourceTree::new();
        tree2
            .insert(
                ResourceId::new("/x"),
                ResourceKind::Namespace,
                ResourceId::root(),
            )
            .unwrap();
        assert!(!tree2.is_empty());
    }

    #[test]
    fn default_tree() {
        let tree = ResourceTree::default();
        assert_eq!(tree.len(), 1);
    }

    // --- Scoring integration tests ---

    #[test]
    fn merkle_changes_when_scoring_changes() {
        let mut tree = ResourceTree::new();
        tree.insert(
            ResourceId::new("/a"),
            ResourceKind::Namespace,
            ResourceId::root(),
        )
        .unwrap();

        tree.recompute_all();
        let hash_before = tree.get(&ResourceId::new("/a")).unwrap().merkle_hash;

        // Change scoring
        tree.update_scoring(
            &ResourceId::new("/a"),
            NodeScoring::new(0.9, 0.9, 0.9, 0.9, 0.9, 0.9),
        );

        let hash_after = tree.get(&ResourceId::new("/a")).unwrap().merkle_hash;
        assert_ne!(hash_before, hash_after);
    }

    #[test]
    fn recompute_all_aggregates_children_scoring() {
        let mut tree = ResourceTree::new();
        tree.insert(
            ResourceId::new("/parent"),
            ResourceKind::Namespace,
            ResourceId::root(),
        )
        .unwrap();
        tree.insert(
            ResourceId::new("/parent/a"),
            ResourceKind::Agent,
            ResourceId::new("/parent"),
        )
        .unwrap();
        tree.insert(
            ResourceId::new("/parent/b"),
            ResourceKind::Agent,
            ResourceId::new("/parent"),
        )
        .unwrap();

        // Set child scorings with equal reward so uniform weighting
        tree.get_mut(&ResourceId::new("/parent/a")).unwrap().scoring =
            NodeScoring::new(1.0, 0.0, 0.5, 0.5, 0.5, 0.5);
        tree.get_mut(&ResourceId::new("/parent/b")).unwrap().scoring =
            NodeScoring::new(0.0, 1.0, 0.5, 0.5, 0.5, 0.5);

        tree.recompute_all();

        let parent = tree.get(&ResourceId::new("/parent")).unwrap();
        // With equal reward=0.5, trust = (1.0*0.5 + 0.0*0.5)/1.0 = 0.5
        assert!((parent.scoring.trust - 0.5).abs() < 1e-6);
        assert!((parent.scoring.performance - 0.5).abs() < 1e-6);
    }

    #[test]
    fn update_scoring_returns_old() {
        let mut tree = ResourceTree::new();
        tree.insert(
            ResourceId::new("/x"),
            ResourceKind::Service,
            ResourceId::root(),
        )
        .unwrap();

        let old = tree
            .update_scoring(
                &ResourceId::new("/x"),
                NodeScoring::new(0.9, 0.9, 0.9, 0.9, 0.9, 0.9),
            )
            .unwrap();

        // Old should be default (0.5 on all dims)
        assert!((old.trust - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn blend_scoring_ema_update() {
        let mut tree = ResourceTree::new();
        tree.insert(
            ResourceId::new("/y"),
            ResourceKind::Agent,
            ResourceId::root(),
        )
        .unwrap();

        let obs = NodeScoring::new(1.0, 1.0, 1.0, 1.0, 1.0, 1.0);
        tree.blend_scoring(&ResourceId::new("/y"), &obs, 0.5);

        let node = tree.get(&ResourceId::new("/y")).unwrap();
        // 0.5 * 0.5 + 1.0 * 0.5 = 0.75
        assert!((node.scoring.trust - 0.75).abs() < 1e-6);
    }

    #[test]
    fn blend_scoring_nonexistent_returns_false() {
        let mut tree = ResourceTree::new();
        let obs = NodeScoring::default();
        assert!(!tree.blend_scoring(&ResourceId::new("/nope"), &obs, 0.5));
    }
}
