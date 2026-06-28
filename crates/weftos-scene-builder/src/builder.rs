//! [`SceneBuilder`] — fluent producer API.
//!
//! ## Usage
//!
//! ```ignore
//! use weftos_scene_builder::{SceneBuilder, Layer, Primitive, Style, Rgba};
//!
//! let mut b = SceneBuilder::new("kernel.ps", 0);
//! b.viewport(800, 480).bg(Rgba::opaque(0x10, 0x10, 0x18));
//!
//! b.insert("ps.header", b.text(Layer::Text, "PID    AGENT    STATE", 4, 12, Rgba::CYAN));
//! b.insert("ps.row[0]", b.text(Layer::Text, "001    kernel   running", 4, 28, Rgba::GREEN));
//!
//! let store = b.build();
//! ```
//!
//! The same producer rebuilds with the same paths on every refresh; the
//! [`super::diff`] function then computes the minimal `SceneOp` delta
//! between the previous and current stores.

use std::collections::BTreeMap;
use std::string::String;
use std::vec::Vec;

use weftos_leaf_scene::{
    path_to_id, BuiltinFont, CursorHint, DisplayId, FontFace, HitShape, InputRegion, KerningHint,
    Layer, Node, NodeId, Primitive, Rect, Rgba, Scene, SceneStore, Style, Transform,
};

/// Fluent host-side scene builder.
///
/// Owns a producer string (used as the `path_to_id` salt) and a
/// `DisplayId`. Nodes inserted by path get a deterministic
/// [`NodeId`](weftos_leaf_scene::NodeId); the same `(producer,
/// display_id, path)` always maps to the same NodeId across runs and
/// reboots — which is what the leaf's glyph cache + AABB cache assume.
///
/// `SceneBuilder` is intentionally **not** clone-cheap; it holds a
/// `BTreeMap<NodeId, Node>` keyed view of the scene plus a
/// `Vec<NodeId>` z-order witness. Producers build, snapshot, drop —
/// they don't pass it around.
#[derive(Debug, Clone)]
pub struct SceneBuilder {
    producer: String,
    display_id: DisplayId,
    bg: Rgba,
    viewport: Rect,
    /// `path` → resolved NodeId. Kept for path-based update / remove.
    path_index: BTreeMap<String, NodeId>,
    /// `NodeId` → node. The wire-shaped storage.
    nodes: BTreeMap<NodeId, Node>,
    /// Insertion order — preserved as intra-layer z-order in the
    /// produced [`Scene`].
    order: Vec<NodeId>,
}

impl SceneBuilder {
    /// Construct a new builder. `producer` is the deterministic salt
    /// for `path_to_id` (e.g. `"kernel.ps"`); `display_id` is the
    /// leaf-side display index (0 for single-display leaves).
    pub fn new(producer: impl Into<String>, display_id: DisplayId) -> Self {
        Self {
            producer: producer.into(),
            display_id,
            bg: Rgba::TRANSPARENT,
            viewport: Rect::ZERO,
            path_index: BTreeMap::new(),
            nodes: BTreeMap::new(),
            order: Vec::new(),
        }
    }

    /// Set the display viewport in integer pixels. Required for the
    /// leaf's damage threshold arithmetic to be meaningful; the v1
    /// renderer treats an empty viewport as "no clipping", which is
    /// usually wrong on real hardware.
    pub fn viewport(&mut self, w_px: i32, h_px: i32) -> &mut Self {
        self.viewport = Rect::from_px(0, 0, w_px, h_px);
        self
    }

    /// Set the display background colour.
    pub fn bg(&mut self, bg: Rgba) -> &mut Self {
        self.bg = bg;
        self
    }

    /// Resolve a path to its deterministic [`NodeId`]. Producers don't
    /// normally call this — use [`insert`](Self::insert) or
    /// [`update`](Self::update) — but a few backends use it to embed
    /// NodeIds in side data (e.g. an input dispatcher keyed by hit-test
    /// node id).
    pub fn id_for(&self, path: &str) -> NodeId {
        path_to_node_id(&self.producer, self.display_id, path)
    }

    /// Insert (or replace) a node at `path`. The path becomes the
    /// hash input for the resulting [`NodeId`].
    ///
    /// Replacement preserves z-order: a re-inserted path keeps its
    /// original slot in `order`. To force a node to a new z-position,
    /// call [`remove`](Self::remove) first.
    pub fn insert(&mut self, path: impl Into<String>, mut node: Node) -> &mut Self {
        let path: String = path.into();
        let id = path_to_node_id(&self.producer, self.display_id, &path);
        node.id = id;
        if let Some(existing) = self.path_index.get(&path) {
            // Path already present — preserve z-order, just swap the
            // node body. Inserting under an existing path is an
            // *update*, semantically.
            let _ = existing;
        } else {
            self.path_index.insert(path, id);
            self.order.push(id);
        }
        self.nodes.insert(id, node);
        self
    }

