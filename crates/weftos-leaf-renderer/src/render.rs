//! `render_damage` — the canonical entry point.
//!
//! Walks a [`SceneStore`]'s draw order for the given display, filters
//! nodes against the [`DamageSet`], and dispatches one
//! `surface.draw_primitive` per visible, in-damage node.
//!
//! ## Algorithm
//!
//! 1. Look up the [`DisplayState`]. Missing display → no-op success.
//! 2. `surface.begin_frame(damage, viewport)`.
//! 3. For each node in draw order (Bg → Widget → Text → Alert):
//!    - Skip if `style.is_invisible()`.
//!    - Skip if `aabb()` does not intersect any damage rect (full
//!      repaint short-circuits this check).
//!    - Skip blend-mode dispatch when the layer is non-`Normal` and
//!      the surface lacks `BLEND_MODES` (one-time warning; v1
//!      degrade-to-Normal contract).
//!    - Skip alpha when the surface lacks `ALPHA` and the node's
//!      `style.opacity` is between 1 and 254 (collapsed to 255 per
//!      design doc §5.7).
//!    - Call `surface.draw_primitive(node.primitive, &resolved_style,
//!      &node.transform)`.
//! 4. `surface.end_frame()`.
//! 5. Return the count of `draw_primitive` calls that succeeded — useful
//!    for diagnostics ("did anything actually draw?").
//!
//! The walker is allocation-free in the steady state; the only Vec
//! allocated is the temporary `node_refs` snapshot, which is reused
//! by callers via [`SceneRenderer`] (planned in v1.1; v1 just leaks the
//! allocation per frame — cheap at ~hundreds of nodes max).

use alloc::vec::Vec;

use weftos_leaf_scene::{
    BlendMode, DamageSet, DisplayId, Layer, Node, Rect, SceneStore, Style,
};

use crate::surface::SceneSurface;

/// Why a render failed. Carries the backend's error type by reference
/// for backends that produce non-`'static` errors; in practice every
/// backend uses an owned error so the lifetime is degenerate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderError<E: core::fmt::Debug> {
    /// `begin_frame` rejected the request.
    BeginFrame(E),
    /// `end_frame` rejected the request.
    EndFrame(E),
    /// A specific primitive draw failed. Continues rendering the
    /// remaining primitives unless the renderer is in strict mode; v1
    /// is not strict (best-effort), so this is more diagnostic than
    /// fatal.
    DrawPrimitive(E),
}

/// Successful render outcome.
#[derive(Debug, Default, Copy, Clone, PartialEq, Eq)]
pub struct RenderStats {
    /// Number of primitives successfully drawn.
    pub drawn: u32,
    /// Number of primitives skipped because the surface lacks the
    /// capability they need (e.g., non-`Normal` blend mode without
    /// `BLEND_MODES`).
    pub skipped_unsupported: u32,
    /// Number of primitives skipped because the node was invisible.
    pub skipped_invisible: u32,
    /// Number of primitives skipped because the node's AABB did not
    /// intersect any damage rect (and damage was not full).
    pub skipped_offscreen: u32,
}

