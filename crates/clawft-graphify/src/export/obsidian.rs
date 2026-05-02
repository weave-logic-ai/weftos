//! Obsidian vault + canvas export for the knowledge graph.
//!
//! Generates:
//! - One `.md` file per entity with YAML frontmatter and wikilinks
//! - An Obsidian `.canvas` file with grid layout grouped by community
//! - Dataview-compatible frontmatter fields
//! - Bridge node highlighting

use std::collections::HashMap;
use std::path::Path;

use crate::model::KnowledgeGraph;
use crate::GraphifyError;

// ---------------------------------------------------------------------------
// Vault export (one .md per entity)
// ---------------------------------------------------------------------------

/// Export the knowledge graph as an Obsidian vault (one .md per entity).
///
/// Each entity gets a markdown file with:
/// - YAML frontmatter (type, source_file, community, degree)
/// - Wikilink connections to neighbors
/// - Bridge node indicator if the entity spans multiple communities
pub fn to_obsidian_vault(
    kg: &KnowledgeGraph,
    output_dir: &Path,
) -> Result<usize, GraphifyError> {
    std::fs::create_dir_all(output_dir).map_err(|e| {
        GraphifyError::ExportError(format!("failed to create output dir: {e}"))
    })?;

    let communities = kg.communities.as_ref();
    let labels = kg.community_labels.as_ref();

    // Build entity -> community lookup.
    let mut entity_community: HashMap<String, usize> = HashMap::new();
    if let Some(comms) = communities {
        for (cid, members) in comms {
            for eid in members {
                entity_community.insert(eid.to_hex(), *cid);
            }
        }
    }

    // Identify bridge nodes (entities connected to multiple communities).
    let mut bridge_nodes = std::collections::HashSet::new();
    for entity in kg.entities() {
        let my_comm = entity_community.get(&entity.id.to_hex());
        let neighbor_comms: std::collections::HashSet<Option<&usize>> = kg
            .neighbors(&entity.id)
            .iter()
            .map(|n| entity_community.get(&n.id.to_hex()))
            .collect();
        if neighbor_comms.len() > 1 || (my_comm.is_some() && neighbor_comms.iter().any(|c| c != &my_comm)) {
            bridge_nodes.insert(entity.id.to_hex());
        }
    }

    let mut count = 0;

    for entity in kg.entities() {
        let filename = safe_filename(&entity.label);
        let community_id = entity_community.get(&entity.id.to_hex());
        let community_name = community_id
            .and_then(|cid| labels.and_then(|l| l.get(cid)))
            .cloned()
            .unwrap_or_else(|| {
                community_id
                    .map(|cid| format!("Community {cid}"))
                    .unwrap_or_else(|| "unclustered".to_string())
            });
        let is_bridge = bridge_nodes.contains(&entity.id.to_hex());
        let degree = kg.degree(&entity.id);

        // YAML frontmatter.
        let mut lines = vec![
            "---".to_string(),
            format!("type: \"{}\"", entity.entity_type.discriminant()),
            format!(
                "source_file: \"{}\"",
                entity.source_file.as_deref().unwrap_or("")
            ),
            format!(
                "source_location: \"{}\"",
                entity.source_location.as_deref().unwrap_or("")
            ),
            format!("community: \"{community_name}\""),
            format!("degree: {degree}"),
        ];
        if is_bridge {
            lines.push("bridge_node: true".to_string());
        }
        lines.push("---".to_string());
        lines.push(String::new());

        // Title.
        lines.push(format!("# {}", entity.label));
        lines.push(String::new());

        if is_bridge {
            lines.push("> Bridge node: connects multiple communities.".to_string());
            lines.push(String::new());
        }

        // Metadata.
        lines.push(format!(
            "**Type:** {}",
            entity.entity_type.discriminant()
        ));
        if let Some(src) = &entity.source_file {
            lines.push(format!("**Source:** `{src}`"));
        }
        lines.push(format!("**Community:** {community_name}"));
        lines.push(format!("**Connections:** {degree}"));
        lines.push(String::new());

        // Connections as wikilinks.
        lines.push("## Connections".to_string());
        lines.push(String::new());

        for (src, tgt, rel) in kg.edges() {
            if src.id == entity.id {
                lines.push(format!(
                    "- [[{}]] -- {} ({})",
                    tgt.label,
                    serde_json::to_string(&rel.relation_type)
                        .unwrap_or_default()
                        .trim_matches('"'),
                    serde_json::to_string(&rel.confidence)
                        .unwrap_or_default()
                        .trim_matches('"'),
                ));
            } else if tgt.id == entity.id {
                lines.push(format!(
                    "- [[{}]] -- {} ({})",
                    src.label,
                    serde_json::to_string(&rel.relation_type)
                        .unwrap_or_default()
                        .trim_matches('"'),
                    serde_json::to_string(&rel.confidence)
                        .unwrap_or_default()
                        .trim_matches('"'),
                ));
            }
        }

        lines.push(String::new());
        lines.push("---".to_string());
        lines.push("*Generated by graphify.*".to_string());

        let content = lines.join("\n");
        let file_path = output_dir.join(format!("{filename}.md"));
        std::fs::write(&file_path, content).map_err(|e| {
            GraphifyError::ExportError(format!("failed to write {}: {e}", file_path.display()))
        })?;
        count += 1;
    }

    Ok(count)
}

