//! Scene-graph node — see
//! [vector-leaf-display.md §4.3 Display types](../../../docs/design/vector-leaf-display.md).
//!
//! A `Node` is a wire-format record: a `NodeId`, what to draw
//! (`Primitive`), how to position it (`Transform`), how to style it
//! (`Style`), whether it's interactive (`InputRegion`), and which
//! z-bucket it lives in (`Layer`).
//!
//! Z-order within a layer follows the order nodes appear in
//! [`crate::scene::Scene::nodes`]; the renderer walks them
//! declaration-first.

use serde::{Deserialize, Serialize};

use crate::geometry::{Rect, Transform};
use crate::id::NodeId;
use crate::primitive::{InputRegion, Layer, Primitive, Style};

/// One drawable element of a scene.
///
/// `Node` is intentionally POD-like: clone-cheap (apart from the
/// `Primitive`'s heap members), `Send + Sync`, serde-serializable.
/// `SceneStore` owns the runtime state (cached AABBs, etc.); a `Node`
/// itself is pure data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub layer: Layer,
    pub transform: Transform,
    pub primitive: Primitive,
    pub style: Style,
    pub input: Option<InputRegion>,
}

impl Node {
    /// Convenience: bare-minimum constructor with identity transform,
    /// default style, no input.
    pub fn new(id: NodeId, layer: Layer, primitive: Primitive) -> Self {
        Self {
            id,
            layer,
            transform: Transform::IDENTITY,
            primitive,
            style: Style::default(),
            input: None,
        }
    }

    /// Compute the node's local AABB *in display coordinates* (after
    /// applying the node's translation; rotation/scale are v1.1 and
    /// fall back to translation-only).
    ///
    /// Returns `None` for primitives without a bounded extent (e.g.,
    /// a `Bitmap` with zero `w`/`h`).
    pub fn aabb(&self) -> Option<Rect> {
        let extent = self.primitive_extent()?;
        if extent.is_empty() {
            return None;
        }
        Some(Rect {
            x: self.transform.x + extent.x,
            y: self.transform.y + extent.y,
            w: extent.w,
            h: extent.h,
        })
    }

    /// Extent of the primitive in its own local coordinates (origin
    /// at `(0, 0)` of the primitive's natural drawing space). Used
    /// for AABB computation and hit-testing.
    fn primitive_extent(&self) -> Option<Rect> {
        use crate::primitive::Primitive::*;
        match &self.primitive {
            Rect { w, h, .. } => Some(crate::geometry::Rect::new(0, 0, *w, *h)),
            Line {
                x2,
                y2,
                thickness_q8,
            } => {
                // Conservative AABB: bounding box of the line including
                // half-thickness padding on every side.
                let pad = (*thickness_q8 as i32 + 1) / 2;
                let (xmin, xmax) = if *x2 >= 0 { (0, *x2) } else { (*x2, 0) };
                let (ymin, ymax) = if *y2 >= 0 { (0, *y2) } else { (*y2, 0) };
                Some(crate::geometry::Rect {
                    x: xmin - pad,
                    y: ymin - pad,
                    w: xmax - xmin + 2 * pad,
                    h: ymax - ymin + 2 * pad,
                })
            }
            Circle { radius_q16 } => {
                // Q16.16 → Q24.8 by >>8, then build a 2r × 2r bounding box.
                let r_q8 = ((*radius_q16 >> 8) as i32).max(0);
                Some(crate::geometry::Rect::new(-r_q8, -r_q8, 2 * r_q8, 2 * r_q8))
            }
            Text {
                content,
                face,
                size_q8,
                ..
            } => {
                // Best-effort text extent: glyph width × char count.
                // v1's renderer overrides this from the real glyph
                // cache; we use it for damage approximation.
                let (gw, gh) = builtin_glyph_size(face, *size_q8);
                let chars = content.chars().count() as i32;
                Some(crate::geometry::Rect::new(0, 0, gw * chars, gh))
            }
            Bitmap { w, h, .. } => Some(crate::geometry::Rect::new(0, 0, *w, *h)),
            Path { commands } => path_bounds(commands),
        }
    }

    /// True when this node has an `InputRegion` and is therefore
    /// hit-testable.
    #[inline]
    pub fn is_interactive(&self) -> bool {
        self.input.is_some()
    }
}

