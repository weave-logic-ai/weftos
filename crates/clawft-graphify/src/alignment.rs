//! KG-015: Entity alignment for multi-repo deduplication.
//!
//! Given two knowledge graphs (e.g. from different repositories or
//! extraction runs), find entities that represent the same concept.
//! Uses a combination of label similarity (edit distance) and
//! structural similarity (Jaccard overlap of neighbor labels).
//!
//! ## Algorithm
//!
//! 1. For each entity in graph A, find candidates in graph B by label
//!    similarity (normalized Levenshtein distance).
//! 2. For the top candidates, compute structural similarity as the
//!    Jaccard coefficient of their neighbor label sets.
//! 3. Combined score = `0.6 * label_sim + 0.4 * structural_sim`.
//! 4. Keep alignments whose combined score exceeds the threshold.
//!
//! ## Reference
//!
//! Paper 11 -- EA-Agent (ACL 2026, arxiv 2604.11686).

use crate::entity::EntityId;
use crate::model::KnowledgeGraph;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Method used to establish an entity alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AlignmentMethod {
    /// Aligned primarily by label similarity.
    LabelMatch,
    /// Aligned primarily by structural (neighbor) similarity.
    StructuralMatch,
    /// Both label and structural similarity contributed.
    Combined,
}

/// A single aligned pair between two knowledge graphs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityAlignment {
    /// Entity ID from graph A.
    pub entity_a: EntityId,
    /// Entity ID from graph B.
    pub entity_b: EntityId,
    /// Combined similarity score in [0, 1].
    pub similarity: f64,
    /// Which method dominated the alignment.
    pub method: AlignmentMethod,
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Tuning parameters for entity alignment.
#[derive(Debug, Clone)]
pub struct AlignmentConfig {
    /// Maximum number of label-similar candidates to consider per
    /// entity (default: 10).
    pub max_candidates: usize,
    /// Minimum label similarity to be considered a candidate (default: 0.5).
    pub label_candidate_threshold: f64,
    /// Weight for label similarity in the combined score (default: 0.6).
    pub label_weight: f64,
    /// Weight for structural similarity in the combined score (default: 0.4).
    pub structural_weight: f64,
}

impl Default for AlignmentConfig {
    fn default() -> Self {
        Self {
            max_candidates: 10,
            label_candidate_threshold: 0.5,
            label_weight: 0.6,
            structural_weight: 0.4,
        }
    }
}

// ---------------------------------------------------------------------------
// String similarity
// ---------------------------------------------------------------------------

