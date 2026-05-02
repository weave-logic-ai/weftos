//! JSON export matching the graphify schema validated by `build_from_json`.
//!
//! The output schema is:
//! ```json
//! {
//!   "directed": false,
//!   "multigraph": false,
//!   "graph": {},
//!   "nodes": [...],
//!   "edges": [...],
//!   "hyperedges": [...]
//! }
//! ```
//!
//! Historically this writer emitted NetworkX-style `"links"` without a
//! `source_file` field, which round-tripped through `build_from_json` only via
//! a reader-side remap and still failed schema validation. The writer now
//! emits `"edges"` directly with `source_file` populated, matching the
//! reader's expectations in `validation.rs` and `build.rs`.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde_json::json;

use crate::entity::EntityId;
use crate::model::KnowledgeGraph;
use crate::GraphifyError;

/// Export a `KnowledgeGraph` to JSON in Python-compatible `node_link_data` format.
pub fn to_json(kg: &KnowledgeGraph, output: &Path) -> Result<(), GraphifyError> {
    let data = to_json_value(kg);
    let json_str = serde_json::to_string_pretty(&data)
        .map_err(|e| GraphifyError::ExportError(format!("JSON serialization failed: {e}")))?;
    fs::write(output, json_str)
        .map_err(|e| GraphifyError::ExportError(format!("Failed to write {}: {e}", output.display())))?;
    Ok(())
}

