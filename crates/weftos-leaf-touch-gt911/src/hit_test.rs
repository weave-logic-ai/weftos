//! Scene-aware hit-test: convert a [`TouchEvent`] into a fully-resolved
//! [`InputEnvelope`] ready to publish on `mesh.leaf.<pk>.input`.
//!
//! The driver-level [`TouchEvent`] carries integer-pixel coords and a
//! [`TouchPhase`]. This module:
//!
//! 1. Converts coordinates from integer pixels to Q24.8.
//! 2. Calls [`SceneStore::hit_test`] to resolve `(x_q8, y_q8) →
//!    Option<NodeId>`.
//! 3. Maps `TouchPhase` to the corresponding [`InputEvent`] variant.
//! 4. Wraps the result in an [`InputEnvelope`] tagged with the current
//!    wire version.
//!
//! Pressure is reported as `0` (unknown) — the GT911 doesn't expose
//! pressure data directly. v1.1 could derive a normalized value from
//! `TouchPoint::size` if a renderer wants to highlight harder presses.

use weftos_leaf_scene::{px, DisplayId, InputEnvelope, InputEvent, SceneStore};

use crate::driver::{TouchEvent, TouchPhase};

/// Convert a raw [`TouchEvent`] to a wire-ready [`InputEnvelope`].
///
/// Coordinates: the driver hands us integer-pixel `(x_px, y_px)`; we
/// shift to Q24.8 via [`px`] before calling [`SceneStore::hit_test`].
/// The hit-test result fills in the envelope's `node_id` slot — `None`
/// when the touch falls outside any interactive region.
pub fn hit_test_event(
    store: &SceneStore,
    display: DisplayId,
    event: TouchEvent,
) -> InputEnvelope {
    let x_q8 = px(event.x_px);
    let y_q8 = px(event.y_px);
    // Up events are emitted from a stale (0, 0) — they always
    // hit-test to `None`, which is what we want: an Up event is
    // routed by track-id, not by position. Down + Move events get a
    // real hit-test.
    let node_id = match event.phase {
        TouchPhase::Up => None,
        _ => store.hit_test(display, x_q8, y_q8),
    };
    let input_event = match event.phase {
        TouchPhase::Down => InputEvent::PointerDown {
            pointer_id: event.pointer_id,
            x: x_q8,
            y: y_q8,
            // GT911 doesn't surface pressure; report 0 = unknown per
            // the InputEvent::PointerDown docs.
            pressure_q8: 0,
        },
        TouchPhase::Move => InputEvent::PointerMove {
            pointer_id: event.pointer_id,
            x: x_q8,
            y: y_q8,
            pressure_q8: 0,
        },
        TouchPhase::Up => InputEvent::PointerUp {
            pointer_id: event.pointer_id,
            x: x_q8,
            y: y_q8,
        },
    };
    InputEnvelope::new(display, node_id, input_event)
}

#[cfg(test)]
mod tests {
    use super::*;
    use weftos_leaf_scene::{
        CursorHint, HitShape, InputRegion, Layer, Node, NodeId, Primitive, Rect, Rgba, SceneOp,
        Style, Transform,
    };

    fn make_button(id: u32, x_px: i32, y_px: i32, w_px: i32, h_px: i32) -> Node {
        Node {
            id: NodeId::from_parts(0, id),
            layer: Layer::Widget,
            transform: Transform::translate(px(x_px), px(y_px)),
            primitive: Primitive::Rect {
                w: px(w_px),
                h: px(h_px),
                radius_q8: 0,
            },
            style: Style {
                fill: Some(Rgba::BLUE),
                stroke: None,
                stroke_width_q8: 0,
                opacity: 255,
                visible: true,
            },
            input: Some(InputRegion {
                shape: HitShape::Aabb {
                    w: px(w_px),
                    h: px(h_px),
                },
                cursor_hint: CursorHint::Pointer,
                capture: false,
            }),
        }
    }

    #[test]
    fn down_inside_button_resolves_node_id() {
        let mut store = SceneStore::new();
        store.set_viewport(0, Rect::from_px(0, 0, 800, 480));
        store.apply_op(0, &SceneOp::Insert(make_button(1, 100, 100, 80, 40)));

        let ev = TouchEvent {
            phase: TouchPhase::Down,
            pointer_id: 0,
            x_px: 120,
            y_px: 120,
            size: 10,
        };
        let env = hit_test_event(&store, 0, ev);
        assert!(env.node_id.is_some(), "press inside button must hit");
        assert_eq!(env.node_id.unwrap().path_hash(), 1);
        match env.event {
            InputEvent::PointerDown { x, y, .. } => {
                assert_eq!(x, px(120));
                assert_eq!(y, px(120));
            }
            _ => panic!("expected PointerDown"),
        }
    }

    #[test]
    fn down_outside_button_yields_no_node() {
        let mut store = SceneStore::new();
        store.set_viewport(0, Rect::from_px(0, 0, 800, 480));
        store.apply_op(0, &SceneOp::Insert(make_button(1, 100, 100, 80, 40)));

        let ev = TouchEvent {
            phase: TouchPhase::Down,
            pointer_id: 0,
            x_px: 10,
            y_px: 10,
            size: 0,
        };
        let env = hit_test_event(&store, 0, ev);
        assert!(env.node_id.is_none(), "press outside button must miss");
    }

    #[test]
    fn up_events_skip_hit_test() {
        let mut store = SceneStore::new();
        store.set_viewport(0, Rect::from_px(0, 0, 800, 480));
        store.apply_op(0, &SceneOp::Insert(make_button(1, 100, 100, 80, 40)));

        let ev = TouchEvent {
            phase: TouchPhase::Up,
            pointer_id: 0,
            x_px: 120,
            y_px: 120, // would hit if we were testing
            size: 0,
        };
        let env = hit_test_event(&store, 0, ev);
        assert!(env.node_id.is_none(), "Up always reports node_id=None");
        assert!(matches!(env.event, InputEvent::PointerUp { .. }));
    }

    #[test]
    fn move_event_resolves_to_input_move() {
        let store = SceneStore::new();
        let ev = TouchEvent {
            phase: TouchPhase::Move,
            pointer_id: 2,
            x_px: 50,
            y_px: 60,
            size: 0,
        };
        let env = hit_test_event(&store, 0, ev);
        match env.event {
            InputEvent::PointerMove {
                pointer_id, x, y, ..
            } => {
                assert_eq!(pointer_id, 2);
                assert_eq!(x, px(50));
                assert_eq!(y, px(60));
            }
            _ => panic!("expected PointerMove"),
        }
    }
}