// ---------------------------------------------------------------------------
// Canvas export (JSON Obsidian canvas format)
// ---------------------------------------------------------------------------

/// Export the knowledge graph as an Obsidian `.canvas` file.
///
/// Nodes are laid out in a grid grouped by community. Edges map to
/// Obsidian canvas connections.
pub fn to_obsidian_canvas(
    kg: &KnowledgeGraph,
    output_path: &Path,
) -> Result<(), GraphifyError> {
    let communities = kg.communities.as_ref();

    // Group entities by community.
    let mut community_entities: HashMap<usize, Vec<String>> = HashMap::new();
    if let Some(comms) = communities {
        for (cid, members) in comms {
            for eid in members {
                community_entities
                    .entry(*cid)
                    .or_default()
                    .push(eid.to_hex());
            }
        }
    }

    // Assign grid positions.
    let mut nodes = Vec::new();
    let mut id_to_canvas_id: HashMap<String, String> = HashMap::new();
    let mut x = 0i64;
    let mut y = 0i64;
    let grid_spacing = 300;
    let community_spacing = 600;

    let sorted_communities: Vec<usize> = {
        let mut keys: Vec<usize> = community_entities.keys().copied().collect();
        keys.sort();
        keys
    };

    for cid in &sorted_communities {
        let members = &community_entities[cid];
        let mut col = 0;
        for eid_hex in members {
            if let Some(entity) = kg.entities().find(|e| e.id.to_hex() == *eid_hex) {
                let canvas_id = format!("node_{}", nodes.len());
                id_to_canvas_id.insert(eid_hex.clone(), canvas_id.clone());
                nodes.push(serde_json::json!({
                    "id": canvas_id,
                    "type": "text",
                    "text": format!("**{}**\n{}", entity.label, entity.entity_type.discriminant()),
                    "x": x + (col as i64 * grid_spacing),
                    "y": y,
                    "width": 250,
                    "height": 60,
                }));
                col += 1;
                if col >= 5 {
                    col = 0;
                    y += 100;
                }
            }
        }
        x += community_spacing;
        y = 0;
    }

    // Build edges.
    let mut edges = Vec::new();
    for (src, tgt, _rel) in kg.edges() {
        let src_hex = src.id.to_hex();
        let tgt_hex = tgt.id.to_hex();
        if let (Some(from_id), Some(to_id)) =
            (id_to_canvas_id.get(&src_hex), id_to_canvas_id.get(&tgt_hex))
        {
            edges.push(serde_json::json!({
                "id": format!("edge_{}", edges.len()),
                "fromNode": from_id,
                "toNode": to_id,
                "fromSide": "right",
                "toSide": "left",
            }));
        }
    }

    let canvas = serde_json::json!({
        "nodes": nodes,
        "edges": edges,
    });

    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            GraphifyError::ExportError(format!("failed to create canvas dir: {e}"))
        })?;
    }

    let content = serde_json::to_string_pretty(&canvas).map_err(|e| {
        GraphifyError::ExportError(format!("failed to serialize canvas: {e}"))
    })?;
    std::fs::write(output_path, content).map_err(|e| {
        GraphifyError::ExportError(format!("failed to write canvas: {e}"))
    })?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Convert a label to a safe filename (no slashes, colons, etc.).
