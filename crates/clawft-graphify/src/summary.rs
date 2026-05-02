//! Community summary generation (KG-002: GraphRAG-style).
//!
//! After community detection, generates text summaries per community
//! describing the community's purpose, top entities, key relationships,
//! and source files. These summaries enable "what is this about?" queries.

use crate::entity::EntityId;
use crate::model::KnowledgeGraph;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// CommunitySummary
// ---------------------------------------------------------------------------

/// A generated text summary for a single community.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommunitySummary {
    /// Community ID.
    pub id: usize,
    /// Short label (from `auto_label`).
    pub label: String,
    /// Number of members.
    pub member_count: usize,
    /// Top entities by degree (up to 5).
    pub top_entities: Vec<String>,
    /// Human-readable description of what this community is about.
    pub description: String,
    /// Most common relationship types within the community.
    pub key_relationships: Vec<String>,
    /// Source files represented in this community.
    pub source_files: Vec<String>,
}

// ---------------------------------------------------------------------------
// Generation
// ---------------------------------------------------------------------------

/// Generate summaries for all communities in the knowledge graph.
///
/// For each community, collects the top entities by degree, the most common
/// relationship types, source file coverage, and assembles a textual
/// description suitable for answering high-level queries.
pub fn generate_community_summaries(
    kg: &KnowledgeGraph,
    communities: &HashMap<usize, Vec<EntityId>>,
    community_labels: &HashMap<usize, String>,
) -> HashMap<usize, CommunitySummary> {
    communities
        .iter()
        .map(|(&cid, members)| {
            let summary = generate_one(kg, cid, members, community_labels);
            (cid, summary)
        })
        .collect()
}

/// Generate a summary for one community.
fn generate_one(
    kg: &KnowledgeGraph,
    cid: usize,
    members: &[EntityId],
    community_labels: &HashMap<usize, String>,
) -> CommunitySummary {
    let label = community_labels
        .get(&cid)
        .cloned()
        .unwrap_or_else(|| format!("Community {cid}"));

    // Top entities by degree (descending), up to 5.
    let mut entity_degrees: Vec<(&EntityId, usize)> = members
        .iter()
        .map(|id| (id, kg.degree(id)))
        .collect();
    entity_degrees.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0 .0.cmp(&b.0 .0)));

    let top_entities: Vec<String> = entity_degrees
        .iter()
        .take(5)
        .filter_map(|(id, _)| kg.entity(id).map(|e| e.label.clone()))
        .collect();

    // Source files in this community.
    let mut source_file_set: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    for id in members {
        if let Some(entity) = kg.entity(id)
            && let Some(ref sf) = entity.source_file
                && !sf.is_empty() {
                    source_file_set.insert(sf.clone());
                }
    }
    let mut source_files: Vec<String> = source_file_set.into_iter().collect();
    source_files.sort();

    // Relationship type frequency within the community subgraph.
    let member_set: std::collections::HashSet<&EntityId> = members.iter().collect();
    let mut rel_counts: HashMap<String, usize> = HashMap::new();
    for (src, tgt, rel) in kg.edges() {
        if member_set.contains(&src.id) && member_set.contains(&tgt.id) {
            *rel_counts
                .entry(rel.relation_type_str())
                .or_insert(0) += 1;
        }
    }
    let mut rel_pairs: Vec<(String, usize)> = rel_counts.into_iter().collect();
    rel_pairs.sort_by(|a, b| b.1.cmp(&a.1));
    let key_relationships: Vec<String> = rel_pairs
        .into_iter()
        .take(5)
        .map(|(rt, count)| format!("{rt} ({count})"))
        .collect();

    // Entity type distribution.
    let mut type_counts: HashMap<String, usize> = HashMap::new();
    for id in members {
        if let Some(entity) = kg.entity(id) {
            *type_counts
                .entry(entity.entity_type.discriminant().to_owned())
                .or_insert(0) += 1;
        }
    }
    let mut type_pairs: Vec<(String, usize)> = type_counts.into_iter().collect();
    type_pairs.sort_by(|a, b| b.1.cmp(&a.1));

    // Build description.
    let description = build_description(
        &label,
        members.len(),
        &top_entities,
        &key_relationships,
        &source_files,
        &type_pairs,
    );

    CommunitySummary {
        id: cid,
        label,
        member_count: members.len(),
        top_entities,
        description,
        key_relationships,
        source_files,
    }
}

