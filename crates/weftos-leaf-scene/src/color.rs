//! Colour types — see
//! [vector-leaf-display.md §4.3 Display types](../../../docs/design/vector-leaf-display.md).
//!
//! `Rgba` is the canonical colour everywhere: fills, strokes, the
//! display background, tween endpoints. Alpha rides on the wire from
//! day one even though the v1 DPI renderer collapses `0..=254` to
//! "transparent" / "opaque" buckets (see §5.7).
//!
//! `Rgb` exists as a convenience for opaque colour declarations; it
//! converts to `Rgba` with `a = 255`.

use serde::{Deserialize, Serialize};

/// 8-bit-per-channel straight (non-premultiplied) RGBA. Wire format
/// is `[r, g, b, a]` via serde's default tuple-struct encoding.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const TRANSPARENT: Rgba = Rgba::new(0, 0, 0, 0);
    pub const BLACK: Rgba = Rgba::new(0, 0, 0, 255);
    pub const WHITE: Rgba = Rgba::new(255, 255, 255, 255);
    pub const RED: Rgba = Rgba::new(255, 0, 0, 255);
    pub const GREEN: Rgba = Rgba::new(0, 255, 0, 255);
    pub const BLUE: Rgba = Rgba::new(0, 0, 255, 255);

    #[inline]
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    #[inline]
    pub const fn opaque(r: u8, g: u8, b: u8) -> Self {
        Self::new(r, g, b, 255)
    }

    /// True if fully transparent — used by the renderer to skip nodes.
    #[inline]
    pub const fn is_transparent(&self) -> bool {
        self.a == 0
    }

    /// True if fully opaque — lets the renderer skip the alpha path.
    #[inline]
    pub const fn is_opaque(&self) -> bool {
        self.a == 255
    }

    /// Pack to `0xRRGGBBAA`. Useful for hashing into glyph caches and
    /// for golden-image tests in Phase B.
    #[inline]
    pub const fn to_rgba_u32(&self) -> u32 {
        ((self.r as u32) << 24) | ((self.g as u32) << 16) | ((self.b as u32) << 8) | (self.a as u32)
    }

    /// Component-wise lerp at `t` ∈ `[0, 256]` (Q8 fraction). `t==0`
    /// returns `self`, `t==256` returns `other`. Saturating on the
    /// edges.
    pub fn lerp_q8(&self, other: &Rgba, t_q8: u16) -> Rgba {
        let t = t_q8.min(256) as i32;
        let inv = 256 - t;
        let lerp = |a: u8, b: u8| -> u8 {
            (((a as i32) * inv + (b as i32) * t + 128) >> 8).clamp(0, 255) as u8
        };
        Rgba {
            r: lerp(self.r, other.r),
            g: lerp(self.g, other.g),
            b: lerp(self.b, other.b),
            a: lerp(self.a, other.a),
        }
    }
}

impl Default for Rgba {
    fn default() -> Self {
        Self::TRANSPARENT
    }
}

/// 8-bit-per-channel opaque RGB. Convenience for declarative
/// configuration; converts to `Rgba` losslessly.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    #[inline]
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

impl From<Rgb> for Rgba {
    #[inline]
    fn from(c: Rgb) -> Self {
        Rgba::new(c.r, c.g, c.b, 255)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_helper() {
        let c = Rgba::opaque(10, 20, 30);
        assert_eq!(c, Rgba::new(10, 20, 30, 255));
        assert!(c.is_opaque());
        assert!(!c.is_transparent());
    }

    #[test]
    fn lerp_endpoints() {
        let a = Rgba::new(0, 0, 0, 0);
        let b = Rgba::new(100, 100, 100, 100);
        assert_eq!(a.lerp_q8(&b, 0), a);
        assert_eq!(a.lerp_q8(&b, 256), b);
    }

    #[test]
    fn lerp_midpoint() {
        let a = Rgba::new(0, 0, 0, 0);
        let b = Rgba::new(100, 100, 100, 100);
        let m = a.lerp_q8(&b, 128);
        // (0 * 128 + 100 * 128 + 128) >> 8 = 12928 >> 8 = 50.
        assert_eq!(m, Rgba::new(50, 50, 50, 50));
    }

    #[test]
    fn rgb_to_rgba_is_opaque() {
        let c: Rgba = Rgb::new(10, 20, 30).into();
        assert_eq!(c, Rgba::new(10, 20, 30, 255));
    }

    #[test]
    fn pack_u32() {
        assert_eq!(Rgba::new(0x12, 0x34, 0x56, 0x78).to_rgba_u32(), 0x1234_5678);
    }
}