    /// Update a node in place. Equivalent to `insert` semantically, but
    /// asserts the path was previously inserted — caller intent is
    /// "mutate", not "create". Silently no-ops if the path is unknown
    /// (the producer can decide whether that's worth logging).
    pub fn update(&mut self, path: &str, node: Node) -> &mut Self {
        if self.path_index.contains_key(path) {
            self.insert(path.to_string(), node);
        }
        self
    }

    /// Remove a node by path. No-op if the path is unknown.
    pub fn remove(&mut self, path: &str) -> &mut Self {
        if let Some(id) = self.path_index.remove(path) {
            self.nodes.remove(&id);
            self.order.retain(|other| *other != id);
        }
        self
    }

    /// True if a node is currently registered at `path`.
    pub fn contains(&self, path: &str) -> bool {
        self.path_index.contains_key(path)
    }

    /// Number of nodes currently in the builder.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// True when no nodes are registered.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// The producer string this builder was constructed with.
    pub fn producer(&self) -> &str {
        &self.producer
    }

    /// The display id this builder targets.
    pub fn display_id(&self) -> DisplayId {
        self.display_id
    }

    /// Snapshot the builder into a [`SceneStore`] suitable for
    /// envelope packaging or diffing.
    pub fn build(&self) -> SceneStore {
        let mut store = SceneStore::new();
        store.set_viewport(self.display_id, self.viewport);
        let _ = store.set_bg(self.display_id, self.bg);
        // Walk in insertion order so the SceneStore's `node_order`
        // matches our intent.
        for id in &self.order {
            if let Some(node) = self.nodes.get(id) {
                let op = weftos_leaf_scene::SceneOp::Insert(node.clone());
                let _ = store.apply_op(self.display_id, &op);
            }
        }
        store
    }

    /// Snapshot the builder into a [`Scene`] (the wire-immediate form).
    /// Equivalent to `self.build().to_snapshot(self.display_id())`.
    pub fn build_scene(&self) -> Scene {
        let mut nodes = Vec::with_capacity(self.order.len());
        for id in &self.order {
            if let Some(n) = self.nodes.get(id) {
                nodes.push(n.clone());
            }
        }
        Scene {
            display_id: self.display_id,
            bg: self.bg,
            viewport: self.viewport,
            layer_blend: [weftos_leaf_scene::BlendMode::Normal; 4],
            nodes,
        }
    }

    // ── Convenience primitive constructors ───────────────────────────
    //
    // These take integer pixel coordinates (we convert to Q24.8
    // internally via `Transform::translate(px(x), px(y))`). Producers
    // that need sub-pixel placement can build `Node`s manually.

    /// Build a text node — most common producer primitive.
    pub fn text(
        &self,
        layer: Layer,
        content: impl Into<String>,
        x_px: i32,
        y_px: i32,
        color: Rgba,
    ) -> Node {
        text_node(layer, content.into(), x_px, y_px, color)
    }

    /// Build a filled rectangle node.
    pub fn rect(
        &self,
        layer: Layer,
        x_px: i32,
        y_px: i32,
        w_px: i32,
        h_px: i32,
        fill: Rgba,
    ) -> Node {
        rect_node(layer, x_px, y_px, w_px, h_px, fill)
    }

    /// Build an interactive (hit-testable) filled rect — useful for
    /// touch-target buttons.
    pub fn button(
        &self,
        layer: Layer,
        x_px: i32,
        y_px: i32,
        w_px: i32,
        h_px: i32,
        fill: Rgba,
    ) -> Node {
        let mut n = rect_node(layer, x_px, y_px, w_px, h_px, fill);
        n.input = Some(InputRegion {
            shape: HitShape::Aabb {
                w: weftos_leaf_scene::px(w_px),
                h: weftos_leaf_scene::px(h_px),
            },
            cursor_hint: CursorHint::Pointer,
            capture: false,
        });
        n
    }
}

/// Small, public alias for the path-hashing rule. Producers that want
/// to compute a NodeId without owning a builder use this directly.
pub fn path_to_node_id(producer: &str, display_id: DisplayId, path: &str) -> NodeId {
    // `path_to_id` expects `&[u16]` for the path. We hash the bytes by
    // packing every two bytes into a u16; for the producer-side
    // identifier this is good enough — collisions are vanishingly rare
    // at the 24-bit truncated-FxHash output regardless.
    let bytes = path.as_bytes();
    let mut buf = Vec::with_capacity(bytes.len().div_ceil(2));
    let mut i = 0;
    while i < bytes.len() {
        let hi = bytes[i] as u16;
        let lo = bytes.get(i + 1).copied().unwrap_or(0) as u16;
        buf.push((hi << 8) | lo);
        i += 2;
    }
    path_to_id(display_id, producer, &buf)
}

// ── Primitive constructors (free functions for tests / ad-hoc use) ────

