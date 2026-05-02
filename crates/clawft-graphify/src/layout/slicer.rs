//! Graph slicer — extract layers from a knowledge graph for progressive
//! drill-down navigation.
//!
//! Level 0 = top-level nodes (no parent via Contains)
//! Level 1 = children of a level-0 node
//! Level N = children of a level-(N-1) node
//!
//! Each slice is a small, self-contained subgraph that can be laid out
//! and rendered independently.

use std::collections::{HashMap, HashSet};

use crate::entity::EntityId;
use crate::model::KnowledgeGraph;
use crate::relationship::RelationType;
use crate::topology::TopologySchema;

use super::positioned::{PositionedGraph, PositionedNode, PositionedEdge, Rect};

/// A slice of the graph at a particular depth.
#[derive(Debug, Clone, serde::Serialize)]
pub struct GraphSlice {
    /// Positioned graph for this slice.
    pub graph: PositionedGraph,
    /// Entity IDs of nodes that have children (can be drilled into).
    pub expandable: Vec<String>,
    /// Breadcrumb trail: list of (id, label) pairs from root to current.
    pub breadcrumbs: Vec<BreadcrumbEntry>,
    /// Total nodes in the full graph (for context).
    pub total_nodes: usize,
    /// Depth level of this slice.
    pub depth: usize,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct BreadcrumbEntry {
    pub id: String,
    pub label: String,
}

/// Extract the top-level slice: nodes with no incoming Contains edges.
pub fn top_level_slice(
    kg: &KnowledgeGraph,
    schema: &TopologySchema,
    width: f64,
    height: f64,
) -> GraphSlice {
    let has_parent: HashSet<EntityId> = kg.edges()
        .filter(|(_, _, rel)| matches!(rel.relation_type, RelationType::Contains))
        .map(|(_, tgt, _)| tgt.id.clone())
        .collect();

    // Filter to nodes that have children (containers) at the top level.
    // Leaf nodes without parents (like imports) are only shown when
    // drilling into their sibling container.
    let containers: std::collections::HashSet<EntityId> = kg.edges()
        .filter(|(_, _, rel)| matches!(rel.relation_type, RelationType::Contains))
        .map(|(src, _, _)| src.id.clone())
        .collect();

    let top_nodes: Vec<EntityId> = kg.entities()
        .filter(|e| !has_parent.contains(&e.id) && containers.contains(&e.id))
        .map(|e| e.id.clone())
        .collect();

    build_slice(kg, schema, &top_nodes, &[], width, height, 0)
}

/// Extract the children slice of a specific node.
pub fn children_slice(
    kg: &KnowledgeGraph,
    schema: &TopologySchema,
    parent_id: &EntityId,
    breadcrumbs: &[BreadcrumbEntry],
    width: f64,
    height: f64,
) -> GraphSlice {
    let children: Vec<EntityId> = kg.edges()
        .filter(|(src, _, rel)| {
            src.id == *parent_id && matches!(rel.relation_type, RelationType::Contains)
        })
        .map(|(_, tgt, _)| tgt.id.clone())
        .collect();

    let depth = breadcrumbs.len() + 1;
    build_slice(kg, schema, &children, breadcrumbs, width, height, depth)
}

fn build_slice(
    kg: &KnowledgeGraph,
    schema: &TopologySchema,
    node_ids: &[EntityId],
    breadcrumbs: &[BreadcrumbEntry],
    width: f64,
    height: f64,
    depth: usize,
) -> GraphSlice {
    let node_set: HashSet<&EntityId> = node_ids.iter().collect();

    // Collect entities for this slice.
    let entities: Vec<_> = kg.entities()
        .filter(|e| node_set.contains(&e.id))
        .collect();

    // Which nodes have children?
    let children_of: HashMap<EntityId, usize> = {
        let mut map = HashMap::new();
        for (src, _, rel) in kg.edges() {
            if matches!(rel.relation_type, RelationType::Contains) && node_set.contains(&src.id) {
                *map.entry(src.id.clone()).or_default() += 1;
            }
        }
        map
    };

    let expandable: Vec<String> = children_of.keys().map(|id| id.to_hex()).collect();

    // Lay out just these nodes.
    let slice_ids: Vec<EntityId> = entities.iter().map(|e| e.id.clone()).collect();
    let id_to_idx: HashMap<&EntityId, usize> = slice_ids.iter().enumerate().map(|(i, id)| (id, i)).collect();

    // Build ancestor map: for any entity, find which slice node it belongs to.
    // This lets us aggregate child-to-child edges onto their parent containers.
    let mut entity_to_slice_node: HashMap<EntityId, EntityId> = HashMap::new();
    for id in node_ids {
        entity_to_slice_node.insert(id.clone(), id.clone());
    }
    fn map_descendants(
        kg: &KnowledgeGraph,
        parent_slice_id: &EntityId,
        parent_id: &EntityId,
        mapping: &mut HashMap<EntityId, EntityId>,
    ) {
        for (src, tgt, rel) in kg.edges() {
            if src.id == *parent_id && matches!(rel.relation_type, RelationType::Contains) {
                mapping.insert(tgt.id.clone(), parent_slice_id.clone());
                map_descendants(kg, parent_slice_id, &tgt.id, mapping);
            }
        }
    }
    for id in node_ids {
        map_descendants(kg, id, id, &mut entity_to_slice_node);
    }

    // Collect edges in three categories:
    // 1. Direct edges between nodes in this slice
    // 2. Edges between descendants aggregated to their slice-level ancestor
    // 3. Portal edges: external nodes that connect to nodes in this slice
    let mut edge_pairs: Vec<(usize, usize)> = Vec::new();
    let mut slice_edges: Vec<(EntityId, EntityId, String)> = Vec::new();
    let mut seen_edge_keys: HashSet<(EntityId, EntityId, String)> = HashSet::new();

    // Also track portal connections (edges from/to outside the slice).
    let mut portal_sources: HashMap<EntityId, Vec<String>> = HashMap::new();

    for (src, tgt, rel) in kg.edges() {
        if matches!(rel.relation_type, RelationType::Contains) {
            continue;
        }

        let rel_label = format!("{:?}", rel.relation_type).to_lowercase();

        // Direct edges between slice nodes.
        if node_set.contains(&src.id) && node_set.contains(&tgt.id) {
            let key = (src.id.clone(), tgt.id.clone(), rel_label.clone());
            if !seen_edge_keys.contains(&key) {
                seen_edge_keys.insert(key);
                if let (Some(&si), Some(&ti)) = (id_to_idx.get(&src.id), id_to_idx.get(&tgt.id))
                    && si != ti {
                        edge_pairs.push((si, ti));
                        slice_edges.push((src.id.clone(), tgt.id.clone(), rel_label.clone()));
                    }
            }
            continue;
        }

        // Aggregated: resolve endpoints to slice-level ancestors.
        let src_slice = entity_to_slice_node.get(&src.id);
        let tgt_slice = entity_to_slice_node.get(&tgt.id);

        match (src_slice, tgt_slice) {
            (Some(src_s), Some(tgt_s)) if src_s != tgt_s => {
                let key = (src_s.clone(), tgt_s.clone(), rel_label.clone());
                if !seen_edge_keys.contains(&key) {
                    seen_edge_keys.insert(key);
                    if let (Some(&si), Some(&ti)) = (id_to_idx.get(src_s), id_to_idx.get(tgt_s)) {
                        edge_pairs.push((si, ti));
                        slice_edges.push((src_s.clone(), tgt_s.clone(), rel_label.clone()));
                    }
                }
            }
            // Portal: one end is inside the slice, the other is outside.
            (Some(src_s), None) if node_set.contains(src_s) => {
                portal_sources.entry(src_s.clone()).or_default().push(
                    format!("{} (external)", tgt.label),
                );
            }
            (None, Some(tgt_s)) if node_set.contains(tgt_s) => {
                portal_sources.entry(tgt_s.clone()).or_default().push(
                    format!("{} (external)", src.label),
                );
            }
            _ => {}
        }
    }

    // Use force layout for this slice.
    // Scale viewport and repulsion so nodes don't overlap.
    let n = slice_ids.len() as f64;
    let scale = (n / 10.0).max(1.0).sqrt();
    let layout_width = width * scale;
    let layout_height = height * scale;
    let positions = super::force_layout::layout(
        &slice_ids,
        &edge_pairs,
        layout_width,
        layout_height,
        &super::force_layout::ForceConfig {
            repulsion: 2000.0 + n * 100.0,
            spring_length: 200.0 + n * 3.0,
            damping: 0.3,
            center_gravity: 0.003,
            iterations: 400,
            ..Default::default()
        },
    );

    // Build positioned nodes.
    let mut nodes: Vec<PositionedNode> = Vec::new();
    for entity in &entities {
        let (x, y) = positions.get(&entity.id).copied().unwrap_or((width / 2.0, height / 2.0));
        let type_key = entity.entity_type.discriminant();
        let config = schema.node_config(type_key);
        let child_count = children_of.get(&entity.id).copied().unwrap_or(0);

        let (shape, color, icon) = match config {
            Some(c) => (c.style.shape, c.style.color.clone(), c.style.icon.clone()),
            None => (crate::topology::NodeShape::Circle, "#a3a3a3".into(), None),
        };

        // Scale radius by child count for containers.
        let base_radius = if child_count > 0 {
            24.0 + (child_count as f64).sqrt() * 6.0
        } else {
            18.0
        };

        let portal_count = portal_sources.get(&entity.id).map(|v| v.len()).unwrap_or(0);
        let label = if child_count > 0 && portal_count > 0 {
            format!("{} ({} children, {} external)", entity.label, child_count, portal_count)
        } else if child_count > 0 {
            format!("{} ({})", entity.label, child_count)
        } else if portal_count > 0 {
            format!("{} [{} ext]", entity.label, portal_count)
        } else {
            entity.label.clone()
        };

        nodes.push(PositionedNode {
            id: entity.id.to_hex(),
            x,
            y,
            width: base_radius * 2.0,
            height: base_radius * 2.0,
            label,
            node_type: type_key.to_string(),
            iri: entity.iri.clone(),
            shape,
            color,
            icon,
            has_subgraph: child_count > 0,
            disposition: None,
            metrics: {
                let mut m = HashMap::new();
                if portal_count > 0 {
                    m.insert("external_connections".into(), portal_count as f64);
                }
                if child_count > 0 {
                    m.insert("children".into(), child_count as f64);
                }
                m
            },
        });
    }

    // Build positioned edges.
    let mut edges: Vec<PositionedEdge> = Vec::new();
    for (src_id, tgt_id, label) in &slice_edges {
        let src_pos = positions.get(src_id).copied().unwrap_or((0.0, 0.0));
        let tgt_pos = positions.get(tgt_id).copied().unwrap_or((0.0, 0.0));

        edges.push(PositionedEdge {
            source_id: src_id.to_hex(),
            target_id: tgt_id.to_hex(),
            label: Some(label.clone()),
            edge_type: label.clone(),
            path: vec![[src_pos.0, src_pos.1], [tgt_pos.0, tgt_pos.1]],
            stroke: "#888888".into(),
            width: 1.0,
            dash: crate::topology::DashStyle::Solid,
            arrow: true,
            animated: false,
        });
    }

    let mut graph = PositionedGraph {
        nodes,
        edges,
        viewport: Rect { x: 0.0, y: 0.0, width, height },
        schema_name: schema.name.clone(),
        schema_version: schema.version.clone(),
    };
    graph.compute_viewport();

    GraphSlice {
        graph,
        expandable,
        breadcrumbs: breadcrumbs.to_vec(),
        total_nodes: kg.entity_count(),
        depth,
    }
}

/// Generate all slices for a graph and write them as JSON files.
/// Returns a manifest mapping node IDs to their slice file paths.
pub fn generate_all_slices(
    kg: &KnowledgeGraph,
    schema: &TopologySchema,
    output_dir: &std::path::Path,
    width: f64,
    height: f64,
) -> Result<SliceManifest, crate::GraphifyError> {
    std::fs::create_dir_all(output_dir)
        .map_err(|e| crate::GraphifyError::ExportError(e.to_string()))?;

    let mut manifest = SliceManifest {
        root: "root.json".into(),
        slices: HashMap::new(),
        total_nodes: kg.entity_count(),
        total_edges: kg.relationship_count(),
    };

    // Top level.
    let top = top_level_slice(kg, schema, width, height);
    let top_json = serde_json::to_string_pretty(&top)
        .map_err(|e| crate::GraphifyError::ExportError(e.to_string()))?;
    std::fs::write(output_dir.join("root.json"), &top_json)
        .map_err(|e| crate::GraphifyError::ExportError(e.to_string()))?;

    // Drill into each expandable node.
    for node_id_hex in &top.expandable {
        let entity = kg.entities().find(|e| e.id.to_hex() == *node_id_hex);
        if let Some(entity) = entity {
            let crumbs = vec![BreadcrumbEntry {
                id: node_id_hex.clone(),
                label: entity.label.clone(),
            }];
            let slice = children_slice(kg, schema, &entity.id, &crumbs, width, height);
            let filename = format!("{}.json", &node_id_hex[..16]);
            let json = serde_json::to_string_pretty(&slice)
                .map_err(|e| crate::GraphifyError::ExportError(e.to_string()))?;
            std::fs::write(output_dir.join(&filename), &json)
                .map_err(|e| crate::GraphifyError::ExportError(e.to_string()))?;
            manifest.slices.insert(node_id_hex.clone(), filename);
        }
    }

    // Write manifest.
    let manifest_json = serde_json::to_string_pretty(&manifest)
        .map_err(|e| crate::GraphifyError::ExportError(e.to_string()))?;
    std::fs::write(output_dir.join("manifest.json"), &manifest_json)
        .map_err(|e| crate::GraphifyError::ExportError(e.to_string()))?;

    Ok(manifest)
}

/// Manifest mapping expandable nodes to their slice files.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SliceManifest {
    pub root: String,
    pub slices: HashMap<String, String>,
    pub total_nodes: usize,
    pub total_edges: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::{DomainTag, EntityType, FileType};
    use crate::model::Entity;
    use crate::relationship::{Confidence, Relationship};

