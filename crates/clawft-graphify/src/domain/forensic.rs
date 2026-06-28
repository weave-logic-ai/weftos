//! Forensic analysis domain configuration.
//!
//! Defines entity types for investigative workflows (Person, Event, Evidence,
//! Location, etc.) and the edge types that connect them (witnessed_by,
//! found_at, contradicts, corroborates, etc.).
//!
//! Compiled only when the `forensic-domain` feature is enabled.

use crate::domain::Domain;
use crate::eml_models::ForensicCoherenceModel;
use crate::entity::{EntityId, EntityType};
use crate::model::KnowledgeGraph;
use crate::relationship::{Confidence, RelationType, Relationship};

// ---------------------------------------------------------------------------
// ForensicDomainConfig
// ---------------------------------------------------------------------------

/// Domain configuration for forensic / investigative analysis.
///
/// Maps the forensic entity types (Person, Event, Evidence, Location,
/// Timeline, Document, Hypothesis, Organization, PhysicalObject,
/// DigitalArtifact, FinancialRecord, Communication) and the 11 forensic
/// relationship types plus shared types.
pub struct ForensicDomainConfig {
    entity_types: Vec<EntityType>,
    edge_types: Vec<RelationType>,
}

impl ForensicDomainConfig {
    /// Create the default forensic domain configuration.
    pub fn new() -> Self {
        Self {
            entity_types: vec![
                EntityType::Person,
                EntityType::Event,
                EntityType::Evidence,
                EntityType::Location,
                EntityType::Timeline,
                EntityType::Document,
                EntityType::Hypothesis,
                EntityType::Organization,
                EntityType::PhysicalObject,
                EntityType::DigitalArtifact,
                EntityType::FinancialRecord,
                EntityType::Communication,
                EntityType::File,
                EntityType::Concept,
            ],
            edge_types: vec![
                RelationType::WitnessedBy,
                RelationType::FoundAt,
                RelationType::Contradicts,
                RelationType::Corroborates,
                RelationType::AlibiedBy,
                RelationType::Precedes,
                RelationType::DocumentedIn,
                RelationType::OwnedBy,
                RelationType::ContactedBy,
                RelationType::LocatedAt,
                RelationType::SemanticallySimilarTo,
                RelationType::RelatedTo,
                RelationType::CaseOf,
            ],
        }
    }
}

impl Default for ForensicDomainConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl Domain for ForensicDomainConfig {
    fn domain_tag(&self) -> &str {
        "forensic"
    }

    fn display_name(&self) -> &str {
        "Forensic Analysis"
    }

    fn entity_types(&self) -> &[EntityType] {
        &self.entity_types
    }

    fn edge_types(&self) -> &[RelationType] {
        &self.edge_types
    }
}

// ---------------------------------------------------------------------------
// Gap
// ---------------------------------------------------------------------------

/// A structural gap identified in a forensic knowledge graph.
#[derive(Debug, Clone)]
pub enum Gap {
    /// Evidence node with degree 0 or 1 (unlinked or weakly linked).
    UnlinkedEvidence {
        entity_id: EntityId,
        label: String,
        degree: usize,
    },
    /// Event nodes without temporal edges (Precedes relationships).
    TimelineDiscontinuity { event_id: EntityId, label: String },
    /// Edges with `Confidence::Ambiguous` that have not been verified.
    UnverifiedClaim {
        source_id: EntityId,
        target_id: EntityId,
        relation_type: RelationType,
    },
    /// Person entities mentioned but not linked to any Event.
    MissingConnection { person_id: EntityId, label: String },
}

// ---------------------------------------------------------------------------
// Gap analysis
// ---------------------------------------------------------------------------

