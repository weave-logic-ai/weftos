//! Active-tween table — see
//! [vector-leaf-display.md §5.6 Tweens](../../../docs/design/vector-leaf-display.md).
//!
//! v1 behaviour: when [`SceneStore::tick`](crate::store::SceneStore::tick)
//! runs, every active tween snaps to `to` and is removed. The wire
//! format and trait surface for full interpolation are already shipped,
//! so v1.1 only has to replace `tick`'s body.
//!
//! ## Coalescing
//!
//! When a new tween arrives for a `(node_id, property)` that already
//! has an active tween, the new tween's `from` is forcibly rewritten to
//! the current **interpolated** state of the old tween (in v1, that's
//! `from` because we haven't interpolated yet — the visual is wrong
//! but the data flow is correct). The old tween is cancelled. The
//! producer-friendly behaviour is "newest tween wins, continuity
//! preserved". Document this contract in [`SceneOp::Tween`](crate::op::SceneOp).

use alloc::vec::Vec;

use serde::{Deserialize, Serialize};

use crate::color::Rgba;
use crate::geometry::Point;
use crate::id::NodeId;
use crate::primitive::EaseCurve;

/// Which property a tween animates. Snapshot-only properties (e.g.
/// text content) appear here so the wire format can carry a "set this
/// value over 0 ms" without a special-case — v1 just snaps.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AnimatableProperty {
    Position,
    Opacity,
    Fill,
    Stroke,
    Scale,
    Rotation,
    /// Snap-only — strings don't interpolate.
    TextContent,
}

/// Tween endpoint value. Tagged for self-describing wire decode and
/// to let `tick` dispatch on the variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PropertyValue {
    Position(Point),
    Opacity(u8),
    Color(Rgba),
    /// Q16.16 scale.
    ScaleQ16(u32),
    /// Q8.8 degrees.
    RotationQ8(i16),
    Text(alloc::string::String),
}

/// A tween currently being interpolated by [`SceneStore`].
///
/// `start_ms` is the leaf-local monotonic ms when the tween becomes
/// active. `tick(now_ms)` evaluates `t = (now - start) / duration`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveTween {
    pub id: NodeId,
    pub property: AnimatableProperty,
    pub from: PropertyValue,
    pub to: PropertyValue,
    pub start_ms: u32,
    pub duration_ms: u32,
    pub curve: EaseCurve,
}

impl ActiveTween {
    /// True when `now_ms >= start_ms + duration_ms`. The tween should
    /// snap to `to` and be removed.
    #[inline]
    pub fn is_complete(&self, now_ms: u32) -> bool {
        now_ms.saturating_sub(self.start_ms) >= self.duration_ms
    }

    /// Current Q8 fraction `[0..=256]` along the tween. v1 callers
    /// ignore this (they snap), but v1.1 will use it directly.
    pub fn fraction_q8(&self, now_ms: u32) -> u16 {
        if self.duration_ms == 0 {
            return 256;
        }
        let elapsed = now_ms.saturating_sub(self.start_ms);
        if elapsed >= self.duration_ms {
            return 256;
        }
        // (elapsed * 256) / duration, with overflow guard.
        let num = (elapsed as u64).saturating_mul(256);
        let den = self.duration_ms as u64;
        (num / den).min(256) as u16
    }
}

/// Per-display table of active tweens. Internal to `SceneStore`.
///
/// Lookup is O(n) but n is small (~tens, not thousands) — embedded
/// targets prefer a flat `Vec` over a `BTreeMap` because the allocator
/// cost dominates at this scale.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TweenTable {
    tweens: Vec<ActiveTween>,
}

impl TweenTable {
    pub const fn new() -> Self {
        Self { tweens: Vec::new() }
    }

    /// Insert a tween, coalescing against any in-flight tween on the
    /// same `(node_id, property)` pair.
    ///
    /// Returns `true` if an existing tween was cancelled. The new
    /// tween's `from` is **not** rewritten here — that's `SceneStore`'s
    /// job once it knows the current interpolated state (in v1 it's
    /// always the prior `from` since we haven't ticked).
    ///
    /// See module docs for the coalescing contract.
    pub fn insert(&mut self, new: ActiveTween) -> bool {
        let mut cancelled = false;
        // Linear scan: small n.
        if let Some(idx) = self
            .tweens
            .iter()
            .position(|t| t.id == new.id && t.property == new.property)
        {
            self.tweens.swap_remove(idx);
            cancelled = true;
        }
        self.tweens.push(new);
        cancelled
    }

    /// Remove all tweens for `id`. If `property` is `Some`, remove only
    /// that property; otherwise remove all properties for the node.
    pub fn cancel(&mut self, id: NodeId, property: Option<AnimatableProperty>) -> usize {
        let before = self.tweens.len();
        match property {
            None => self.tweens.retain(|t| t.id != id),
            Some(p) => self.tweens.retain(|t| !(t.id == id && t.property == p)),
        }
        before - self.tweens.len()
    }