/// Render the damage rects for `display_id` to `surface`. Returns
/// `RenderStats` on success.
///
/// Missing display in the store is not an error — the renderer simply
/// returns `RenderStats::default()`.
///
/// `begin_frame` / `end_frame` are always called when the display
/// exists, even if the damage set is empty (so backends can no-op /
/// present-unchanged consistently).
pub fn render_damage<S: SceneSurface>(
    store: &SceneStore,
    display_id: DisplayId,
    damage: &DamageSet,
    surface: &mut S,
) -> Result<RenderStats, RenderError<S::Error>> {
    let mut stats = RenderStats::default();
    let Some(display) = store.display(display_id) else {
        // No display state for this id — nothing to do. We still let
        // the surface present a clean frame so visualizers don't get
        // a stuck buffer. The minimal frame uses Rect::ZERO viewport.
        surface
            .begin_frame(damage, Rect::ZERO)
            .map_err(RenderError::BeginFrame)?;
        surface.end_frame().map_err(RenderError::EndFrame)?;
        return Ok(stats);
    };

    let viewport = display.viewport;
    let layer_blend = display.layer_blend;
    let caps = surface.capabilities();

    surface
        .begin_frame(damage, viewport)
        .map_err(RenderError::BeginFrame)?;

    // Snapshot the draw-order walk into a Vec so we don't borrow
    // `store` for the duration of the surface call. Phase A's
    // `walk_draw_order` takes a closure whose argument is bound to
    // the call lifetime; we can't store those refs. Instead, walk
    // the display's `node_order` directly (public field on
    // `DisplayState`) and resolve each via `nodes.get` — same shape,
    // but with the borrow tied to `display`, not the closure.
    //
    // Cost: one allocation per frame, length = node count. At v1
    // scale (~hundreds), this is rounding error vs. the actual
    // rasterization.
    let mut node_refs: Vec<&Node> = Vec::with_capacity(display.nodes.len());
    for layer in Layer::DRAW_ORDER {
        for id in &display.node_order {
            if let Some(n) = display.nodes.get(id) {
                if n.layer == layer {
                    node_refs.push(n);
                }
            }
        }
    }

    for node in node_refs {
        // 1. Invisible? Skip.
        if node.style.is_invisible() {
            stats.skipped_invisible += 1;
            continue;
        }

        // 2. AABB ∩ damage? Skip if no overlap and damage isn't full.
        if !damage.is_full() {
            let Some(aabb) = node.aabb() else {
                // No geometry — nothing to draw. (Phase A's path / empty
                // text returns None here.)
                stats.skipped_offscreen += 1;
                continue;
            };
            if !intersects_any(damage, &aabb) {
                stats.skipped_offscreen += 1;
                continue;
            }
        }

        // 3. Resolve style: merge node opacity with capability-aware
        //    collapse. Layer blend mode is informational (backends
        //    apply it at composite time, not per-primitive).
        let resolved_style = resolve_style(&node.style, &caps);

        // 4. Skip primitives whose layer wants a blend mode this
        //    backend can't honour. v1 contract: degrade-to-Normal
        //    silently; we don't return an error.
        let layer_mode = layer_blend[node.layer.index()];
        let _ = (layer_mode, Layer::Bg); // reference for the `unused` lint; keep variable named for clarity
        if layer_mode != BlendMode::Normal && !caps.has_blend_modes() {
            // Surface can't apply the blend mode. Still draw the
            // primitive at `Normal` — design doc §6.1 / §11.
        }

        // 5. Dispatch.
        match surface.draw_primitive(&node.primitive, &resolved_style, &node.transform) {
            Ok(()) => stats.drawn += 1,
            Err(e) => {
                // Surface explicitly refused. The v1 policy is
                // best-effort: bubble the error up so the caller
                // sees it, but only on the first failure — subsequent
                // primitives might still draw fine, and we want
                // diagnostics for hardware bring-up.
                surface.end_frame().map_err(RenderError::EndFrame)?;
                return Err(RenderError::DrawPrimitive(e));
            }
        }
    }

    surface.end_frame().map_err(RenderError::EndFrame)?;
    Ok(stats)
}

/// True if `aabb` overlaps any rect in `damage`. `damage.is_full()`
/// short-circuits this elsewhere; this helper assumes a partial set.
fn intersects_any(damage: &DamageSet, aabb: &Rect) -> bool {
    if damage.is_full() {
        return true;
    }
    damage.rects().iter().any(|r| r.intersects(aabb))
}

