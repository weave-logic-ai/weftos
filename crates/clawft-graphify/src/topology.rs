//! TopologySchema — geometry declarations for the universal topology browser.
//!
//! A `.topology.yaml` file describes how a knowledge graph should be laid out
//! and navigated. It does not define the data model — it describes the spatial
//! behavior of nodes and edges that already exist in a `KnowledgeGraph`.
//!
//! Schemas are composable: base + project + local layers merge in order
//! (Docker-config style). IRIs provide globally unique concept identity.

use std::collections::HashMap;
use serde::{Deserialize, Serialize};

/// Top-level topology schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopologySchema {
    pub name: String,
    pub label: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extends: Option<String>,
    pub nodes: HashMap<String, NodeTypeConfig>,
    pub edges: Vec<EdgeTypeConfig>,
    #[serde(default)]
    pub modes: ModesConfig,
    #[serde(default)]
    pub constraints: ConstraintsConfig,
}

/// How instances of a node type are spatially arranged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum Geometry {
    #[default]
    Force,
    Tree,
    Layered,
    Timeline,
    Stream,
    Grid,
    Geo,
    Radial,
    Wardley,
}


/// Configuration for a node type in the topology.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeTypeConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iri: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub same_as: Vec<String>,
    #[serde(default)]
    pub geometry: Geometry,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contains: Vec<String>,
    #[serde(default)]
    pub style: NodeStyle,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_field: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_field: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lat_field: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lng_field: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

/// Visual defaults for a node type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeStyle {
    #[serde(default = "default_shape")]
    pub shape: NodeShape,
    #[serde(default = "default_color")]
    pub color: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default = "default_min_radius")]
    pub min_radius: u32,
    #[serde(default = "default_max_radius")]
    pub max_radius: u32,
}

impl Default for NodeStyle {
    fn default() -> Self {
        Self {
            shape: NodeShape::Circle,
            color: "#a3a3a3".into(),
            icon: None,
            min_radius: 8,
            max_radius: 48,
        }
    }
}

fn default_shape() -> NodeShape { NodeShape::Circle }
fn default_color() -> String { "#a3a3a3".into() }
fn default_min_radius() -> u32 { 8 }
fn default_max_radius() -> u32 { 48 }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeShape {
    Circle,
    Rect,
    Diamond,
    Hexagon,
    Pill,
}

/// Configuration for an edge type in the topology.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeTypeConfig {
    #[serde(rename = "type")]
    pub edge_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iri: Option<String>,
    pub from: String,
    pub to: String,
    #[serde(default = "default_cardinality")]
    pub cardinality: String,
    #[serde(default)]
    pub style: EdgeStyle,
    #[serde(default)]
    pub animated: bool,
}

fn default_cardinality() -> String { "N:M".into() }

/// Visual defaults for an edge type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeStyle {
    #[serde(default = "default_stroke")]
    pub stroke: String,
    #[serde(default = "default_edge_width")]
    pub width: f64,
    #[serde(default)]
    pub dash: DashStyle,
    #[serde(default = "default_true")]
    pub arrow: bool,
    #[serde(default)]
    pub label: bool,
}

impl Default for EdgeStyle {
    fn default() -> Self {
        Self {
            stroke: "#888888".into(),
            width: 1.0,
            dash: DashStyle::Solid,
            arrow: true,
            label: false,
        }
    }
}

fn default_stroke() -> String { "#888888".into() }
fn default_edge_width() -> f64 { 1.0 }
fn default_true() -> bool { true }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DashStyle {
    #[default]
    Solid,
    Dashed,
    Dotted,
}

// ---------------------------------------------------------------------------
// Mode configuration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModesConfig {
    #[serde(default)]
    pub structure: StructureMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<DiffMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heatmap: Option<HeatmapMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flow: Option<FlowMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeline: Option<TimelineMode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructureMode {
    #[serde(default)]
    pub root_geometry: Geometry,
}

