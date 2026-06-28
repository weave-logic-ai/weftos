//! End-to-end smoke: build a multi-node scene + render via `SimSurface`,
//! verify the pixel buffer contains non-default content.
//!
//! Mirrors the shape of `examples/boot.rs` but stays headless (no
//! window), so it runs in every CI environment regardless of SDL2.

use embedded_graphics::{geometry::Point as EgPoint, pixelcolor::Rgb888};
use weftos_leaf_renderer::render_damage;
use weftos_leaf_scene::{
    px, BuiltinFont, DamageSet, FontFace, KerningHint, Layer, Node, NodeId, Primitive, Rect, Rgba,
    SceneOp, SceneStore, Style, Transform,
};
use weftos_leaf_sim::SimSurface;

fn nid(d: u8, n: u32) -> NodeId {
    NodeId::from_parts(d, n)
}

#[test]
fn boot_scene_renders_without_panic() {
    let mut store = SceneStore::new();
    store.set_viewport(0, Rect::from_px(0, 0, 800, 480));
    store.set_bg(0, Rgba::opaque(0x10, 0x12, 0x18));

    // Header rect — Widget layer.
    store.apply_op(
        0,
        &SceneOp::Insert(Node {
            id: nid(0, 1),
            layer: Layer::Widget,
            transform: Transform::translate(px(0), px(0)),
            primitive: Primitive::Rect {
                w: px(800),
                h: px(40),
                radius_q8: 0,
            },
            style: Style::filled(Rgba::opaque(0x22, 0x55, 0x99)),
            input: None,
        }),
    );
    // Header title — Text layer.
    store.apply_op(
        0,
        &SceneOp::Insert(Node {
            id: nid(0, 2),
            layer: Layer::Text,
            transform: Transform::translate(px(12), px(10)),
            primitive: Primitive::Text {
                content: String::from("smoke"),
                face: FontFace::Builtin(BuiltinFont::Mono10x20),
                size_q8: 20 << 8,
                weight: 400,
                kerning: KerningHint::Auto,
            },
            style: Style::filled(Rgba::WHITE),
            input: None,
        }),
    );

    let mut surface = SimSurface::new(800, 480, "smoke");
    surface.set_clear_color(Rgba::opaque(0x10, 0x12, 0x18));
    let stats = render_damage(&store, 0, &DamageSet::full(), &mut surface).expect("render");
    assert_eq!(stats.drawn, 2);

    // Sample a pixel in the middle of the header bar — should be the
    // header colour (0x22, 0x55, 0x99), not the clear colour.
    let p = surface.display().get_pixel(EgPoint::new(400, 20));
    assert_eq!(p, Rgb888::new(0x22, 0x55, 0x99), "header bar pixel");

    // Sample a pixel well outside the header — should be the clear colour.
    let bg = surface.display().get_pixel(EgPoint::new(400, 300));
    assert_eq!(bg, Rgb888::new(0x10, 0x12, 0x18), "background pixel");
}

#[test]
fn partial_damage_preserves_undamaged_pixels() {
    // Frame 1: render full-screen rect.
    let mut store = SceneStore::new();
    store.set_viewport(0, Rect::from_px(0, 0, 800, 480));
    store.apply_op(
        0,
        &SceneOp::Insert(Node {
            id: nid(0, 1),
            layer: Layer::Widget,
            transform: Transform::translate(px(0), px(0)),
            primitive: Primitive::Rect {
                w: px(800),
                h: px(480),
                radius_q8: 0,
            },
            style: Style::filled(Rgba::RED),
            input: None,
        }),
    );

    let mut surface = SimSurface::new(800, 480, "smoke");
    surface.set_clear_color(Rgba::BLACK);
    let _ = render_damage(&store, 0, &DamageSet::full(), &mut surface).expect("frame 1");
    // Whole display should be red.
    assert_eq!(
        surface.display().get_pixel(EgPoint::new(400, 200)),
        Rgb888::new(0xFF, 0, 0)
    );

    // Frame 2: insert a small green node, render with partial damage
    // covering just that node. The rest of the buffer (red) must
    // survive — that's the load-bearing partial-damage guarantee.
    store.apply_op(
        0,
        &SceneOp::Insert(Node {
            id: nid(0, 2),
            layer: Layer::Widget,
            transform: Transform::translate(px(10), px(10)),
            primitive: Primitive::Rect {
                w: px(20),
                h: px(20),
                radius_q8: 0,
            },
            style: Style::filled(Rgba::GREEN),
            input: None,
        }),
    );
    let mut damage = DamageSet::none();
    damage.add(Rect::from_px(10, 10, 20, 20), Rect::from_px(0, 0, 800, 480));

    let _ = render_damage(&store, 0, &damage, &mut surface).expect("frame 2");
    // The small region is green.
    assert_eq!(
        surface.display().get_pixel(EgPoint::new(15, 15)),
        Rgb888::new(0, 0xFF, 0)
    );
    // The rest is still red (we did NOT clear on partial damage).
    assert_eq!(
        surface.display().get_pixel(EgPoint::new(400, 200)),
        Rgb888::new(0xFF, 0, 0)
    );
}

#[test]
fn full_damage_clears_to_configured_color() {
    let mut store = SceneStore::new();
    store.set_viewport(0, Rect::from_px(0, 0, 100, 100));
    // No nodes — just verify the clear path.
    let mut surface = SimSurface::new(100, 100, "smoke");
    surface.set_clear_color(Rgba::opaque(0x88, 0x44, 0x22));
    let _ = render_damage(&store, 0, &DamageSet::full(), &mut surface).expect("render");
    assert_eq!(
        surface.display().get_pixel(EgPoint::new(50, 50)),
        Rgb888::new(0x88, 0x44, 0x22)
    );
}
