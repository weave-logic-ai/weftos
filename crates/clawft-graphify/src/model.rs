//! Core data model: `Entity`, `ExtractionResult`, `KnowledgeGraph`, and
//! supporting types.

use std::collections::{HashMap, HashSet, VecDeque};

use petgraph::graph::{EdgeIndex, NodeIndex};
use petgraph::visit::EdgeRef;
use petgraph::Direction;
use serde::{Deserialize, Serialize};

use crate::entity::{EntityId, EntityType, FileType};
use crate::relationship::Relationship;

// ---------------------------------------------------------------------------
// Entity
// ---------------------------------------------------------------------------

/// A node in the knowledge graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    pub id: EntityId,
    pub entity_type: EntityType,
    pub label: String,
    /// Globally unique concept IRI for ontology interoperability.
    /// The word is a label; the IRI is the concept. Two entities with the same
    /// label but different IRIs are different things (e.g., "Service" in
    /// architecture vs "Service" in customer support).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iri: Option<String>,
    pub source_file: Option<String>,
    pub source_location: Option<String>,
    pub file_type: FileType,
    pub metadata: serde_json::Value,
    /// Optional legacy Python-style string ID (for backward-compatible export).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub legacy_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Hyperedge
// ---------------------------------------------------------------------------

/// A hyperedge connecting multiple entities (e.g., a function signature group).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hyperedge {
    pub label: String,
    pub entity_ids: Vec<EntityId>,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

// ---------------------------------------------------------------------------
// ExtractionResult
// ---------------------------------------------------------------------------

/// The output of extracting entities and relationships from a single file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExtractionResult {
    pub source_file: String,
    pub entities: Vec<Entity>,
    pub relationships: Vec<Relationship>,
    #[serde(default)]
    pub hyperedges: Vec<Hyperedge>,
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub errors: Vec<String>,
}

impl ExtractionResult {
    /// Create an empty extraction result for a source file.
    pub fn empty(source_file: impl AsRef<std::path::Path>) -> Self {
        Self {
            source_file: source_file.as_ref().to_string_lossy().to_string(),
            entities: Vec::new(),
            relationships: Vec::new(),
            hyperedges: Vec::new(),
            input_tokens: 0,
            output_tokens: 0,
            errors: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// ExtractionStats
// ---------------------------------------------------------------------------

/// Aggregate statistics for an extraction run.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExtractionStats {
    pub files_processed: usize,
    pub files_skipped: usize,
    pub entities_extracted: usize,
    pub relationships_extracted: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_hits: usize,
    pub extraction_duration_ms: u64,
}

// ---------------------------------------------------------------------------
// GodNode / SurprisingConnection
// ---------------------------------------------------------------------------

/// A node with disproportionately high connectivity (coupling risk).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GodNode {
    pub entity_id: EntityId,
    pub label: String,
    pub degree: usize,
    pub entity_type: EntityType,
    pub source_file: Option<String>,
}

/// An edge connecting nodes from different communities (unexpected coupling).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SurprisingConnection {
    pub source_id: EntityId,
    pub source_label: String,
    pub target_id: EntityId,
    pub target_label: String,
    pub source_community: Option<usize>,
    pub target_community: Option<usize>,
}

/// An automatically generated investigation question.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuggestedQuestion {
    pub question: String,
    pub entity_ids: Vec<EntityId>,
}

/// Delta between two knowledge graphs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GraphDiff {
    pub entities_added: Vec<EntityId>,
    pub entities_removed: Vec<EntityId>,
    pub relationships_added: usize,
    pub relationships_removed: usize,
}

// ---------------------------------------------------------------------------
// KnowledgeGraph
// ---------------------------------------------------------------------------

/// In-memory knowledge graph backed by petgraph.
///
/// This is the standalone representation used for analysis and export. It can
/// optionally be bridged into the ECC subsystems (CausalGraph, HNSW,
/// CrossRefStore) via the `kernel-bridge` feature.
pub struct KnowledgeGraph {
    #[allow(unused)]
    graph: petgraph::Graph<Entity, Relationship, petgraph::Directed>,
    entity_index: HashMap<EntityId, NodeIndex>,
    /// Community assignments after clustering.
    pub communities: Option<HashMap<usize, Vec<EntityId>>>,
    /// Community labels (auto-generated or user-provided).
    pub community_labels: Option<HashMap<usize, String>>,
    /// Community summaries (generated by KG-002 GraphRAG).
    pub community_summaries: Option<HashMap<usize, crate::summary::CommunitySummary>>,
    /// Extraction statistics.
    pub stats: ExtractionStats,
    /// Hyperedges stored at graph level.
    pub hyperedges: Vec<Hyperedge>,
}

