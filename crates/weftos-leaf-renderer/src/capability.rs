//! Renderer capability bitflags — see
//! [vector-leaf-display.md §6 Renderer Trait](../../../docs/design/vector-leaf-display.md).
//!
//! Each backend declares (via [`SceneSurface::capabilities`](crate::SceneSurface::capabilities))
//! which optional features it can natively render. The renderer uses
//! these flags to skip the degraded fallback for capable backends and
//! to short-circuit unsupported primitives on minimal ones.
//!
//! A backend that returns `CapabilityMask::empty()` is the v1 baseline:
//! opaque-only, integer-pixel, mono fonts, raw bitmaps, `Normal` blend.

use bitflags::bitflags;

bitflags! {
    /// Optional feature set a renderer backend can declare. Composed
    /// with `|`; checked with `contains`.
    ///
    /// | Flag | Meaning |
    /// |---|---|
    /// | `ALPHA`         | Proper per-pixel alpha compositing (style.opacity 1..=254 honoured). |
    /// | `SUBPIXEL`      | Q24.8-aware rasterizer (sub-pixel positioning, no integer rounding at the boundary). |
    /// | `ANTIALIASED`   | Glyph + line AA enabled. |
    /// | `VECTOR_FONTS`  | `FontFace::Vector` / `FontFace::Inline` honoured (v1.1). |
    /// | `BITMAP_QOI`    | `BitmapFormat::Qoi` decoded natively. |
    /// | `BITMAP_PNG`    | `BitmapFormat::Png` decoded natively. |
    /// | `BLEND_MODES`   | Non-`Normal` `BlendMode` honoured per layer. |
    /// | `ANIMATION`     | `tick` honoured (interpolated) instead of v1's snap-to-end. |
    /// | `HIT_TEST_PATH` | `HitShape::Path` rasterized + tested. |
    ///
    /// Phase B + C + D defaults (these match design doc §6):
    ///
    /// | Backend | Mask |
    /// |---|---|
    /// | `DpiSurface` (Phase C, embedded)       | `empty()` |
    /// | `SimSurface` (Phase B, dev tool)       | `ALPHA \| SUBPIXEL \| ANTIALIASED \| BLEND_MODES` |
    /// | `CanvasSurface` (Phase D, browser)     | `ALPHA \| SUBPIXEL \| ANTIALIASED \| BLEND_MODES \| BITMAP_PNG` |
    #[derive(Debug, Default, Copy, Clone, PartialEq, Eq, Hash)]
    pub struct CapabilityMask: u32 {
        const ALPHA          = 1 << 0;
        const SUBPIXEL       = 1 << 1;
        const ANTIALIASED    = 1 << 2;
        const VECTOR_FONTS   = 1 << 3;
        const BITMAP_QOI     = 1 << 4;
        const BITMAP_PNG     = 1 << 5;
        const BLEND_MODES    = 1 << 6;
        const ANIMATION      = 1 << 7;
        const HIT_TEST_PATH  = 1 << 8;
    }
}

impl CapabilityMask {
    /// True if this backend can honour a per-pixel alpha value other
    /// than `0` or `255`. v1 DPI returns `false`; sim + canvas return `true`.
    #[inline]
    pub fn has_alpha(self) -> bool {
        self.contains(Self::ALPHA)
    }

    /// True if this backend honours non-`Normal` blend modes per layer.
    #[inline]
    pub fn has_blend_modes(self) -> bool {
        self.contains(Self::BLEND_MODES)
    }

    /// True if this backend rasterizes glyphs / lines with AA.
    #[inline]
    pub fn is_antialiased(self) -> bool {
        self.contains(Self::ANTIALIASED)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_v1_dpi_baseline() {
        let m = CapabilityMask::empty();
        assert!(!m.has_alpha());
        assert!(!m.has_blend_modes());
        assert!(!m.is_antialiased());
    }

    #[test]
    fn sim_default_is_rich() {
        let m = CapabilityMask::ALPHA
            | CapabilityMask::SUBPIXEL
            | CapabilityMask::ANTIALIASED
            | CapabilityMask::BLEND_MODES;
        assert!(m.has_alpha());
        assert!(m.has_blend_modes());
        assert!(m.is_antialiased());
        assert!(!m.contains(CapabilityMask::BITMAP_PNG));
    }

    #[test]
    fn canvas_default_adds_png() {
        let m = CapabilityMask::ALPHA
            | CapabilityMask::SUBPIXEL
            | CapabilityMask::ANTIALIASED
            | CapabilityMask::BLEND_MODES
            | CapabilityMask::BITMAP_PNG;
        assert!(m.contains(CapabilityMask::BITMAP_PNG));
        assert!(!m.contains(CapabilityMask::BITMAP_QOI));
    }

    #[test]
    fn debug_shows_flag_names() {
        let m = CapabilityMask::ALPHA | CapabilityMask::ANTIALIASED;
        let s = alloc::format!("{:?}", m);
        assert!(s.contains("ALPHA"));
        assert!(s.contains("ANTIALIASED"));
    }
}
