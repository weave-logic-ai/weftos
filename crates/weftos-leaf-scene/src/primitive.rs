//! Primitives, styles, font faces, hit-shapes, blend modes — see
//! [vector-leaf-display.md §4.3 Display types](../../../docs/design/vector-leaf-display.md)
//! and §5.5 (text/fonts), §5.7 (alpha/blend), §5.8 (hit-test).
//!
//! All wire-format types live here. The renderer (Phase B) consumes
//! them; producers (Phase E) emit them.

use alloc::string::String;
use alloc::vec::Vec;

use serde::{Deserialize, Serialize};

use crate::color::Rgba;

/// Z-order bucket. Within a layer, NodeState declaration order
/// determines sibling z-order. See §5.3.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Layer {
    Bg = 0,
    Widget = 1,
    Text = 2,
    Alert = 3,
}

impl Layer {
    /// Layers ordered top-of-stack first. Useful for hit-test which
    /// walks back-to-front.
    pub const TOP_DOWN: [Layer; 4] = [Layer::Alert, Layer::Text, Layer::Widget, Layer::Bg];

    /// Layers in draw order (bottom-up).
    pub const DRAW_ORDER: [Layer; 4] = [Layer::Bg, Layer::Widget, Layer::Text, Layer::Alert];

    #[inline]
    pub const fn index(self) -> usize {
        self as usize
    }
}

/// Per-layer compositing mode. v1 honours only `Normal`; all others
/// degrade to `Normal` with a one-time warning in the renderer.
#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BlendMode {
    #[default]
    Normal,
    Multiply,
    Screen,
    Additive,
    SrcOver,
    DstOver,
}

/// Built-in bitmap font atlas identifier. v1 ships `Mono10x20` and
/// `Mono6x10`; the renderer's glyph cache (Phase B) stores their
/// pre-rasterized bitmaps.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BuiltinFont {
    Mono6x10,
    Mono10x20,
}

/// Font selection. The extension hinge for v1.1 vector fonts.
///
/// v1 honours only `Builtin(_)`. `Vector` and `Inline` return
/// `Unsupported` from the renderer and the affected node is skipped.
///
/// `Hash` is required by the Phase B renderer's [`GlyphCache`] key
/// (`(FontFace, char, size_q8)`); derived here so cache lookups stay
/// O(1). The `Vector` variant's `String` family hashes by content.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FontFace {
    Builtin(BuiltinFont),
    /// Reference into a leaf-installed face. v1.1.
    Vector {
        family: String,
        style: FontStyle,
    },
    /// Inline font subset, uploaded via a future `SceneOp::UploadFont`. v1.1.
    Inline {
        id: u32,
    },
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FontStyle {
    Normal,
    Italic,
    Oblique,
}

/// Text kerning policy. `Auto` defers to the font; `None` disables
/// kerning; `Pairs` carries inline overrides for fixed-width fonts.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum KerningHint {
    #[default]
    Auto,
    None,
    Pairs(Vec<(char, char, i8)>),
}

/// Path drawing command. v1's renderer does not honour `Path`
/// primitives; ships in the wire so v1.1 can land without breaking
/// envelopes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PathCmd {
    MoveTo {
        x: i32,
        y: i32,
    },
    LineTo {
        x: i32,
        y: i32,
    },
    QuadTo {
        cx: i32,
        cy: i32,
        x: i32,
        y: i32,
    },
    CubicTo {
        c1x: i32,
        c1y: i32,
        c2x: i32,
        c2y: i32,
        x: i32,
        y: i32,
    },
    Close,
}

/// Bitmap encoding format. v1 supports only `Raw8888` and `Raw565`;
/// everything else returns `Unsupported` from the renderer's
/// `decode_bitmap` hook and the affected node is skipped.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BitmapFormat {
    Raw8888,
    Raw565,
    Qoi,
    Png,
    Rle,
    WebP,
}

