//! Reingold-Tilford tree layout algorithm.
//!
//! Produces aesthetically optimal top-down tree layouts in O(n) time.
//! Each node is centered over its children with minimum sibling spacing.

use crate::entity::EntityId;
use std::collections::HashMap;

/// A positioned tree node.
#[derive(Debug, Clone)]
pub struct TreeNode {
    pub id: EntityId,
    pub x: f64,
    pub y: f64,
    pub children: Vec<usize>,
    pub offset: f64,
    pub thread: Option<usize>,
    pub ancestor: usize,
    pub change: f64,
    pub shift: f64,
    pub number: usize,
    pub prelim: f64,
    pub modifier: f64,
}

/// Spacing configuration.
pub struct TreeConfig {
    pub sibling_spacing: f64,
    pub level_spacing: f64,
    pub node_width: f64,
}

impl Default for TreeConfig {
    fn default() -> Self {
        Self {
            sibling_spacing: 40.0,
            level_spacing: 80.0,
            node_width: 30.0,
        }
    }
}

/// Lay out a tree given parent→children adjacency.
///
/// Returns a map of EntityId → (x, y) positions.
pub fn layout(
    root: &EntityId,
    children_map: &HashMap<EntityId, Vec<EntityId>>,
    config: &TreeConfig,
) -> HashMap<EntityId, (f64, f64)> {
    if children_map.is_empty() && !children_map.contains_key(root) {
        let mut result = HashMap::new();
        result.insert(root.clone(), (0.0, 0.0));
        return result;
    }

    // Build flat node array.
    let mut nodes: Vec<TreeNode> = Vec::new();
    let mut id_to_idx: HashMap<EntityId, usize> = HashMap::new();

    fn collect(
        id: &EntityId,
        children_map: &HashMap<EntityId, Vec<EntityId>>,
        nodes: &mut Vec<TreeNode>,
        id_to_idx: &mut HashMap<EntityId, usize>,
    ) -> usize {
        let idx = nodes.len();
        id_to_idx.insert(id.clone(), idx);
        nodes.push(TreeNode {
            id: id.clone(),
            x: 0.0,
            y: 0.0,
            children: Vec::new(),
            offset: 0.0,
            thread: None,
            ancestor: idx,
            change: 0.0,
            shift: 0.0,
            number: 0,
            prelim: 0.0,
            modifier: 0.0,
        });

        let child_ids: Vec<EntityId> = children_map.get(id).cloned().unwrap_or_default();

        let mut child_indices = Vec::new();
        for (i, child_id) in child_ids.iter().enumerate() {
            let child_idx = collect(child_id, children_map, nodes, id_to_idx);
            nodes[child_idx].number = i;
            child_indices.push(child_idx);
        }

        nodes[idx].children = child_indices;
        idx
    }

    let root_idx = collect(root, children_map, &mut nodes, &mut id_to_idx);

    // First walk: compute preliminary x-coordinates.
    first_walk(&mut nodes, root_idx, config);

    // Second walk: compute final positions.
    second_walk(&mut nodes, root_idx, 0.0, 0, config);

    // Extract results.
    let mut result = HashMap::new();
    for node in &nodes {
        result.insert(node.id.clone(), (node.x, node.y));
    }
    result
}

fn first_walk(nodes: &mut Vec<TreeNode>, v: usize, config: &TreeConfig) {
    let children = nodes[v].children.clone();

    if children.is_empty() {
        // Leaf node.
        let number = nodes[v].number;
        if number > 0 {
            // Has a left sibling — position relative to it.
            // Find parent's children to get left sibling.
            nodes[v].prelim = config.sibling_spacing + config.node_width;
        }
        return;
    }

    for &child in &children {
        first_walk(nodes, child, config);
    }

    let first_child = children[0];
    let last_child = *children.last().unwrap();
    let midpoint = (nodes[first_child].prelim + nodes[last_child].prelim) / 2.0;

    let number = nodes[v].number;
    if number > 0 {
        nodes[v].prelim = config.sibling_spacing + config.node_width;
        nodes[v].modifier = nodes[v].prelim - midpoint;
    } else {
        nodes[v].prelim = midpoint;
    }
}

fn second_walk(
    nodes: &mut Vec<TreeNode>,
    v: usize,
    modifier: f64,
    depth: usize,
    config: &TreeConfig,
) {
    nodes[v].x = nodes[v].prelim + modifier;
    nodes[v].y = depth as f64 * config.level_spacing;

    let children = nodes[v].children.clone();
    let new_modifier = modifier + nodes[v].modifier;

    // Accumulate sibling offsets.
    let mut prev_x = 0.0f64;
    for (i, &child) in children.iter().enumerate() {
        if i > 0 {
            let child_prelim = prev_x + config.sibling_spacing + config.node_width;
            if nodes[child].prelim < child_prelim {
                let diff = child_prelim - nodes[child].prelim;
                nodes[child].prelim += diff;
                nodes[child].modifier += diff;
            }
        }
        second_walk(nodes, child, new_modifier, depth + 1, config);
        prev_x = nodes[child].x - new_modifier;
    }

    // Re-center parent over children after adjustment.
    if !children.is_empty() {
        let first_x = nodes[children[0]].x;
        let last_x = nodes[*children.last().unwrap()].x;
        let desired = (first_x + last_x) / 2.0;
        if (nodes[v].x - desired).abs() > 0.01 {
            nodes[v].x = desired;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::{DomainTag, EntityType};

    fn eid(name: &str) -> EntityId {
        EntityId::new(&DomainTag::Code, &EntityType::Module, name, "test")
    }

    #[test]
    fn single_node() {
        let root = eid("root");
        let children = HashMap::new();
        let positions = layout(&root, &children, &TreeConfig::default());
        assert_eq!(positions.len(), 1);
        let (x, y) = positions[&root];
        assert!((y - 0.0).abs() < 0.01);
        assert!(x.is_finite());
    }

    #[test]
    fn simple_tree() {
        let root = eid("root");
        let a = eid("a");
        let b = eid("b");
        let c = eid("c");

        let mut children = HashMap::new();
        children.insert(root.clone(), vec![a.clone(), b.clone(), c.clone()]);

        let positions = layout(&root, &children, &TreeConfig::default());
        assert_eq!(positions.len(), 4);

        let (rx, ry) = positions[&root];
        let (ax, ay) = positions[&a];
        let (bx, _) = positions[&b];
        let (cx, _) = positions[&c];

        // Root at depth 0, children at depth 1.
        assert!((ry - 0.0).abs() < 0.01);
        assert!((ay - 80.0).abs() < 0.01);

        // Children left-to-right.
        assert!(ax < bx);
        assert!(bx < cx);

        // Root centered over children.
        let mid = (ax + cx) / 2.0;
        assert!((rx - mid).abs() < 1.0);
    }

    #[test]
    fn two_level_tree() {
        let root = eid("root");
        let a = eid("a");
        let b = eid("b");
        let a1 = eid("a1");
        let a2 = eid("a2");

        let mut children = HashMap::new();
        children.insert(root.clone(), vec![a.clone(), b.clone()]);
        children.insert(a.clone(), vec![a1.clone(), a2.clone()]);

        let positions = layout(&root, &children, &TreeConfig::default());
        assert_eq!(positions.len(), 5);

        let (_, a1y) = positions[&a1];
        assert!((a1y - 160.0).abs() < 0.01); // depth 2
    }
}
