//! Bootstrap and checkpoint functions for the resource tree.

use crate::error::{TreeError, TreeResult};
use crate::model::{ResourceId, ResourceKind, ResourceNode};
use crate::tree::ResourceTree;

/// Create the well-known WeftOS namespaces on a fresh tree.
///
/// Creates:
/// - `/kernel`
/// - `/kernel/services`
/// - `/kernel/processes`
/// - `/kernel/agents`
/// - `/network`
/// - `/network/peers`
/// - `/apps`
/// - `/environments`
pub fn bootstrap_fresh(tree: &mut ResourceTree) -> TreeResult<()> {
    let root = ResourceId::root();

    tree.insert(
        ResourceId::new("/kernel"),
        ResourceKind::Namespace,
        root.clone(),
    )?;
    tree.insert(
        ResourceId::new("/kernel/services"),
        ResourceKind::Namespace,
        ResourceId::new("/kernel"),
    )?;
    tree.insert(
        ResourceId::new("/kernel/processes"),
        ResourceKind::Namespace,
        ResourceId::new("/kernel"),
    )?;
    tree.insert(
        ResourceId::new("/kernel/agents"),
        ResourceKind::Namespace,
        ResourceId::new("/kernel"),
    )?;
    tree.insert(
        ResourceId::new("/network"),
        ResourceKind::Namespace,
        root.clone(),
    )?;
    tree.insert(
        ResourceId::new("/network/peers"),
        ResourceKind::Namespace,
        ResourceId::new("/network"),
    )?;
    tree.insert(
        ResourceId::new("/apps"),
        ResourceKind::Namespace,
        root.clone(),
    )?;
    tree.insert(
        ResourceId::new("/environments"),
        ResourceKind::Namespace,
        root,
    )?;

    // Recompute Merkle hashes after bootstrap
    tree.recompute_all();

    Ok(())
}

/// Deserialize a tree from a JSON checkpoint.
pub fn from_checkpoint(data: &[u8]) -> TreeResult<ResourceTree> {
    let nodes: Vec<ResourceNode> = serde_json::from_slice(data)
        .map_err(|e| TreeError::Checkpoint(format!("failed to deserialize: {e}")))?;

    if nodes.is_empty() {
        return Err(TreeError::Checkpoint("checkpoint is empty".to_string()));
    }

    Ok(ResourceTree::from_nodes(nodes))
}

/// Serialize the tree to a JSON checkpoint.
pub fn to_checkpoint(tree: &ResourceTree) -> TreeResult<Vec<u8>> {
    let nodes = tree.to_serializable();
    serde_json::to_vec(&nodes)
        .map_err(|e| TreeError::Checkpoint(format!("failed to serialize: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_creates_well_known_namespaces() {
        let mut tree = ResourceTree::new();
        bootstrap_fresh(&mut tree).unwrap();

        // Should have root + 8 well-known namespaces = 9 total
        assert_eq!(tree.len(), 9);

        assert!(tree.get(&ResourceId::new("/kernel")).is_some());
        assert!(tree.get(&ResourceId::new("/kernel/services")).is_some());
        assert!(tree.get(&ResourceId::new("/kernel/processes")).is_some());
        assert!(tree.get(&ResourceId::new("/kernel/agents")).is_some());
        assert!(tree.get(&ResourceId::new("/network")).is_some());
        assert!(tree.get(&ResourceId::new("/network/peers")).is_some());
        assert!(tree.get(&ResourceId::new("/apps")).is_some());
        assert!(tree.get(&ResourceId::new("/environments")).is_some());

        // Root hash should be non-zero after bootstrap
        assert_ne!(tree.root_hash(), [0u8; 32]);
    }

    #[test]
    fn bootstrap_idempotent_fails_on_second_call() {
        let mut tree = ResourceTree::new();
        bootstrap_fresh(&mut tree).unwrap();

        // Second call should fail because nodes already exist
        let err = bootstrap_fresh(&mut tree).unwrap_err();
        assert!(matches!(err, TreeError::AlreadyExists { .. }));
    }

    #[test]
    fn checkpoint_roundtrip() {
        let mut tree = ResourceTree::new();
        bootstrap_fresh(&mut tree).unwrap();

        let data = to_checkpoint(&tree).unwrap();
        let restored = from_checkpoint(&data).unwrap();

        assert_eq!(restored.len(), tree.len());
        assert!(restored.get(&ResourceId::new("/kernel")).is_some());
        assert!(restored.get(&ResourceId::new("/apps")).is_some());
    }

    #[test]
    fn checkpoint_empty_data_fails() {
        let err = from_checkpoint(b"[]").unwrap_err();
        assert!(matches!(err, TreeError::Checkpoint(_)));
    }

    #[test]
    fn checkpoint_invalid_json_fails() {
        let err = from_checkpoint(b"not json").unwrap_err();
        assert!(matches!(err, TreeError::Checkpoint(_)));
    }

    #[test]
    fn checkpoint_preserves_merkle_hashes() {
        let mut tree = ResourceTree::new();
        bootstrap_fresh(&mut tree).unwrap();

        let original_hash = tree.root_hash();
        let data = to_checkpoint(&tree).unwrap();
        let restored = from_checkpoint(&data).unwrap();

        assert_eq!(restored.root_hash(), original_hash);
    }
}
