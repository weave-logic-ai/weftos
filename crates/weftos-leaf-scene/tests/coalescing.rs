//! Tween coalescing — design doc §5.6 + op.rs contract.
//!
//! When a `Tween` op arrives for a (node, property) that already has
//! an active tween, the prior tween is cancelled. Only the newest
//! tween survives in the table. v1 then snap-completes everything on
//! tick.

use weftos_leaf_scene::{
    geometry::{px, Rect},
    primitive::{EaseCurve, Layer, Primitive, Style},
    AnimatableProperty, Node, NodeId, PropertyValue, Rgba, SceneOp, SceneStore,
};

fn id(n: u32) -> NodeId {
    NodeId::from_parts(0, n)
}

fn small_rect_node(id: NodeId) -> Node {
    Node {
        id,
        layer: Layer::Widget,
        transform: weftos_leaf_scene::Transform::IDENTITY,
        primitive: Primitive::Rect {
            w: px(20),
            h: px(20),
            radius_q8: 0,
        },
        style: Style::filled(Rgba::BLUE),
        input: None,
    }
}

#[test]
fn second_tween_on_same_property_replaces_first() {
    let mut store = SceneStore::new();
    store.set_viewport(0, Rect::from_px(0, 0, 800, 480));
    store.apply_op(0, &SceneOp::Insert(small_rect_node(id(1))));

    // First tween: opacity 0 → 100 over 500 ms.
    store.apply_op(
        0,
        &SceneOp::Tween {
            id: id(1),
            property: AnimatableProperty::Opacity,
            from: PropertyValue::Opacity(0),
            to: PropertyValue::Opacity(100),
            duration_ms: 500,
            start_at: None,
            curve: EaseCurve::Linear,
        },
    );
    let display = store.display(0).expect("display present");
    assert_eq!(display.tweens.len(), 1);

    // Second tween on the SAME (id, property): opacity → 255. Should
    // cancel the first and leave exactly one tween in flight.
    store.apply_op(
        0,
        &SceneOp::Tween {
            id: id(1),
            property: AnimatableProperty::Opacity,
            from: PropertyValue::Opacity(100),
            to: PropertyValue::Opacity(255),
            duration_ms: 300,
            start_at: None,
            curve: EaseCurve::Linear,
        },
    );
    let display = store.display(0).expect("display present");
    assert_eq!(display.tweens.len(), 1);

    // The active tween should target opacity=255.
    let active = display.tweens.active();
    match &active[0].to {
        PropertyValue::Opacity(v) => assert_eq!(*v, 255),
        other => panic!("unexpected variant: {other:?}"),
    }
}

#[test]
fn tweens_on_distinct_properties_coexist() {
    let mut store = SceneStore::new();
    store.apply_op(0, &SceneOp::Insert(small_rect_node(id(2))));

    store.apply_op(
        0,
        &SceneOp::Tween {
            id: id(2),
            property: AnimatableProperty::Opacity,
            from: PropertyValue::Opacity(0),
            to: PropertyValue::Opacity(255),
            duration_ms: 100,
            start_at: None,
            curve: EaseCurve::Linear,
        },
    );
    store.apply_op(
        0,
        &SceneOp::Tween {
            id: id(2),
            property: AnimatableProperty::Position,
            from: PropertyValue::Position(weftos_leaf_scene::Point::new(0, 0)),
            to: PropertyValue::Position(weftos_leaf_scene::Point::from_px(50, 50)),
            duration_ms: 200,
            start_at: None,
            curve: EaseCurve::Linear,
        },
    );

    assert_eq!(store.display(0).unwrap().tweens.len(), 2);
}

#[test]
fn cancel_tween_specific_property_only() {
    let mut store = SceneStore::new();
    store.apply_op(0, &SceneOp::Insert(small_rect_node(id(3))));

    for prop in [AnimatableProperty::Opacity, AnimatableProperty::Position] {
        let val = match prop {
            AnimatableProperty::Opacity => PropertyValue::Opacity(255),
            AnimatableProperty::Position => {
                PropertyValue::Position(weftos_leaf_scene::Point::new(0, 0))
            }
            _ => unreachable!(),
        };
        store.apply_op(
            0,
            &SceneOp::Tween {
                id: id(3),
                property: prop,
                from: val.clone(),
                to: val,
                duration_ms: 100,
                start_at: None,
                curve: EaseCurve::Linear,
            },
        );
    }
    assert_eq!(store.display(0).unwrap().tweens.len(), 2);

    store.apply_op(
        0,
        &SceneOp::CancelTween {
            id: id(3),
            property: Some(AnimatableProperty::Opacity),
        },
    );
    let active = store.display(0).unwrap().tweens.active();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].property, AnimatableProperty::Position);
}

#[test]
fn cancel_tween_all_for_node() {
    let mut store = SceneStore::new();
    store.apply_op(0, &SceneOp::Insert(small_rect_node(id(4))));

    for prop in [AnimatableProperty::Opacity, AnimatableProperty::Fill] {
        let val = match prop {
            AnimatableProperty::Opacity => PropertyValue::Opacity(255),
            AnimatableProperty::Fill => PropertyValue::Color(Rgba::BLUE),
            _ => unreachable!(),
        };
        store.apply_op(
            0,
            &SceneOp::Tween {
                id: id(4),
                property: prop,
                from: val.clone(),
                to: val,
                duration_ms: 100,
                start_at: None,
                curve: EaseCurve::Linear,
            },
        );
    }

    store.apply_op(
        0,
        &SceneOp::CancelTween {
            id: id(4),
            property: None,
        },
    );
    assert!(store.display(0).unwrap().tweens.is_empty());
}

#[test]
fn tick_snaps_position_tween() {
    let mut store = SceneStore::new();
    store.apply_op(0, &SceneOp::Insert(small_rect_node(id(5))));
    store.apply_op(
        0,
        &SceneOp::Tween {
            id: id(5),
            property: AnimatableProperty::Position,
            from: PropertyValue::Position(weftos_leaf_scene::Point::new(0, 0)),
            to: PropertyValue::Position(weftos_leaf_scene::Point::from_px(100, 50)),
            duration_ms: 200,
            start_at: None,
            curve: EaseCurve::Linear,
        },
    );
    // v1 tick snaps regardless of `now_ms` — any value works.
    let damage = store.tick(50);
    assert!(!damage.is_empty());

    let n = store.node(id(5)).expect("present");
    assert_eq!(n.transform.x, px(100));
    assert_eq!(n.transform.y, px(50));
    // Tween has been drained.
    assert!(store.display(0).unwrap().tweens.is_empty());
}

#[test]
fn tick_snaps_fill_color() {
    let mut store = SceneStore::new();
    store.apply_op(0, &SceneOp::Insert(small_rect_node(id(6))));
    store.apply_op(
        0,
        &SceneOp::Tween {
            id: id(6),
            property: AnimatableProperty::Fill,
            from: PropertyValue::Color(Rgba::BLUE),
            to: PropertyValue::Color(Rgba::RED),
            duration_ms: 500,
            start_at: None,
            curve: EaseCurve::Linear,
        },
    );
    store.tick(0);
    let n = store.node(id(6)).expect("present");
    assert_eq!(n.style.fill, Some(Rgba::RED));
}