/// v1 built-in font metrics in Q24.8 pixels. Real glyph metrics live
/// in the Phase B glyph cache; this helper exists only for AABB
/// estimation when the renderer hasn't run yet.
fn builtin_glyph_size(face: &crate::primitive::FontFace, _size_q8: u16) -> (i32, i32) {
    use crate::geometry::px;
    use crate::primitive::{BuiltinFont, FontFace};
    match face {
        FontFace::Builtin(BuiltinFont::Mono6x10) => (px(6), px(10)),
        FontFace::Builtin(BuiltinFont::Mono10x20) => (px(10), px(20)),
        // v1.1 vector/inline fonts — fall back to a conservative
        // 10×20 cell so damage stays generous.
        FontFace::Vector { .. } | FontFace::Inline { .. } => (px(10), px(20)),
    }
}

/// Bounding box of a path, in Q24.8. `None` for empty paths.
fn path_bounds(cmds: &[crate::primitive::PathCmd]) -> Option<crate::geometry::Rect> {
    use crate::primitive::PathCmd::*;
    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;
    let mut visited = false;
    let mut visit = |x: i32, y: i32| {
        if x < min_x {
            min_x = x;
        }
        if y < min_y {
            min_y = y;
        }
        if x > max_x {
            max_x = x;
        }
        if y > max_y {
            max_y = y;
        }
    };
    for c in cmds {
        match c {
            MoveTo { x, y } | LineTo { x, y } => {
                visited = true;
                visit(*x, *y);
            }
            QuadTo { cx, cy, x, y } => {
                visited = true;
                visit(*cx, *cy);
                visit(*x, *y);
            }
            CubicTo {
                c1x,
                c1y,
                c2x,
                c2y,
                x,
                y,
            } => {
                visited = true;
                visit(*c1x, *c1y);
                visit(*c2x, *c2y);
                visit(*x, *y);
            }
            Close => {}
        }
    }
    if !visited {
        return None;
    }
    Some(crate::geometry::Rect {
        x: min_x,
        y: min_y,
        w: max_x.saturating_sub(min_x),
        h: max_y.saturating_sub(min_y),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgba;
    use crate::geometry::{px, Rect};
    use crate::primitive::{HitShape, InputRegion, Layer, Primitive, Style};

    fn id(n: u32) -> NodeId {
        NodeId::from_parts(0, n)
    }

    #[test]
    fn rect_node_aabb() {
        let n = Node {
            id: id(1),
            layer: Layer::Widget,
            transform: crate::geometry::Transform::translate(px(10), px(20)),
            primitive: Primitive::Rect {
                w: px(50),
                h: px(40),
                radius_q8: 0,
            },
            style: Style::filled(Rgba::RED),
            input: None,
        };
        assert_eq!(n.aabb(), Some(Rect::from_px(10, 20, 50, 40)));
    }

    #[test]
    fn line_node_aabb_includes_thickness() {
        let n = Node::new(
            id(2),
            Layer::Widget,
            Primitive::Line {
                x2: px(30),
                y2: px(40),
                thickness_q8: px(4) as u16,
            },
        );
        let aabb = n.aabb().unwrap();
        // Thickness padding = 2 px on every side.
        assert_eq!(aabb.x, -px(2));
        assert_eq!(aabb.y, -px(2));
    }

    #[test]
    fn empty_text_has_zero_width_aabb() {
        let n = Node::new(
            id(3),
            Layer::Text,
            Primitive::Text {
                content: alloc::string::String::new(),
                face: crate::primitive::FontFace::Builtin(crate::primitive::BuiltinFont::Mono6x10),
                size_q8: 0,
                weight: 400,
                kerning: crate::primitive::KerningHint::Auto,
            },
        );
        // Width = 0 → empty rect → None.
        assert_eq!(n.aabb(), None);
    }

    #[test]
    fn is_interactive_reflects_input_region() {
        let mut n = Node::new(
            id(4),
            Layer::Widget,
            Primitive::Rect {
                w: px(10),
                h: px(10),
                radius_q8: 0,
            },
        );
        assert!(!n.is_interactive());
        n.input = Some(InputRegion {
            shape: HitShape::Aabb {
                w: px(10),
                h: px(10),
            },
            cursor_hint: crate::primitive::CursorHint::Pointer,
            capture: false,
        });
        assert!(n.is_interactive());
    }
}