/// Normalized Levenshtein similarity in [0, 1].
///
/// Returns 1.0 for identical strings, 0.0 for completely different.
fn label_similarity(a: &str, b: &str) -> f64 {
    if a == b {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let dist = levenshtein(a, b);
    let max_len = a.len().max(b.len());
    1.0 - (dist as f64 / max_len as f64)
}

/// Classic Levenshtein distance (dynamic programming, O(n*m)).
fn levenshtein(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let n = a_chars.len();
    let m = b_chars.len();

    // prev and curr rows of the DP table.
    let mut prev: Vec<usize> = (0..=m).collect();
    let mut curr = vec![0usize; m + 1];

    for i in 1..=n {
        curr[0] = i;
        for j in 1..=m {
            let cost = if a_chars[i - 1] == b_chars[j - 1] {
                0
            } else {
                1
            };
            curr[j] = (prev[j] + 1) // deletion
                .min(curr[j - 1] + 1) // insertion
                .min(prev[j - 1] + cost); // substitution
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[m]
}

// ---------------------------------------------------------------------------
// Structural similarity
// ---------------------------------------------------------------------------

/// Collect the set of neighbor labels for an entity.
fn neighbor_labels(kg: &KnowledgeGraph, id: &EntityId) -> HashSet<String> {
    kg.neighbors(id)
        .into_iter()
        .map(|e| e.label.clone())
        .collect()
}

/// Jaccard coefficient of two sets.
fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 0.0;
    }
    let intersection = a.intersection(b).count();
    let union = a.union(b).count();
    intersection as f64 / union as f64
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Find entities across two knowledge graphs that represent the same
/// concept.
///
/// Uses label similarity + structural position (neighbor overlap).
/// Returns all alignments whose combined score exceeds `threshold`.
pub fn align_entities(
    graph_a: &KnowledgeGraph,
    graph_b: &KnowledgeGraph,
    threshold: f64,
) -> Vec<EntityAlignment> {
    align_entities_with_config(graph_a, graph_b, threshold, &AlignmentConfig::default())
}

/// Like [`align_entities`] but with explicit configuration.
pub fn align_entities_with_config(
    graph_a: &KnowledgeGraph,
    graph_b: &KnowledgeGraph,
    threshold: f64,
    config: &AlignmentConfig,
) -> Vec<EntityAlignment> {
    let mut alignments = Vec::new();

    // Pre-collect graph B entities for candidate search.
    let b_entities: Vec<_> = graph_b.entities().collect();

    for entity_a in graph_a.entities() {
        let label_a = &entity_a.label;

        // Step 1: find top label-similar candidates in B.
        let mut candidates: Vec<(&EntityId, f64)> = b_entities
            .iter()
            .filter_map(|entity_b| {
                let sim = label_similarity(label_a, &entity_b.label);
                if sim >= config.label_candidate_threshold {
                    Some((&entity_b.id, sim))
                } else {
                    None
                }
            })
            .collect();

        // Sort descending by similarity.
        candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        candidates.truncate(config.max_candidates);

        if candidates.is_empty() {
            continue;
        }

        // Step 2: compute structural similarity for top candidates.
        let neighbors_a = neighbor_labels(graph_a, &entity_a.id);

        for (id_b, label_sim) in &candidates {
            let neighbors_b = neighbor_labels(graph_b, id_b);
            let structural_sim = jaccard(&neighbors_a, &neighbors_b);

            // Step 3: combined score.
            let combined =
                config.label_weight * label_sim + config.structural_weight * structural_sim;

            if combined >= threshold {
                let method = if *label_sim >= 0.8 && structural_sim >= 0.3 {
                    AlignmentMethod::Combined
                } else if *label_sim >= 0.8 {
                    AlignmentMethod::LabelMatch
                } else {
                    AlignmentMethod::StructuralMatch
                };

                alignments.push(EntityAlignment {
                    entity_a: entity_a.id.clone(),
                    entity_b: (*id_b).clone(),
                    similarity: combined,
                    method,
                });
            }
        }
    }

    // Sort by similarity descending for deterministic output.
    alignments.sort_by(|a, b| {
        b.similarity
            .partial_cmp(&a.similarity)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    alignments
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

    fn entity(name: &str, source: &str) -> Entity {
        Entity {
            id: EntityId::new(&DomainTag::Code, &EntityType::Module, name, source),
            entity_type: EntityType::Module,
            label: name.to_string(),
            source_file: Some(source.into()),
            source_location: None,
            file_type: FileType::Code,
            metadata: serde_json::json!({}),
            legacy_id: None,
            iri: None,
        }
    }

    fn rel(src: &Entity, tgt: &Entity) -> Relationship {
        Relationship {
            source: src.id.clone(),
            target: tgt.id.clone(),
            relation_type: RelationType::Imports,
            confidence: Confidence::Extracted,
            weight: 1.0,
            source_file: None,
            source_location: None,
            metadata: serde_json::json!({}),
        }
    }

    #[test]
    fn levenshtein_basic() {
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("abc", "abc"), 0);
    }

    #[test]
    fn label_similarity_identical() {
        assert!((label_similarity("auth", "auth") - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn label_similarity_empty() {
        assert!(label_similarity("", "abc").abs() < f64::EPSILON);
    }

    #[test]
    fn label_similarity_similar() {
        let sim = label_similarity("auth_service", "auth_srvice");
        assert!(sim > 0.8, "Expected > 0.8 but got {sim}");
    }

    #[test]
    fn jaccard_empty_sets() {
        let a: HashSet<String> = HashSet::new();
        let b: HashSet<String> = HashSet::new();
        assert!(jaccard(&a, &b).abs() < f64::EPSILON);
    }

    #[test]
    fn jaccard_identical_sets() {
        let a: HashSet<String> = ["x", "y"].iter().map(|s| s.to_string()).collect();
        let b = a.clone();
        assert!((jaccard(&a, &b) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn jaccard_partial_overlap() {
        let a: HashSet<String> = ["x", "y", "z"].iter().map(|s| s.to_string()).collect();
        let b: HashSet<String> = ["y", "z", "w"].iter().map(|s| s.to_string()).collect();
        // intersection=2, union=4 => 0.5
        assert!((jaccard(&a, &b) - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn align_identical_graphs() {
        let e1 = entity("auth_service", "a.rs");
        let e2 = entity("db_pool", "a.rs");
        let r = rel(&e1, &e2);

        let mut kg_a = KnowledgeGraph::new();
        kg_a.add_entity(e1.clone());
        kg_a.add_entity(e2.clone());
        kg_a.add_relationship(r.clone());

        let mut kg_b = KnowledgeGraph::new();
        kg_b.add_entity(e1);
        kg_b.add_entity(e2);
        kg_b.add_relationship(r);

        let alignments = align_entities(&kg_a, &kg_b, 0.5);
        assert!(
            alignments.len() >= 2,
            "Expected >= 2 alignments for identical graphs, got {}",
            alignments.len()
        );
        // Perfect matches should have similarity ~1.0
        for a in &alignments {
            assert!(a.similarity > 0.9, "Expected > 0.9, got {}", a.similarity);
        }
    }

    #[test]
    fn align_no_match() {
        let mut kg_a = KnowledgeGraph::new();
        kg_a.add_entity(entity("alpha", "a.rs"));

        let mut kg_b = KnowledgeGraph::new();
        kg_b.add_entity(entity("zzzzz_completely_different", "b.rs"));

        let alignments = align_entities(&kg_a, &kg_b, 0.8);
        assert!(alignments.is_empty());
    }

    #[test]
    fn align_partial_label_match() {
        let mut kg_a = KnowledgeGraph::new();
        kg_a.add_entity(entity("user_service", "a.rs"));

        let mut kg_b = KnowledgeGraph::new();
        kg_b.add_entity(entity("user_srvice", "b.rs")); // typo

        // Label similarity ~0.9, no neighbors, so combined ~0.54
        let alignments = align_entities(&kg_a, &kg_b, 0.5);
        assert_eq!(alignments.len(), 1);
        assert_eq!(alignments[0].method, AlignmentMethod::LabelMatch);
    }

    #[test]
    fn align_structural_boost() {
        // Two graphs with slightly different entity names but same neighbors.
        let ea1 = entity("auth_mod", "a.rs");
        let ea2 = entity("db_pool", "a.rs");
        let ea3 = entity("config", "a.rs");

        let mut kg_a = KnowledgeGraph::new();
        kg_a.add_entity(ea1.clone());
        kg_a.add_entity(ea2.clone());
        kg_a.add_entity(ea3.clone());
        kg_a.add_relationship(rel(&ea1, &ea2));
        kg_a.add_relationship(rel(&ea1, &ea3));

        // Graph B: same structure, same neighbor labels, similar root label.
        let eb1 = entity("auth_module", "b.rs"); // similar to "auth_mod"
        let eb2 = entity("db_pool", "b.rs"); // same neighbor
        let eb3 = entity("config", "b.rs"); // same neighbor

        let mut kg_b = KnowledgeGraph::new();
        kg_b.add_entity(eb1.clone());
        kg_b.add_entity(eb2.clone());
        kg_b.add_entity(eb3.clone());
        kg_b.add_relationship(rel(&eb1, &eb2));
        kg_b.add_relationship(rel(&eb1, &eb3));

        // auth_mod <-> auth_module should get a structural boost.
        let alignments = align_entities(&kg_a, &kg_b, 0.5);
        let auth_alignment = alignments
            .iter()
            .find(|a| a.entity_a == ea1.id && a.entity_b == eb1.id);
        assert!(
            auth_alignment.is_some(),
            "Expected alignment between auth_mod and auth_module"
        );
        let auth = auth_alignment.unwrap();
        assert!(
            auth.similarity > 0.6,
            "Expected structural boost, got {}",
            auth.similarity
        );
    }

    #[test]
    fn align_empty_graphs() {
        let kg_a = KnowledgeGraph::new();
        let kg_b = KnowledgeGraph::new();
        let alignments = align_entities(&kg_a, &kg_b, 0.5);
        assert!(alignments.is_empty());
    }

    #[test]
    fn align_custom_config() {
        let mut kg_a = KnowledgeGraph::new();
        kg_a.add_entity(entity("foo", "a.rs"));

        let mut kg_b = KnowledgeGraph::new();
        kg_b.add_entity(entity("foo", "b.rs"));

        let config = AlignmentConfig {
            max_candidates: 1,
            label_candidate_threshold: 0.9,
            label_weight: 1.0,
            structural_weight: 0.0,
            ..Default::default()
        };
        let alignments = align_entities_with_config(&kg_a, &kg_b, 0.5, &config);
        assert_eq!(alignments.len(), 1);
        assert!((alignments[0].similarity - 1.0).abs() < f64::EPSILON);
    }
}
