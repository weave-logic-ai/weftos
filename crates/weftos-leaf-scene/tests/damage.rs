//! Damage merge / clip / threshold behaviour at the integration boundary.

use weftos_leaf_scene::{
    damage::DamageSet,
    geometry::{px, Rect},
};

#[test]
fn empty_is_empty() {
    let d = DamageSet::none();
    assert!(d.is_empty());
}

#[test]
fn from_zero_collapses() {
    let d = DamageSet::from_rect(Rect::ZERO);
    assert!(d.is_empty());
}

#[test]
fn nine_disjoint_rects_escalate_to_full_repaint() {
    let mut d = DamageSet::none();
    let vp = Rect::from_px(0, 0, 10_000, 10_000);
    for i in 0..9 {
        d.add(Rect::from_px(i * 100, 0, 10, 10), vp);
    }
    assert!(d.is_full());
}

#[test]
fn overlapping_rects_merge_into_one() {
    let mut d = DamageSet::none();
    let vp = Rect::from_px(0, 0, 800, 480);
    d.add(Rect::from_px(10, 10, 50, 50), vp);
    d.add(Rect::from_px(20, 20, 50, 50), vp);
    assert_eq!(d.len(), 1);
    let r = d.rects()[0];
    assert!(r.contains(px(15), px(15)));
    assert!(r.contains(px(65), px(65)));
}

#[test]
fn fifty_one_percent_area_escalates() {
    let mut d = DamageSet::none();
    let vp = Rect::from_px(0, 0, 100, 100);
    // 60×100 = 60% area → escalate.
    d.add(Rect::from_px(0, 0, 60, 100), vp);
    assert!(d.is_full());
}

#[test]
fn forty_nine_percent_area_does_not_escalate() {
    let mut d = DamageSet::none();
    let vp = Rect::from_px(0, 0, 100, 100);
    d.add(Rect::from_px(0, 0, 49, 100), vp);
    assert!(!d.is_full());
    assert_eq!(d.len(), 1);
}

#[test]
fn merging_full_damage_is_sticky() {
    let mut d = DamageSet::full();
    let mut other = DamageSet::none();
    other.add(Rect::from_px(0, 0, 5, 5), Rect::from_px(0, 0, 800, 480));
    d.merge(&other, Rect::from_px(0, 0, 800, 480));
    assert!(d.is_full());
}

#[test]
fn merging_other_full_promotes_self() {
    let mut d = DamageSet::none();
    d.add(Rect::from_px(0, 0, 5, 5), Rect::from_px(0, 0, 800, 480));
    let other = DamageSet::full();
    d.merge(&other, Rect::from_px(0, 0, 800, 480));
    assert!(d.is_full());
}
