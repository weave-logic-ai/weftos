//! Hit-test correctness: AABB, Circle, layer priority, invisibility.

use weftos_leaf_scene::{
    geometry::{px, Transform},
    primitive::{CursorHint, HitShape, InputRegion, Layer, Primitive, Style},
    Node, NodeId, Rgba, SceneOp, SceneStore,
};

fn id(n: u32) -> NodeId {
    NodeId::from_parts(0, n)
}

fn aabb_node(node_id: NodeId, layer: Layer, x: i32, y: i32, w: i32, h: i32) -> Node {
    Node {
        id: node_id,
        layer,
        transform: Transform::translate(px(x), px(y)),
        primitive: Primitive::Rect {
            w: px(w),
            h: px(h),
            radius_q8: 0,
        },
        style: Style::filled(Rgba::WHITE),
        input: Some(InputRegion {
            shape: HitShape::Aabb { w: px(w), h: px(h) },
            cursor_hint: CursorHint::Pointer,
            capture: false,
        }),
    }
}

#[test]
fn empty_store_returns_none() {
    let s = SceneStore::new();
    assert_eq!(s.hit_test(0, px(50), px(50)), None);
}

#[test]
fn aabb_hit_inside_returns_id() {
    let mut s = SceneStore::new();
    s.apply_op(
        0,
        &SceneOp::Insert(aabb_node(id(1), Layer::Widget, 10, 10, 30, 30)),
    );
    assert_eq!(s.hit_test(0, px(20), px(20)), Some(id(1)));
}

#[test]
fn aabb_miss_outside_returns_none() {
    let mut s = SceneStore::new();
    s.apply_op(
        0,
        &SceneOp::Insert(aabb_node(id(1), Layer::Widget, 10, 10, 30, 30)),
    );
    assert_eq!(s.hit_test(0, px(100), px(100)), None);
}

#[test]
fn alert_layer_wins_over_widget() {
    let mut s = SceneStore::new();
    // Widget under, Alert over — both at (0, 0, 50, 50).
    s.apply_op(
        0,
        &SceneOp::Insert(aabb_node(id(1), Layer::Widget, 0, 0, 50, 50)),
    );
    s.apply_op(
        0,
        &SceneOp::Insert(aabb_node(id(2), Layer::Alert, 0, 0, 50, 50)),
    );
    assert_eq!(s.hit_test(0, px(10), px(10)), Some(id(2)));
}

#[test]
fn later_sibling_wins_within_layer() {
    let mut s = SceneStore::new();
    s.apply_op(
        0,
        &SceneOp::Insert(aabb_node(id(1), Layer::Widget, 0, 0, 50, 50)),
    );
    s.apply_op(
        0,
        &SceneOp::Insert(aabb_node(id(2), Layer::Widget, 0, 0, 50, 50)),
    );
    // Later-inserted = higher z. (2) wins.
    assert_eq!(s.hit_test(0, px(10), px(10)), Some(id(2)));
}

#[test]
fn invisible_node_skipped() {
    let mut s = SceneStore::new();
    let mut n = aabb_node(id(1), Layer::Widget, 0, 0, 50, 50);
    n.style.visible = false;
    s.apply_op(0, &SceneOp::Insert(n));
    assert_eq!(s.hit_test(0, px(10), px(10)), None);
}

#[test]
fn zero_opacity_skipped() {
    let mut s = SceneStore::new();
    let mut n = aabb_node(id(1), Layer::Widget, 0, 0, 50, 50);
    n.style.opacity = 0;
    s.apply_op(0, &SceneOp::Insert(n));
    assert_eq!(s.hit_test(0, px(10), px(10)), None);
}

#[test]
fn circle_hit_inside_radius() {
    let mut s = SceneStore::new();
    let n = Node {
        id: id(1),
        layer: Layer::Widget,
        transform: Transform::translate(px(100), px(100)),
        primitive: Primitive::Circle {
            radius_q16: 50u32 << 16,
        },
        style: Style::filled(Rgba::WHITE),
        input: Some(InputRegion {
            shape: HitShape::Circle {
                radius_q16: 50u32 << 16,
            },
            cursor_hint: CursorHint::Pointer,
            capture: false,
        }),
    };
    s.apply_op(0, &SceneOp::Insert(n));
    // Center: hits.
    assert_eq!(s.hit_test(0, px(100), px(100)), Some(id(1)));
    // 30 px from center along x: inside.
    assert_eq!(s.hit_test(0, px(130), px(100)), Some(id(1)));
    // 60 px from center: outside.
    assert_eq!(s.hit_test(0, px(160), px(100)), None);
}

#[test]
fn non_interactive_node_ignored() {
    let mut s = SceneStore::new();
    let mut n = aabb_node(id(1), Layer::Widget, 0, 0, 50, 50);
    n.input = None;
    s.apply_op(0, &SceneOp::Insert(n));
    assert_eq!(s.hit_test(0, px(10), px(10)), None);
}

#[test]
fn path_hitshape_is_no_hit_v1() {
    let mut s = SceneStore::new();
    let n = Node {
        id: id(1),
        layer: Layer::Widget,
        transform: Transform::IDENTITY,
        primitive: Primitive::Rect {
            w: px(50),
            h: px(50),
            radius_q8: 0,
        },
        style: Style::filled(Rgba::WHITE),
        input: Some(InputRegion {
            // v1.1 behaviour: hit-test always misses Path.
            shape: HitShape::Path(alloc_owned_vec()),
            cursor_hint: CursorHint::Pointer,
            capture: false,
        }),
    };
    s.apply_op(0, &SceneOp::Insert(n));
    assert_eq!(s.hit_test(0, px(10), px(10)), None);
}

// Helper to avoid pulling alloc explicitly in test.
fn alloc_owned_vec() -> Vec<weftos_leaf_scene::PathCmd> {
    Vec::new()
}
