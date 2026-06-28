//! Positioned geometry — the output of layout algorithms.
//! Thin renderers consume this to paint pixels.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::topology::{DashStyle, Disposition, NodeShape};

/// A fully positioned graph ready for rendering.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PositionedGraph {
    pub nodes: Vec<PositionedNode>,
    pub edges: Vec<PositionedEdge>,
    pub viewport: Rect,
    pub schema_name: String,
    pub schema_version: String,
}

/// A node with computed screen coordinates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PositionedNode {
    pub id: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub label: String,
    pub node_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iri: Option<String>,
    pub shape: NodeShape,
    pub color: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub has_subgraph: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disposition: Option<Disposition>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metrics: HashMap<String, f64>,
}

/// An edge with computed path coordinates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PositionedEdge {
    pub source_id: String,
    pub target_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub edge_type: String,
    pub path: Vec<[f64; 2]>,
    pub stroke: String,
    pub width: f64,
    pub dash: DashStyle,
    pub arrow: bool,
    pub animated: bool,
}

/// Viewport bounding rectangle.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl PositionedGraph {
    /// Compute the bounding box of all nodes.
    pub fn compute_viewport(&mut self) {
        if self.nodes.is_empty() {
            self.viewport = Rect {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 100.0,
            };
            return;
        }
        let mut min_x = f64::MAX;
        let mut min_y = f64::MAX;
        let mut max_x = f64::MIN;
        let mut max_y = f64::MIN;
        for n in &self.nodes {
            min_x = min_x.min(n.x - n.width / 2.0);
            min_y = min_y.min(n.y - n.height / 2.0);
            max_x = max_x.max(n.x + n.width / 2.0);
            max_y = max_y.max(n.y + n.height / 2.0);
        }
        let pad = 40.0;
        self.viewport = Rect {
            x: min_x - pad,
            y: min_y - pad,
            width: (max_x - min_x) + pad * 2.0,
            height: (max_y - min_y) + pad * 2.0,
        };
    }
}