/// The drawable primitive carried by a `Node`. All coordinates are
/// relative to the node's `Transform` and are Q24.8.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Primitive {
    /// Filled or stroked rect with optional corner radius.
    Rect {
        /// Width in Q24.8 pixels.
        w: i32,
        /// Height in Q24.8 pixels.
        h: i32,
        /// Corner radius in Q24.8. `0` for sharp corners.
        radius_q8: u16,
    },
    /// Line from the node's origin to `(x2, y2)`. Q24.8.
    Line { x2: i32, y2: i32, thickness_q8: u16 },
    /// Filled or stroked circle centered on the node's origin.
    Circle {
        /// Radius in Q16.16 to preserve sub-pixel for AA in v1.1.
        radius_q16: u32,
    },
    /// Single-line text. Width / height are inferred from `face`
    /// + `size_q8` + glyph metrics.
    Text {
        content: String,
        face: FontFace,
        /// Size in Q8.8 pixels. v1 rounds to int.
        size_q8: u16,
        /// CSS-like weight 100..=900. v1 ignores.
        weight: u16,
        kerning: KerningHint,
    },
    /// Arbitrary bitmap. `data` is the raw bytes in `format` order;
    /// `w` × `h` give the decoded extent.
    Bitmap {
        w: i32,
        h: i32,
        format: BitmapFormat,
        data: Vec<u8>,
    },
    /// Vector path. v1.1.
    Path { commands: Vec<PathCmd> },
}

/// Hit-region shape. v1 supports `Aabb` and `Circle`; `Path` is v1.1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HitShape {
    /// Axis-aligned box, relative to node transform. Q24.8.
    Aabb { w: i32, h: i32 },
    /// Circle centered on node origin. Q16.16 radius.
    Circle { radius_q16: u32 },
    /// Arbitrary path. v1.1; renderer/hit-test returns "no hit".
    Path(Vec<PathCmd>),
}

/// UI cursor hint for downstream pointing devices. Reserved; v1 leaf
/// doesn't surface cursors.
#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CursorHint {
    #[default]
    None,
    Pointer,
    Text,
    Grab,
}

/// Per-node touch / pointer region. Optional on `Node` — most nodes
/// are non-interactive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputRegion {
    pub shape: HitShape,
    pub cursor_hint: CursorHint,
    /// Capture drag events after `Down`.
    pub capture: bool,
}

/// Visual styling — colour, opacity, stroke width, visibility.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Style {
    pub fill: Option<Rgba>,
    pub stroke: Option<Rgba>,
    /// Stroke width in Q8.8 pixels. `0` for hairline.
    pub stroke_width_q8: u16,
    /// Per-node opacity multiplier. v1 collapses `1..=254` to `255`.
    pub opacity: u8,
    pub visible: bool,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            fill: None,
            stroke: None,
            stroke_width_q8: 0,
            opacity: 255,
            visible: true,
        }
    }
}

impl Style {
    #[inline]
    pub fn filled(fill: Rgba) -> Self {
        Self {
            fill: Some(fill),
            ..Default::default()
        }
    }

    /// True when the renderer can skip drawing without affecting damage.
    #[inline]
    pub fn is_invisible(&self) -> bool {
        !self.visible || self.opacity == 0
    }
}

/// Easing curve for tweens. v1 ignores; v1.1 honours.
#[derive(Debug, Default, Copy, Clone, PartialEq, Serialize, Deserialize)]
pub enum EaseCurve {
    #[default]
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    /// Cubic bezier `(c1, c2)`. Each component is Q8.8 in `[-0x4000,
    /// +0x4000]` (CSS-like).
    Cubic {
        c1x: i16,
        c1y: i16,
        c2x: i16,
        c2y: i16,
    },
}

/// Eq for EaseCurve — components are integers, no float drift.
impl Eq for EaseCurve {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_index() {
        assert_eq!(Layer::Bg.index(), 0);
        assert_eq!(Layer::Alert.index(), 3);
    }

    #[test]
    fn layer_top_down_starts_with_alert() {
        assert_eq!(Layer::TOP_DOWN[0], Layer::Alert);
        assert_eq!(Layer::TOP_DOWN[3], Layer::Bg);
    }

    #[test]
    fn style_default_is_opaque_visible_no_fill() {
        let s = Style::default();
        assert!(s.visible);
        assert_eq!(s.opacity, 255);
        assert!(s.fill.is_none());
        assert!(!s.is_invisible());
    }

    #[test]
    fn style_is_invisible_when_hidden_or_alpha_zero() {
        let mut s = Style::filled(Rgba::BLUE);
        s.visible = false;
        assert!(s.is_invisible());

        let mut s = Style::filled(Rgba::BLUE);
        s.opacity = 0;
        assert!(s.is_invisible());
    }
}