/// Identify structural gaps in a forensic knowledge graph.
///
/// Detects:
/// - Unlinked evidence (evidence nodes with degree 0-1)
/// - Timeline discontinuities (events without temporal edges)
/// - Unverified claims (edges with confidence = AMBIGUOUS)
/// - Missing connections (persons not linked to events)
pub fn gap_analysis(kg: &KnowledgeGraph) -> Vec<Gap> {
    let mut gaps = Vec::new();

    // 1. Unlinked evidence: evidence nodes with degree 0-1.
    for entity in kg.entities() {
        if entity.entity_type == EntityType::Evidence {
            let deg = kg.degree(&entity.id);
            if deg <= 1 {
                gaps.push(Gap::UnlinkedEvidence {
                    entity_id: entity.id.clone(),
                    label: entity.label.clone(),
                    degree: deg,
                });
            }
        }
    }

    // 2. Timeline discontinuities: Event nodes without Precedes edges.
    for entity in kg.entities() {
        if entity.entity_type == EntityType::Event {
            let has_temporal = kg.edges().any(|(src, _tgt, rel)| {
                (src.id == entity.id || rel.target == entity.id)
                    && rel.relation_type == RelationType::Precedes
            });
            if !has_temporal {
                gaps.push(Gap::TimelineDiscontinuity {
                    event_id: entity.id.clone(),
                    label: entity.label.clone(),
                });
            }
        }
    }

    // 3. Unverified claims: edges with Ambiguous confidence.
    for (_src, _tgt, rel) in kg.edges() {
        if rel.confidence == Confidence::Ambiguous {
            gaps.push(Gap::UnverifiedClaim {
                source_id: rel.source.clone(),
                target_id: rel.target.clone(),
                relation_type: rel.relation_type.clone(),
            });
        }
    }

    // 4. Missing connections: Person nodes not linked to any Event.
    let event_ids: std::collections::HashSet<&EntityId> = kg
        .entities()
        .filter(|e| e.entity_type == EntityType::Event)
        .map(|e| &e.id)
        .collect();

    for entity in kg.entities() {
        if entity.entity_type == EntityType::Person {
            let linked_to_event = kg.edges().any(|(src, tgt, _rel)| {
                (src.id == entity.id && event_ids.contains(&tgt.id))
                    || (tgt.id == entity.id && event_ids.contains(&src.id))
            });
            if !linked_to_event {
                gaps.push(Gap::MissingConnection {
                    person_id: entity.id.clone(),
                    label: entity.label.clone(),
                });
            }
        }
    }

    gaps
}

// ---------------------------------------------------------------------------
// Coherence score
// ---------------------------------------------------------------------------

/// Compute a coherence score for the knowledge graph.
///
/// Uses a simplified spectral-like measure: the ratio of actual edges to
/// the maximum possible edges in the graph (density), weighted by the
/// average confidence of all edges. A fully connected graph with all
/// EXTRACTED-confidence edges scores 1.0.
///
/// Returns a value in [0.0, 1.0].
pub fn coherence_score(kg: &KnowledgeGraph) -> f64 {
    let n = kg.entity_count();
    if n < 2 {
        return if n == 1 { 1.0 } else { 0.0 };
    }

    let max_edges = n * (n - 1); // directed graph
    let actual_edges = kg.relationship_count();
    if actual_edges == 0 {
        return 0.0;
    }

    let density = actual_edges as f64 / max_edges as f64;

    // Average confidence-weighted score across all edges.
    let total_weight: f64 = kg
        .edges()
        .map(|(_, _, rel)| rel.confidence.to_score())
        .sum();
    let avg_confidence = total_weight / actual_edges as f64;

    // Coherence = density * avg_confidence (both in [0,1]).
    density * avg_confidence
}

/// Compute coherence with an optional EML model.
///
/// When `eml_model` is `Some` and trained, uses the learned function.
/// Otherwise falls back to the original `density * avg_confidence` formula.
pub fn coherence_score_eml(kg: &KnowledgeGraph, eml_model: Option<&ForensicCoherenceModel>) -> f64 {
    let n = kg.entity_count();
    if n < 2 {
        return if n == 1 { 1.0 } else { 0.0 };
    }

    let max_edges = n * (n - 1);
    let actual_edges = kg.relationship_count();
    if actual_edges == 0 {
        return 0.0;
    }

    let density = actual_edges as f64 / max_edges as f64;

    let total_weight: f64 = kg
        .edges()
        .map(|(_, _, rel)| rel.confidence.to_score())
        .sum();
    let avg_confidence = total_weight / actual_edges as f64;

    match eml_model {
        Some(model) if model.is_trained() => {
            model.predict(density, avg_confidence, n as f64, actual_edges as f64)
        }
        _ => density * avg_confidence,
    }
}