impl std::fmt::Debug for KnowledgeGraph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KnowledgeGraph")
            .field("nodes", &self.graph.node_count())
            .field("edges", &self.graph.edge_count())
            .finish()
    }
}

impl Default for KnowledgeGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl KnowledgeGraph {
    /// Create an empty knowledge graph.
    pub fn new() -> Self {
        Self {
            graph: petgraph::Graph::new(),
            entity_index: HashMap::new(),
            communities: None,
            community_labels: None,
            community_summaries: None,
            stats: ExtractionStats::default(),
            hyperedges: Vec::new(),
        }
    }

    /// Add an entity (idempotent: if the ID already exists, overwrite attributes).
    ///
    /// Returns the petgraph `NodeIndex`.
    pub fn add_entity(&mut self, entity: Entity) -> NodeIndex {
        if let Some(&idx) = self.entity_index.get(&entity.id) {
            // Last-write-wins: overwrite the existing node's data.
            self.graph[idx] = entity;
            idx
        } else {
            let id = entity.id.clone();
            let idx = self.graph.add_node(entity);
            self.entity_index.insert(id, idx);
            idx
        }
    }

    /// Add a relationship between two entities.
    ///
    /// Returns `None` if source or target is not in the graph (silently skips,
    /// matching Python behavior for external/stdlib imports).
    pub fn add_relationship(&mut self, rel: Relationship) -> Option<EdgeIndex> {
        let src_idx = self.entity_index.get(&rel.source)?;
        let tgt_idx = self.entity_index.get(&rel.target)?;
        Some(self.graph.add_edge(*src_idx, *tgt_idx, rel))
    }

    /// Look up an entity by its ID.
    pub fn entity(&self, id: &EntityId) -> Option<&Entity> {
        self.entity_index.get(id).map(|&idx| &self.graph[idx])
    }

    /// Number of entities in the graph.
    pub fn entity_count(&self) -> usize {
        self.graph.node_count()
    }

    /// Number of relationships in the graph.
    pub fn relationship_count(&self) -> usize {
        self.graph.edge_count()
    }

    /// Return the neighbors of an entity (both incoming and outgoing).
    pub fn neighbors(&self, id: &EntityId) -> Vec<&Entity> {
        let Some(&idx) = self.entity_index.get(id) else {
            return Vec::new();
        };
        let mut seen = std::collections::HashSet::new();
        let mut result = Vec::new();
        for neighbor_idx in self.graph.neighbors_undirected(idx) {
            if seen.insert(neighbor_idx) {
                result.push(&self.graph[neighbor_idx]);
            }
        }
        result
    }

    /// Return the degree (number of edges, both directions) of an entity.
    pub fn degree(&self, id: &EntityId) -> usize {
        let Some(&idx) = self.entity_index.get(id) else {
            return 0;
        };
        self.graph.edges_directed(idx, Direction::Outgoing).count()
            + self.graph.edges_directed(idx, Direction::Incoming).count()
    }

    /// Iterator over all edges as (source_entity, target_entity, relationship).
    pub fn edges(&self) -> impl Iterator<Item = (&Entity, &Entity, &Relationship)> {
        self.graph.edge_references().map(move |e| {
            let src = &self.graph[e.source()];
            let tgt = &self.graph[e.target()];
            (src, tgt, e.weight())
        })
    }

    /// Iterate over all entity IDs.
    pub fn node_ids(&self) -> impl Iterator<Item = &EntityId> {
        self.entity_index.keys()
    }

    /// Iterate over all entities.
    pub fn entities(&self) -> impl Iterator<Item = &Entity> {
        self.graph.node_weights()
    }

    /// Extract a subgraph containing only the specified entity IDs.
    ///
    /// Edges between included nodes are preserved.
    pub fn subgraph(&self, ids: &[EntityId]) -> KnowledgeGraph {
        let mut sub = KnowledgeGraph::new();
        let id_set: std::collections::HashSet<&EntityId> = ids.iter().collect();

        // Add nodes.
        for id in ids {
            if let Some(entity) = self.entity(id) {
                sub.add_entity(entity.clone());
            }
        }

        // Add edges between included nodes.
        for edge_ref in self.graph.edge_references() {
            let src_entity = &self.graph[edge_ref.source()];
            let tgt_entity = &self.graph[edge_ref.target()];
            if id_set.contains(&src_entity.id) && id_set.contains(&tgt_entity.id) {
                sub.add_relationship(edge_ref.weight().clone());
            }
        }

        sub
    }

