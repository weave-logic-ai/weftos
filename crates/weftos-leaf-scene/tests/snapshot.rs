//! `to_snapshot` + `Replace(Scene)` round-trip. Producer policy says
//! "snapshot on connect + every 5 s in steady state"; this test
//! confirms the materialization is faithful enough to support that.

use weftos_leaf_scene::{
    geometry::{px, Rect, Transform},
    primitive::{BlendMode, Layer, Primitive, Style},
    Node, NodeId, Rgba, Scene, SceneOp, SceneStore,
};

fn id(d: u8, n: u32) -> NodeId {
    NodeId::from_parts(d, n)
}

fn rect_node(node_id: NodeId, x: i32, y: i32, w: i32, h: i32) -> Node {
    Node {
        id: node_id,
        layer: Layer::Widget,
        transform: Transform::translate(px(x), px(y)),
        primitive: Primitive::Rect {
            w: px(w),
            h: px(h),
            radius_q8: 0,
        },
        style: Style::filled(Rgba::WHITE),
        input: None,
    }
}

#[test]
fn empty_store_snapshot_is_empty_scene() {
    let s = SceneStore::new();
    let snap = s.to_snapshot(0);
    assert_eq!(snap, Scene::empty(0));
}

#[test]
fn snapshot_preserves_bg_viewport_and_blends() {
    let mut s = SceneStore::new();
    s.set_viewport(0, Rect::from_px(0, 0, 800, 480));
    s.set_bg(0, Rgba::opaque(1, 2, 3));
    s.apply_op(
        0,
        &SceneOp::SetLayerBlend {
            layer: Layer::Text,
            mode: BlendMode::Multiply,
        },
    );

    let snap = s.to_snapshot(0);
    assert_eq!(snap.bg, Rgba::opaque(1, 2, 3));
    assert_eq!(snap.viewport, Rect::from_px(0, 0, 800, 480));
    assert_eq!(snap.layer_blend[Layer::Text.index()], BlendMode::Multiply);
}

#[test]
fn snapshot_preserves_z_order() {
    let mut s = SceneStore::new();
    s.apply_op(0, &SceneOp::Insert(rect_node(id(0, 1), 0, 0, 10, 10)));
    s.apply_op(0, &SceneOp::Insert(rect_node(id(0, 2), 10, 0, 10, 10)));
    s.apply_op(0, &SceneOp::Insert(rect_node(id(0, 3), 20, 0, 10, 10)));

    let snap = s.to_snapshot(0);
    let order: Vec<u32> = snap.nodes.iter().map(|n| n.id.path_hash()).collect();
    assert_eq!(order, vec![1, 2, 3]);
}

#[test]
fn replace_then_snapshot_roundtrip() {
    let mut a = SceneStore::new();
    a.set_viewport(0, Rect::from_px(0, 0, 800, 480));
    a.set_bg(0, Rgba::opaque(10, 20, 30));
    a.apply_op(0, &SceneOp::Insert(rect_node(id(0, 7), 100, 100, 20, 20)));
    a.apply_op(0, &SceneOp::Insert(rect_node(id(0, 8), 200, 100, 20, 20)));

    let snap_a = a.to_snapshot(0);

    let mut b = SceneStore::new();
    let dmg = b.apply_op(0, &SceneOp::Replace(snap_a.clone()));
    assert!(dmg.is_full());
    let snap_b = b.to_snapshot(0);

    assert_eq!(snap_a, snap_b);
}

#[test]
fn snapshot_includes_only_target_display() {
    let mut s = SceneStore::new();
    s.apply_op(0, &SceneOp::Insert(rect_node(id(0, 1), 0, 0, 10, 10)));
    s.apply_op(1, &SceneOp::Insert(rect_node(id(1, 1), 0, 0, 10, 10)));

    let snap_zero = s.to_snapshot(0);
    let snap_one = s.to_snapshot(1);

    assert_eq!(snap_zero.nodes.len(), 1);
    assert_eq!(snap_one.nodes.len(), 1);
    assert_eq!(snap_zero.nodes[0].id.display_id(), 0);
    assert_eq!(snap_one.nodes[0].id.display_id(), 1);
}