    /// Drain completed tweens at `now_ms`. Returns the drained set so
    /// the caller (SceneStore) can apply each tween's `to` value to
    /// the affected node and emit damage.
    ///
    /// v1 implementation: every tween is treated as complete on first
    /// tick (snap-to-`to`). The `now_ms` parameter is recorded for
    /// v1.1 where partial elapsed-time tweens stay in the table.
    ///
    /// v1.1: replace this body with eased interpolation per frame;
    /// only fully-elapsed tweens drain out.
    pub fn tick_v1_snap(&mut self, _now_ms: u32) -> Vec<ActiveTween> {
        // v1.1: change to `self.tweens.iter().filter(|t| t.is_complete(now_ms))`
        // and drain in place. v1 drains everything.
        core::mem::take(&mut self.tweens)
    }

    /// Number of active tweens. Useful for damage estimation.
    #[inline]
    pub fn len(&self) -> usize {
        self.tweens.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.tweens.is_empty()
    }

    /// All active tweens — needed by `to_snapshot()` so a full
    /// snapshot can re-establish in-flight animations after a reboot.
    /// (Whether the snapshot consumer chooses to honor mid-flight
    /// tweens is producer-side policy.)
    #[inline]
    pub fn active(&self) -> &[ActiveTween] {
        &self.tweens
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u32) -> NodeId {
        NodeId::from_parts(0, n)
    }

    fn tween(node: u32, prop: AnimatableProperty, opacity_from: u8, opacity_to: u8) -> ActiveTween {
        ActiveTween {
            id: id(node),
            property: prop,
            from: PropertyValue::Opacity(opacity_from),
            to: PropertyValue::Opacity(opacity_to),
            start_ms: 0,
            duration_ms: 500,
            curve: EaseCurve::Linear,
        }
    }

    #[test]
    fn insert_then_coalesce_same_node_same_property() {
        let mut t = TweenTable::new();
        assert!(!t.insert(tween(1, AnimatableProperty::Opacity, 0, 255)));
        assert_eq!(t.len(), 1);
        // Second tween on the same (id, property) cancels the first.
        assert!(t.insert(tween(1, AnimatableProperty::Opacity, 0, 128)));
        assert_eq!(t.len(), 1);
        // The surviving tween is the new one (target opacity 128).
        match &t.active()[0].to {
            PropertyValue::Opacity(v) => assert_eq!(*v, 128),
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn distinct_properties_coexist() {
        let mut t = TweenTable::new();
        t.insert(tween(1, AnimatableProperty::Opacity, 0, 255));
        t.insert(tween(1, AnimatableProperty::Position, 0, 0));
        assert_eq!(t.len(), 2);
    }

    #[test]
    fn distinct_nodes_coexist() {
        let mut t = TweenTable::new();
        t.insert(tween(1, AnimatableProperty::Opacity, 0, 255));
        t.insert(tween(2, AnimatableProperty::Opacity, 0, 255));
        assert_eq!(t.len(), 2);
    }

    #[test]
    fn cancel_specific_property() {
        let mut t = TweenTable::new();
        t.insert(tween(1, AnimatableProperty::Opacity, 0, 255));
        t.insert(tween(1, AnimatableProperty::Position, 0, 0));
        assert_eq!(t.cancel(id(1), Some(AnimatableProperty::Opacity)), 1);
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn cancel_all_for_node() {
        let mut t = TweenTable::new();
        t.insert(tween(1, AnimatableProperty::Opacity, 0, 255));
        t.insert(tween(1, AnimatableProperty::Position, 0, 0));
        t.insert(tween(2, AnimatableProperty::Opacity, 0, 255));
        assert_eq!(t.cancel(id(1), None), 2);
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn tick_v1_drains_everything() {
        let mut t = TweenTable::new();
        t.insert(tween(1, AnimatableProperty::Opacity, 0, 255));
        t.insert(tween(2, AnimatableProperty::Opacity, 0, 128));
        let drained = t.tick_v1_snap(0);
        assert_eq!(drained.len(), 2);
        assert_eq!(t.len(), 0);
    }

    #[test]
    fn fraction_q8_endpoints() {
        let tw = tween(1, AnimatableProperty::Opacity, 0, 255);
        assert_eq!(tw.fraction_q8(0), 0);
        assert_eq!(tw.fraction_q8(500), 256);
        assert_eq!(tw.fraction_q8(250), 128);
        // Past the end clamps at 256.
        assert_eq!(tw.fraction_q8(10_000), 256);
    }

    #[test]
    fn zero_duration_is_immediate() {
        let mut tw = tween(1, AnimatableProperty::Opacity, 0, 255);
        tw.duration_ms = 0;
        assert!(tw.is_complete(0));
        assert_eq!(tw.fraction_q8(0), 256);
    }
}
