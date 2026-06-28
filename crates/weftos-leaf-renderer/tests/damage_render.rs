//! Integration tests for `render_damage`.
//!
//! Drives the renderer against a `SurfaceMock` and verifies the damage
//! walk, AABB filtering, layer order, opacity-skip behaviour. These
//! complement the unit tests in `src/render.rs` by exercising the
//! crate's public re-exports rather than `crate::*` shortcuts.

use weftos_leaf_renderer::{render_damage, CapabilityMask, SceneSurface};
use weftos_leaf_scene::{
    px, BlendMode, BuiltinFont, DamageSet, FontFace, KerningHint, Layer, Node, NodeId, Primitive,
    Rect, Rgba, SceneOp, SceneStore, Style, Transform,
};

#[derive(Debug, Default)]
struct SurfaceMock {
    caps: CapabilityMask,
    begin_count: u32,
    end_count: u32,
    last_viewport: Option<Rect>,
    draws: Vec<(Primitive, Style, Transform)>,
}

impl SceneSurface for SurfaceMock {
    type Error = &'static str;
    fn capabilities(&self) -> CapabilityMask {
        self.caps
    }
    fn begin_frame(&mut self, _damage: &DamageSet, viewport: Rect) -> Result<(), Self::Error> {
        self.begin_count += 1;
        self.last_viewport = Some(viewport);
        Ok(())
    }
    fn draw_primitive(
        &mut self,
        p: &Primitive,
        s: &Style,
        t: &Transform,
    ) -> Result<(), Self::Error> {
        self.draws.push((p.clone(), s.clone(), *t));
        Ok(())
    }
    fn end_frame(&mut self) -> Result<(), Self::Error> {
        self.end_count += 1;
        Ok(())
    }
}

fn nid(d: u8, n: u32) -> NodeId {
    NodeId::from_parts(d, n)
}

fn rect_node(id: NodeId, layer: Layer, x: i32, y: i32, w: i32, h: i32) -> Node {
    Node {
        id,
        layer,
        transform: Transform::translate(px(x), px(y)),
        primitive: Primitive::Rect {
            w: px(w),
            h: px(h),
            radius_q8: 0,
        },
        style: Style::filled(Rgba::RED),
        input: None,
    }
}

#[test]
fn scissoring_filters_to_overlapping_nodes_only() {
    let mut store = SceneStore::new();
    store.set_viewport(0, Rect::from_px(0, 0, 800, 480));
    // Three nodes spread across the display.
    store.apply_op(0, &SceneOp::Insert(rect_node(nid(0, 1), Layer::Widget, 0, 0, 20, 20)));
    store.apply_op(0, &SceneOp::Insert(rect_node(nid(0, 2), Layer::Widget, 100, 100, 20, 20)));
    store.apply_op(0, &SceneOp::Insert(rect_node(nid(0, 3), Layer::Widget, 500, 300, 20, 20)));

    // Damage rect covers only the first two.
    let mut damage = DamageSet::none();
    damage.add(Rect::from_px(0, 0, 150, 150), Rect::from_px(0, 0, 800, 480));

    let mut s = SurfaceMock::default();
    let stats = render_damage(&store, 0, &damage, &mut s).unwrap();
    assert_eq!(stats.drawn, 2);
    assert_eq!(stats.skipped_offscreen, 1);
}

#[test]
fn aabb_intersection_is_inclusive_on_overlap() {
    let mut store = SceneStore::new();
    store.set_viewport(0, Rect::from_px(0, 0, 800, 480));
    // Node at (10..30), damage at (20..40) -> overlap by (20..30).
    store.apply_op(0, &SceneOp::Insert(rect_node(nid(0, 1), Layer::Widget, 10, 10, 20, 20)));

    let mut damage = DamageSet::none();
    damage.add(Rect::from_px(20, 20, 20, 20), Rect::from_px(0, 0, 800, 480));

    let mut s = SurfaceMock::default();
    let stats = render_damage(&store, 0, &damage, &mut s).unwrap();
    assert_eq!(stats.drawn, 1);
}

#[test]
fn aabb_touching_edges_do_not_count_as_intersection() {
    let mut store = SceneStore::new();
    store.set_viewport(0, Rect::from_px(0, 0, 800, 480));
    // Node at (0..10), damage at (10..30) -> edges touch only.
    store.apply_op(0, &SceneOp::Insert(rect_node(nid(0, 1), Layer::Widget, 0, 0, 10, 10)));

    let mut damage = DamageSet::none();
    damage.add(Rect::from_px(10, 0, 20, 20), Rect::from_px(0, 0, 800, 480));

    let mut s = SurfaceMock::default();
    let stats = render_damage(&store, 0, &damage, &mut s).unwrap();
    // Half-open: touching does not intersect (matches Phase A semantics).
    assert_eq!(stats.drawn, 0);
}