    fn entity(name: &str, etype: EntityType) -> Entity {
        Entity {
            id: EntityId::new(&DomainTag::Code, &etype, name, "test.rs"),
            entity_type: etype,
            label: name.to_string(),
            iri: None,
            source_file: Some("test.rs".into()),
            source_location: None,
            file_type: FileType::Code,
            metadata: serde_json::json!({}),
            legacy_id: None,
        }
    }

    fn contains(src: &Entity, tgt: &Entity) -> Relationship {
        Relationship {
            source: src.id.clone(),
            target: tgt.id.clone(),
            relation_type: RelationType::Contains,
            confidence: Confidence::Extracted,
            weight: 1.0,
            source_file: None, source_location: None,
            metadata: serde_json::json!({}),
        }
    }

    #[test]
    fn top_level_returns_roots_only() {
        let mut kg = KnowledgeGraph::new();
        let p = entity("mypackage", EntityType::Package);
        let m1 = entity("auth", EntityType::Module);
        let m2 = entity("db", EntityType::Module);
        let f = entity("login", EntityType::Function);
        kg.add_entity(p.clone());
        kg.add_entity(m1.clone());
        kg.add_entity(m2.clone());
        kg.add_entity(f.clone());
        kg.add_relationship(contains(&p, &m1));
        kg.add_relationship(contains(&p, &m2));
        kg.add_relationship(contains(&m1, &f));

        let schema = TopologySchema::from_yaml(r##"
name: test
label: T
version: "1.0.0"
nodes:
  "*":
    geometry: force
edges: []
"##).unwrap();

        let slice = top_level_slice(&kg, &schema, 800.0, 600.0);
        // Only the package should be at top level.
        assert_eq!(slice.graph.nodes.len(), 1);
        assert_eq!(slice.graph.nodes[0].node_type, "package");
        assert!(slice.graph.nodes[0].has_subgraph);
        assert!(!slice.expandable.is_empty());
        assert_eq!(slice.depth, 0);
    }

    #[test]
    fn children_slice_returns_direct_children() {
        let mut kg = KnowledgeGraph::new();
        let p = entity("mypackage", EntityType::Package);
        let m1 = entity("auth", EntityType::Module);
        let m2 = entity("db", EntityType::Module);
        let f = entity("login", EntityType::Function);
        kg.add_entity(p.clone());
        kg.add_entity(m1.clone());
        kg.add_entity(m2.clone());
        kg.add_entity(f.clone());
        kg.add_relationship(contains(&p, &m1));
        kg.add_relationship(contains(&p, &m2));
        kg.add_relationship(contains(&m1, &f));

        let schema = TopologySchema::from_yaml(r##"
name: test
label: T
version: "1.0.0"
nodes:
  "*":
    geometry: force
edges: []
"##).unwrap();

        let crumbs = vec![BreadcrumbEntry { id: p.id.to_hex(), label: "mypackage".into() }];
        let slice = children_slice(&kg, &schema, &p.id, &crumbs, 800.0, 600.0);
        // Should have auth and db modules, not the function.
        assert_eq!(slice.graph.nodes.len(), 2);
        assert_eq!(slice.depth, 2); // breadcrumbs(1) + 1
        assert_eq!(slice.breadcrumbs.len(), 1);
    }
}
