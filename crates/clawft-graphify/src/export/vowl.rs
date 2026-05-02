//! VOWL JSON export — emit WebVOWL-compatible JSON from a KnowledgeGraph.
//!
//! Produces the 8-key VOWL JSON format that WebVOWL and our React navigator
//! can consume directly. Maps graphify entity types to OWL class types and
//! relationship types to OWL property types.

use std::collections::HashMap;

use crate::model::KnowledgeGraph;
use crate::topology::TopologySchema;
use crate::GraphifyError;

/// Generate VOWL-compatible JSON from a knowledge graph and schema.
pub fn to_vowl_json(
    kg: &KnowledgeGraph,
    schema: &TopologySchema,
) -> Result<serde_json::Value, GraphifyError> {
    let mut classes = Vec::new();
    let mut class_attrs = Vec::new();
    let mut properties = Vec::new();
    let mut property_attrs = Vec::new();

    // Map entity IDs to VOWL class IDs.
    let mut entity_to_cid: HashMap<String, String> = HashMap::new();

    for (i, entity) in kg.entities().enumerate() {
        let cid = format!("c{i}");
        let hex = entity.id.to_hex();
        entity_to_cid.insert(hex, cid.clone());

        let type_key = entity.entity_type.discriminant();
        let config = schema.node_config(type_key);

        // Map to OWL class type based on entity characteristics.
        let owl_type = match type_key {
            "constant" | "enum_" => "rdfs:Datatype",
            _ => "owl:Class",
        };

        classes.push(serde_json::json!({
            "id": cid,
            "type": owl_type,
        }));

        let mut label = serde_json::Map::new();
        label.insert("en".into(), serde_json::Value::String(entity.label.clone()));

        let mut attr = serde_json::json!({
            "id": cid,
            "label": label,
        });

        if let Some(iri) = &entity.iri {
            attr["iri"] = serde_json::Value::String(iri.clone());
        } else if let Some(c) = config
            && let Some(iri) = &c.iri {
                attr["baseIri"] = serde_json::Value::String(iri.clone());
            }

        if let Some(c) = config
            && let Some(name) = &c.display_name {
                attr["comment"] = serde_json::json!({"en": name});
            }

        if let Some(desc) = entity.metadata.get("description").and_then(|v| v.as_str()) {
            attr["description"] = serde_json::Value::String(desc.to_string());
        }

        class_attrs.push(attr);
    }

    // Map relationships to VOWL properties.
    for (i, (src, tgt, rel)) in kg.edges().enumerate() {
        let pid = format!("p{i}");
        let src_hex = src.id.to_hex();
        let tgt_hex = tgt.id.to_hex();

        let src_cid = match entity_to_cid.get(&src_hex) {
            Some(c) => c.clone(),
            None => continue,
        };
        let tgt_cid = match entity_to_cid.get(&tgt_hex) {
            Some(c) => c.clone(),
            None => continue,
        };

        let rel_type_str = format!("{:?}", rel.relation_type);
        let owl_prop_type = match &rel.relation_type {
            crate::relationship::RelationType::Contains => "rdfs:subClassOf",
            crate::relationship::RelationType::Extends => "rdfs:subClassOf",
            _ => "owl:ObjectProperty",
        };

        properties.push(serde_json::json!({
            "id": pid,
            "type": owl_prop_type,
        }));

        let mut label = serde_json::Map::new();
        label.insert("en".into(), serde_json::Value::String(snake_case(&rel_type_str)));

        let mut prop_attr = serde_json::json!({
            "id": pid,
            "domain": src_cid,
            "range": tgt_cid,
            "label": label,
        });

        // Add IRI from schema edge config if available.
        let edge_config = schema.edges.iter().find(|e| {
            e.edge_type == rel_type_str.to_lowercase()
                || e.edge_type == snake_case(&rel_type_str)
        });
        if let Some(ec) = edge_config
            && let Some(iri) = &ec.iri {
                prop_attr["iri"] = serde_json::Value::String(iri.clone());
            }

        property_attrs.push(prop_attr);
    }

    // Metrics.
    let obj_prop_count = properties.iter()
        .filter(|p| p["type"] == "owl:ObjectProperty")
        .count();
    let dt_prop_count = properties.iter()
        .filter(|p| p["type"] == "owl:DatatypeProperty")
        .count();

    let vowl = serde_json::json!({
        "header": {
            "title": schema.label,
            "iri": schema.iri,
            "version": schema.version,
            "languages": ["en"],
        },
        "namespace": [],
        "metrics": {
            "classCount": classes.len(),
            "objectPropertyCount": obj_prop_count,
            "datatypePropertyCount": dt_prop_count,
            "individualCount": 0,
        },
        "class": classes,
        "classAttribute": class_attrs,
        "property": properties,
        "propertyAttribute": property_attrs,
    });

    Ok(vowl)
}

fn snake_case(s: &str) -> String {
    let mut result = String::with_capacity(s.len() + 4);
    for (i, ch) in s.chars().enumerate() {
        if ch.is_uppercase() && i > 0 {
            result.push('_');
        }
        result.push(ch.to_ascii_lowercase());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::{DomainTag, EntityId, EntityType, FileType};
    use crate::model::Entity;
    use crate::relationship::{Confidence, RelationType, Relationship};

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
    fn vowl_json_has_required_keys() {
        let mut kg = KnowledgeGraph::new();
        let m = entity("auth", EntityType::Module);
        let f = entity("login", EntityType::Function);
        kg.add_entity(m.clone());
        kg.add_entity(f.clone());
        kg.add_relationship(Relationship {
            source: m.id.clone(),
            target: f.id.clone(),
            relation_type: RelationType::Contains,
            confidence: Confidence::Extracted,
            weight: 1.0,
            source_file: None,
            source_location: None,
            metadata: serde_json::json!({}),
        });

        let schema = TopologySchema::from_yaml(r##"
name: test
label: "Test"
version: "1.0.0"
nodes:
  module:
    geometry: tree
  function:
    geometry: force
  "*":
    geometry: force
edges:
  - type: contains
    from: module
    to: function
"##).unwrap();

        let vowl = to_vowl_json(&kg, &schema).unwrap();

        assert!(vowl["header"].is_object());
        assert!(vowl["class"].is_array());
        assert!(vowl["classAttribute"].is_array());
        assert!(vowl["property"].is_array());
        assert!(vowl["propertyAttribute"].is_array());
        assert_eq!(vowl["class"].as_array().unwrap().len(), 2);
        assert_eq!(vowl["property"].as_array().unwrap().len(), 1);
        assert_eq!(vowl["metrics"]["classCount"], 2);
    }
}
