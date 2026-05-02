//! Wikipedia-style article generation from the knowledge graph.
//!
//! Generates:
//! - `index.md` -- agent entry point, catalog of all articles
//! - `<CommunityName>.md` -- one article per community
//! - `<GodNodeLabel>.md` -- one article per god node
//!
//! Ported from Python `graphify/wiki.py`.

use std::collections::HashMap;
use std::path::Path;

use crate::entity::EntityId;
use crate::model::KnowledgeGraph;
use crate::GraphifyError;

/// Data about a "god node" (most connected entity).
#[derive(Debug, Clone)]
pub struct GodNodeInfo {
    pub id: EntityId,
    pub label: String,
    pub edges: usize,
}

fn safe_filename(name: &str) -> String {
    name.replace('/', "-")
        .replace(' ', "_")
        .replace(':', "-")
        .replace(['<', '>', '"'], "")
}

fn cross_community_links(
    kg: &KnowledgeGraph,
    nodes: &[EntityId],
    own_cid: usize,
    labels: &HashMap<usize, String>,
) -> Vec<(String, usize)> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    let entity_comm = build_entity_community_map(kg);

    for nid in nodes {
        for neighbor in kg.neighbors(nid) {
            if let Some(&ncid) = entity_comm.get(&neighbor.id.to_hex())
                && ncid != own_cid {
                    let label = labels
                        .get(&ncid)
                        .cloned()
                        .unwrap_or_else(|| format!("Community {ncid}"));
                    *counts.entry(label).or_default() += 1;
                }
        }
    }

    let mut result: Vec<(String, usize)> = counts.into_iter().collect();
    result.sort_by(|a, b| b.1.cmp(&a.1));
    result
}

fn build_entity_community_map(kg: &KnowledgeGraph) -> HashMap<String, usize> {
    let mut map = HashMap::new();
    if let Some(comms) = &kg.communities {
        for (cid, members) in comms {
            for eid in members {
                map.insert(eid.to_hex(), *cid);
            }
        }
    }
    map
}

fn community_article(
    kg: &KnowledgeGraph,
    cid: usize,
    nodes: &[EntityId],
    label: &str,
    labels: &HashMap<usize, String>,
    cohesion: Option<f64>,
) -> String {
    let mut top_nodes: Vec<(&EntityId, usize)> = nodes
        .iter()
        .map(|id| (id, kg.degree(id)))
        .collect();
    top_nodes.sort_by(|a, b| b.1.cmp(&a.1));
    top_nodes.truncate(25);

    let cross = cross_community_links(kg, nodes, cid, labels);

    let mut sources = std::collections::BTreeSet::new();
    for nid in nodes {
        if let Some(entity) = kg.entity(nid)
            && let Some(ref src) = entity.source_file
                && !src.is_empty() {
                    sources.insert(src.clone());
                }
    }

    let mut lines = Vec::new();
    lines.push(format!("# {label}"));
    lines.push(String::new());

    let mut meta_parts = vec![format!("{} nodes", nodes.len())];
    if let Some(c) = cohesion {
        meta_parts.push(format!("cohesion {c:.2}"));
    }
    lines.push(format!("> {}", meta_parts.join(" / ")));
    lines.push(String::new());

    lines.push("## Key Concepts".to_string());
    lines.push(String::new());
    for (nid, degree) in &top_nodes {
        if let Some(entity) = kg.entity(nid) {
            let src_str = entity.source_file.as_ref()
                .map(|s| format!(" -- `{s}`"))
                .unwrap_or_default();
            lines.push(format!("- **{}** ({degree} connections){src_str}", entity.label));
        }
    }
    let remaining = nodes.len().saturating_sub(25);
    if remaining > 0 {
        lines.push(format!("- *... and {remaining} more nodes*"));
    }
    lines.push(String::new());

    lines.push("## Relationships".to_string());
    lines.push(String::new());
    if cross.is_empty() {
        lines.push("- No strong cross-community connections detected".to_string());
    } else {
        for (other_label, count) in cross.iter().take(12) {
            lines.push(format!("- [[{other_label}]] ({count} shared connections)"));
        }
    }
    lines.push(String::new());

    if !sources.is_empty() {
        lines.push("## Source Files".to_string());
        lines.push(String::new());
        for src in sources.iter().take(20) {
            lines.push(format!("- `{src}`"));
        }
        lines.push(String::new());
    }

    lines.push("---".to_string());
    lines.push(String::new());
    lines.push("*Part of the graphify knowledge wiki. See [[index]] to navigate.*".to_string());
    lines.join("\n")
}

