//! Q24.8 fixed-point geometry — see
//! [vector-leaf-display.md §5.4 Coordinate system](../../../docs/design/vector-leaf-display.md).
//!
//! Every `x`, `y`, `w`, `h` on the wire is Q24.8 fixed-point pixels:
//! 24 bits of pixel integer + 8 bits of sub-pixel fraction, signed.
//! The v1 renderer rounds to nearest integer at the rasterization
//! boundary; v1.1's sub-pixel rasterizer reads the same bits.
//!
//! Integer-only is a v1 implementation detail, never a wire constraint.

use serde::{Deserialize, Serialize};

/// Convert an integer pixel count to Q24.8.
#[inline]
pub const fn px(n: i32) -> i32 {
    n << 8
}

/// Round a Q24.8 value to nearest integer pixel.
///
/// Banker's-rounding is not used here — we add half a pixel (0x80)
/// then arithmetic-shift. Matches the rasterizer's expected behaviour.
#[inline]
pub const fn from_px_q8(q: i32) -> i32 {
    // For negative values, this still rounds toward +∞ at exactly
    // -0.5; that matches piet / cairo defaults. v1.1 can revisit.
    (q + 128) >> 8
}

/// Floor a Q24.8 value to the integer pixel below.
#[inline]
pub const fn floor_q8(q: i32) -> i32 {
    q >> 8
}

/// Ceil a Q24.8 value to the integer pixel above.
#[inline]
pub const fn ceil_q8(q: i32) -> i32 {
    (q + 0xFF) >> 8
}

/// A point in Q24.8 pixel coordinates.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    #[inline]
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    /// Construct from integer pixels.
    #[inline]
    pub const fn from_px(x: i32, y: i32) -> Self {
        Self { x: px(x), y: px(y) }
    }
}

/// A size in Q24.8 pixel units. Always non-negative on the wire; the
/// constructor does not enforce this (a producer that emits negative
/// sizes is malformed and the renderer will skip the node).
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Size {
    pub w: i32,
    pub h: i32,
}

impl Size {
    #[inline]
    pub const fn new(w: i32, h: i32) -> Self {
        Self { w, h }
    }

    #[inline]
    pub const fn from_px(w: i32, h: i32) -> Self {
        Self { w: px(w), h: px(h) }
    }

    /// True if `w == 0 || h == 0`.
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }
}

/// Axis-aligned rectangle in Q24.8 pixel coordinates. `(x, y)` is the
/// top-left corner; width / height grow to the right and down.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    /// An empty rect at the origin.
    pub const ZERO: Rect = Rect {
        x: 0,
        y: 0,
        w: 0,
        h: 0,
    };

    #[inline]
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    /// Construct from integer pixels.
    #[inline]
    pub const fn from_px(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self {
            x: px(x),
            y: px(y),
            w: px(w),
            h: px(h),
        }
    }

    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }

    #[inline]
    pub const fn right(&self) -> i32 {
        self.x.saturating_add(self.w)
    }

    #[inline]
    pub const fn bottom(&self) -> i32 {
        self.y.saturating_add(self.h)
    }

    /// Q24.8 area. Saturates on overflow.
    #[inline]
    pub fn area_q16(&self) -> i64 {
        (self.w as i64) * (self.h as i64)
    }

    /// True if `(px, py)` is inside this rect (half-open: right and
    /// bottom edges are excluded). All coords are Q24.8.
    #[inline]
    pub fn contains(&self, px_q8: i32, py_q8: i32) -> bool {
        px_q8 >= self.x && px_q8 < self.right() && py_q8 >= self.y && py_q8 < self.bottom()
    }

    /// True if `self` overlaps `other` (touching edges do not count).
    pub fn intersects(&self, other: &Rect) -> bool {
        if self.is_empty() || other.is_empty() {
            return false;
        }
        self.x < other.right()
            && other.x < self.right()
            && self.y < other.bottom()
            && other.y < self.bottom()
    }

    /// Smallest rect enclosing both. Empty rects are absorbed.
    pub fn union(&self, other: &Rect) -> Rect {
        if self.is_empty() {
            return *other;
        }
        if other.is_empty() {
            return *self;
        }
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let r = self.right().max(other.right());
        let b = self.bottom().max(other.bottom());
        Rect {
            x,
            y,
            w: r - x,
            h: b - y,
        }
    }

    /// Intersection. Returns an empty rect when there's no overlap.
    pub fn intersection(&self, other: &Rect) -> Rect {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let r = self.right().min(other.right());
        let b = self.bottom().min(other.bottom());
        if r <= x || b <= y {
            return Rect::ZERO;
        }
        Rect {
            x,
            y,
            w: r - x,
            h: b - y,
        }
    }

    /// Clip `self` to `bounds`. Empty result is preserved.
    #[inline]
    pub fn clip_to(&self, bounds: &Rect) -> Rect {
        self.intersection(bounds)
    }

    /// Inflate by `dx`, `dy` on all sides. Negative values shrink;
    /// shrinking past zero clamps to empty.
    pub fn inflate(&self, dx: i32, dy: i32) -> Rect {
        let new_w = (self.w + 2 * dx).max(0);
        let new_h = (self.h + 2 * dy).max(0);
        Rect {
            x: self.x - dx,
            y: self.y - dy,
            w: new_w,
            h: new_h,
        }
    }
}

