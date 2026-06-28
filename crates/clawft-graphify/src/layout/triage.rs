//! Tree calculus triage — classify graph nodes as Atom, Sequence, or Branch
//! based on their containment structure.

use crate::entity::EntityId;
use crate::model::KnowledgeGraph;
use crate::relationship::RelationType;

/// Tree calculus form: every node in a topology is one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopologyForm {
    /// Leaf — no children, terminal node.
    Atom,
    /// Stem — ordered single-type children (timeline, stream).
    Sequence,
    /// Fork — typed branching children (tree, hierarchy).
    Branch,
}

/// Classify a node's topology form based on its Contains edges.
pub fn classify(kg: &KnowledgeGraph, node_id: &EntityId) -> TopologyForm {
    let children: Vec<_> = kg
        .edges()
        .filter(|(src, _, rel)| {
            src.id == *node_id && matches!(rel.relation_type, RelationType::Contains)
        })
        .collect();

    if children.is_empty() {
        return TopologyForm::Atom;
    }

    // Check if all children are the same type (sequence) or mixed (branch).
    let first_type = &children[0].1.entity_type;
    let all_same = children
        .iter()
        .all(|(_, tgt, _)| &tgt.entity_type == first_type);

    if all_same {
        TopologyForm::Sequence
    } else {
        TopologyForm::Branch
    }
}

/// Get the children of a node via Contains edges.
pub fn children_of(kg: &KnowledgeGraph, node_id: &EntityId) -> Vec<EntityId> {
    kg.edges()
        .filter(|(src, _, rel)| {
            src.id == *node_id && matches!(rel.relation_type, RelationType::Contains)
        })
        .map(|(_, tgt, _)| tgt.id.clone())
        .collect()
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
            source_file: None,
            source_location: None,
            metadata: serde_json::json!({}),
        }
    }

    #[test]
    fn atom_has_no_children() {
        let mut kg = KnowledgeGraph::new();
        let f = entity("main", EntityType::Function);
        kg.add_entity(f.clone());
        assert_eq!(classify(&kg, &f.id), TopologyForm::Atom);
    }

    #[test]
    fn sequence_has_same_type_children() {
        let mut kg = KnowledgeGraph::new();
        let m = entity("app", EntityType::Module);
        let f1 = entity("foo", EntityType::Function);
        let f2 = entity("bar", EntityType::Function);
        kg.add_entity(m.clone());
        kg.add_entity(f1.clone());
        kg.add_entity(f2.clone());
        kg.add_relationship(contains(&m, &f1));
        kg.add_relationship(contains(&m, &f2));
        assert_eq!(classify(&kg, &m.id), TopologyForm::Sequence);
    }

    #[test]
    fn branch_has_mixed_type_children() {
        let mut kg = KnowledgeGraph::new();
        let m = entity("app", EntityType::Module);
        let c = entity("App", EntityType::Class);
        let f = entity("main", EntityType::Function);
        kg.add_entity(m.clone());
        kg.add_entity(c.clone());
        kg.add_entity(f.clone());
        kg.add_relationship(contains(&m, &c));
        kg.add_relationship(contains(&m, &f));
        assert_eq!(classify(&kg, &m.id), TopologyForm::Branch);
    }
}