impl Default for StructureMode {
    fn default() -> Self {
        Self { root_geometry: Geometry::Force }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffMode {
    pub sources: DiffSources,
    #[serde(default)]
    pub colors: DiffColors,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffSources {
    pub before: String,
    pub after: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffColors {
    #[serde(default = "default_added_color")]
    pub added: String,
    #[serde(default = "default_removed_color")]
    pub removed: String,
    #[serde(default = "default_modified_color")]
    pub modified: String,
}

impl Default for DiffColors {
    fn default() -> Self {
        Self {
            added: "#22c55e".into(),
            removed: "#ef4444".into(),
            modified: "#eab308".into(),
        }
    }
}

fn default_added_color() -> String { "#22c55e".into() }
fn default_removed_color() -> String { "#ef4444".into() }
fn default_modified_color() -> String { "#eab308".into() }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeatmapMode {
    pub metrics: Vec<HeatmapMetric>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeatmapMetric {
    pub field: String,
    pub label: String,
    #[serde(default = "default_palette")]
    pub palette: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<[f64; 2]>,
}

fn default_palette() -> String { "viridis".into() }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowMode {
    pub edge_types: Vec<String>,
    #[serde(default = "default_flow_speed")]
    pub speed: f64,
    #[serde(default = "default_flow_color")]
    pub color: String,
}

fn default_flow_speed() -> f64 { 120.0 }
fn default_flow_color() -> String { "#60a5fa".into() }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineMode {
    pub node_types: Vec<String>,
    pub lane_field: String,
}

// ---------------------------------------------------------------------------
// Constraints
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConstraintsConfig {
    #[serde(default = "default_max_nodes")]
    pub max_visible_nodes: usize,
    #[serde(default = "default_max_edges")]
    pub max_visible_edges: usize,
    #[serde(default)]
    pub min_confidence: f64,
    #[serde(default = "default_true")]
    pub auto_cluster: bool,
}

impl Default for ConstraintsConfig {
    fn default() -> Self {
        Self {
            max_visible_nodes: 2000,
            max_visible_edges: 5000,
            min_confidence: 0.0,
            auto_cluster: true,
        }
    }
}

fn default_max_nodes() -> usize { 2000 }
fn default_max_edges() -> usize { 5000 }

// ---------------------------------------------------------------------------
// Schema loading and merging
// ---------------------------------------------------------------------------

impl TopologySchema {
    /// Parse a schema from YAML.
    pub fn from_yaml(yaml: &str) -> Result<Self, crate::GraphifyError> {
        serde_yaml::from_str(yaml)
            .map_err(|e| crate::GraphifyError::ValidationError(format!("schema parse: {e}")))
    }

    /// Merge another schema on top of this one (Docker-config style).
    /// The `other` schema's entries override `self` where keys collide.
    pub fn merge(&mut self, other: &TopologySchema) {
        for (key, config) in &other.nodes {
            self.nodes.insert(key.clone(), config.clone());
        }
        for edge in &other.edges {
            let exists = self.edges.iter().any(|e| e.edge_type == edge.edge_type
                && e.from == edge.from && e.to == edge.to);
            if !exists {
                self.edges.push(edge.clone());
            }
        }
        if other.modes.diff.is_some() {
            self.modes.diff = other.modes.diff.clone();
        }
        if other.modes.heatmap.is_some() {
            self.modes.heatmap = other.modes.heatmap.clone();
        }
        if other.modes.flow.is_some() {
            self.modes.flow = other.modes.flow.clone();
        }
        if other.modes.timeline.is_some() {
            self.modes.timeline = other.modes.timeline.clone();
        }
    }

    /// Look up the config for a node type, falling back to wildcard `"*"`.
    pub fn node_config(&self, entity_type: &str) -> Option<&NodeTypeConfig> {
        self.nodes.get(entity_type).or_else(|| self.nodes.get("*"))
    }

    /// Validate the schema and return warnings.
    pub fn validate(&self) -> Vec<String> {
        let mut warnings = Vec::new();

        for (key, config) in &self.nodes {
            if matches!(config.geometry, Geometry::Timeline | Geometry::Stream)
                && config.time_field.is_none()
            {
                warnings.push(format!(
                    "node type '{key}' has geometry {:?} but no time_field",
                    config.geometry
                ));
            }
            if config.geometry == Geometry::Geo
                && (config.lat_field.is_none() || config.lng_field.is_none())
            {
                warnings.push(format!(
                    "node type '{key}' has geometry geo but missing lat_field/lng_field"
                ));
            }
            for child in &config.contains {
                if child != "*" && !self.nodes.contains_key(child) {
                    warnings.push(format!(
                        "node type '{key}' contains '{child}' which is not defined in the schema"
                    ));
                }
            }
        }

        warnings
    }
}

// ---------------------------------------------------------------------------
// 7R Disposition
// ---------------------------------------------------------------------------

/// Assessment disposition for a discovered entity (7R model).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    Rehost,
    Replatform,
    Refactor,
    Repurchase,
    Retire,
    Retain,
    Ratify,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL_SCHEMA: &str = r##"
name: test
label: "Test Schema"
version: "1.0.0"
nodes:
  module:
    geometry: tree
    contains: [function]
    style:
      shape: rect
      color: "#6366f1"
  function:
    geometry: force
    style:
      shape: circle
      color: "#22c55e"
  "*":
    geometry: force
edges:
  - type: contains
    from: module
    to: function
    cardinality: "1:N"
  - type: calls
    from: function
    to: function
"##;

    #[test]
    fn parse_minimal_schema() {
        let schema = TopologySchema::from_yaml(MINIMAL_SCHEMA).unwrap();
        assert_eq!(schema.name, "test");
        assert_eq!(schema.nodes.len(), 3);
        assert_eq!(schema.edges.len(), 2);

        let module = schema.node_config("module").unwrap();
        assert_eq!(module.geometry, Geometry::Tree);
        assert_eq!(module.contains, vec!["function"]);

        let unknown = schema.node_config("unknown_type").unwrap();
        assert_eq!(unknown.geometry, Geometry::Force);
    }

    #[test]
    fn validate_catches_missing_time_field() {
        let yaml = r#"
name: bad
label: "Bad"
version: "1.0.0"
nodes:
  event:
    geometry: timeline
edges: []
"#;
        let schema = TopologySchema::from_yaml(yaml).unwrap();
        let warnings = schema.validate();
        assert!(warnings.iter().any(|w| w.contains("time_field")));
    }

    #[test]
    fn merge_schemas() {
        let mut base = TopologySchema::from_yaml(MINIMAL_SCHEMA).unwrap();
        let overlay_yaml = r##"
name: overlay
label: "Overlay"
version: "1.0.0"
nodes:
  service:
    geometry: force
    iri: "weftos:arch#Service"
    style:
      shape: hexagon
      color: "#ec4899"
edges:
  - type: depends_on
    from: service
    to: service
"##;
        let overlay = TopologySchema::from_yaml(overlay_yaml).unwrap();
        base.merge(&overlay);
        assert!(base.nodes.contains_key("service"));
        assert_eq!(base.edges.len(), 3);
    }

    #[test]
    fn schema_with_iri() {
        let yaml = r##"
name: iri-test
label: "IRI Test"
version: "1.0.0"
iri: "https://weftos.weavelogic.ai/schema/test/1.0"
nodes:
  person:
    iri: "https://weftos.weavelogic.ai/ontology/forensic#Person"
    same_as:
      - "http://xmlns.com/foaf/0.1/Person"
      - "http://schema.org/Person"
    geometry: force
edges: []
"##;
        let schema = TopologySchema::from_yaml(yaml).unwrap();
        let person = schema.node_config("person").unwrap();
        assert_eq!(person.iri.as_deref(), Some("https://weftos.weavelogic.ai/ontology/forensic#Person"));
        assert_eq!(person.same_as.len(), 2);
    }

    #[test]
    fn disposition_serializes() {
        let d = Disposition::Ratify;
        let json = serde_json::to_string(&d).unwrap();
        assert_eq!(json, "\"ratify\"");
    }
}