    /// Access the underlying petgraph (read-only, for analysis algorithms).
    pub fn inner_graph(&self) -> &petgraph::Graph<Entity, Relationship, petgraph::Directed> {
        &self.graph
    }

    /// Access the entity index (read-only).
    pub fn entity_index(&self) -> &HashMap<EntityId, NodeIndex> {
        &self.entity_index
    }

    /// Alias for `entity_count()` -- used by analysis modules.
    pub fn node_count(&self) -> usize {
        self.entity_count()
    }

    /// Alias for `relationship_count()` -- used by analysis modules.
    pub fn edge_count(&self) -> usize {
        self.relationship_count()
    }

    /// Iterate over all entity IDs (alias for `node_ids()`).
    pub fn entity_ids(&self) -> impl Iterator<Item = &EntityId> {
        self.node_ids()
    }

    /// Remove an entity and all its incident edges.
    ///
    /// Returns `true` if the entity existed and was removed.
    pub fn remove_entity(&mut self, id: &EntityId) -> bool {
        let Some(idx) = self.entity_index.remove(id) else {
            return false;
        };

        // Collect edge indices to remove (both directions).
        let edge_indices: Vec<petgraph::graph::EdgeIndex> = self
            .graph
            .edges_directed(idx, Direction::Outgoing)
            .chain(self.graph.edges_directed(idx, Direction::Incoming))
            .map(|e| e.id())
            .collect();

        // Remove edges in reverse-sorted order to keep indices stable.
        let mut sorted = edge_indices;
        sorted.sort_unstable();
        sorted.dedup();
        for ei in sorted.into_iter().rev() {
            self.graph.remove_edge(ei);
        }

        // Remove the node itself. petgraph swaps the last node into this slot,
        // so we must fix up entity_index for the swapped node.
        let last_idx = NodeIndex::new(self.graph.node_count() - 1);
        self.graph.remove_node(idx);

        // If the removed node was not the last, petgraph moved last_idx -> idx.
        if idx != last_idx && idx.index() < self.graph.node_count() {
            // The node that was at last_idx is now at idx.
            let moved_id = self.graph[idx].id.clone();
            self.entity_index.insert(moved_id, idx);
        }

        true
    }

    /// Collect all distinct source files referenced by entities.
    pub fn source_files(&self) -> Vec<String> {
        let mut files: std::collections::HashSet<String> = std::collections::HashSet::new();
        for e in self.entities() {
            if let Some(ref sf) = e.source_file
                && !sf.is_empty() {
                    files.insert(sf.clone());
                }
        }
        let mut v: Vec<String> = files.into_iter().collect();
        v.sort();
        v
    }

    /// Return all relationships (edges) incident on a given entity.
    pub fn edges_of(&self, id: &EntityId) -> Vec<&Relationship> {
        let Some(&idx) = self.entity_index.get(id) else {
            return Vec::new();
        };
        let mut result = Vec::new();
        for edge_ref in self.graph.edge_references() {
            if edge_ref.source() == idx || edge_ref.target() == idx {
                result.push(edge_ref.weight());
            }
        }
        result
    }

    /// Build a `KnowledgeGraph` from pre-collected entities, relationships, and hyperedges.
    ///
    /// This is a convenience constructor used by pipeline and test code.
    pub fn from_parts(
        entities: Vec<Entity>,
        relationships: Vec<Relationship>,
        hyperedges: Vec<Hyperedge>,
    ) -> Self {
        let mut kg = Self::new();
        for e in entities {
            kg.add_entity(e);
        }
        for r in relationships {
            kg.add_relationship(r);
        }
        kg.hyperedges = hyperedges;
        kg
    }
}

// ---------------------------------------------------------------------------
// KG-006: BFS Data Flow Tracing
// ---------------------------------------------------------------------------

/// Direction for data-flow tracing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FlowDirection {
    /// Follow outgoing edges (who does this entity call / depend on?).
    Forward,
    /// Follow incoming edges (who calls / depends on this entity?).
    Backward,
}

/// A single step in a data-flow trace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataFlowStep {
    pub entity: EntityId,
    pub label: String,
    pub depth: usize,
    pub edge_type: String,
    pub source_file: Option<String>,
}

/// Edge types that represent data/control flow.
const FLOW_EDGE_TYPES: &[&str] = &[
    "calls",
    "imports",
    "imports_from",
    "depends_on",
    "uses",
];

