//! Schema inference — generate a TopologySchema from an existing KnowledgeGraph.
//!
//! Analyzes entity types, relationship patterns, and structural properties
//! to produce a draft schema. The inferred schema can be diffed against a
//! declared schema to detect architectural drift.

use std::collections::{HashMap, HashSet};

use crate::model::KnowledgeGraph;
use crate::relationship::RelationType;
use crate::topology::*;

/// Infer a TopologySchema from a KnowledgeGraph.
pub fn infer_schema(kg: &KnowledgeGraph, name: &str) -> TopologySchema {
    let mut nodes: HashMap<String, NodeTypeConfig> = HashMap::new();
    let mut edge_configs: Vec<EdgeTypeConfig> = Vec::new();

    // Count entity types.
    let mut type_counts: HashMap<String, usize> = HashMap::new();
    for entity in kg.entities() {
        *type_counts.entry(entity.entity_type.discriminant().to_string()).or_default() += 1;
    }

    // Build children map for containment analysis.
    let mut children_types: HashMap<String, HashSet<String>> = HashMap::new();
    for (src, tgt, rel) in kg.edges() {
        if matches!(rel.relation_type, RelationType::Contains) {
            children_types
                .entry(src.entity_type.discriminant().to_string())
                .or_default()
                .insert(tgt.entity_type.discriminant().to_string());
        }
    }

    // Infer geometry per entity type.
    let colors = [
        "#6366f1", "#3b82f6", "#0ea5e9", "#14b8a6", "#22c55e",
        "#84cc16", "#f59e0b", "#f97316", "#ef4444", "#ec4899",
        "#a855f7", "#8b5cf6", "#78716c",
    ];

    let mut sorted_types: Vec<_> = type_counts.iter().collect();
    sorted_types.sort_by(|a, b| b.1.cmp(a.1));

    for (color_idx, (type_key, _count)) in sorted_types.into_iter().enumerate() {
        let contains: Vec<String> = children_types
            .get(type_key.as_str())
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default();

        let has_children = !contains.is_empty();

        // Check if this type has timestamps.
        let has_timestamps = kg.entities()
            .filter(|e| e.entity_type.discriminant() == type_key.as_str())
            .any(|e| {
                e.metadata.get("timestamp").is_some()
                    || e.metadata.get("created_at").is_some()
                    || e.metadata.get("date").is_some()
            });

        let geometry = if has_timestamps {
            Geometry::Timeline
        } else if has_children {
            Geometry::Tree
        } else {
            Geometry::Force
        };

        let shape = if has_children {
            NodeShape::Rect
        } else if has_timestamps {
            NodeShape::Diamond
        } else {
            NodeShape::Circle
        };

        let color = colors[color_idx % colors.len()].to_string();

        let time_field = if has_timestamps {
            Some("timestamp".to_string())
        } else {
            None
        };

        nodes.insert(type_key.clone(), NodeTypeConfig {
            iri: Some(format!("https://weftos.weavelogic.ai/ontology/inferred#{type_key}")),
            same_as: vec![],
            geometry,
            contains,
            style: NodeStyle {
                shape,
                color,
                icon: None,
                min_radius: 8,
                max_radius: 48,
            },
            size_field: None,
            time_field,
            lat_field: None,
            lng_field: None,
            display_name: Some(title_case(type_key)),
        });
    }

    // Add wildcard fallback.
    nodes.insert("*".to_string(), NodeTypeConfig {
        iri: None,
        same_as: vec![],
        geometry: Geometry::Force,
        contains: vec![],
        style: NodeStyle::default(),
        size_field: None,
        time_field: None,
        lat_field: None,
        lng_field: None,
        display_name: Some("Entity".into()),
    });

    // Infer edge types from relationship patterns.
    let mut edge_patterns: HashMap<(String, String, String), usize> = HashMap::new();
    for (src, tgt, rel) in kg.edges() {
        let key = (
            src.entity_type.discriminant().to_string(),
            tgt.entity_type.discriminant().to_string(),
            format!("{:?}", rel.relation_type).to_lowercase(),
        );
        *edge_patterns.entry(key).or_default() += 1;
    }

    let mut seen_types: HashSet<String> = HashSet::new();
    for ((from, to, rel_type), count) in &edge_patterns {
        if seen_types.contains(rel_type) {
            continue;
        }

        let cardinality = if *count == 1 { "1:1" } else { "N:M" }.to_string();

        edge_configs.push(EdgeTypeConfig {
            edge_type: rel_type.clone(),
            iri: Some(format!("https://weftos.weavelogic.ai/ontology/inferred#{rel_type}")),
            from: from.clone(),
            to: to.clone(),
            cardinality,
            style: EdgeStyle::default(),
            animated: false,
        });

        seen_types.insert(rel_type.clone());
    }

    // Detect root geometry.
    let contains_count = kg.edges()
        .filter(|(_, _, rel)| matches!(rel.relation_type, RelationType::Contains))
        .count();
    let total_edges = kg.relationship_count();
    let root_geometry = if total_edges > 0 && contains_count as f64 / total_edges as f64 > 0.6 {
        Geometry::Tree
    } else {
        Geometry::Force
    };

    TopologySchema {
        name: name.to_string(),
        label: format!("Inferred from {name}"),
        version: "0.1.0".into(),
        domain: None,
        iri: Some(format!("https://weftos.weavelogic.ai/schema/inferred/{name}")),
        extends: None,
        nodes,
        edges: edge_configs,
        modes: ModesConfig {
            structure: StructureMode { root_geometry },
            ..Default::default()
        },
        constraints: ConstraintsConfig::default(),
    }
}

