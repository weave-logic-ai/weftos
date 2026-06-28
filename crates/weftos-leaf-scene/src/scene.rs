//! Immutable scene snapshot — see
//! [vector-leaf-display.md §5.1 Types](../../../docs/design/vector-leaf-display.md).
//!
//! `Scene` is the wire form of "full state". It's emitted by
//! [`SceneStore::to_snapshot`](crate::store::SceneStore::to_snapshot)
//! and consumed by [`SceneOp::Replace`](crate::op::SceneOp::Replace).
//! Producers send a snapshot on mesh-connect and every ~5 s in steady
//! state (see design doc §4.1). The cadence is a producer-side policy;
//! this crate just provides the materialization.

use alloc::vec::Vec;

use serde::{Deserialize, Serialize};

use crate::color::Rgba;
use crate::geometry::Rect;
use crate::id::DisplayId;
use crate::node::Node;
use crate::primitive::{BlendMode, Layer};

/// Full state of one display at a single instant in time.
///
/// Within `nodes`, order **is** sibling z-order inside each layer.
/// The renderer walks them in declaration order per layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scene {
    /// Which display on this leaf this scene describes.
    pub display_id: DisplayId,
    /// Display background colour.
    pub bg: Rgba,
    /// Viewport extent in Q24.8 pixels. Used by damage threshold
    /// arithmetic; the producer is expected to report the physical
    /// panel size. Empty viewport disables threshold escalation.
    pub viewport: Rect,
    /// Per-layer blend mode (index = `Layer::index()`). Default Normal.
    pub layer_blend: [BlendMode; 4],
    /// All nodes for this display, in z-order within their layer.
    pub nodes: Vec<Node>,
}

impl Scene {
    /// Empty scene with transparent bg, zero viewport, all layers Normal.
    pub fn empty(display_id: DisplayId) -> Self {
        Self {
            display_id,
            bg: Rgba::TRANSPARENT,
            viewport: Rect::ZERO,
            layer_blend: [BlendMode::Normal; 4],
            nodes: Vec::new(),
        }
    }

    /// Look up a node by id. Linear scan; for steady-state queries use
    /// [`SceneStore::node`](crate::store::SceneStore::node) instead.
    pub fn node(&self, id: crate::id::NodeId) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// Filter to a single layer, preserving z-order.
    pub fn layer_nodes(&self, layer: Layer) -> impl Iterator<Item = &Node> {
        self.nodes.iter().filter(move |n| n.layer == layer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::NodeId;
    use crate::primitive::Primitive;

    #[test]
    fn empty_scene_has_normal_layers() {
        let s = Scene::empty(0);
        assert_eq!(s.layer_blend, [BlendMode::Normal; 4]);
        assert!(s.nodes.is_empty());
    }

    #[test]
    fn layer_iter_filters_correctly() {
        let mut s = Scene::empty(0);
        s.nodes.push(Node::new(
            NodeId::from_parts(0, 1),
            Layer::Bg,
            Primitive::Rect {
                w: 0,
                h: 0,
                radius_q8: 0,
            },
        ));
        s.nodes.push(Node::new(
            NodeId::from_parts(0, 2),
            Layer::Text,
            Primitive::Rect {
                w: 0,
                h: 0,
                radius_q8: 0,
            },
        ));
        assert_eq!(s.layer_nodes(Layer::Bg).count(), 1);
        assert_eq!(s.layer_nodes(Layer::Text).count(), 1);
        assert_eq!(s.layer_nodes(Layer::Widget).count(), 0);
    }
}