impl KnowledgeGraph {
    /// Trace data flow through call/import dependencies using BFS.
    ///
    /// Starting from `start`, follows flow-relevant edges (Calls, Imports,
    /// Uses, DependsOn) in the specified direction up to `max_depth` hops.
    pub fn trace_data_flow(
        &self,
        start: &EntityId,
        direction: FlowDirection,
        max_depth: usize,
    ) -> Vec<DataFlowStep> {
        let Some(&start_idx) = self.entity_index.get(start) else {
            return Vec::new();
        };

        let petgraph_dir = match direction {
            FlowDirection::Forward => Direction::Outgoing,
            FlowDirection::Backward => Direction::Incoming,
        };

        let mut visited: HashSet<NodeIndex> = HashSet::new();
        visited.insert(start_idx);

        // Queue entries: (node_index, depth, edge_type_that_led_here)
        let mut queue: VecDeque<(NodeIndex, usize, String)> = VecDeque::new();
        queue.push_back((start_idx, 0, String::new()));

        let mut result: Vec<DataFlowStep> = Vec::new();

        while let Some((current_idx, depth, edge_type)) = queue.pop_front() {
            // Record step (skip the start node itself).
            if current_idx != start_idx {
                let entity = &self.graph[current_idx];
                result.push(DataFlowStep {
                    entity: entity.id.clone(),
                    label: entity.label.clone(),
                    depth,
                    edge_type,
                    source_file: entity.source_file.clone(),
                });
            }

            if depth >= max_depth {
                continue;
            }

            // Expand neighbors along flow-relevant edges.
            for edge_ref in self.graph.edges_directed(current_idx, petgraph_dir) {
                let rel = edge_ref.weight();
                let rel_str = rel.relation_type_str();

                // Only follow flow-relevant edge types.
                if !FLOW_EDGE_TYPES.contains(&rel_str.as_str()) {
                    // Also accept Custom("uses") etc.
                    if !rel_str.starts_with("uses") {
                        continue;
                    }
                }

                let neighbor_idx = match petgraph_dir {
                    Direction::Outgoing => edge_ref.target(),
                    Direction::Incoming => edge_ref.source(),
                    // petgraph only has Outgoing/Incoming
                };

                if visited.insert(neighbor_idx) {
                    queue.push_back((neighbor_idx, depth + 1, rel_str));
                }
            }
        }

        result
    }
}

// ---------------------------------------------------------------------------
// KG-008: Entity Deduplication
// ---------------------------------------------------------------------------

