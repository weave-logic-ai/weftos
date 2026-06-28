//! # weftos-leaf-renderer
//!
//! Phase B of the WeftOS vector-first leaf display. Consumes a
//! [`weftos_leaf_scene::SceneStore`] (Phase A) + a [`DamageSet`] and
//! drives a backend-supplied [`SceneSurface`] to produce pixels.
//!
//! Canonical design doc: `docs/design/vector-leaf-display.md` (§6
//! Renderer Trait, §7 Damage Computation).
//!
//! ## Shape at a glance
//!
//! ```text
//!   SceneStore ─┐
//!   DamageSet  ─┼──► render_damage ──► SceneSurface::draw_primitive(...) ──► pixels
//!   DisplayId  ─┘                       ▲
//!                                       │
//!                                  (sim, dpi, canvas backends)
//! ```
//!
//! ## What's in this crate
//!
//! | Module             | Contents                                                              |
//! |--------------------|-----------------------------------------------------------------------|
//! | [`surface`]        | [`SceneSurface`] trait, [`RenderError`]                               |
//! | [`capability`]     | [`CapabilityMask`] bitflags                                           |
//! | [`glyph_cache`]    | [`GlyphCache`] — size-bounded LRU keyed on `(face, char, size_q8)`    |
//! | [`bitmap_decode`]  | `decode_bitmap`: Raw8888 + Raw565 happy path                          |
//! | [`render`]         | [`render_damage`] — the canonical entry point                         |
//! | [`color`]          | `to_rgb888`, `to_rgb565`, `to_rgb565_be` (CrowPanel byte-swap)        |
//!
//! ## What this crate does NOT do
//!
//! - Open windows / talk to hardware — backends do that. The renderer
//!   is pure damage-walker + capability-aware dispatcher.
//! - Frame pacing / requestAnimationFrame — backend concern.
//! - Async / await — `render_damage` is synchronous; backends that
//!   need async (canvas) handle it at their boundary.
//! - Fonts beyond the Phase A built-ins (Mono6x10 / Mono10x20). Vector
//!   fonts land in v1.1 alongside `FontFace::Vector` rasterization.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod bitmap_decode;
pub mod capability;
pub mod color;
pub mod glyph_cache;
pub mod render;
pub mod surface;

// Re-exports for ergonomics. Backends `use weftos_leaf_renderer::*`.
pub use bitmap_decode::{decode_bitmap, BitmapError, DecodedBitmap};
pub use capability::CapabilityMask;
pub use color::{to_rgb565, to_rgb565_be, to_rgb888};
pub use glyph_cache::{Glyph, GlyphCache, GlyphKey};
pub use render::{render_damage, RenderError};
pub use surface::SceneSurface;

// Compile-time guards: every transport / wire-shaped public type
// stays `Send + Sync`. `SceneSurface` is intentionally NOT required to
// be `Send + Sync` (backends own hardware handles); the renderer is
// single-threaded per surface. See design doc Appendix B.
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<CapabilityMask>();
    assert_send_sync::<GlyphKey>();
    assert_send_sync::<Glyph>();
    assert_send_sync::<DecodedBitmap>();
    assert_send_sync::<BitmapError>();
    // RenderError is generic over the surface's error type; assert
    // the unit-error specialization is Send+Sync (it is — only carries
    // a unit when the backend's error is unit).
    assert_send_sync::<RenderError<()>>();
};
