//! The core resource tree data structure.

use std::collections::HashMap;

use sha3::{
    Shake256,
    digest::{ExtendableOutput, Update, XofReader},
};

/// Compute 256-bit (32-byte) SHAKE-256 hash.
/// Inlined from rvf-crypto to remove the external path dependency.
pub(crate) fn shake256_256(data: &[u8]) -> [u8; 32] {
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
    /// Flat path → node map. `pub(crate)` so [`crate::merkle`] can walk
    /// hashes without a public iterator API for every sync path (WEFT-106).
    pub(crate) nodes: HashMap<ResourceId, ResourceNode>,
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

    /// Load all node-attached ACL policies and delegation certs into a
    /// [`crate::permission::CapabilityChecker`] (WEFT-554).
    ///
    /// Existing checker entries are not cleared — call
    /// [`CapabilityChecker::new`] first for a clean load.
    pub fn load_acl_into(&self, checker: &crate::permission::CapabilityChecker) {
        for (id, node) in &self.nodes {
            for policy in &node.rbac_policies {
                checker.add_policy(id.clone(), policy.clone());
            }
            for cert in &node.delegation_certs {
                checker.add_delegation(id.clone(), cert.clone());
            }
        }
    }

    /// Attach a delegation cert to a node and mirror it into `checker`.
    pub fn grant_delegation(
        &mut self,
        resource: &ResourceId,
        cert: crate::delegation::DelegationCert,
        checker: &crate::permission::CapabilityChecker,
    ) -> TreeResult<()> {
        let node = self
            .nodes
            .get_mut(resource)
            .ok_or_else(|| TreeError::NotFound {
                id: resource.clone(),
            })?;
        node.delegation_certs.push(cert.clone());
        node.updated_at = chrono::Utc::now();
        checker.add_delegation(resource.clone(), cert);
        Ok(())
    }

    /// Revoke delegations for `grantee` on a node (and mirror into checker).
    pub fn revoke_delegation(
        &mut self,
        resource: &ResourceId,
        grantee: &crate::permission::Principal,
        checker: &crate::permission::CapabilityChecker,
    ) -> TreeResult<usize> {
        let node = self
            .nodes
            .get_mut(resource)
            .ok_or_else(|| TreeError::NotFound {
                id: resource.clone(),
            })?;
        let before = node.delegation_certs.len();
        node.delegation_certs
            .retain(|c| c.grantee != grantee.as_str());
        let removed_node = before - node.delegation_certs.len();
        node.updated_at = chrono::Utc::now();
        let removed_checker = checker.revoke_delegation(resource, grantee);
        Ok(removed_node.max(removed_checker))
    }

    /// Permission check via a [`CapabilityChecker`] (SPARC §5.1 / WEFT-554).
    pub fn check(
        &self,
        checker: &crate::permission::CapabilityChecker,
        principal: &crate::permission::Principal,
        role: &crate::model::Role,
        action: &crate::model::Action,
        resource: &ResourceId,
    ) -> crate::permission::Decision {
        let _ = self; // tree identity reserved for future direct node walks
        checker.check_permission(principal, role, action, resource)
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
    /// Does **not** clear the dirty flag — use [`Self::recompute_dirty`] or
    /// [`Self::recompute_path`] for incremental updates.
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

    /// Mark `id` and every ancestor dirty (O(depth)).
    ///
    /// Walks the full ancestor chain. Early-stops only when a node is already
    /// dirty **and** its parent is also dirty (or it is root), so the
    /// invariant "dirty ⇒ ancestors dirty" holds. Newly constructed nodes
    /// start dirty without parents marked — this path still propagates.
    pub fn mark_dirty(&mut self, id: &ResourceId) {
        let mut current = Some(id.clone());
        while let Some(cid) = current {
            let (parent, was_dirty) = match self.nodes.get_mut(&cid) {
                Some(node) => {
                    let was = node.dirty;
                    node.dirty = true;
                    (node.parent.clone(), was)
                }
                None => break,
            };
            if was_dirty {
                // Already dirty: stop only if the parent is dirty too
                // (or there is no parent). Otherwise keep climbing to
                // restore the dirty⇒ancestors-dirty invariant.
                let parent_ok = match &parent {
                    None => true,
                    Some(pid) => self.nodes.get(pid).map(|n| n.dirty).unwrap_or(false),
                };
                if parent_ok {
                    break;
                }
            }
            current = parent;
        }
    }

    /// Whether any node currently carries a dirty flag.
    pub fn has_dirty(&self) -> bool {
        self.nodes.values().any(|n| n.dirty)
    }

    /// Count of nodes with the dirty flag set.
    pub fn dirty_count(&self) -> usize {
        self.nodes.values().filter(|n| n.dirty).count()
    }

    /// Aggregate children's scoring into `id` when it is a non-leaf.
    ///
    /// Mirrors the scoring step used by [`Self::recompute_all`] so
    /// incremental and full recompute paths stay equivalent.
    fn aggregate_scoring_for(&mut self, id: &ResourceId) {
        let child_scorings: Vec<NodeScoring> = {
            let node = match self.nodes.get(id) {
                Some(n) => n,
                None => return,
            };
            if node.children.is_empty() {
                return;
            }
            node.children
                .iter()
                .filter_map(|cid| self.nodes.get(cid).map(|c| c.scoring))
                .collect()
        };
        if child_scorings.is_empty() {
            return;
        }
        let refs: Vec<&NodeScoring> = child_scorings.iter().collect();
        let aggregated = NodeScoring::aggregate(&refs);
        if let Some(node) = self.nodes.get_mut(id) {
            node.scoring = aggregated;
        }
    }

    /// Recompute Merkle hashes for dirty nodes only (bottom-up).
    ///
    /// Collects dirty IDs, sorts deepest-first so children hash before
    /// parents, aggregates scoring for non-leaves, hashes, and clears
    /// dirty. Returns the number of nodes recomputed.
    ///
    /// For a single mutation path this is O(depth). Multiple independent
    /// dirty subtrees recompute only those subtrees, not the whole tree.
    pub fn recompute_dirty(&mut self) -> usize {
        let mut id_depth: Vec<(ResourceId, usize)> = self
            .nodes
            .iter()
            .filter(|(_, n)| n.dirty)
            .map(|(id, _)| {
                let depth = self.ancestors(id).len();
                (id.clone(), depth)
            })
            .collect();

        if id_depth.is_empty() {
            return 0;
        }

        // Deepest first (leaves before parents)
        id_depth.sort_by_key(|b| std::cmp::Reverse(b.1));

        let count = id_depth.len();
        for (id, _) in &id_depth {
            self.aggregate_scoring_for(id);
            self.recompute_merkle(id);
            if let Some(node) = self.nodes.get_mut(id) {
                node.dirty = false;
            }
        }
        count
    }

    /// Mark the path from `id` to root dirty and recompute only that path.
    ///
    /// Primary mutation entry point for structural changes (insert / remove /
    /// meta) — O(depth) rather than O(tree). Non-leaf nodes on the path
    /// (including `id`) re-aggregate child scoring before hashing, matching
    /// [`Self::recompute_all`].
    ///
    /// Returns the number of nodes recomputed.
    pub fn recompute_path(&mut self, id: &ResourceId) -> usize {
        self.recompute_path_inner(id, /*aggregate_origin=*/ true)
    }

    /// Like [`Self::recompute_path`], but keeps `id`'s scoring vector as-is.
    ///
    /// Used by [`Self::update_scoring`] / [`Self::blend_scoring`] so an
    /// explicit observation on a non-leaf is not immediately overwritten by
    /// child aggregation. Ancestors still re-aggregate (O(depth)).
    pub fn recompute_path_preserve_scoring(&mut self, id: &ResourceId) -> usize {
        self.recompute_path_inner(id, /*aggregate_origin=*/ false)
    }

    fn recompute_path_inner(&mut self, id: &ResourceId, aggregate_origin: bool) -> usize {
        if !self.nodes.contains_key(id) {
            return 0;
        }
        self.mark_dirty(id);

        // Origin first (deepest).
        if aggregate_origin {
            self.aggregate_scoring_for(id);
        }
        self.recompute_merkle(id);
        if let Some(node) = self.nodes.get_mut(id) {
            node.dirty = false;
        }

        // Ancestors: parent → … → root (already deepest-first for a path).
        let ancestors = self.ancestors(id);
        let mut count = 1usize;
        for anc in &ancestors {
            self.aggregate_scoring_for(anc);
            self.recompute_merkle(anc);
            if let Some(node) = self.nodes.get_mut(anc) {
                node.dirty = false;
            }
            count += 1;
        }
        count
    }

    /// Bottom-up Merkle recomputation for the entire tree.
    ///
    /// Uses a topological sort (leaves first) to ensure children are
    /// hashed before their parents. For non-leaf nodes, the parent's
    /// scoring is aggregated from its children via reward-weighted mean
    /// before hashing. Clears all dirty flags.
    ///
    /// Prefer [`Self::recompute_path`] for per-mutation updates (WEFT-145).
    /// Keep `recompute_all` for bootstrap, checkpoint restore, and tests
    /// that need a full consistency pass.
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
        id_depth.sort_by_key(|b| std::cmp::Reverse(b.1));

        // Recompute in order: aggregate children's scoring, then hash
        for (id, _) in &id_depth {
            self.aggregate_scoring_for(id);
            self.recompute_merkle(id);
            if let Some(node) = self.nodes.get_mut(id) {
                node.dirty = false;
            }
        }
    }

    /// Set the scoring vector for a node and recompute its Merkle path.
    ///
    /// Returns the old scoring, or `None` if the node was not found.
    /// Bubbles the hash update to the root (O(depth)). Preserves the
    /// explicit scoring on `id` (see [`Self::recompute_path_preserve_scoring`]).
    pub fn update_scoring(&mut self, id: &ResourceId, scoring: NodeScoring) -> Option<NodeScoring> {
        let old = {
            let node = self.nodes.get_mut(id)?;
            let old = node.scoring;
            node.scoring = scoring;
            node.updated_at = chrono::Utc::now();
            old
        };
        self.recompute_path_preserve_scoring(id);
        Some(old)
    }

    /// EMA-blend an observation into a node's scoring and recompute its path.
    ///
    /// Returns `true` if the node was found and updated.
    /// Bubbles the hash update to the root (O(depth)). Preserves the
    /// blended scoring on `id` (see [`Self::recompute_path_preserve_scoring`]).
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
            self.recompute_path_preserve_scoring(id);
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

    // --- WEFT-145: incremental Merkle updates ---

    /// Build a balanced-ish tree with `width` children under root and
    /// `depth` levels of single-child chains under each branch.
    fn build_wide_tree(width: usize, depth: usize) -> ResourceTree {
        let mut tree = ResourceTree::new();
        for w in 0..width {
            let mut parent = ResourceId::root();
            for d in 0..depth {
                let id = if d == 0 {
                    ResourceId::new(format!("/b{w}"))
                } else {
                    ResourceId::new(format!("/b{w}/d{d}"))
                };
                tree.insert(id.clone(), ResourceKind::Namespace, parent)
                    .unwrap();
                parent = id;
            }
        }
        tree.recompute_all();
        tree
    }

    #[test]
    fn mark_dirty_propagates_to_root() {
        let mut tree = ResourceTree::new();
        tree.insert(
            ResourceId::new("/a"),
            ResourceKind::Namespace,
            ResourceId::root(),
        )
        .unwrap();
        tree.insert(
            ResourceId::new("/a/b"),
            ResourceKind::Namespace,
            ResourceId::new("/a"),
        )
        .unwrap();
        tree.recompute_all();
        assert!(!tree.has_dirty());

        tree.mark_dirty(&ResourceId::new("/a/b"));
        assert!(tree.get(&ResourceId::new("/a/b")).unwrap().dirty);
        assert!(tree.get(&ResourceId::new("/a")).unwrap().dirty);
        assert!(tree.get(&ResourceId::root()).unwrap().dirty);
        assert_eq!(tree.dirty_count(), 3);
    }

    #[test]
    fn mark_dirty_early_exit_when_ancestor_already_dirty() {
        let mut tree = ResourceTree::new();
        tree.insert(
            ResourceId::new("/a"),
            ResourceKind::Namespace,
            ResourceId::root(),
        )
        .unwrap();
        tree.insert(
            ResourceId::new("/a/b"),
            ResourceKind::Service,
            ResourceId::new("/a"),
        )
        .unwrap();
        tree.recompute_all();

        tree.mark_dirty(&ResourceId::new("/a"));
        assert_eq!(tree.dirty_count(), 2); // /a + root
        // Marking child expands only the child; parent already dirty
        tree.mark_dirty(&ResourceId::new("/a/b"));
        assert!(tree.get(&ResourceId::new("/a/b")).unwrap().dirty);
        assert_eq!(tree.dirty_count(), 3); // + /a/b only
    }

    #[test]
    fn mark_dirty_propagates_from_new_node_already_dirty() {
        // ResourceNode::new starts dirty without ancestors marked.
        let mut tree = ResourceTree::new();
        tree.recompute_all(); // clear root dirty
        tree.insert(
            ResourceId::new("/a"),
            ResourceKind::Namespace,
            ResourceId::root(),
        )
        .unwrap();
        assert!(tree.get(&ResourceId::new("/a")).unwrap().dirty);
        assert!(!tree.get(&ResourceId::root()).unwrap().dirty);

        tree.mark_dirty(&ResourceId::new("/a"));
        assert!(tree.get(&ResourceId::root()).unwrap().dirty);
        assert_eq!(tree.dirty_count(), 2);
    }

    #[test]
    fn recompute_path_matches_recompute_all_root_hash() {
        let mut incr = ResourceTree::new();
        let mut full = ResourceTree::new();
        for path in ["/a", "/a/b", "/a/b/c", "/x", "/x/y"] {
            let id = ResourceId::new(path);
            let parent = id.parent().unwrap();
            incr.insert(id.clone(), ResourceKind::Namespace, parent.clone())
                .unwrap();
            full.insert(id, ResourceKind::Namespace, parent).unwrap();
        }

        // Seed both with full recompute
        incr.recompute_all();
        full.recompute_all();
        assert_eq!(incr.root_hash(), full.root_hash());

        // Mutate a deep leaf on both trees the same way
        for tree in [&mut incr, &mut full] {
            tree.get_mut(&ResourceId::new("/a/b/c"))
                .unwrap()
                .metadata
                .insert("k".into(), serde_json::json!("v"));
        }

        let recomputed = incr.recompute_path(&ResourceId::new("/a/b/c"));
        assert_eq!(recomputed, 4); // /a/b/c, /a/b, /a, /
        assert!(!incr.has_dirty());

        full.recompute_all();
        assert_eq!(incr.root_hash(), full.root_hash());

        // Per-node hashes must match too
        for (id, node) in incr.iter() {
            let other = full.get(id).unwrap();
            assert_eq!(
                node.merkle_hash, other.merkle_hash,
                "hash mismatch at {id}"
            );
            assert_eq!(
                node.scoring.as_array(),
                other.scoring.as_array(),
                "scoring mismatch at {id}"
            );
        }
    }

    #[test]
    fn recompute_path_after_insert_matches_full() {
        let mut incr = build_wide_tree(4, 3);
        let mut full = build_wide_tree(4, 3);
        assert_eq!(incr.root_hash(), full.root_hash());

        let leaf = ResourceId::new("/b0/d1/leaf");
        let parent = ResourceId::new("/b0/d1");
        incr.insert(leaf.clone(), ResourceKind::Service, parent.clone())
            .unwrap();
        full.insert(leaf.clone(), ResourceKind::Service, parent)
            .unwrap();

        let n = incr.recompute_path(&leaf);
        // leaf + /b0/d1 + /b0 + root = 4 (depth chain)
        assert_eq!(n, 4);
        full.recompute_all();
        assert_eq!(incr.root_hash(), full.root_hash());
    }

    #[test]
    fn recompute_path_after_remove_matches_full() {
        let mut incr = build_wide_tree(3, 3);
        let mut full = build_wide_tree(3, 3);

        // remove the deepest leaf under b1
        let deepest = ResourceId::new("/b1/d2");
        let parent = ResourceId::new("/b1/d1");
        incr.remove(deepest.clone()).unwrap();
        full.remove(deepest).unwrap();

        let n = incr.recompute_path(&parent);
        // parent /b1/d1 + /b1 + root
        assert_eq!(n, 3);
        // sibling branches stay clean
        assert!(!incr.get(&ResourceId::new("/b0")).unwrap().dirty);
        full.recompute_all();
        assert_eq!(incr.root_hash(), full.root_hash());
    }

    #[test]
    fn recompute_path_scoring_update_matches_full() {
        let mut incr = build_wide_tree(2, 4);
        let mut full = build_wide_tree(2, 4);
        // depth=4 ⇒ chain /b0 → /b0/d1 → /b0/d2 → /b0/d3
        let target = ResourceId::new("/b0/d3");
        let scoring = NodeScoring::new(0.9, 0.1, 0.5, 0.8, 0.5, 0.5);

        incr.update_scoring(&target, scoring);
        full.get_mut(&target).unwrap().scoring = scoring;
        full.recompute_all();

        assert_eq!(incr.root_hash(), full.root_hash());
        // Parent scoring must have been re-aggregated
        let parent = ResourceId::new("/b0/d2");
        assert_eq!(
            incr.get(&parent).unwrap().scoring.as_array(),
            full.get(&parent).unwrap().scoring.as_array()
        );
    }

    #[test]
    fn recompute_path_is_o_depth_not_o_size() {
        // Large tree: 50 branches × 20 depth ≈ 1000+ nodes
        let width = 50;
        let depth = 20;
        let mut tree = build_wide_tree(width, depth);
        let n_nodes = tree.len();
        assert!(n_nodes > 900, "expected large tree, got {n_nodes}");

        // Touch one deep leaf
        let leaf = ResourceId::new(format!("/b0/d{}", depth - 1));
        tree.get_mut(&leaf)
            .unwrap()
            .metadata
            .insert("touch".into(), serde_json::json!(1));

        let recomputed = tree.recompute_path(&leaf);
        // Path length = depth levels under branch + root = depth + 1
        // (b0, d1..d{depth-1}, root) = depth + 1? 
        // structure: /b0 (d=0), /b0/d1, ..., /b0/d{depth-1}
        // ancestors of leaf: d{depth-2}..d1, b0, root → depth nodes on path total
        // path nodes = depth (under branch) + 0? Wait:
        // depth=20 → nodes: /b0 + /b0/d1..d19 = 20 nodes under root, + root = 21
        assert_eq!(
            recomputed,
            depth + 1,
            "expected O(depth)={}+1, got {recomputed} (tree size {n_nodes})",
            depth
        );
        assert!(
            recomputed * 10 < n_nodes,
            "recomputed {recomputed} is not << tree size {n_nodes}"
        );
        assert!(!tree.has_dirty());
    }

    #[test]
    fn recompute_dirty_noop_when_clean() {
        let mut tree = build_wide_tree(2, 2);
        assert_eq!(tree.recompute_dirty(), 0);
    }

    #[test]
    fn multi_branch_dirty_only_touches_dirty_paths() {
        let mut tree = build_wide_tree(5, 5);
        tree.mark_dirty(&ResourceId::new("/b0/d4"));
        tree.mark_dirty(&ResourceId::new("/b2/d4"));
        // Each path: leaf + 4 ancestors under branch + root, but root shared
        // b0/d4, b0/d3, b0/d2, b0/d1, b0, root = 6
        // b2/d4, b2/d3, b2/d2, b2/d1, b2 = 5 (root already dirty)
        assert_eq!(tree.dirty_count(), 11);

        let n = tree.recompute_dirty();
        assert_eq!(n, 11);
        assert!(!tree.get(&ResourceId::new("/b1")).unwrap().dirty);
        assert!(!tree.get(&ResourceId::new("/b3/d2")).unwrap().dirty);
    }

    // --- WEFT-554: tree ↔ CapabilityChecker ACL wiring ---

    #[test]
    fn grant_delegation_on_node_and_check() {
        use crate::delegation::DelegationCert;
        use crate::did::Did;
        use crate::model::{Action, Role};
        use crate::permission::{CapabilityChecker, Decision, Principal};
        use chrono::{Duration, Utc};
        use ed25519_dalek::SigningKey;

        let mut tree = ResourceTree::new();
        let apps = ResourceId::new("/apps");
        tree.insert(apps.clone(), ResourceKind::Namespace, ResourceId::root())
            .unwrap();

        let checker = CapabilityChecker::new();
        let key = SigningKey::from_bytes(&[11u8; 32]);
        let worker = Principal::from_did(Did::weft("worker").unwrap());
        let cert = DelegationCert::grant(
            "did:weft:admin",
            worker.as_str(),
            apps.clone(),
            &key,
            Some(Utc::now() + Duration::hours(1)),
            None,
        );
        tree.grant_delegation(&apps, cert, &checker).unwrap();
        assert_eq!(tree.get(&apps).unwrap().delegation_certs().len(), 1);

        let d = tree.check(
            &checker,
            &worker,
            &Role::Operator,
            &Action::Read,
            &apps,
        );
        assert_eq!(d, Decision::Delegate);

        let n = tree
            .revoke_delegation(&apps, &worker, &checker)
            .unwrap();
        assert_eq!(n, 1);
        assert_eq!(
            tree.check(&checker, &worker, &Role::Operator, &Action::Read, &apps),
            Decision::Deny
        );
    }

    #[test]
    fn load_acl_into_checker_from_node_policies() {
        use crate::model::Action;
        use crate::permission::{AclPolicy, CapabilityChecker, Decision, Principal};
        use crate::model::Role;

        let mut tree = ResourceTree::new();
        let path = ResourceId::new("/svc");
        tree.insert(path.clone(), ResourceKind::Service, ResourceId::root())
            .unwrap();
        let alice = Principal::from_did(crate::did::Did::weft("alice").unwrap());
        tree.get_mut(&path)
            .unwrap()
            .rbac_policies
            .push(AclPolicy::allow(alice.clone(), Action::Read));

        let checker = CapabilityChecker::new();
        tree.load_acl_into(&checker);
        assert_eq!(
            checker.check_permission(&alice, &Role::Viewer, &Action::Read, &path),
            Decision::Allow
        );
    }
}
