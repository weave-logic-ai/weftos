//! `tween_demo` example — demonstrates an applied tween. In v1 this
//! snaps to the `to` value on first tick (the design doc's v1
//! behaviour, §5.6); the window updates once to show the post-tween
//! state. v1.1 will animate the same envelope smoothly without any
//! producer-side change.
//!
//! Run with:
//!
//! ```bash
//! cd crates/weftos-leaf-sim
//! cargo run --release --example tween_demo
//! ```

use weftos_leaf_renderer::render_damage;
use weftos_leaf_scene::{
    px, AnimatableProperty, BuiltinFont, EaseCurve, FontFace, KerningHint, Layer, Node, NodeId,
    Point, Primitive, PropertyValue, Rect, Rgba, SceneOp, SceneStore, Style, Transform,
};
use weftos_leaf_sim::SimSurface;

fn main() {
    let mut store = SceneStore::new();
    store.set_viewport(0, Rect::from_px(0, 0, 800, 480));
    store.set_bg(0, Rgba::opaque(0x08, 0x08, 0x0C));

    // Title.
    store.apply_op(
        0,
        &SceneOp::Insert(Node {
            id: NodeId::from_parts(0, 1),
            layer: Layer::Text,
            transform: Transform::translate(px(20), px(20)),
            primitive: Primitive::Text {
                content: String::from("tween_demo — snaps in v1, animates in v1.1"),
                face: FontFace::Builtin(BuiltinFont::Mono10x20),
                size_q8: 20 << 8,
                weight: 400,
                kerning: KerningHint::Auto,
            },
            style: Style::filled(Rgba::WHITE),
            input: None,
        }),
    );

    // The moving rect: starts at (40, 80).
    let rect_id = NodeId::from_parts(0, 2);
    store.apply_op(
        0,
        &SceneOp::Insert(Node {
            id: rect_id,
            layer: Layer::Widget,
            transform: Transform::translate(px(40), px(80)),
            primitive: Primitive::Rect {
                w: px(80),
                h: px(80),
                radius_q8: 0,
            },
            style: Style::filled(Rgba::opaque(0xCC, 0x33, 0x66)),
            input: None,
        }),
    );

    // Tween: move from (40, 80) -> (600, 300) over 500 ms.
    store.apply_op(
        0,
        &SceneOp::Tween {
            id: rect_id,
            property: AnimatableProperty::Position,
            from: PropertyValue::Position(Point::from_px(40, 80)),
            to: PropertyValue::Position(Point::from_px(600, 300)),
            duration_ms: 500,
            start_at: None,
            curve: EaseCurve::EaseInOut,
        },
    );

    // Tick the store — v1 snaps the tween to `to`. v1.1 will leave
    // the tween mid-animation here and ship a per-frame `tick` loop;
    // the producer envelope (above) stays unchanged.
    let _post_tick_damage = store.tick(0);

    // After the tick, the rect's position is the tween's `to` value.
    let final_node = store.node(rect_id).expect("rect still present");
    eprintln!(
        "after tick: rect.transform = ({}, {})  (px = ({}, {}))",
        final_node.transform.x,
        final_node.transform.y,
        weftos_leaf_scene::from_px_q8(final_node.transform.x),
        weftos_leaf_scene::from_px_q8(final_node.transform.y),
    );

    let mut surface = SimSurface::new(800, 480, "WeftOS leaf — tween_demo");
    surface.set_clear_color(Rgba::opaque(0x08, 0x08, 0x0C));
    let stats = render_damage(&store, 0, &weftos_leaf_scene::DamageSet::full(), &mut surface)
        .expect("render_damage");
    eprintln!(
        "rendered {} primitives (note: v1 snaps tweens; v1.1 will animate this)",
        stats.drawn
    );

    let headless = std::env::var_os("WEFTOS_SIM_HEADLESS").is_some()
        || std::env::args().any(|a| a == "--headless");
    if headless {
        eprintln!("headless mode — skipping window");
        return;
    }

    surface.show();
}