fn god_node_article(
    kg: &KnowledgeGraph,
    nid: &EntityId,
    labels: &HashMap<usize, String>,
) -> String {
    let entity = match kg.entity(nid) {
        Some(e) => e,
        None => return String::new(),
    };

    let entity_comm = build_entity_community_map(kg);
    let community_name = entity_comm.get(&nid.to_hex())
        .and_then(|cid| labels.get(cid))
        .cloned();

    let mut lines = Vec::new();
    lines.push(format!("# {}", entity.label));
    lines.push(String::new());
    lines.push(format!(
        "> God node / {} connections / `{}`",
        kg.degree(nid),
        entity.source_file.as_deref().unwrap_or("")
    ));
    lines.push(String::new());

    if let Some(ref cn) = community_name {
        lines.push(format!("**Community:** [[{cn}]]"));
        lines.push(String::new());
    }

    // Group neighbors by relation type.
    let mut by_relation: HashMap<String, Vec<String>> = HashMap::new();
    for (src, tgt, rel) in kg.edges() {
        if src.id == *nid {
            let rel_name = serde_json::to_string(&rel.relation_type)
                .unwrap_or_default()
                .trim_matches('"')
                .to_string();
            let conf = serde_json::to_string(&rel.confidence)
                .unwrap_or_default()
                .trim_matches('"')
                .to_string();
            by_relation.entry(rel_name).or_default()
                .push(format!("[[{}]] `{conf}`", tgt.label));
        } else if tgt.id == *nid {
            let rel_name = serde_json::to_string(&rel.relation_type)
                .unwrap_or_default()
                .trim_matches('"')
                .to_string();
            let conf = serde_json::to_string(&rel.confidence)
                .unwrap_or_default()
                .trim_matches('"')
                .to_string();
            by_relation.entry(rel_name).or_default()
                .push(format!("[[{}]] `{conf}`", src.label));
        }
    }

    lines.push("## Connections by Relation".to_string());
    lines.push(String::new());
    let mut sorted_rels: Vec<String> = by_relation.keys().cloned().collect();
    sorted_rels.sort();
    for rel in &sorted_rels {
        lines.push(format!("### {rel}"));
        for t in by_relation[rel].iter().take(20) {
            lines.push(format!("- {t}"));
        }
        lines.push(String::new());
    }

    lines.push("---".to_string());
    lines.push(String::new());
    lines.push("*Part of the graphify knowledge wiki. See [[index]] to navigate.*".to_string());
    lines.join("\n")
}

fn index_md(
    communities: &HashMap<usize, Vec<EntityId>>,
    labels: &HashMap<usize, String>,
    god_nodes: &[GodNodeInfo],
    total_nodes: usize,
    total_edges: usize,
) -> String {
    let mut lines = vec![
        "# Knowledge Graph Index".to_string(),
        String::new(),
        "> Auto-generated by graphify. Start here.".to_string(),
        String::new(),
        format!("**{total_nodes} nodes / {total_edges} edges / {} communities**", communities.len()),
        String::new(),
        "---".to_string(),
        String::new(),
        "## Communities".to_string(),
        "(sorted by size, largest first)".to_string(),
        String::new(),
    ];

    let mut sorted: Vec<(&usize, &Vec<EntityId>)> = communities.iter().collect();
    sorted.sort_by(|a, b| b.1.len().cmp(&a.1.len()));
    for (cid, nodes) in &sorted {
        let label = labels.get(cid).cloned().unwrap_or_else(|| format!("Community {cid}"));
        lines.push(format!("- [[{label}]] -- {} nodes", nodes.len()));
    }
    lines.push(String::new());

    if !god_nodes.is_empty() {
        lines.push("## God Nodes".to_string());
        lines.push("(most connected concepts)".to_string());
        lines.push(String::new());
        for node in god_nodes {
            lines.push(format!("- [[{}]] -- {} connections", node.label, node.edges));
        }
        lines.push(String::new());
    }

    lines.push("---".to_string());
    lines.push(String::new());
    lines.push("*Generated by graphify.*".to_string());
    lines.join("\n")
}