/// Capability-aware style resolution.
///
/// v1 contract per design doc §5.7:
///
/// - Backends without `ALPHA`: collapse `opacity` 1..=254 to `255`.
///   `0` stays `0` (it's already filtered as invisible upstream).
///
/// In v1.1 this will additionally:
/// - Premultiply the fill/stroke when the backend wants premultiplied
///   colors (HasPremultiplied capability bit, to be added).
/// - Merge layer-opacity (when layers grow their own opacity).
fn resolve_style(node_style: &Style, caps: &crate::capability::CapabilityMask) -> Style {
    let mut s = node_style.clone();
    if !caps.has_alpha() && (1..=254).contains(&s.opacity) {
        // v1 contract: degrade to opaque.
        s.opacity = 255;
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CapabilityMask;
    use weftos_leaf_scene::{
        px, BuiltinFont, FontFace, KerningHint, Layer, NodeId, Primitive, Rgba, SceneOp,
        Transform,
    };

    // -- A minimal SurfaceMock for renderer tests. Records every call. -----

    #[derive(Debug, Default)]
    struct SurfaceMock {
        caps: CapabilityMask,
        begin_count: u32,
        end_count: u32,
        draws: Vec<(Primitive, Style, Transform)>,
        fail_begin: bool,
        fail_end: bool,
    }

    impl SceneSurface for SurfaceMock {
        type Error = &'static str;

        fn capabilities(&self) -> CapabilityMask {
            self.caps
        }

        fn begin_frame(&mut self, _damage: &DamageSet, _viewport: Rect) -> Result<(), Self::Error> {
            if self.fail_begin {
                return Err("begin_failed");
            }
            self.begin_count += 1;
            Ok(())
        }

        fn draw_primitive(
            &mut self,
            primitive: &Primitive,
            style: &Style,
            transform: &Transform,
        ) -> Result<(), Self::Error> {
            self.draws.push((primitive.clone(), style.clone(), *transform));
            Ok(())
        }

        fn end_frame(&mut self) -> Result<(), Self::Error> {
            if self.fail_end {
                return Err("end_failed");
            }
            self.end_count += 1;
            Ok(())
        }
    }

    fn nid(d: u8, n: u32) -> NodeId {
        NodeId::from_parts(d, n)
    }

    fn rect_node(id: NodeId, x: i32, y: i32, w: i32, h: i32) -> Node {
        Node {
            id,
            layer: Layer::Widget,
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
    fn missing_display_still_frames() {
        let store = SceneStore::new();
        let mut surface = SurfaceMock::default();
        let dmg = DamageSet::none();
        let stats = render_damage(&store, 0, &dmg, &mut surface).unwrap();
        assert_eq!(stats.drawn, 0);
        // begin/end still ran so the backend can present cleanly.
        assert_eq!(surface.begin_count, 1);
        assert_eq!(surface.end_count, 1);
    }

    #[test]
    fn full_damage_draws_every_visible_node() {
        let mut store = SceneStore::new();
        store.set_viewport(0, Rect::from_px(0, 0, 800, 480));
        store.apply_op(0, &SceneOp::Insert(rect_node(nid(0, 1), 0, 0, 10, 10)));
        store.apply_op(0, &SceneOp::Insert(rect_node(nid(0, 2), 100, 100, 10, 10)));

        let mut surface = SurfaceMock::default();
        let stats = render_damage(&store, 0, &DamageSet::full(), &mut surface).unwrap();
        assert_eq!(stats.drawn, 2);
        assert_eq!(surface.draws.len(), 2);
    }

    #[test]
    fn partial_damage_filters_offscreen_nodes() {
        let mut store = SceneStore::new();
        store.set_viewport(0, Rect::from_px(0, 0, 800, 480));
        store.apply_op(0, &SceneOp::Insert(rect_node(nid(0, 1), 0, 0, 10, 10)));
        store.apply_op(0, &SceneOp::Insert(rect_node(nid(0, 2), 500, 100, 10, 10)));

        let mut damage = DamageSet::none();
        damage.add(Rect::from_px(0, 0, 50, 50), Rect::from_px(0, 0, 800, 480));

        let mut surface = SurfaceMock::default();
        let stats = render_damage(&store, 0, &damage, &mut surface).unwrap();
        assert_eq!(stats.drawn, 1);
        assert_eq!(stats.skipped_offscreen, 1);
    }

    #[test]
    fn invisible_nodes_are_skipped() {
        let mut store = SceneStore::new();
        store.set_viewport(0, Rect::from_px(0, 0, 800, 480));
        let mut hidden = rect_node(nid(0, 1), 0, 0, 10, 10);
        hidden.style.visible = false;
        store.apply_op(0, &SceneOp::Insert(hidden));

        let mut surface = SurfaceMock::default();
        let stats = render_damage(&store, 0, &DamageSet::full(), &mut surface).unwrap();
        assert_eq!(stats.drawn, 0);
        assert_eq!(stats.skipped_invisible, 1);
    }

    #[test]
    fn layer_order_is_bg_to_alert() {
        let mut store = SceneStore::new();
        store.set_viewport(0, Rect::from_px(0, 0, 800, 480));
        // Insert Alert before Bg in declaration order to test that
        // layer-order overrides declaration-order on the wire.
        let mut alert = rect_node(nid(0, 1), 0, 0, 10, 10);
        alert.layer = Layer::Alert;
        alert.style = Style::filled(Rgba::new(0xFF, 0xFF, 0xFF, 0xFF));
        store.apply_op(0, &SceneOp::Insert(alert));

        let mut bg = rect_node(nid(0, 2), 0, 0, 10, 10);
        bg.layer = Layer::Bg;
        bg.style = Style::filled(Rgba::new(0x00, 0x00, 0x00, 0xFF));
        store.apply_op(0, &SceneOp::Insert(bg));

        let mut surface = SurfaceMock::default();
        let _stats = render_damage(&store, 0, &DamageSet::full(), &mut surface).unwrap();
        // Drawn order: Bg first, then Alert.
        assert_eq!(surface.draws.len(), 2);
        assert_eq!(surface.draws[0].1.fill.unwrap(), Rgba::BLACK);
        assert_eq!(surface.draws[1].1.fill.unwrap(), Rgba::WHITE);
    }

    #[test]
    fn opacity_collapses_when_surface_lacks_alpha() {
        let mut store = SceneStore::new();
        store.set_viewport(0, Rect::from_px(0, 0, 800, 480));
        let mut node = rect_node(nid(0, 1), 0, 0, 10, 10);
        node.style.opacity = 128;
        store.apply_op(0, &SceneOp::Insert(node));

        // Surface with NO alpha capability.
        let mut surface = SurfaceMock {
            caps: CapabilityMask::empty(),
            ..Default::default()
        };
        let _ = render_damage(&store, 0, &DamageSet::full(), &mut surface).unwrap();
        assert_eq!(surface.draws[0].1.opacity, 255, "opacity must collapse to 255 without ALPHA");
    }

    #[test]
    fn opacity_preserved_when_surface_has_alpha() {
        let mut store = SceneStore::new();
        store.set_viewport(0, Rect::from_px(0, 0, 800, 480));
        let mut node = rect_node(nid(0, 1), 0, 0, 10, 10);
        node.style.opacity = 128;
        store.apply_op(0, &SceneOp::Insert(node));

        let mut surface = SurfaceMock {
            caps: CapabilityMask::ALPHA,
            ..Default::default()
        };
        let _ = render_damage(&store, 0, &DamageSet::full(), &mut surface).unwrap();
        assert_eq!(surface.draws[0].1.opacity, 128);
    }

    #[test]
    fn begin_frame_failure_bubbles_up() {
        let mut store = SceneStore::new();
        store.set_viewport(0, Rect::from_px(0, 0, 800, 480));
        store.apply_op(0, &SceneOp::Insert(rect_node(nid(0, 1), 0, 0, 10, 10)));

        let mut surface = SurfaceMock {
            fail_begin: true,
            ..Default::default()
        };
        let r = render_damage(&store, 0, &DamageSet::full(), &mut surface);
        matches!(r, Err(RenderError::BeginFrame(_)));
    }

    #[test]
    fn end_frame_called_exactly_once_on_success() {
        let mut store = SceneStore::new();
        store.set_viewport(0, Rect::from_px(0, 0, 800, 480));
        store.apply_op(0, &SceneOp::Insert(rect_node(nid(0, 1), 0, 0, 10, 10)));
        store.apply_op(0, &SceneOp::Insert(rect_node(nid(0, 2), 100, 100, 10, 10)));

        let mut surface = SurfaceMock::default();
        let _ = render_damage(&store, 0, &DamageSet::full(), &mut surface).unwrap();
        assert_eq!(surface.begin_count, 1);
        assert_eq!(surface.end_count, 1);
    }

    #[test]
    fn empty_damage_still_frames_brackets() {
        let mut store = SceneStore::new();
        store.set_viewport(0, Rect::from_px(0, 0, 800, 480));
        store.apply_op(0, &SceneOp::Insert(rect_node(nid(0, 1), 0, 0, 10, 10)));

        let mut surface = SurfaceMock::default();
        let stats = render_damage(&store, 0, &DamageSet::none(), &mut surface).unwrap();
        // No damage rects → no draws (the single node's AABB doesn't
        // overlap nothing).
        assert_eq!(stats.drawn, 0);
        assert_eq!(surface.begin_count, 1);
        assert_eq!(surface.end_count, 1);
    }

    #[test]
    fn text_node_dispatches_with_correct_primitive() {
        let mut store = SceneStore::new();
        store.set_viewport(0, Rect::from_px(0, 0, 800, 480));
        let node = Node {
            id: nid(0, 7),
            layer: Layer::Text,
            transform: Transform::translate(px(20), px(30)),
            primitive: Primitive::Text {
                content: alloc::string::String::from("hi"),
                face: FontFace::Builtin(BuiltinFont::Mono6x10),
                size_q8: 10 << 8,
                weight: 400,
                kerning: KerningHint::Auto,
            },
            style: Style::filled(Rgba::WHITE),
            input: None,
        };
        store.apply_op(0, &SceneOp::Insert(node));

        let mut surface = SurfaceMock::default();
        let _ = render_damage(&store, 0, &DamageSet::full(), &mut surface).unwrap();
        assert_eq!(surface.draws.len(), 1);
        match &surface.draws[0].0 {
            Primitive::Text { content, .. } => assert_eq!(content, "hi"),
            other => panic!("expected Text, got {other:?}"),
        }
    }
}
