//! The `SceneSurface` trait — every backend implements this.
//!
//! Canonical reference: `docs/design/vector-leaf-display.md` §6.
//!
//! This is the **single-primitive** shape of the trait. The renderer
//! has already AABB-filtered against damage and merged style /
//! transform context; the surface receives one `draw_primitive` call
//! per visible node per frame.
//!
//! Phase A's design doc sketches a more granular fill_rect /
//! stroke_line / draw_glyph variant; we collapse those into one
//! `draw_primitive(Primitive, Style, Transform)` here because
//!
//! 1. Backends that decompose primitives into rasterization steps
//!    (DPI, canvas) do so once per primitive anyway — the renderer
//!    shouldn't pre-decompose.
//! 2. The sim backend can hand the primitive straight to
//!    `embedded-graphics` without an intermediate IR.
//! 3. v1.1's `Primitive::Path` lands by simply teaching backends to
//!    handle one more variant; the trait surface doesn't churn.
//!
//! Backends that lack a capability (per [`SceneSurface::capabilities`])
//! may return an error from `draw_primitive` rather than degrade
//! silently; the renderer reports them through [`crate::RenderError`].

use weftos_leaf_scene::{DamageSet, Primitive, Rect, Style, Transform};

use crate::capability::CapabilityMask;

/// Generic scene-to-pixels backend.
///
/// Every backend (sim, DPI, canvas, future) implements this. The
/// renderer's [`crate::render_damage`] is the canonical caller; tests
/// and golden harnesses also drive it directly.
///
/// ## Required `Send + Sync`?
///
/// No — by design. Surfaces own hardware handles (DPI bus, SDL2
/// window, browser `OffscreenCanvas`) that aren't `Send` in general.
/// The renderer is single-threaded per surface; see design doc
/// Appendix B for why this is fine.
pub trait SceneSurface {
    /// Backend-specific error. `Debug` is enough — the renderer wraps
    /// in [`crate::RenderError::Backend`] for the call site.
    type Error: core::fmt::Debug;

    /// Declare what this backend can natively render. Used by
    /// [`crate::render_damage`] to short-circuit unsupported paths
    /// (e.g., skip alpha compositing on the DPI surface, skip
    /// `BlendMode::Multiply` on every v1 backend that lacks
    /// `BLEND_MODES`).
    ///
    /// Returning `CapabilityMask::empty()` is the v1 DPI baseline.
    fn capabilities(&self) -> CapabilityMask;

    /// Begin a frame scoped to the given damage rects.
    ///
    /// `viewport` is the display's full clip — backends that scissor
    /// in hardware (DPI's DMA window, canvas's `clip()` path) take
    /// it as a fallback when `damage.is_full()`.
    ///
    /// Called exactly once per frame, before any `draw_primitive`.
    fn begin_frame(&mut self, damage: &DamageSet, viewport: Rect) -> Result<(), Self::Error>;

    /// Draw one primitive at the resolved style + transform.
    ///
    /// The renderer guarantees:
    ///
    /// - The node's AABB overlaps at least one damage rect (or
    ///   `damage.is_full()`).
    /// - `style.is_invisible()` is false (the renderer skipped it
    ///   otherwise).
    /// - `transform` is the merged node-on-layer transform; for v1
    ///   that's just `node.transform` (no parent stacks yet).
    /// - `style.opacity` is preserved verbatim — backends without
    ///   `ALPHA` should treat `0` as "skip" and anything else as `255`.
    ///
    /// Backends that lack a capability needed by the primitive may
    /// return `Err`; the renderer logs and continues.
    fn draw_primitive(
        &mut self,
        primitive: &Primitive,
        style: &Style,
        transform: &Transform,
    ) -> Result<(), Self::Error>;

    /// End the frame. Backends present pixels here (canvas presents
    /// on the next rAF; DPI does the dcache flush + swap; the sim
    /// updates its `Window`). Called exactly once per frame, after
    /// every `draw_primitive`.
    fn end_frame(&mut self) -> Result<(), Self::Error>;
}
