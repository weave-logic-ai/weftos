//! Damage rect computation — see
//! [vector-leaf-display.md §7 Damage Computation](../../../docs/design/vector-leaf-display.md).
//!
//! `DamageSet` is the renderer's contract: "redraw these rects (or
//! the whole viewport)". Every `SceneStore::apply` returns one.
//!
//! Two invariants:
//!
//! 1. **Bounded budget**: at most [`DamageSet::MAX_RECTS`] discrete
//!    rects. Overflow flips `full_repaint = true` and drops the list.
//!    Mirrors LVGL's `lv_refr_join_area`.
//!
//! 2. **Threshold escalation**: when accumulated damage area exceeds
//!    [`DamageSet::FULL_REPAINT_AREA_FRACTION`] of the viewport, we
//!    also flip `full_repaint`. The renderer is then free to skip the
//!    rect list and redraw everything — usually cheaper than walking
//!    8 overlapping rectangles.

use alloc::vec::Vec;

use crate::geometry::Rect;

/// Set of damaged rectangles for one frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DamageSet {
    rects: Vec<Rect>,
    /// When true, the renderer redraws the entire viewport regardless
    /// of `rects`.
    full_repaint: bool,
}

impl DamageSet {
    /// Maximum rect count before we degrade to full-repaint. Matches
    /// LVGL's 8-slot join-area buffer.
    pub const MAX_RECTS: usize = 8;

    /// Fraction of viewport area (in 1/256ths) above which we flip
    /// `full_repaint`. 128/256 = 50%. Tuned to match the design doc's
    /// "more than half the screen → full repaint" rule.
    pub const FULL_REPAINT_AREA_FRACTION: u32 = 128;

    /// Empty damage — nothing changed this frame.
    pub fn none() -> Self {
        Self {
            rects: Vec::new(),
            full_repaint: false,
        }
    }

    /// Full-viewport repaint. The renderer ignores `rects`.
    pub fn full() -> Self {
        Self {
            rects: Vec::new(),
            full_repaint: true,
        }
    }

    /// Damage from a single rect. Empty rect collapses to `none()`.
    pub fn from_rect(r: Rect) -> Self {
        if r.is_empty() {
            return Self::none();
        }
        let mut s = Self::none();
        s.rects.push(r);
        s
    }

    /// Add a damaged rect. May escalate to full-repaint if either
    /// the budget or the area threshold is breached.
    ///
    /// `viewport` is required to evaluate the area threshold. Pass
    /// `Rect::ZERO` (or the unknown sentinel) to skip the threshold
    /// check — only the budget cap applies.
    pub fn add(&mut self, rect: Rect, viewport: Rect) {
        if self.full_repaint {
            return;
        }
        if rect.is_empty() {
            return;
        }
        // Try to merge with an existing overlapping rect to keep the
        // list short. Linear scan is fine — at most 8 entries.
        for existing in self.rects.iter_mut() {
            if existing.intersects(&rect) || touches(existing, &rect) {
                *existing = existing.union(&rect);
                self.check_thresholds(viewport);
                return;
            }
        }
        self.rects.push(rect);
        if self.rects.len() > Self::MAX_RECTS {
            self.escalate();
            return;
        }
        self.check_thresholds(viewport);
    }

    /// Merge another DamageSet into self. Full-repaint dominates.
    pub fn merge(&mut self, other: &DamageSet, viewport: Rect) {
        if self.full_repaint {
            return;
        }
        if other.full_repaint {
            self.escalate();
            return;
        }
        for r in &other.rects {
            self.add(*r, viewport);
            if self.full_repaint {
                return;
            }
        }
    }

    fn escalate(&mut self) {
        self.full_repaint = true;
        self.rects.clear();
    }

    /// Re-evaluate the area threshold. Cheap — sums up to 8 rect areas.
    fn check_thresholds(&mut self, viewport: Rect) {
        if viewport.is_empty() {
            return;
        }
        let total: i64 = self.rects.iter().map(|r| r.area_q16()).sum();
        let vp_area = viewport.area_q16();
        if vp_area <= 0 {
            return;
        }
        // total / vp_area >= FRACTION / 256
        // => total * 256 >= FRACTION * vp_area
        if total.saturating_mul(256)
            >= (Self::FULL_REPAINT_AREA_FRACTION as i64).saturating_mul(vp_area)
        {
            self.escalate();
        }
    }

    /// The current rect list. Empty when `is_full()` is true OR when
    /// no damage has accumulated.
    pub fn rects(&self) -> &[Rect] {
        &self.rects
    }