/// Assemble a human-readable description from community statistics.
fn build_description(
    label: &str,
    member_count: usize,
    top_entities: &[String],
    key_relationships: &[String],
    source_files: &[String],
    type_pairs: &[(String, usize)],
) -> String {
    let mut parts: Vec<String> = Vec::new();

    // Opening line.
    let type_summary = if type_pairs.is_empty() {
        String::new()
    } else {
        let top_type = &type_pairs[0].0;
        format!(" primarily composed of {top_type} entities")
    };
    parts.push(format!(
        "Community \"{label}\" contains {member_count} entities{type_summary}."
    ));

    // Top entities.
    if !top_entities.is_empty() {
        let names = top_entities.join(", ");
        parts.push(format!("Key entities: {names}."));
    }

    // Relationship types.
    if !key_relationships.is_empty() {
        let rels = key_relationships.join(", ");
        parts.push(format!("Dominant relationships: {rels}."));
    }

    // Source files.
    if !source_files.is_empty() {
        if source_files.len() <= 5 {
            let files = source_files.join(", ");
            parts.push(format!("Source files: {files}."));
        } else {
            let shown: Vec<&str> = source_files.iter().take(5).map(|s| s.as_str()).collect();
            parts.push(format!(
                "Source files: {} and {} more.",
                shown.join(", "),
                source_files.len() - 5
            ));
        }
    }

    parts.join(" ")
}