#[test]
fn layer_order_bg_widget_text_alert() {
    let mut store = SceneStore::new();
    store.set_viewport(0, Rect::from_px(0, 0, 800, 480));

    // Declaration order: Alert, Text, Widget, Bg. After layer sort:
    // Bg, Widget, Text, Alert. Verify the surface sees that order.
    let mut alert = rect_node(nid(0, 1), Layer::Alert, 0, 0, 5, 5);
    alert.style.fill = Some(Rgba::new(0xAA, 0xAA, 0xAA, 0xFF));
    let mut text = rect_node(nid(0, 2), Layer::Text, 0, 0, 5, 5);
    text.style.fill = Some(Rgba::new(0xBB, 0xBB, 0xBB, 0xFF));
    let mut widget = rect_node(nid(0, 3), Layer::Widget, 0, 0, 5, 5);
    widget.style.fill = Some(Rgba::new(0xCC, 0xCC, 0xCC, 0xFF));
    let mut bg = rect_node(nid(0, 4), Layer::Bg, 0, 0, 5, 5);
    bg.style.fill = Some(Rgba::new(0xDD, 0xDD, 0xDD, 0xFF));

    for n in [alert, text, widget, bg] {
        store.apply_op(0, &SceneOp::Insert(n));
    }

    let mut s = SurfaceMock::default();
    let _ = render_damage(&store, 0, &DamageSet::full(), &mut s).unwrap();
    assert_eq!(s.draws.len(), 4);
    // Bg (0xDD), Widget (0xCC), Text (0xBB), Alert (0xAA).
    assert_eq!(s.draws[0].1.fill.unwrap().r, 0xDD);
    assert_eq!(s.draws[1].1.fill.unwrap().r, 0xCC);
    assert_eq!(s.draws[2].1.fill.unwrap().r, 0xBB);
    assert_eq!(s.draws[3].1.fill.unwrap().r, 0xAA);
}

#[test]
fn zero_opacity_is_skipped_outright() {
    let mut store = SceneStore::new();
    store.set_viewport(0, Rect::from_px(0, 0, 800, 480));
    let mut transparent = rect_node(nid(0, 1), Layer::Widget, 0, 0, 10, 10);
    transparent.style.opacity = 0;
    store.apply_op(0, &SceneOp::Insert(transparent));

    let mut s = SurfaceMock::default();
    let stats = render_damage(&store, 0, &DamageSet::full(), &mut s).unwrap();
    assert_eq!(stats.drawn, 0);
    assert_eq!(stats.skipped_invisible, 1);
}

#[test]
fn full_repaint_skips_aabb_filter() {
    let mut store = SceneStore::new();
    store.set_viewport(0, Rect::from_px(0, 0, 800, 480));
    // Node off-damage-rect — would normally be filtered out.
    store.apply_op(
        0,
        &SceneOp::Insert(rect_node(nid(0, 1), Layer::Widget, 500, 300, 20, 20)),
    );

    let mut s = SurfaceMock::default();
    let _ = render_damage(&store, 0, &DamageSet::full(), &mut s).unwrap();
    assert_eq!(s.draws.len(), 1, "full damage should bypass AABB filter");
}

#[test]
fn viewport_passed_to_begin_frame() {
    let mut store = SceneStore::new();
    store.set_viewport(0, Rect::from_px(0, 0, 800, 480));
    let mut s = SurfaceMock::default();
    let _ = render_damage(&store, 0, &DamageSet::full(), &mut s).unwrap();
    assert_eq!(s.last_viewport, Some(Rect::from_px(0, 0, 800, 480)));
}

#[test]
fn text_primitive_routes_through() {
    let mut store = SceneStore::new();
    store.set_viewport(0, Rect::from_px(0, 0, 800, 480));
    let node = Node {
        id: nid(0, 7),
        layer: Layer::Text,
        transform: Transform::translate(px(10), px(20)),
        primitive: Primitive::Text {
            content: String::from("hello"),
            face: FontFace::Builtin(BuiltinFont::Mono6x10),
            size_q8: 10 << 8,
            weight: 400,
            kerning: KerningHint::Auto,
        },
        style: Style::filled(Rgba::WHITE),
        input: None,
    };
    store.apply_op(0, &SceneOp::Insert(node));

    let mut s = SurfaceMock::default();
    let _ = render_damage(&store, 0, &DamageSet::full(), &mut s).unwrap();
    assert_eq!(s.draws.len(), 1);
    match &s.draws[0].0 {
        Primitive::Text { content, .. } => assert_eq!(content, "hello"),
        other => panic!("expected Text, got {other:?}"),
    }
}

#[test]
fn layer_blend_mode_does_not_change_dispatch_when_missing_capability() {
    // Surface without BLEND_MODES; layer with Multiply. Renderer
    // should still dispatch the primitive — backend degrades to Normal
    // silently.
    let mut store = SceneStore::new();
    store.set_viewport(0, Rect::from_px(0, 0, 800, 480));
    store.apply_op(0, &SceneOp::Insert(rect_node(nid(0, 1), Layer::Widget, 0, 0, 10, 10)));
    store.apply_op(
        0,
        &SceneOp::SetLayerBlend {
            layer: Layer::Widget,
            mode: BlendMode::Multiply,
        },
    );

    let mut s = SurfaceMock {
        caps: CapabilityMask::empty(),
        ..Default::default()
    };
    let stats = render_damage(&store, 0, &DamageSet::full(), &mut s).unwrap();
    assert_eq!(stats.drawn, 1);
}