fn safe_filename(name: &str) -> String {
    name.replace(['/', '\\'], "-")
        .replace(' ', "_")
        .replace(':', "-")
        .replace(['<', '>', '"'], "")
        .replace('|', "-")
        .replace(['?', '*'], "")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::{DomainTag, EntityType, FileType};
    use crate::model::Entity;
    use crate::relationship::{Confidence, RelationType, Relationship};

    fn test_graph() -> KnowledgeGraph {
        let mut kg = KnowledgeGraph::new();
        let e1 = Entity {
            id: crate::EntityId::new(&DomainTag::Code, &EntityType::Module, "auth", "auth.py"),
            entity_type: EntityType::Module,
            label: "auth".to_string(),
            source_file: Some("auth.py".into()),
            source_location: Some("L1".into()),
            file_type: FileType::Code,
            metadata: serde_json::json!({}),
            legacy_id: None,
            iri: None,
        };
        let e2 = Entity {
            id: crate::EntityId::new(&DomainTag::Code, &EntityType::Class, "AuthService", "auth.py"),
            entity_type: EntityType::Class,
            label: "AuthService".to_string(),
            source_file: Some("auth.py".into()),
            source_location: Some("L10".into()),
            file_type: FileType::Code,
            metadata: serde_json::json!({}),
            legacy_id: None,
            iri: None,
        };
        kg.add_entity(e1.clone());
        kg.add_entity(e2.clone());
        kg.add_relationship(Relationship {
            source: e1.id.clone(),
            target: e2.id.clone(),
            relation_type: RelationType::Contains,
            confidence: Confidence::Extracted,
            weight: 1.0,
            source_file: Some("auth.py".into()),
            source_location: Some("L1".into()),
            metadata: serde_json::json!({}),
        });
        kg
    }

    #[test]
    fn obsidian_vault_creates_files() {
        let kg = test_graph();
        let dir = std::env::temp_dir().join("graphify_test_obsidian");
        let _ = std::fs::remove_dir_all(&dir);

        let count = to_obsidian_vault(&kg, &dir).unwrap();
        assert_eq!(count, 2);

        let auth_file = dir.join("auth.md");
        assert!(auth_file.exists());
        let content = std::fs::read_to_string(&auth_file).unwrap();
        assert!(content.contains("# auth"));
        assert!(content.contains("type: \"module\""));
        assert!(content.contains("[[AuthService]]"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn obsidian_canvas_creates_file() {
        let kg = test_graph();
        let dir = std::env::temp_dir().join("graphify_test_canvas");
        let _ = std::fs::remove_dir_all(&dir);

        let canvas_path = dir.join("graph.canvas");
        to_obsidian_canvas(&kg, &canvas_path).unwrap();

        assert!(canvas_path.exists());
        let content = std::fs::read_to_string(&canvas_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(parsed["nodes"].is_array());
        assert!(parsed["edges"].is_array());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn safe_filename_strips_special() {
        assert_eq!(safe_filename("auth/service"), "auth-service");
        assert_eq!(safe_filename("my class"), "my_class");
        assert_eq!(safe_filename("a:b:c"), "a-b-c");
    }
}