    pub fn is_full(&self) -> bool {
        self.full_repaint
    }

    pub fn is_empty(&self) -> bool {
        !self.full_repaint && self.rects.is_empty()
    }

    /// Number of rects currently tracked. Returns 0 for a full repaint.
    pub fn len(&self) -> usize {
        self.rects.len()
    }
}

impl Default for DamageSet {
    fn default() -> Self {
        Self::none()
    }
}

/// True if two rects share an edge but do not overlap. Treating edge-
/// adjacent rects as mergeable keeps the rect list compact when an op
/// damages two adjacent cells.
fn touches(a: &Rect, b: &Rect) -> bool {
    // Vertically aligned + horizontally adjacent (or vice-versa).
    let horizontal_edge =
        (a.right() == b.x || b.right() == a.x) && a.y < b.bottom() && b.y < a.bottom();
    let vertical_edge =
        (a.bottom() == b.y || b.bottom() == a.y) && a.x < b.right() && b.x < a.right();
    horizontal_edge || vertical_edge
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::px;

    fn vp() -> Rect {
        Rect::from_px(0, 0, 800, 480)
    }

    #[test]
    fn empty_starts_empty() {
        let d = DamageSet::none();
        assert!(d.is_empty());
        assert!(!d.is_full());
        assert_eq!(d.len(), 0);
    }

    #[test]
    fn single_rect_lands() {
        let mut d = DamageSet::none();
        d.add(Rect::from_px(10, 10, 20, 20), vp());
        assert!(!d.is_full());
        assert_eq!(d.len(), 1);
    }

    #[test]
    fn overlapping_rects_merge() {
        let mut d = DamageSet::none();
        d.add(Rect::from_px(10, 10, 20, 20), vp());
        d.add(Rect::from_px(20, 20, 20, 20), vp());
        assert_eq!(d.len(), 1);
        assert_eq!(d.rects()[0], Rect::from_px(10, 10, 30, 30));
    }

    #[test]
    fn edge_adjacent_rects_merge() {
        let mut d = DamageSet::none();
        d.add(Rect::from_px(10, 10, 20, 20), vp());
        d.add(Rect::from_px(30, 10, 20, 20), vp());
        assert_eq!(d.len(), 1);
        assert_eq!(d.rects()[0], Rect::from_px(10, 10, 40, 20));
    }

    #[test]
    fn disjoint_rects_accumulate() {
        let mut d = DamageSet::none();
        d.add(Rect::from_px(10, 10, 20, 20), vp());
        d.add(Rect::from_px(100, 100, 20, 20), vp());
        assert_eq!(d.len(), 2);
    }

    #[test]
    fn budget_overflow_escalates_to_full_repaint() {
        let mut d = DamageSet::none();
        // Place 9 disjoint 10×10 rects spaced 100 apart.
        for i in 0..9 {
            d.add(
                Rect::from_px(i * 100, 0, 10, 10),
                Rect::from_px(0, 0, 10_000, 10_000),
            );
        }
        assert!(d.is_full());
        assert_eq!(d.rects().len(), 0);
    }

    #[test]
    fn area_threshold_escalates_to_full_repaint() {
        let mut d = DamageSet::none();
        // 600×400 rect on an 800×480 viewport = 62.5% area → escalate.
        d.add(Rect::from_px(0, 0, 600, 400), vp());
        assert!(d.is_full());
    }

    #[test]
    fn area_threshold_under_50pct_stays_partial() {
        let mut d = DamageSet::none();
        // 400×400 / (800×480) ≈ 41.6% → no escalation.
        d.add(Rect::from_px(0, 0, 400, 400), vp());
        assert!(!d.is_full());
    }

    #[test]
    fn merge_full_dominates() {
        let mut a = DamageSet::none();
        a.add(Rect::from_px(10, 10, 5, 5), vp());
        let b = DamageSet::full();
        a.merge(&b, vp());
        assert!(a.is_full());
    }

    #[test]
    fn merge_into_full_is_noop() {
        let mut a = DamageSet::full();
        let mut b = DamageSet::none();
        b.add(Rect::from_px(10, 10, 5, 5), vp());
        a.merge(&b, vp());
        assert!(a.is_full());
    }

    #[test]
    fn from_rect_collapses_empty() {
        let d = DamageSet::from_rect(Rect::ZERO);
        assert!(d.is_empty());
    }

    #[test]
    fn px_helper_compiles_with_const() {
        // Sanity: ensure we can use these helpers in const-ish places.
        let _r = Rect::from_px(0, 0, 800, 480);
        let _x = px(10);
    }
}