/// Generate a Wikipedia-style wiki from the knowledge graph.
///
/// Returns the number of articles written (excluding index.md).
pub fn to_wiki(
    kg: &KnowledgeGraph,
    output_dir: &Path,
    god_nodes: &[GodNodeInfo],
    cohesion: Option<&HashMap<usize, f64>>,
) -> Result<usize, GraphifyError> {
    std::fs::create_dir_all(output_dir).map_err(|e| {
        GraphifyError::ExportError(format!("failed to create wiki dir: {e}"))
    })?;

    let communities = kg.communities.as_ref().cloned().unwrap_or_default();
    let labels = kg.community_labels.as_ref().cloned().unwrap_or_default();

    let mut count = 0;

    // Community articles.
    for (cid, nodes) in &communities {
        let label = labels.get(cid).cloned().unwrap_or_else(|| format!("Community {cid}"));
        let article = community_article(
            kg, *cid, nodes, &label, &labels,
            cohesion.and_then(|c| c.get(cid).copied()),
        );
        let filename = format!("{}.md", safe_filename(&label));
        std::fs::write(output_dir.join(&filename), &article).map_err(|e| {
            GraphifyError::ExportError(format!("failed to write {filename}: {e}"))
        })?;
        count += 1;
    }

    // God node articles.
    for node_info in god_nodes {
        let article = god_node_article(kg, &node_info.id, &labels);
        if !article.is_empty() {
            let filename = format!("{}.md", safe_filename(&node_info.label));
            std::fs::write(output_dir.join(&filename), &article).map_err(|e| {
                GraphifyError::ExportError(format!("failed to write {filename}: {e}"))
            })?;
            count += 1;
        }
    }

    // Index.
    let idx = index_md(
        &communities, &labels, god_nodes,
        kg.entity_count(), kg.relationship_count(),
    );
    std::fs::write(output_dir.join("index.md"), &idx).map_err(|e| {
        GraphifyError::ExportError(format!("failed to write index.md: {e}"))
    })?;

    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::{DomainTag, EntityType, FileType};
    use crate::model::Entity;
    use crate::relationship::{Confidence, RelationType, Relationship};

    fn test_graph() -> KnowledgeGraph {
        let mut kg = KnowledgeGraph::new();
        let e1 = Entity {
            id: EntityId::new(&DomainTag::Code, &EntityType::Module, "auth", "auth.py"),
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
            id: EntityId::new(&DomainTag::Code, &EntityType::Class, "AuthService", "auth.py"),
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

        let mut comms = HashMap::new();
        comms.insert(0, vec![e1.id.clone(), e2.id.clone()]);
        kg.communities = Some(comms);

        let mut labels = HashMap::new();
        labels.insert(0, "Authentication".to_string());
        kg.community_labels = Some(labels);

        kg
    }

    #[test]
    fn wiki_creates_files() {
        let kg = test_graph();
        let dir = std::env::temp_dir().join("graphify_test_wiki");
        let _ = std::fs::remove_dir_all(&dir);

        let god_nodes = vec![GodNodeInfo {
            id: EntityId::new(&DomainTag::Code, &EntityType::Module, "auth", "auth.py"),
            label: "auth".to_string(),
            edges: 1,
        }];

        let count = to_wiki(&kg, &dir, &god_nodes, None).unwrap();
        assert!(count >= 1);

        assert!(dir.join("index.md").exists());
        let idx = std::fs::read_to_string(dir.join("index.md")).unwrap();
        assert!(idx.contains("Knowledge Graph Index"));
        assert!(idx.contains("Authentication"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