/// Compute normalized edit distance between two strings (0.0 = identical, 1.0 = completely different).
fn normalized_edit_distance(a: &str, b: &str) -> f64 {
    let a_lower = a.to_lowercase();
    let b_lower = b.to_lowercase();
    let a_chars: Vec<char> = a_lower.chars().collect();
    let b_chars: Vec<char> = b_lower.chars().collect();
    let len_a = a_chars.len();
    let len_b = b_chars.len();

    if len_a == 0 && len_b == 0 {
        return 0.0;
    }
    if len_a == 0 || len_b == 0 {
        return 1.0;
    }

    // Classic DP Levenshtein.
    let mut prev: Vec<usize> = (0..=len_b).collect();
    let mut curr = vec![0usize; len_b + 1];

    for i in 1..=len_a {
        curr[0] = i;
        for j in 1..=len_b {
            let cost = if a_chars[i - 1] == b_chars[j - 1] {
                0
            } else {
                1
            };
            curr[j] = (prev[j] + 1)
                .min(curr[j - 1] + 1)
                .min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    let edit_dist = prev[len_b];
    let max_len = len_a.max(len_b);
    edit_dist as f64 / max_len as f64
}

/// Find near-duplicate entity pairs by label similarity.
///
/// Returns pairs of entity IDs where the normalized edit distance is below
/// `(1.0 - threshold)` AND both entities share the same `entity_type`.
pub fn find_duplicates(kg: &KnowledgeGraph, threshold: f64) -> Vec<(EntityId, EntityId)> {
    let entities: Vec<&Entity> = kg.entities().collect();
    let mut pairs: Vec<(EntityId, EntityId)> = Vec::new();

    for i in 0..entities.len() {
        for j in (i + 1)..entities.len() {
            let a = entities[i];
            let b = entities[j];

            // Must be the same entity type to be considered duplicates.
            if a.entity_type != b.entity_type {
                continue;
            }

            // Skip very short labels (high false-positive rate).
            if a.label.len() < 2 || b.label.len() < 2 {
                continue;
            }

            let distance = normalized_edit_distance(&a.label, &b.label);
            let similarity = 1.0 - distance;

            if similarity >= threshold {
                pairs.push((a.id.clone(), b.id.clone()));
            }
        }
    }

    pairs
}

impl KnowledgeGraph {
    /// Deduplicate near-duplicate entities by label similarity.
    ///
    /// For each duplicate pair, keeps the entity with more edges and redirects
    /// all edges from the removed entity to the kept one.
    pub fn dedup(&mut self, threshold: f64) -> usize {
        let pairs = find_duplicates(self, threshold);
        let mut merged_count = 0;

        // Track which IDs have already been merged away.
        let mut removed: HashSet<EntityId> = HashSet::new();

        for (id_a, id_b) in &pairs {
            if removed.contains(id_a) || removed.contains(id_b) {
                continue;
            }

            let deg_a = self.degree(id_a);
            let deg_b = self.degree(id_b);

            // Keep the one with more edges; tie-break: keep the first.
            let (keep, discard) = if deg_a >= deg_b {
                (id_a, id_b)
            } else {
                (id_b, id_a)
            };

            // Collect edges incident on the discarded entity to redirect.
            let edges_to_redirect: Vec<Relationship> = self
                .edges()
                .filter(|(src, tgt, _)| src.id == *discard || tgt.id == *discard)
                .map(|(_, _, rel)| {
                    let mut new_rel = rel.clone();
                    if new_rel.source == *discard {
                        new_rel.source = keep.clone();
                    }
                    if new_rel.target == *discard {
                        new_rel.target = keep.clone();
                    }
                    new_rel
                })
                // Skip self-loops that would result from redirection.
                .filter(|r| r.source != r.target)
                .collect();

            self.remove_entity(discard);

            // Re-add redirected edges (duplicates silently overlap via petgraph).
            for rel in edges_to_redirect {
                self.add_relationship(rel);
            }

            removed.insert(discard.clone());
            merged_count += 1;
        }

        merged_count
    }
}

// ---------------------------------------------------------------------------
// DetectionResult
// ---------------------------------------------------------------------------

/// Result of file detection / classification phase.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DetectionResult {
    pub total_files: usize,
    pub total_words: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::{DomainTag, EntityType, FileType};
    use crate::relationship::{Confidence, RelationType};

    fn make_entity(name: &str) -> Entity {
        Entity {
            id: EntityId::new(&DomainTag::Code, &EntityType::Module, name, "test.py"),
            entity_type: EntityType::Module,
            label: name.to_string(),
            source_file: Some("test.py".into()),
            source_location: Some("L1".into()),
            file_type: FileType::Code,
            metadata: serde_json::json!({}),
            legacy_id: None,
            iri: None,
        }
    }

    fn make_rel(src: &Entity, tgt: &Entity) -> Relationship {
        Relationship {
            source: src.id.clone(),
            target: tgt.id.clone(),
            relation_type: RelationType::Imports,
            confidence: Confidence::Extracted,
            weight: 1.0,
            source_file: Some("test.py".into()),
            source_location: Some("L1".into()),
            metadata: serde_json::json!({}),
        }
    }

    #[test]
    fn add_entity_idempotent() {
        let mut kg = KnowledgeGraph::new();
        let mut e = make_entity("auth");
        let idx1 = kg.add_entity(e.clone());
        e.label = "auth_updated".into();
        let idx2 = kg.add_entity(e);
        assert_eq!(idx1, idx2);
        assert_eq!(kg.entity_count(), 1);
        assert_eq!(kg.entity(&EntityId::new(&DomainTag::Code, &EntityType::Module, "auth", "test.py")).unwrap().label, "auth_updated");
    }

    #[test]
    fn add_relationship_skips_missing() {
        let mut kg = KnowledgeGraph::new();
        let e1 = make_entity("a");
        let e2 = make_entity("b");
        kg.add_entity(e1.clone());
        // e2 not added -- relationship should be silently skipped.
        let result = kg.add_relationship(make_rel(&e1, &e2));
        assert!(result.is_none());
        assert_eq!(kg.relationship_count(), 0);
    }

    #[test]
    fn neighbors_and_degree() {
        let mut kg = KnowledgeGraph::new();
        let e1 = make_entity("a");
        let e2 = make_entity("b");
        let e3 = make_entity("c");
        kg.add_entity(e1.clone());
        kg.add_entity(e2.clone());
        kg.add_entity(e3.clone());
        kg.add_relationship(make_rel(&e1, &e2));
        kg.add_relationship(make_rel(&e1, &e3));

        let neighbors = kg.neighbors(&e1.id);
        assert_eq!(neighbors.len(), 2);
        assert_eq!(kg.degree(&e1.id), 2);
        assert_eq!(kg.degree(&e2.id), 1);
    }

    #[test]
    fn subgraph_preserves_edges() {
        let mut kg = KnowledgeGraph::new();
        let e1 = make_entity("a");
        let e2 = make_entity("b");
        let e3 = make_entity("c");
        kg.add_entity(e1.clone());
        kg.add_entity(e2.clone());
        kg.add_entity(e3.clone());
        kg.add_relationship(make_rel(&e1, &e2));
        kg.add_relationship(make_rel(&e2, &e3));

        let sub = kg.subgraph(&[e1.id.clone(), e2.id.clone()]);
        assert_eq!(sub.entity_count(), 2);
        assert_eq!(sub.relationship_count(), 1); // only e1->e2, not e2->e3
    }

    #[test]
    fn remove_entity_basic() {
        let mut kg = KnowledgeGraph::new();
        let e1 = make_entity("a");
        let e2 = make_entity("b");
        kg.add_entity(e1.clone());
        kg.add_entity(e2.clone());
        kg.add_relationship(make_rel(&e1, &e2));
        assert_eq!(kg.entity_count(), 2);
        assert_eq!(kg.relationship_count(), 1);

        assert!(kg.remove_entity(&e2.id));
        assert_eq!(kg.entity_count(), 1);
        assert_eq!(kg.relationship_count(), 0);
        assert!(kg.entity(&e1.id).is_some());
        assert!(kg.entity(&e2.id).is_none());
    }

    #[test]
    fn remove_entity_not_found() {
        let mut kg = KnowledgeGraph::new();
        let e1 = make_entity("a");
        assert!(!kg.remove_entity(&e1.id));
    }

    #[test]
    fn remove_entity_preserves_others() {
        let mut kg = KnowledgeGraph::new();
        let e1 = make_entity("a");
        let e2 = make_entity("b");
        let e3 = make_entity("c");
        kg.add_entity(e1.clone());
        kg.add_entity(e2.clone());
        kg.add_entity(e3.clone());
        kg.add_relationship(make_rel(&e1, &e2));
        kg.add_relationship(make_rel(&e2, &e3));

        kg.remove_entity(&e2.id);
        assert_eq!(kg.entity_count(), 2);
        assert_eq!(kg.relationship_count(), 0);
        // Both remaining entities should still be findable.
        assert!(kg.entity(&e1.id).is_some());
        assert!(kg.entity(&e3.id).is_some());
    }

    #[test]
    fn scale_test_1000_entities() {
        let mut kg = KnowledgeGraph::new();
        let entities: Vec<Entity> = (0..1000)
            .map(|i| {
                Entity {
                    id: EntityId::new(&DomainTag::Code, &EntityType::Function, &format!("fn_{i}"), "big.py"),
                    entity_type: EntityType::Function,
                    label: format!("fn_{i}"),
                    source_file: Some("big.py".into()),
                    source_location: None,
                    file_type: FileType::Code,
                    metadata: serde_json::json!({}),
                    legacy_id: None,
                    iri: None,
                }
            })
            .collect();

        for e in &entities {
            kg.add_entity(e.clone());
        }
        assert_eq!(kg.entity_count(), 1000);

        // Add 3000 edges (each entity calls the next 3).
        let mut edge_count = 0;
        for i in 0..1000 {
            for j in 1..=3 {
                let tgt = (i + j) % 1000;
                let rel = Relationship {
                    source: entities[i].id.clone(),
                    target: entities[tgt].id.clone(),
                    relation_type: RelationType::Calls,
                    confidence: Confidence::Inferred,
                    weight: 0.8,
                    source_file: None,
                    source_location: None,
                    metadata: serde_json::json!({}),
                };
                if kg.add_relationship(rel).is_some() {
                    edge_count += 1;
                }
            }
        }
        assert_eq!(kg.entity_count(), 1000);
        assert_eq!(kg.relationship_count(), edge_count);
    }

    // -----------------------------------------------------------------------
    // KG-006: BFS Data Flow Tracing
    // -----------------------------------------------------------------------

    fn make_entity_fn(name: &str, file: &str) -> Entity {
        Entity {
            id: EntityId::new(&DomainTag::Code, &EntityType::Function, name, file),
            entity_type: EntityType::Function,
            label: name.to_string(),
            source_file: Some(file.into()),
            source_location: None,
            file_type: FileType::Code,
            metadata: serde_json::json!({}),
            legacy_id: None,
            iri: None,
        }
    }

    fn make_calls_rel(src: &Entity, tgt: &Entity) -> Relationship {
        Relationship {
            source: src.id.clone(),
            target: tgt.id.clone(),
            relation_type: RelationType::Calls,
            confidence: Confidence::Extracted,
            weight: 1.0,
            source_file: None,
            source_location: None,
            metadata: serde_json::json!({}),
        }
    }

    #[test]
    fn trace_data_flow_forward() {
        let mut kg = KnowledgeGraph::new();
        let a = make_entity_fn("a", "a.py");
        let b = make_entity_fn("b", "b.py");
        let c = make_entity_fn("c", "c.py");
        kg.add_entity(a.clone());
        kg.add_entity(b.clone());
        kg.add_entity(c.clone());
        kg.add_relationship(make_calls_rel(&a, &b));
        kg.add_relationship(make_calls_rel(&b, &c));

        let steps = kg.trace_data_flow(&a.id, FlowDirection::Forward, 3);
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].depth, 1);
        assert_eq!(steps[0].label, "b");
        assert_eq!(steps[1].depth, 2);
        assert_eq!(steps[1].label, "c");
    }

    #[test]
    fn trace_data_flow_backward() {
        let mut kg = KnowledgeGraph::new();
        let a = make_entity_fn("a", "a.py");
        let b = make_entity_fn("b", "b.py");
        let c = make_entity_fn("c", "c.py");
        kg.add_entity(a.clone());
        kg.add_entity(b.clone());
        kg.add_entity(c.clone());
        kg.add_relationship(make_calls_rel(&a, &b));
        kg.add_relationship(make_calls_rel(&b, &c));

        let steps = kg.trace_data_flow(&c.id, FlowDirection::Backward, 3);
        assert_eq!(steps.len(), 2);
        // BFS: first b (depth 1), then a (depth 2)
        assert_eq!(steps[0].depth, 1);
        assert_eq!(steps[0].label, "b");
        assert_eq!(steps[1].depth, 2);
        assert_eq!(steps[1].label, "a");
    }

    #[test]
    fn trace_data_flow_respects_max_depth() {
        let mut kg = KnowledgeGraph::new();
        let a = make_entity_fn("a", "a.py");
        let b = make_entity_fn("b", "b.py");
        let c = make_entity_fn("c", "c.py");
        kg.add_entity(a.clone());
        kg.add_entity(b.clone());
        kg.add_entity(c.clone());
        kg.add_relationship(make_calls_rel(&a, &b));
        kg.add_relationship(make_calls_rel(&b, &c));

        let steps = kg.trace_data_flow(&a.id, FlowDirection::Forward, 1);
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].label, "b");
    }

    #[test]
    fn trace_data_flow_unknown_start() {
        let kg = KnowledgeGraph::new();
        let fake_id = EntityId::new(&DomainTag::Code, &EntityType::Function, "nope", "x.py");
        let steps = kg.trace_data_flow(&fake_id, FlowDirection::Forward, 5);
        assert!(steps.is_empty());
    }

    #[test]
    fn trace_data_flow_ignores_contains_edges() {
        let mut kg = KnowledgeGraph::new();
        let a = make_entity_fn("a", "a.py");
        let b = make_entity_fn("b", "b.py");
        kg.add_entity(a.clone());
        kg.add_entity(b.clone());
        kg.add_relationship(Relationship {
            source: a.id.clone(),
            target: b.id.clone(),
            relation_type: RelationType::Contains,
            confidence: Confidence::Extracted,
            weight: 1.0,
            source_file: None,
            source_location: None,
            metadata: serde_json::json!({}),
        });

        let steps = kg.trace_data_flow(&a.id, FlowDirection::Forward, 3);
        assert!(steps.is_empty(), "Contains edges should not be followed in data flow");
    }

    #[test]
    fn trace_data_flow_follows_imports() {
        let mut kg = KnowledgeGraph::new();
        let a = make_entity_fn("a", "a.py");
        let b = make_entity_fn("b", "b.py");
        kg.add_entity(a.clone());
        kg.add_entity(b.clone());
        kg.add_relationship(Relationship {
            source: a.id.clone(),
            target: b.id.clone(),
            relation_type: RelationType::Imports,
            confidence: Confidence::Extracted,
            weight: 1.0,
            source_file: None,
            source_location: None,
            metadata: serde_json::json!({}),
        });

        let steps = kg.trace_data_flow(&a.id, FlowDirection::Forward, 3);
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].edge_type, "imports");
    }

    // -----------------------------------------------------------------------
    // KG-008: Entity Deduplication
    // -----------------------------------------------------------------------

    #[test]
    fn find_duplicates_by_label_similarity() {
        let mut kg = KnowledgeGraph::new();
        let e1 = Entity {
            id: EntityId::new(&DomainTag::Code, &EntityType::Function, "auth_service", "a.py"),
            entity_type: EntityType::Function,
            label: "AuthService".to_string(),
            source_file: Some("a.py".into()),
            source_location: None,
            file_type: FileType::Code,
            metadata: serde_json::json!({}),
            legacy_id: None,
            iri: None,
        };
        let e2 = Entity {
            id: EntityId::new(&DomainTag::Code, &EntityType::Function, "authservice", "b.py"),
            entity_type: EntityType::Function,
            label: "authservice".to_string(),
            source_file: Some("b.py".into()),
            source_location: None,
            file_type: FileType::Code,
            metadata: serde_json::json!({}),
            legacy_id: None,
            iri: None,
        };
        kg.add_entity(e1);
        kg.add_entity(e2);

        let pairs = find_duplicates(&kg, 0.85);
        // "AuthService" vs "authservice" (case-insensitive) -> identical -> similarity 1.0
        assert_eq!(pairs.len(), 1);
    }

    #[test]
    fn find_duplicates_different_types_not_matched() {
        let mut kg = KnowledgeGraph::new();
        let e1 = Entity {
            id: EntityId::new(&DomainTag::Code, &EntityType::Function, "auth", "a.py"),
            entity_type: EntityType::Function,
            label: "auth".to_string(),
            source_file: Some("a.py".into()),
            source_location: None,
            file_type: FileType::Code,
            metadata: serde_json::json!({}),
            legacy_id: None,
            iri: None,
        };
        let e2 = Entity {
            id: EntityId::new(&DomainTag::Code, &EntityType::Module, "auth", "b.py"),
            entity_type: EntityType::Module,
            label: "auth".to_string(),
            source_file: Some("b.py".into()),
            source_location: None,
            file_type: FileType::Code,
            metadata: serde_json::json!({}),
            legacy_id: None,
            iri: None,
        };
        kg.add_entity(e1);
        kg.add_entity(e2);

        let pairs = find_duplicates(&kg, 0.9);
        assert!(pairs.is_empty(), "Different entity types should not be matched");
    }

    #[test]
    fn dedup_merges_and_redirects_edges() {
        let mut kg = KnowledgeGraph::new();
        let e1 = Entity {
            id: EntityId::new(&DomainTag::Code, &EntityType::Function, "auth_service", "a.py"),
            entity_type: EntityType::Function,
            label: "AuthService".to_string(),
            source_file: Some("a.py".into()),
            source_location: None,
            file_type: FileType::Code,
            metadata: serde_json::json!({}),
            legacy_id: None,
            iri: None,
        };
        let e2 = Entity {
            id: EntityId::new(&DomainTag::Code, &EntityType::Function, "authservice", "b.py"),
            entity_type: EntityType::Function,
            label: "authservice".to_string(),
            source_file: Some("b.py".into()),
            source_location: None,
            file_type: FileType::Code,
            metadata: serde_json::json!({}),
            legacy_id: None,
            iri: None,
        };
        let e3 = Entity {
            id: EntityId::new(&DomainTag::Code, &EntityType::Function, "caller", "c.py"),
            entity_type: EntityType::Function,
            label: "Caller".to_string(),
            source_file: Some("c.py".into()),
            source_location: None,
            file_type: FileType::Code,
            metadata: serde_json::json!({}),
            legacy_id: None,
            iri: None,
        };

        kg.add_entity(e1.clone());
        kg.add_entity(e2.clone());
        kg.add_entity(e3.clone());

        // e3 calls e1, and e2 calls e3 => e1 has 1 edge, e2 has 1 edge
        kg.add_relationship(Relationship {
            source: e3.id.clone(),
            target: e1.id.clone(),
            relation_type: RelationType::Calls,
            confidence: Confidence::Extracted,
            weight: 1.0,
            source_file: None,
            source_location: None,
            metadata: serde_json::json!({}),
        });
        kg.add_relationship(Relationship {
            source: e2.id.clone(),
            target: e3.id.clone(),
            relation_type: RelationType::Calls,
            confidence: Confidence::Extracted,
            weight: 1.0,
            source_file: None,
            source_location: None,
            metadata: serde_json::json!({}),
        });

        assert_eq!(kg.entity_count(), 3);

        let merged = kg.dedup(0.85);
        assert_eq!(merged, 1);
        assert_eq!(kg.entity_count(), 2);
    }

    #[test]
    fn normalized_edit_distance_identical() {
        assert!((super::normalized_edit_distance("hello", "hello") - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn normalized_edit_distance_completely_different() {
        let d = super::normalized_edit_distance("abc", "xyz");
        assert!((d - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn normalized_edit_distance_case_insensitive() {
        let d = super::normalized_edit_distance("Hello", "hello");
        assert!(d < f64::EPSILON, "Case difference should be ignored");
    }
}