/// Stand-alone builder for a text node. Useful when caller already has
/// a `producer` + `display` and just wants a `Node` it can mutate
/// before passing to `insert`.
pub fn text_node(layer: Layer, content: String, x_px: i32, y_px: i32, color: Rgba) -> Node {
    use weftos_leaf_scene::px;
    Node {
        // Placeholder id; SceneBuilder::insert overwrites it.
        id: NodeId::from_parts(0, 0),
        layer,
        transform: Transform::translate(px(x_px), px(y_px)),
        primitive: Primitive::Text {
            content,
            face: FontFace::Builtin(BuiltinFont::Mono10x20),
            // 10 px ≈ 0x0A00 in Q8.8. The renderer rounds this to
            // built-in cell size anyway.
            size_q8: 10 << 8,
            weight: 400,
            kerning: KerningHint::Auto,
        },
        style: Style {
            fill: Some(color),
            stroke: None,
            stroke_width_q8: 0,
            opacity: 255,
            visible: true,
        },
        input: None,
    }
}

/// Stand-alone builder for a filled-rect node.
pub fn rect_node(layer: Layer, x_px: i32, y_px: i32, w_px: i32, h_px: i32, fill: Rgba) -> Node {
    use weftos_leaf_scene::px;
    Node {
        id: NodeId::from_parts(0, 0),
        layer,
        transform: Transform::translate(px(x_px), px(y_px)),
        primitive: Primitive::Rect {
            w: px(w_px),
            h: px(h_px),
            radius_q8: 0,
        },
        style: Style {
            fill: Some(fill),
            stroke: None,
            stroke_width_q8: 0,
            opacity: 255,
            visible: true,
        },
        input: None,
    }
}

/// Builder helper returned by [`SceneBuilder::node`] (for callers that
/// want a node-typed builder pattern). Reserved for v1.1 — v1 keeps
/// the API surface narrow.
pub struct NodeBuilder {
    _private: (),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_assigns_deterministic_id() {
        let mut b = SceneBuilder::new("kernel.ps", 0);
        b.insert(
            "ps.row[0]",
            text_node(Layer::Text, "hi".into(), 0, 0, Rgba::WHITE),
        );
        let id = b.id_for("ps.row[0]");
        // The NodeId we stored at the path must match the path-hashed id.
        assert_eq!(b.nodes.get(&id).map(|n| n.id), Some(id));
    }

    #[test]
    fn deterministic_across_builders() {
        let b1 = SceneBuilder::new("kernel.ps", 0);
        let b2 = SceneBuilder::new("kernel.ps", 0);
        assert_eq!(b1.id_for("ps.row[0]"), b2.id_for("ps.row[0]"));
    }

    #[test]
    fn distinct_producers_distinct_ids() {
        let b1 = SceneBuilder::new("kernel.ps", 0);
        let b2 = SceneBuilder::new("kernel.logs", 0);
        assert_ne!(b1.id_for("row[0]"), b2.id_for("row[0]"));
    }

    #[test]
    fn distinct_displays_distinct_ids() {
        let b1 = SceneBuilder::new("kernel.ps", 0);
        let b2 = SceneBuilder::new("kernel.ps", 1);
        assert_ne!(b1.id_for("ps.row[0]"), b2.id_for("ps.row[0]"));
        // ...but the 24-bit path hash is the same.
        assert_eq!(
            b1.id_for("ps.row[0]").path_hash(),
            b2.id_for("ps.row[0]").path_hash()
        );
    }

    #[test]
    fn insert_then_update_preserves_zorder() {
        let mut b = SceneBuilder::new("kernel.ps", 0);
        b.insert("a", text_node(Layer::Text, "1".into(), 0, 0, Rgba::WHITE));
        b.insert("b", text_node(Layer::Text, "2".into(), 0, 0, Rgba::WHITE));
        let order_before = b.order.clone();
        b.update("a", text_node(Layer::Text, "1!".into(), 5, 5, Rgba::RED));
        assert_eq!(b.order, order_before, "z-order must not shift on update");
    }

    #[test]
    fn remove_drops_node_and_index() {
        let mut b = SceneBuilder::new("kernel.ps", 0);
        b.insert("a", text_node(Layer::Text, "1".into(), 0, 0, Rgba::WHITE));
        assert!(b.contains("a"));
        b.remove("a");
        assert!(!b.contains("a"));
        assert!(b.is_empty());
    }

    #[test]
    fn build_emits_store_with_nodes() {
        let mut b = SceneBuilder::new("kernel.ps", 0);
        b.viewport(800, 480).bg(Rgba::opaque(0x10, 0x10, 0x18));
        b.insert("a", text_node(Layer::Text, "1".into(), 0, 0, Rgba::WHITE));
        b.insert("b", text_node(Layer::Text, "2".into(), 0, 0, Rgba::WHITE));
        let store = b.build();
        let display = store.display(0).expect("display 0");
        assert_eq!(display.nodes.len(), 2);
        assert_eq!(display.viewport, Rect::from_px(0, 0, 800, 480));
    }

    #[test]
    fn button_carries_input_region() {
        let b = SceneBuilder::new("kernel.ps", 0);
        let n = b.button(Layer::Widget, 0, 0, 100, 50, Rgba::BLUE);
        assert!(n.is_interactive());
    }
}