/// Build the JSON value without writing to disk (useful for testing and
/// in-memory pipelines).
pub fn to_json_value(kg: &KnowledgeGraph) -> serde_json::Value {
    // Build community lookup: EntityId -> community index.
    let community_map: HashMap<&EntityId, usize> = kg
        .communities
        .as_ref()
        .map(|comms| {
            comms
                .iter()
                .flat_map(|(&cid, ids)| ids.iter().map(move |id| (id, cid)))
                .collect()
        })
        .unwrap_or_default();

    // Serialize nodes.
    let nodes: Vec<serde_json::Value> = kg
        .entities()
        .map(|entity| {
            let id_str = entity
                .legacy_id
                .as_deref()
                .unwrap_or("")
                .to_string();
            let id_display = if id_str.is_empty() {
                entity.id.to_hex()
            } else {
                id_str
            };

            let mut node = json!({
                "id": id_display,
                "label": entity.label,
                "file_type": entity.file_type,
                "source_file": entity.source_file,
                "source_location": entity.source_location,
            });

            // Add community if available.
            if let Some(&cid) = community_map.get(&entity.id) {
                node.as_object_mut().unwrap().insert("community".into(), json!(cid));
            } else {
                node.as_object_mut().unwrap().insert("community".into(), serde_json::Value::Null);
            }

            node
        })
        .collect();

    // Serialize edges.
    let edges: Vec<serde_json::Value> = kg
        .edges()
        .map(|(src, tgt, rel)| {
            let src_str = src
                .legacy_id
                .as_deref()
                .unwrap_or("")
                .to_string();
            let tgt_str = tgt
                .legacy_id
                .as_deref()
                .unwrap_or("")
                .to_string();
            let src_display = if src_str.is_empty() { src.id.to_hex() } else { src_str.clone() };
            let tgt_display = if tgt_str.is_empty() { tgt.id.to_hex() } else { tgt_str.clone() };

            // Use _src/_tgt from metadata if present (for roundtrip fidelity).
            let meta_src = rel
                .metadata
                .get("_src")
                .and_then(|v| v.as_str())
                .unwrap_or(&src_display);
            let meta_tgt = rel
                .metadata
                .get("_tgt")
                .and_then(|v| v.as_str())
                .unwrap_or(&tgt_display);

            // `source_file` is required by the reader's schema validator. Prefer
            // the relationship's own provenance, then fall back to the source
            // node's source_file, then the source id string.
            let source_file = rel
                .source_file
                .clone()
                .or_else(|| src.source_file.clone())
                .unwrap_or_else(|| src_display.clone());

            let mut edge = json!({
                "source": src_display,
                "target": tgt_display,
                "relation": rel.relation_type,
                "confidence": rel.confidence,
                "confidence_score": rel.confidence.to_score(),
                "weight": rel.weight,
                "source_file": source_file,
                "_src": meta_src,
                "_tgt": meta_tgt,
            });
            if let Some(loc) = rel.source_location.as_ref() {
                edge.as_object_mut()
                    .unwrap()
                    .insert("source_location".into(), json!(loc));
            }
            edge
        })
        .collect();

    // Serialize hyperedges.
    let hyperedges: Vec<serde_json::Value> = kg
        .hyperedges
        .iter()
        .map(|he| {
            json!({
                "label": he.label,
                "nodes": he.entity_ids.iter().map(|id| id.to_hex()).collect::<Vec<_>>(),
                "metadata": he.metadata,
            })
        })
        .collect();

    json!({
        "directed": false,
        "multigraph": false,
        "graph": {},
        "nodes": nodes,
        "edges": edges,
        "hyperedges": hyperedges,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::{DomainTag, EntityId, EntityType, FileType};
    use crate::model::Entity;
    use crate::relationship::{Confidence, RelationType, Relationship};
    use tempfile::TempDir;

    fn sample_kg() -> KnowledgeGraph {
        let mut kg = KnowledgeGraph::new();
        let e1 = Entity {
            id: EntityId::new(&DomainTag::Code, &EntityType::Module, "auth", "auth.py"),
            entity_type: EntityType::Module,
            label: "auth".into(),
            source_file: Some("auth.py".into()),
            source_location: Some("L1".into()),
            file_type: FileType::Code,
            metadata: json!({}),
            legacy_id: Some("auth".into()),
            iri: None,
        };
        let e2 = Entity {
            id: EntityId::new(&DomainTag::Code, &EntityType::Module, "db", "db.py"),
            entity_type: EntityType::Module,
            label: "db".into(),
            source_file: Some("db.py".into()),
            source_location: Some("L1".into()),
            file_type: FileType::Code,
            metadata: json!({}),
            legacy_id: Some("db".into()),
            iri: None,
        };
        kg.add_entity(e1.clone());
        kg.add_entity(e2.clone());
        kg.add_relationship(Relationship {
            source: e1.id.clone(),
            target: e2.id.clone(),
            relation_type: RelationType::Imports,
            confidence: Confidence::Extracted,
            weight: 1.0,
            source_file: Some("auth.py".into()),
            source_location: Some("L3".into()),
            metadata: json!({"_src": "auth", "_tgt": "db"}),
        });
        kg
    }

    #[test]
    fn json_structure() {
        let kg = sample_kg();
        let val = to_json_value(&kg);
        assert_eq!(val["directed"], false);
        assert_eq!(val["multigraph"], false);
        assert!(val["nodes"].as_array().unwrap().len() == 2);
        assert!(val["edges"].as_array().unwrap().len() == 1);
    }

    #[test]
    fn confidence_score_defaults() {
        let kg = sample_kg();
        let val = to_json_value(&kg);
        let edge = &val["edges"][0];
        assert_eq!(edge["confidence"], "EXTRACTED");
        assert_eq!(edge["confidence_score"], 1.0);
    }

    #[test]
    fn edge_has_source_file() {
        let kg = sample_kg();
        let val = to_json_value(&kg);
        let edge = &val["edges"][0];
        // source_file must be populated so the reader's schema validator passes.
        assert_eq!(edge["source_file"], "auth.py");
    }

    #[test]
    fn round_trip_through_build_from_json() {
        use crate::build::build_from_json;
        use crate::validation::validate_extraction;

        let kg = sample_kg();
        let val = to_json_value(&kg);

        // Schema-validates cleanly (the reported bug was that validation failed
        // with "Edge 0 missing required field 'source_file'").
        let errs: Vec<_> = validate_extraction(&val)
            .into_iter()
            .filter(|e| !e.contains("does not match any node id"))
            .collect();
        assert!(errs.is_empty(), "validation errors: {errs:?}");

        // Round-trips through the reader.
        let rebuilt = build_from_json(&val).expect("build_from_json should succeed");
        assert_eq!(rebuilt.entity_count(), 2);
        assert_eq!(rebuilt.relationship_count(), 1);
    }

    #[test]
    fn community_on_nodes() {
        let mut kg = sample_kg();
        let ids: Vec<EntityId> = kg.node_ids().cloned().collect();
        let mut comms = HashMap::new();
        comms.insert(0, ids);
        kg.communities = Some(comms);

        let val = to_json_value(&kg);
        for node in val["nodes"].as_array().unwrap() {
            assert_eq!(node["community"], 0);
        }
    }

    #[test]
    fn write_to_file() {
        let tmp = TempDir::new().unwrap();
        let output = tmp.path().join("graph.json");
        let kg = sample_kg();
        to_json(&kg, &output).unwrap();
        let content = fs::read_to_string(&output).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["directed"], false);
    }
}