// ---------------------------------------------------------------------------
// Counterfactual delta
// ---------------------------------------------------------------------------

/// Predict the coherence improvement if a hypothetical relationship were added.
///
/// Returns the delta: `coherence_after - coherence_before`. A positive value
/// means the hypothetical edge would improve graph coherence.
///
/// This is a lightweight approximation: it computes the analytical delta
/// without mutating the original graph.
pub fn counterfactual_delta(kg: &KnowledgeGraph, hypothetical: &Relationship) -> f64 {
    let current = coherence_score(kg);

    let n = kg.entity_count();
    if n < 2 {
        return 0.0;
    }

    let max_edges = (n * (n - 1)) as f64;
    let m = kg.relationship_count() as f64;

    // Current total confidence.
    let current_total: f64 = kg
        .edges()
        .map(|(_, _, rel)| rel.confidence.to_score())
        .sum();

    // After adding one edge.
    let new_m = m + 1.0;
    let new_total = current_total + hypothetical.confidence.to_score();
    let new_density = new_m / max_edges;
    let new_avg = new_total / new_m;
    let predicted = new_density * new_avg;

    predicted - current
}

/// Predict coherence improvement with an optional EML model.
///
/// Same as [`counterfactual_delta`] but uses the EML model for both
/// current and predicted coherence when trained.
pub fn counterfactual_delta_eml(
    kg: &KnowledgeGraph,
    hypothetical: &Relationship,
    eml_model: Option<&ForensicCoherenceModel>,
) -> f64 {
    let current = coherence_score_eml(kg, eml_model);

    let n = kg.entity_count();
    if n < 2 {
        return 0.0;
    }

    let max_edges = (n * (n - 1)) as f64;
    let m = kg.relationship_count() as f64;

    let current_total: f64 = kg
        .edges()
        .map(|(_, _, rel)| rel.confidence.to_score())
        .sum();

    let new_m = m + 1.0;
    let new_total = current_total + hypothetical.confidence.to_score();
    let new_density = new_m / max_edges;
    let new_avg = new_total / new_m;

    let predicted = match eml_model {
        Some(model) if model.is_trained() => model.predict(new_density, new_avg, n as f64, new_m),
        _ => new_density * new_avg,
    };

    predicted - current
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::{DomainTag, FileType};
    use crate::model::Entity;

    fn make_forensic_entity(name: &str, et: EntityType) -> Entity {
        Entity {
            id: EntityId::new(&DomainTag::Forensic, &et, name, "case.json"),
            entity_type: et,
            label: name.to_string(),
            source_file: Some("case.json".into()),
            source_location: None,
            file_type: FileType::Document,
            metadata: serde_json::json!({}),
            legacy_id: None,
            iri: None,
        }
    }

    fn make_forensic_rel(
        src: &Entity,
        tgt: &Entity,
        rt: RelationType,
        conf: Confidence,
    ) -> Relationship {
        Relationship {
            source: src.id.clone(),
            target: tgt.id.clone(),
            relation_type: rt,
            confidence: conf,
            weight: conf.to_weight(),
            source_file: Some("case.json".into()),
            source_location: None,
            metadata: serde_json::json!({}),
        }
    }

    #[test]
    fn forensic_domain_entity_types() {
        let d = ForensicDomainConfig::new();
        assert!(d.accepts_entity(&EntityType::Person));
        assert!(d.accepts_entity(&EntityType::Evidence));
        assert!(d.accepts_entity(&EntityType::Event));
        assert!(d.accepts_entity(&EntityType::Hypothesis));
        assert!(!d.accepts_entity(&EntityType::Function));
        assert!(!d.accepts_entity(&EntityType::Module));
    }

    #[test]
    fn forensic_domain_edge_types() {
        let d = ForensicDomainConfig::new();
        assert!(d.accepts_edge(&RelationType::WitnessedBy));
        assert!(d.accepts_edge(&RelationType::Contradicts));
        assert!(d.accepts_edge(&RelationType::Corroborates));
        assert!(d.accepts_edge(&RelationType::Precedes));
        assert!(!d.accepts_edge(&RelationType::Calls));
        assert!(!d.accepts_edge(&RelationType::Imports));
    }

    #[test]
    fn forensic_domain_tag() {
        let d = ForensicDomainConfig::new();
        assert_eq!(d.domain_tag(), "forensic");
        assert_eq!(d.display_name(), "Forensic Analysis");
    }

    #[test]
    fn gap_unlinked_evidence() {
        let mut kg = KnowledgeGraph::new();
        let evidence = make_forensic_entity("bloodstain", EntityType::Evidence);
        kg.add_entity(evidence.clone());
        // Evidence with degree 0 should be flagged.
        let gaps = gap_analysis(&kg);
        assert!(
            gaps.iter()
                .any(|g| matches!(g, Gap::UnlinkedEvidence { degree: 0, .. }))
        );
    }

    #[test]
    fn gap_timeline_discontinuity() {
        let mut kg = KnowledgeGraph::new();
        let event = make_forensic_entity("break-in", EntityType::Event);
        kg.add_entity(event.clone());
        // Event without Precedes edges.
        let gaps = gap_analysis(&kg);
        assert!(
            gaps.iter()
                .any(|g| matches!(g, Gap::TimelineDiscontinuity { .. }))
        );
    }

    #[test]
    fn gap_unverified_claim() {
        let mut kg = KnowledgeGraph::new();
        let person = make_forensic_entity("suspect", EntityType::Person);
        let event = make_forensic_entity("crime", EntityType::Event);
        kg.add_entity(person.clone());
        kg.add_entity(event.clone());
        kg.add_relationship(make_forensic_rel(
            &person,
            &event,
            RelationType::WitnessedBy,
            Confidence::Ambiguous,
        ));
        let gaps = gap_analysis(&kg);
        assert!(
            gaps.iter()
                .any(|g| matches!(g, Gap::UnverifiedClaim { .. }))
        );
    }

    #[test]
    fn gap_missing_connection() {
        let mut kg = KnowledgeGraph::new();
        let person = make_forensic_entity("John", EntityType::Person);
        let event = make_forensic_entity("incident", EntityType::Event);
        let other = make_forensic_entity("building", EntityType::Location);
        kg.add_entity(person.clone());
        kg.add_entity(event.clone());
        kg.add_entity(other.clone());
        // Person not linked to any event.
        kg.add_relationship(make_forensic_rel(
            &person,
            &other,
            RelationType::LocatedAt,
            Confidence::Extracted,
        ));
        let gaps = gap_analysis(&kg);
        assert!(
            gaps.iter()
                .any(|g| matches!(g, Gap::MissingConnection { .. }))
        );
    }

    #[test]
    fn coherence_empty_graph() {
        let kg = KnowledgeGraph::new();
        assert!((coherence_score(&kg) - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn coherence_single_node() {
        let mut kg = KnowledgeGraph::new();
        kg.add_entity(make_forensic_entity("alone", EntityType::Person));
        assert!((coherence_score(&kg) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn coherence_positive_for_connected_graph() {
        let mut kg = KnowledgeGraph::new();
        let a = make_forensic_entity("suspect", EntityType::Person);
        let b = make_forensic_entity("scene", EntityType::Event);
        kg.add_entity(a.clone());
        kg.add_entity(b.clone());
        kg.add_relationship(make_forensic_rel(
            &a,
            &b,
            RelationType::WitnessedBy,
            Confidence::Extracted,
        ));
        let score = coherence_score(&kg);
        assert!(score > 0.0);
        assert!(score <= 1.0);
    }

    #[test]
    fn counterfactual_delta_positive() {
        let mut kg = KnowledgeGraph::new();
        let a = make_forensic_entity("suspect", EntityType::Person);
        let b = make_forensic_entity("scene", EntityType::Event);
        let c = make_forensic_entity("weapon", EntityType::Evidence);
        kg.add_entity(a.clone());
        kg.add_entity(b.clone());
        kg.add_entity(c.clone());
        kg.add_relationship(make_forensic_rel(
            &a,
            &b,
            RelationType::WitnessedBy,
            Confidence::Extracted,
        ));

        let hypothetical = make_forensic_rel(&c, &b, RelationType::FoundAt, Confidence::Extracted);
        let delta = counterfactual_delta(&kg, &hypothetical);
        assert!(delta > 0.0, "Adding an edge should improve coherence");
    }
}