/// 2D affine transform — Q24.8 translation + Q8.8 rotation + Q16.16
/// scale.
///
/// v1 honours translation only. Non-zero `rotation_deg_q8` and `scale_q16
/// != 0x0001_0000` are ignored by the v1 renderer (it logs a one-shot
/// warning) but they ride on the wire so v1.1 doesn't need a wire bump.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transform {
    /// Q24.8 px.
    pub x: i32,
    /// Q24.8 px.
    pub y: i32,
    /// Q8.8 degrees. v1 ignores non-zero.
    pub rotation_deg_q8: i16,
    /// Q16.16. 0x0001_0000 = 1.0. v1 ignores anything else.
    pub scale_q16: u32,
}

impl Transform {
    /// Q16.16 representation of 1.0 — identity scale.
    pub const SCALE_ONE_Q16: u32 = 0x0001_0000;

    /// Identity transform.
    pub const IDENTITY: Transform = Transform {
        x: 0,
        y: 0,
        rotation_deg_q8: 0,
        scale_q16: Self::SCALE_ONE_Q16,
    };

    #[inline]
    pub const fn translate(x_q8: i32, y_q8: i32) -> Self {
        Self {
            x: x_q8,
            y: y_q8,
            rotation_deg_q8: 0,
            scale_q16: Self::SCALE_ONE_Q16,
        }
    }

    /// True when this transform is pure translation (v1's fast path).
    #[inline]
    pub const fn is_translation_only(&self) -> bool {
        self.rotation_deg_q8 == 0 && self.scale_q16 == Self::SCALE_ONE_Q16
    }
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn q24_8_roundtrip() {
        assert_eq!(from_px_q8(px(10)), 10);
        assert_eq!(from_px_q8(px(-5)), -5);
    }

    #[test]
    fn rounding_rounds_half_up() {
        // 0.5 px → 1
        assert_eq!(from_px_q8(0x80), 1);
        // 0.49 px → 0 (one tick under half)
        assert_eq!(from_px_q8(0x7F), 0);
    }

    #[test]
    fn rect_contains_half_open() {
        let r = Rect::from_px(10, 10, 20, 20);
        assert!(r.contains(px(10), px(10)));
        assert!(r.contains(px(29), px(29)));
        // Right and bottom edges excluded.
        assert!(!r.contains(px(30), px(20)));
        assert!(!r.contains(px(20), px(30)));
    }

    #[test]
    fn rect_union_absorbs_empty() {
        let a = Rect::from_px(10, 10, 5, 5);
        let b = Rect::ZERO;
        assert_eq!(a.union(&b), a);
        assert_eq!(b.union(&a), a);
    }

    #[test]
    fn rect_union_expands_to_enclose_both() {
        let a = Rect::from_px(10, 10, 5, 5);
        let b = Rect::from_px(20, 20, 10, 10);
        let u = a.union(&b);
        assert_eq!(u, Rect::from_px(10, 10, 20, 20));
    }

    #[test]
    fn rect_intersection_disjoint_is_empty() {
        let a = Rect::from_px(0, 0, 10, 10);
        let b = Rect::from_px(100, 100, 10, 10);
        assert!(a.intersection(&b).is_empty());
    }

    #[test]
    fn rect_intersection_overlap() {
        let a = Rect::from_px(0, 0, 20, 20);
        let b = Rect::from_px(10, 10, 20, 20);
        assert_eq!(a.intersection(&b), Rect::from_px(10, 10, 10, 10));
    }

    #[test]
    fn rect_intersects_touching_edges_do_not_count() {
        let a = Rect::from_px(0, 0, 10, 10);
        let b = Rect::from_px(10, 0, 10, 10);
        assert!(!a.intersects(&b));
    }

    #[test]
    fn transform_identity_is_translation_only() {
        assert!(Transform::IDENTITY.is_translation_only());
        assert!(Transform::translate(px(5), px(7)).is_translation_only());

        let rotated = Transform {
            rotation_deg_q8: 0x0100, // 1°
            ..Transform::IDENTITY
        };
        assert!(!rotated.is_translation_only());

        let scaled = Transform {
            scale_q16: 0x0002_0000, // 2.0
            ..Transform::IDENTITY
        };
        assert!(!scaled.is_translation_only());
    }
}
