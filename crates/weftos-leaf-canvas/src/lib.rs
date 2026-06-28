//! # weftos-leaf-canvas
//!
//! Phase D of the WeftOS vector-first leaf display. Implements
//! [`weftos_leaf_renderer::SceneSurface`] against
//! [`web_sys::CanvasRenderingContext2d`] so a browser tab can render
//! the same `kernel.ps` scene that ships to the embedded CrowPanel
//! leaf — minus the DPI bus, plus everything Canvas2D gives us for
//! free.
//!
//! Canonical design doc: `docs/design/vector-leaf-display.md` (§4.6
//! Browser Backend, §6 Renderer Trait, §11 capability matrix).
//!
//! ## Shape at a glance
//!
//! ```text
//!   SceneStore ─┐
//!   DamageSet  ─┼──► render_damage ──► CanvasSurface::draw_primitive ─► ctx ops
//!   DisplayId  ─┘                       ▲
//!                                       │
//!                                  (looked up by canvas_id)
//! ```
//!
//! ## Usage
//!
//! ```no_run
//! use weftos_leaf_canvas::CanvasSurface;
//! use weftos_leaf_renderer::render_damage;
//! use weftos_leaf_scene::{DamageSet, Rect, SceneStore};
//!
//! let mut store = SceneStore::new();
//! store.set_viewport(0, Rect::from_px(0, 0, 800, 480));
//! // ... apply some ops ...
//!
//! let mut surface = CanvasSurface::new("leaf-canvas").expect("canvas missing");
//! render_damage(&store, 0, &DamageSet::full(), &mut surface).unwrap();
//! ```
//!
//! ## Capability profile
//!
//! The canvas declares
//! `ALPHA | SUBPIXEL | ANTIALIASED | BLEND_MODES | BITMAP_PNG` —
//! Canvas2D honours per-pixel alpha (`globalAlpha`), sub-pixel
//! positioning, glyph + line AA, every blend mode in v1's enum (via
//! `globalCompositeOperation`), and PNG decoding through
//! `createImageBitmap`. This lets [`render_damage`] skip the
//! degradation fallbacks the embedded `DpiSurface` triggers.
//!
//! ## What this crate does *not* do
//!
//! - **Mesh transport** — Phase E lands the browser-side WebSocket
//!   bridge that feeds `SceneEnvelope`s into a `SceneStore`. This
//!   crate just turns a populated store into pixels.
//! - **Async PNG decode** — `createImageBitmap` is async, but the
//!   [`SceneSurface`] contract is synchronous. v1 dispatches PNG bytes
//!   through the renderer's `decode_bitmap` shim, which returns
//!   `Unsupported(Png)` for now; v1.1 swaps in a sync-from-cache path
//!   that pre-decodes via an off-frame `Promise`. The capability flag
//!   is declared today so Phase E producers can target it without a
//!   second wire bump.
//! - **`requestAnimationFrame` pacing** — backend concern of whatever
//!   harness drives the render loop. The surface itself is a one-shot
//!   `draw_primitive` dispatcher.
//!
//! [`render_damage`]: weftos_leaf_renderer::render_damage

pub mod bitmap;
pub mod canvas_surface;
pub mod path;
pub mod text;

pub use canvas_surface::{CanvasError, CanvasSurface};

// Compile-time guards: `CanvasSurface` is intentionally NOT required to
// be `Send + Sync`. `web_sys::CanvasRenderingContext2d` is a
// `JsValue` newtype and is `!Send + !Sync` by construction; the
// `SceneSurface` trait does not require Send/Sync (see design doc
// Appendix B). The renderer is single-threaded per surface, which is
// fine on wasm32 where there's only one thread to begin with.
//
// We *do* assert the error type is Send + Sync — it carries no JS
// references, only static strings + the bitmap-decode error from the
// renderer. This keeps `RenderError<CanvasError>` ergonomic for callers
// who hold it across async boundaries (e.g., into a `Promise` chain).
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<CanvasError>();
};