/// Search community summaries for a query string.
///
/// Returns `(community_id, match_score)` pairs sorted by descending score.
/// Useful for "what is this about?" style queries where the user wants
/// high-level understanding rather than specific entity matches.
pub fn search_summaries(
    summaries: &HashMap<usize, CommunitySummary>,
    query: &str,
) -> Vec<(usize, f64)> {
    let terms: Vec<String> = query
        .split_whitespace()
        .filter(|t| t.len() > 2)
        .map(|t| t.to_lowercase())
        .collect();

    if terms.is_empty() {
        return Vec::new();
    }

    let mut scored: Vec<(usize, f64)> = summaries
        .iter()
        .filter_map(|(&cid, summary)| {
            let haystack = format!(
                "{} {} {} {}",
                summary.label,
                summary.description,
                summary.top_entities.join(" "),
                summary.key_relationships.join(" "),
            )
            .to_lowercase();

            let score: f64 = terms
                .iter()
                .map(|t| {
                    if haystack.contains(t.as_str()) {
                        1.0
                    } else {
                        0.0
                    }
                })
                .sum();

            if score > 0.0 {
                Some((cid, score))
            } else {
                None
            }
        })
        .collect();

    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    scored
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

    fn make_entity(name: &str, source_file: &str) -> Entity {
        Entity {
            id: EntityId::new(&DomainTag::Code, &EntityType::Function, name, source_file),
            entity_type: EntityType::Function,
            label: name.to_owned(),
            source_file: Some(source_file.to_owned()),
            source_location: None,
            file_type: FileType::Code,
            metadata: serde_json::json!({}),
            legacy_id: None,
            iri: None,
        }
    }

    fn make_rel(src_name: &str, src_file: &str, tgt_name: &str, tgt_file: &str) -> Relationship {
        Relationship {
            source: EntityId::new(&DomainTag::Code, &EntityType::Function, src_name, src_file),
            target: EntityId::new(&DomainTag::Code, &EntityType::Function, tgt_name, tgt_file),
            relation_type: RelationType::Calls,
            confidence: Confidence::Extracted,
            weight: 1.0,
            source_file: None,
            source_location: None,
            metadata: serde_json::json!({}),
        }
    }

    fn build_test_graph() -> (KnowledgeGraph, HashMap<usize, Vec<EntityId>>, HashMap<usize, String>) {
        let entities = vec![
            make_entity("auth_login", "auth.py"),
            make_entity("auth_logout", "auth.py"),
            make_entity("auth_check", "auth.py"),
            make_entity("db_connect", "db.py"),
            make_entity("db_query", "db.py"),
        ];
        let rels = vec![
            make_rel("auth_login", "auth.py", "auth_check", "auth.py"),
            make_rel("auth_logout", "auth.py", "auth_check", "auth.py"),
            make_rel("db_connect", "db.py", "db_query", "db.py"),
        ];
        let kg = KnowledgeGraph::from_parts(entities.clone(), rels, vec![]);

        let mut communities = HashMap::new();
        communities.insert(
            0,
            vec![
                entities[0].id.clone(),
                entities[1].id.clone(),
                entities[2].id.clone(),
            ],
        );
        communities.insert(
            1,
            vec![entities[3].id.clone(), entities[4].id.clone()],
        );

        let mut labels = HashMap::new();
        labels.insert(0, "auth".to_owned());
        labels.insert(1, "db".to_owned());

        (kg, communities, labels)
    }

    #[test]
    fn generate_summaries_covers_all_communities() {
        let (kg, communities, labels) = build_test_graph();
        let summaries = generate_community_summaries(&kg, &communities, &labels);
        assert_eq!(summaries.len(), 2);
        assert!(summaries.contains_key(&0));
        assert!(summaries.contains_key(&1));
    }

    #[test]
    fn summary_member_count_matches() {
        let (kg, communities, labels) = build_test_graph();
        let summaries = generate_community_summaries(&kg, &communities, &labels);
        assert_eq!(summaries[&0].member_count, 3);
        assert_eq!(summaries[&1].member_count, 2);
    }

    #[test]
    fn summary_label_from_auto_label() {
        let (kg, communities, labels) = build_test_graph();
        let summaries = generate_community_summaries(&kg, &communities, &labels);
        assert_eq!(summaries[&0].label, "auth");
        assert_eq!(summaries[&1].label, "db");
    }

    #[test]
    fn summary_description_not_empty() {
        let (kg, communities, labels) = build_test_graph();
        let summaries = generate_community_summaries(&kg, &communities, &labels);
        for s in summaries.values() {
            assert!(!s.description.is_empty(), "description should not be empty");
        }
    }

    #[test]
    fn summary_top_entities_limited_to_5() {
        let (kg, communities, labels) = build_test_graph();
        let summaries = generate_community_summaries(&kg, &communities, &labels);
        for s in summaries.values() {
            assert!(s.top_entities.len() <= 5);
        }
    }

    #[test]
    fn summary_source_files_populated() {
        let (kg, communities, labels) = build_test_graph();
        let summaries = generate_community_summaries(&kg, &communities, &labels);
        assert!(summaries[&0].source_files.contains(&"auth.py".to_owned()));
        assert!(summaries[&1].source_files.contains(&"db.py".to_owned()));
    }

    #[test]
    fn summary_key_relationships_populated() {
        let (kg, communities, labels) = build_test_graph();
        let summaries = generate_community_summaries(&kg, &communities, &labels);
        // Community 0 has 2 "calls" relationships
        assert!(!summaries[&0].key_relationships.is_empty());
    }

    #[test]
    fn search_summaries_finds_matching() {
        let (kg, communities, labels) = build_test_graph();
        let summaries = generate_community_summaries(&kg, &communities, &labels);
        let results = search_summaries(&summaries, "auth login");
        assert!(!results.is_empty());
        // Auth community should be the top match
        assert_eq!(results[0].0, 0);
    }

    #[test]
    fn search_summaries_no_match_returns_empty() {
        let (kg, communities, labels) = build_test_graph();
        let summaries = generate_community_summaries(&kg, &communities, &labels);
        let results = search_summaries(&summaries, "zzz nonexistent xyz");
        assert!(results.is_empty());
    }

    #[test]
    fn search_summaries_short_terms_filtered() {
        let (kg, communities, labels) = build_test_graph();
        let summaries = generate_community_summaries(&kg, &communities, &labels);
        // All terms are <= 2 chars, should return empty
        let results = search_summaries(&summaries, "a b c");
        assert!(results.is_empty());
    }

    #[test]
    fn empty_graph_empty_summaries() {
        let kg = KnowledgeGraph::new();
        let communities = HashMap::new();
        let labels = HashMap::new();
        let summaries = generate_community_summaries(&kg, &communities, &labels);
        assert!(summaries.is_empty());
    }
}
