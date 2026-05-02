//! Schema validation for extraction JSON, matching Python's `validate.py`.

use crate::entity::FileType;
use crate::relationship::Confidence;

/// Valid file type strings (Python compatibility -- does NOT include "config" or "unknown"
/// which are Rust-only extensions; the Python validator only knows these 4).
const VALID_FILE_TYPES_PYTHON: &[&str] = &["code", "document", "paper", "image"];

/// Required fields on each node object.
const REQUIRED_NODE_FIELDS: &[&str] = &["id", "label", "file_type", "source_file"];

/// Required fields on each edge object.
const REQUIRED_EDGE_FIELDS: &[&str] = &["source", "target", "relation", "confidence", "source_file"];

/// Validate an extraction JSON value against the graphify schema.
///
/// Returns a list of error/warning strings. An empty list means the data is
/// valid. Dangling edge references (source/target not matching any node ID)
/// are included as warnings but are expected for external/stdlib imports.
pub fn validate_extraction(data: &serde_json::Value) -> Vec<String> {
    let mut errors = Vec::new();

    if !data.is_object() {
        errors.push("Extraction must be a JSON object".into());
        return errors;
    }

    // -- Nodes --
    match data.get("nodes") {
        None => errors.push("Missing required key 'nodes'".into()),
        Some(nodes) => {
            if let Some(arr) = nodes.as_array() {
                for (i, node) in arr.iter().enumerate() {
                    if !node.is_object() {
                        errors.push(format!("Node {i} must be an object"));
                        continue;
                    }
                    let node_id_display = node
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?");
                    for &field in REQUIRED_NODE_FIELDS {
                        if node.get(field).is_none() {
                            errors.push(format!(
                                "Node {i} (id={node_id_display:?}) missing required field '{field}'"
                            ));
                        }
                    }
                    if let Some(ft) = node.get("file_type").and_then(|v| v.as_str()) {
                        // Accept both Python-compatible and Rust-extended file types.
                        if !VALID_FILE_TYPES_PYTHON.contains(&ft)
                            && FileType::from_str_loose(ft).is_none()
                        {
                            errors.push(format!(
                                "Node {i} (id={node_id_display:?}) has invalid file_type '{ft}'"
                            ));
                        }
                    }
                }
            } else {
                errors.push("'nodes' must be a list".into());
            }
        }
    }

    // -- Edges --
    match data.get("edges") {
        None => errors.push("Missing required key 'edges'".into()),
        Some(edges) => {
            if let Some(arr) = edges.as_array() {
                // Build set of node IDs for reference checking.
                let node_ids: std::collections::HashSet<&str> = data
                    .get("nodes")
                    .and_then(|v| v.as_array())
                    .map(|nodes| {
                        nodes
                            .iter()
                            .filter_map(|n| n.get("id").and_then(|v| v.as_str()))
                            .collect()
                    })
                    .unwrap_or_default();

                for (i, edge) in arr.iter().enumerate() {
                    if !edge.is_object() {
                        errors.push(format!("Edge {i} must be an object"));
                        continue;
                    }
                    for &field in REQUIRED_EDGE_FIELDS {
                        if edge.get(field).is_none() {
                            errors.push(format!("Edge {i} missing required field '{field}'"));
                        }
                    }
                    if let Some(conf) = edge.get("confidence").and_then(|v| v.as_str())
                        && Confidence::from_str_loose(conf).is_none() {
                            errors.push(format!(
                                "Edge {i} has invalid confidence '{conf}' - must be one of {:?}",
                                Confidence::VALID_STRINGS,
                            ));
                        }
                    // Dangling references: warn but don't block.
                    if !node_ids.is_empty() {
                        if let Some(src) = edge.get("source").and_then(|v| v.as_str())
                            && !node_ids.contains(src) {
                                errors.push(format!(
                                    "Edge {i} source '{src}' does not match any node id"
                                ));
                            }
                        if let Some(tgt) = edge.get("target").and_then(|v| v.as_str())
                            && !node_ids.contains(tgt) {
                                errors.push(format!(
                                    "Edge {i} target '{tgt}' does not match any node id"
                                ));
                            }
                    }
                }
            } else {
                errors.push("'edges' must be a list".into());
            }
        }
    }

    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_extraction_passes() {
        let data = serde_json::json!({
            "nodes": [
                {"id": "a", "label": "A", "file_type": "code", "source_file": "a.py"},
            ],
            "edges": [
                {"source": "a", "target": "a", "relation": "self_ref", "confidence": "EXTRACTED", "source_file": "a.py"},
            ],
        });
        let errs = validate_extraction(&data);
        assert!(errs.is_empty(), "unexpected errors: {errs:?}");
    }

    #[test]
    fn missing_nodes_key() {
        let data = serde_json::json!({"edges": []});
        let errs = validate_extraction(&data);
        assert!(errs.iter().any(|e| e.contains("Missing required key 'nodes'")));
    }

    #[test]
    fn invalid_file_type() {
        let data = serde_json::json!({
            "nodes": [
                {"id": "x", "label": "X", "file_type": "banana", "source_file": "x.py"},
            ],
            "edges": [],
        });
        let errs = validate_extraction(&data);
        assert!(errs.iter().any(|e| e.contains("invalid file_type")));
    }

    #[test]
    fn invalid_confidence() {
        let data = serde_json::json!({
            "nodes": [
                {"id": "a", "label": "A", "file_type": "code", "source_file": "a.py"},
            ],
            "edges": [
                {"source": "a", "target": "a", "relation": "r", "confidence": "MAYBE", "source_file": "a.py"},
            ],
        });
        let errs = validate_extraction(&data);
        assert!(errs.iter().any(|e| e.contains("invalid confidence")));
    }

    #[test]
    fn dangling_edge_reference_is_warning() {
        let data = serde_json::json!({
            "nodes": [
                {"id": "a", "label": "A", "file_type": "code", "source_file": "a.py"},
            ],
            "edges": [
                {"source": "a", "target": "external", "relation": "imports", "confidence": "EXTRACTED", "source_file": "a.py"},
            ],
        });
        let errs = validate_extraction(&data);
        assert!(errs.iter().any(|e| e.contains("does not match any node id")));
    }

    #[test]
    fn missing_edge_fields() {
        let data = serde_json::json!({
            "nodes": [],
            "edges": [{"source": "a"}],
        });
        let errs = validate_extraction(&data);
        assert!(errs.iter().any(|e| e.contains("missing required field")));
    }

    #[test]
    fn rust_extended_file_types_valid() {
        let data = serde_json::json!({
            "nodes": [
                {"id": "a", "label": "A", "file_type": "config", "source_file": "a.toml"},
                {"id": "b", "label": "B", "file_type": "unknown", "source_file": "b.bin"},
            ],
            "edges": [],
        });
        let errs = validate_extraction(&data);
        assert!(errs.is_empty(), "unexpected errors: {errs:?}");
    }
}