fn title_case(s: &str) -> String {
    s.split('_')
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Diff two schemas and report differences.
pub fn diff_schemas(declared: &TopologySchema, inferred: &TopologySchema) -> SchemaDiff {
    let declared_types: HashSet<&String> = declared.nodes.keys().collect();
    let inferred_types: HashSet<&String> = inferred.nodes.keys().collect();

    let added_types: Vec<String> = inferred_types.difference(&declared_types)
        .filter(|k| **k != "*")
        .map(|k| (*k).clone())
        .collect();
    let removed_types: Vec<String> = declared_types.difference(&inferred_types)
        .filter(|k| **k != "*")
        .map(|k| (*k).clone())
        .collect();
    let common_types: Vec<String> = declared_types.intersection(&inferred_types)
        .filter(|k| ***k != "*")
        .map(|k| (*k).clone())
        .collect();

    let mut geometry_changes = Vec::new();
    for key in &common_types {
        let dg = declared.nodes.get(key).map(|n| n.geometry);
        let ig = inferred.nodes.get(key).map(|n| n.geometry);
        if dg != ig {
            geometry_changes.push(format!(
                "{key}: declared={:?}, inferred={:?}",
                dg.unwrap_or(Geometry::Force),
                ig.unwrap_or(Geometry::Force),
            ));
        }
    }

    let declared_edge_types: HashSet<&str> = declared.edges.iter().map(|e| e.edge_type.as_str()).collect();
    let inferred_edge_types: HashSet<&str> = inferred.edges.iter().map(|e| e.edge_type.as_str()).collect();

    let added_edges: Vec<String> = inferred_edge_types.difference(&declared_edge_types)
        .map(|s| s.to_string())
        .collect();
    let removed_edges: Vec<String> = declared_edge_types.difference(&inferred_edge_types)
        .map(|s| s.to_string())
        .collect();

    SchemaDiff {
        added_types,
        removed_types,
        geometry_changes,
        added_edges,
        removed_edges,
    }
}

/// Differences between a declared and inferred schema.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SchemaDiff {
    pub added_types: Vec<String>,
    pub removed_types: Vec<String>,
    pub geometry_changes: Vec<String>,
    pub added_edges: Vec<String>,
    pub removed_edges: Vec<String>,
}

impl SchemaDiff {
    pub fn is_empty(&self) -> bool {
        self.added_types.is_empty()
            && self.removed_types.is_empty()
            && self.geometry_changes.is_empty()
            && self.added_edges.is_empty()
            && self.removed_edges.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::{DomainTag, EntityId, EntityType, FileType};
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

    #[test]
    fn infer_from_simple_graph() {
        let mut kg = KnowledgeGraph::new();
        let m = entity("app", EntityType::Module);
        let f1 = entity("foo", EntityType::Function);
        let f2 = entity("bar", EntityType::Function);
        kg.add_entity(m.clone());
        kg.add_entity(f1.clone());
        kg.add_entity(f2.clone());
        kg.add_relationship(Relationship {
            source: m.id.clone(),
            target: f1.id.clone(),
            relation_type: RelationType::Contains,
            confidence: Confidence::Extracted,
            weight: 1.0,
            source_file: None,
            source_location: None,
            metadata: serde_json::json!({}),
        });
        kg.add_relationship(Relationship {
            source: m.id.clone(),
            target: f2.id.clone(),
            relation_type: RelationType::Contains,
            confidence: Confidence::Extracted,
            weight: 1.0,
            source_file: None,
            source_location: None,
            metadata: serde_json::json!({}),
        });

        let schema = infer_schema(&kg, "test-project");
        assert!(schema.nodes.contains_key("module"));
        assert!(schema.nodes.contains_key("function"));
        assert_eq!(schema.nodes["module"].geometry, Geometry::Tree);
        assert!(schema.nodes["module"].contains.contains(&"function".to_string()));
    }

    #[test]
    fn diff_detects_added_types() {
        let declared = TopologySchema::from_yaml(r##"
name: declared
label: "D"
version: "1.0.0"
nodes:
  module:
    geometry: tree
  function:
    geometry: force
edges: []
"##).unwrap();

        let inferred = TopologySchema::from_yaml(r##"
name: inferred
label: "I"
version: "1.0.0"
nodes:
  module:
    geometry: tree
  function:
    geometry: force
  service:
    geometry: force
edges: []
"##).unwrap();

        let diff = diff_schemas(&declared, &inferred);
        assert!(diff.added_types.contains(&"service".to_string()));
        assert!(diff.removed_types.is_empty());
    }
}
