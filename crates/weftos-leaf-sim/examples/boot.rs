//! `boot` example — opens a window, builds a small scene, renders one
//! frame, and blocks until the user closes the window.
//!
//! Run with:
//!
//! ```bash
//! cd crates/weftos-leaf-sim
//! cargo run --release --example boot
//! ```
//!
//! Representative of a "leaf displays a header + a few text rows"
//! scene: matches the `kernel.ps` rendering shape that producers will
//! ship in Phase E.

use weftos_leaf_renderer::render_damage;
use weftos_leaf_scene::{
    px, BuiltinFont, FontFace, KerningHint, Layer, Node, NodeId, Primitive, Rect, Rgba, SceneOp,
    SceneStore, Style, Transform,
};
use weftos_leaf_sim::SimSurface;

fn nid(d: u8, n: u32) -> NodeId {
    NodeId::from_parts(d, n)
}

fn text_node(id: NodeId, x: i32, y: i32, content: &str, color: Rgba) -> Node {
    Node {
        id,
        layer: Layer::Text,
        transform: Transform::translate(px(x), px(y)),
        primitive: Primitive::Text {
            content: content.to_string(),
            face: FontFace::Builtin(BuiltinFont::Mono10x20),
            size_q8: 20 << 8,
            weight: 400,
            kerning: KerningHint::Auto,
        },
        style: Style::filled(color),
        input: None,
    }
}

fn rect_node(id: NodeId, x: i32, y: i32, w: i32, h: i32, color: Rgba) -> Node {
    Node {
        id,
        layer: Layer::Widget,
        transform: Transform::translate(px(x), px(y)),
        primitive: Primitive::Rect {
            w: px(w),
            h: px(h),
            radius_q8: 0,
        },
        style: Style::filled(color),
        input: None,
    }
}

fn main() {
    let mut store = SceneStore::new();
    store.set_viewport(0, Rect::from_px(0, 0, 800, 480));
    store.set_bg(0, Rgba::opaque(0x10, 0x12, 0x18));

    // Header bar — Widget layer, full-width.
    store.apply_op(
        0,
        &SceneOp::Insert(rect_node(nid(0, 1), 0, 0, 800, 40, Rgba::opaque(0x22, 0x55, 0x99))),
    );
    // Header title — Text layer, white.
    store.apply_op(
        0,
        &SceneOp::Insert(text_node(nid(0, 2), 12, 10, "WeftOS leaf — boot example", Rgba::WHITE)),
    );

    // A few status rows.
    let rows = ["agent.kernel", "service.weave", "leaf.dis08070h", "scene.store"];
    for (i, name) in rows.iter().enumerate() {
        let y = 60 + (i as i32) * 30;
        store.apply_op(
            0,
            &SceneOp::Insert(text_node(
                nid(0, 100 + i as u32),
                20,
                y,
                name,
                Rgba::opaque(0xE8, 0xE8, 0xE8),
            )),
        );
        store.apply_op(
            0,
            &SceneOp::Insert(text_node(
                nid(0, 200 + i as u32),
                300,
                y,
                "OK",
                Rgba::opaque(0x88, 0xCC, 0x88),
            )),
        );
    }

    // Footer line — Widget, full-width 2px line drawn via thin rect.
    store.apply_op(
        0,
        &SceneOp::Insert(rect_node(nid(0, 99), 0, 460, 800, 2, Rgba::opaque(0x44, 0x44, 0x55))),
    );

    let mut surface = SimSurface::new(800, 480, "WeftOS leaf — boot");
    surface.set_clear_color(Rgba::opaque(0x10, 0x12, 0x18));
    let stats = render_damage(&store, 0, &weftos_leaf_scene::DamageSet::full(), &mut surface)
        .expect("render_damage");
    eprintln!(
        "rendered {} primitives ({} invisible, {} offscreen)",
        stats.drawn, stats.skipped_invisible, stats.skipped_offscreen
    );

    // Headless mode: skip the window so CI / smoke runs return quickly.
    // Set `WEFTOS_SIM_HEADLESS=1` (or pass `--headless`) to enable.
    let headless = std::env::var_os("WEFTOS_SIM_HEADLESS").is_some()
        || std::env::args().any(|a| a == "--headless");
    if headless {
        eprintln!("headless mode — skipping window");
        return;
    }

    // Block until the window is closed. The window is opened via SDL2
    // (gated on the `window` feature). `show()` uses `show_static`,
    // which runs the event loop until the user closes the window.
    surface.show();
}
