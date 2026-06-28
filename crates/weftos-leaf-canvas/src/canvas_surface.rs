//! `CanvasSurface` — [`weftos_leaf_renderer::SceneSurface`] impl backed
//! by [`web_sys::CanvasRenderingContext2d`].
//!
//! Renders every `weftos-leaf-scene` primitive through native Canvas2D
//! calls:
//!
//! - `Primitive::Rect`   → `fill_rect` / `stroke_rect`
//! - `Primitive::Line`   → `begin_path` + `move_to` + `line_to` + `stroke`
//! - `Primitive::Circle` → `begin_path` + `arc` + `fill`/`stroke`
//! - `Primitive::Text`   → `set_font` + `fill_text`
//! - `Primitive::Bitmap` → `put_image_data` (Raw8888 + Raw565 in v1;
//!   PNG via `createImageBitmap` lands in v1.1)
//! - `Primitive::Path`   → reserved for `Path2d` translation in v1.1;
//!   currently returns `Unsupported`
//!
//! The canvas declares
//! `ALPHA | SUBPIXEL | ANTIALIASED | BLEND_MODES | BITMAP_PNG`. Canvas2D
//! honours all of these natively, so [`render_damage`] skips the
//! degraded fallback paths the embedded DPI surface triggers.
//!
//! [`render_damage`]: weftos_leaf_renderer::render_damage

use wasm_bindgen::{JsCast, JsValue};
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement};

use weftos_leaf_renderer::{BitmapError, CapabilityMask, SceneSurface};
use weftos_leaf_scene::{from_px_q8, DamageSet, Primitive, Rect, Rgba, Style, Transform};

use crate::{bitmap, path as canvas_path, text};

/// Backend error type.
///
/// Most operations succeed unconditionally — Canvas2D doesn't fail at
/// the call site for missing capabilities, it just degrades silently.
/// The error path exists for the small set of cases where we *want* the
/// renderer to skip the affected node and log:
///
/// - The named canvas element doesn't exist in the document
///   ([`CanvasSurface::new`]).
/// - The canvas's `getContext("2d")` returned null (a 2D context was
///   already claimed by a `webgl` request, or the browser is too old).
/// - A `Primitive::Path` arrived but v1 hasn't taught `path.rs` to
///   translate `PathCmd` → `Path2d` yet.
/// - A bitmap decode failed (renderer's `decode_bitmap` returns
///   `Unsupported` for QOI / WebP, plus PNG until v1.1 wires the async
///   path).
/// - A vector / inline font was requested — v1.1.
///
/// `JsValue` is intentionally NOT carried in this enum — `JsValue` is
/// `!Send + !Sync`, which would poison every caller that wants to
/// stash the error in a future or send it across a channel. The
/// surface logs the underlying JS error to the browser console at the
/// catch site and propagates a clean Rust string instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanvasError {
    /// `document.get_element_by_id` returned null.
    CanvasNotFound(String),
    /// The element exists but isn't an `<canvas>`.
    NotACanvas(String),
    /// `getContext("2d")` returned null.
    Context2dUnavailable,
    /// Browser globals (`window`, `document`) were missing — running in
    /// a worker or a non-DOM JS host.
    NoDom,
    /// Primitive variant not implemented in v1 (`Path`).
    UnsupportedPrimitive(&'static str),
    /// Bitmap decode failed (renderer's `decode_bitmap` returned an
    /// error — `Unsupported(format)`, `InvalidExtent`, or
    /// `SizeMismatch`).
    BitmapDecode(BitmapError),
    /// Vector / inline font requested — v1.1.
    UnsupportedFont(&'static str),
}

impl core::fmt::Display for CanvasError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::CanvasNotFound(id) => write!(f, "canvas element '{id}' not found"),
            Self::NotACanvas(id) => write!(f, "element '{id}' is not a <canvas>"),
            Self::Context2dUnavailable => f.write_str("canvas 2d context unavailable"),
            Self::NoDom => f.write_str("no DOM available (window or document missing)"),
            Self::UnsupportedPrimitive(s) => write!(f, "unsupported primitive: {s}"),
            Self::BitmapDecode(e) => write!(f, "bitmap decode failed: {e:?}"),
            Self::UnsupportedFont(s) => write!(f, "unsupported font: {s}"),
        }
    }
}

impl std::error::Error for CanvasError {}

/// Browser canvas surface.
///
/// Wraps a borrowed [`HtmlCanvasElement`] + its 2D context. Construction
/// looks up the canvas by DOM id; subsequent frames issue
/// `ctx.*` calls directly. `Drop` is a no-op — the JS side owns the
/// canvas and we don't release the context.
pub struct CanvasSurface {
    /// The canvas element. Kept around so [`begin_frame`] can read the
    /// element's `width` / `height` for the full-repaint clear (the
    /// 2D context exposes per-call clipping but not the canvas size).
    canvas: HtmlCanvasElement,
    /// The rendering context. Every draw call routes through this.
    ctx: CanvasRenderingContext2d,
    /// Background colour for `begin_frame` clears on full-repaint.
    /// Matches `SceneStore::set_bg` semantics; defaults to opaque
    /// black so an empty frame is visible.
    clear_color: Rgba,
}

impl CanvasSurface {
    /// Look up a `<canvas id="…">` in the current document and bind a
    /// 2D rendering context to it.
    ///
    /// Errors:
    ///
    /// - [`CanvasError::NoDom`] if `window()` or `document()` is null
    ///   (running in a non-DOM JS host like a service worker).
    /// - [`CanvasError::CanvasNotFound`] if the id doesn't resolve.
    /// - [`CanvasError::NotACanvas`] if the element isn't a `<canvas>`.
    /// - [`CanvasError::Context2dUnavailable`] if `getContext("2d")`
    ///   returned null (rare: a `webgl` context was already claimed,
    ///   or the browser is ancient).
    ///
    /// Returns `Result<Self, JsValue>` for the call-site ergonomics
    /// `#[wasm_bindgen]` expects — JS callers see a thrown JsValue
    /// rather than a wrapped Rust enum.
    pub fn new(canvas_id: &str) -> Result<Self, JsValue> {
        let window = web_sys::window().ok_or_else(|| {
            JsValue::from_str(&CanvasError::NoDom.to_string())
        })?;
        let document = window.document().ok_or_else(|| {
            JsValue::from_str(&CanvasError::NoDom.to_string())
        })?;
        let element = document.get_element_by_id(canvas_id).ok_or_else(|| {
            JsValue::from_str(&CanvasError::CanvasNotFound(canvas_id.to_string()).to_string())
        })?;
        let canvas: HtmlCanvasElement = element.dyn_into::<HtmlCanvasElement>().map_err(|_| {
            JsValue::from_str(&CanvasError::NotACanvas(canvas_id.to_string()).to_string())
        })?;
        let ctx_obj = canvas
            .get_context("2d")?
            .ok_or_else(|| JsValue::from_str(&CanvasError::Context2dUnavailable.to_string()))?;
        let ctx: CanvasRenderingContext2d = ctx_obj.dyn_into().map_err(|_| {
            JsValue::from_str(&CanvasError::Context2dUnavailable.to_string())
        })?;
        Ok(Self {
            canvas,
            ctx,
            clear_color: Rgba::BLACK,
        })
    }

    /// Build a surface around an already-resolved canvas element. The
    /// public ergonomic entry is [`CanvasSurface::new`]; this hook
    /// exists for callers that have a `HtmlCanvasElement` in hand (e.g.,
    /// an `OffscreenCanvas` adapter, or unit-test scaffolding).
    pub fn from_canvas(canvas: HtmlCanvasElement) -> Result<Self, JsValue> {
        let ctx_obj = canvas
            .get_context("2d")?
            .ok_or_else(|| JsValue::from_str(&CanvasError::Context2dUnavailable.to_string()))?;
        let ctx: CanvasRenderingContext2d = ctx_obj.dyn_into().map_err(|_| {
            JsValue::from_str(&CanvasError::Context2dUnavailable.to_string())
        })?;
        Ok(Self {
            canvas,
            ctx,
            clear_color: Rgba::BLACK,
        })
    }

    /// Set the colour [`SceneSurface::begin_frame`] clears to on
    /// full-repaint.
    ///
    /// Producers normally set this via `SceneStore::set_bg` + the
    /// renderer reading `display.bg`; this hook exists for examples
    /// that want to override (e.g., a white-on-dark preview).
    pub fn set_clear_color(&mut self, color: Rgba) {
        self.clear_color = color;
    }

    /// Borrow the underlying `<canvas>`. Useful for testing harnesses
    /// that want to read back pixels via `toDataURL` or attach event
    /// listeners.
    #[inline]
    pub fn canvas(&self) -> &HtmlCanvasElement {
        &self.canvas
    }

    /// Borrow the rendering context. Lets advanced callers issue raw
    /// canvas ops between [`SceneSurface::begin_frame`] and
    /// [`SceneSurface::end_frame`] — for example, drawing a custom
    /// FPS overlay outside the scene graph.
    #[inline]
    pub fn ctx(&self) -> &CanvasRenderingContext2d {
        &self.ctx
    }
}

impl SceneSurface for CanvasSurface {
    type Error = CanvasError;

    fn capabilities(&self) -> CapabilityMask {
        // The browser leapfrogs the embedded surface for features
        // Canvas2D gives us for free. Phase C DPI returns empty();
        // sim returns ALPHA|SUBPIXEL|ANTIALIASED|BLEND_MODES; this
        // adds BITMAP_PNG because `createImageBitmap` lands PNG
        // decoding natively (v1.1 wires the async cache; the flag
        // ships today so producers don't need a wire bump).
        CapabilityMask::ALPHA
            | CapabilityMask::SUBPIXEL
            | CapabilityMask::ANTIALIASED
            | CapabilityMask::BLEND_MODES
            | CapabilityMask::BITMAP_PNG
    }

    fn begin_frame(&mut self, damage: &DamageSet, viewport: Rect) -> Result<(), Self::Error> {
        // Save the context state once per frame so [`end_frame`] can
        // restore the transform / alpha / globalCompositeOperation
        // cleanly, even if a primitive mutated them mid-frame.
        self.ctx.save();

        if damage.is_full() {
            // Full-repaint: clear the whole canvas to the configured
            // background. Two paths possible:
            //
            //   1. `ctx.clear_rect(0, 0, w, h)` — leaves pixels as
            //      `rgba(0,0,0,0)` (transparent), letting the page
            //      behind show through. Matches the sim's behaviour
            //      when `clear_color` has `a < 255`.
            //   2. `ctx.fill_rect(...)` with the background colour —
            //      paints opaque pixels, matches the DPI surface's
            //      "background lands at clear time" semantics.
            //
            // We pick path 2 when the configured clear is opaque, and
            // path 1 otherwise. Producers that don't care end up on
            // the explicit-fill path because `Rgba::BLACK` is opaque.
            let w = self.canvas.width() as f64;
            let h = self.canvas.height() as f64;
            if self.clear_color.is_opaque() {
                self.ctx
                    .set_fill_style_str(&crate::canvas_surface::css_color(self.clear_color));
                self.ctx.fill_rect(0.0, 0.0, w, h);
            } else {
                self.ctx.clear_rect(0.0, 0.0, w, h);
            }
        } else {
            // Partial damage: clear each rect individually. The
            // renderer has already filtered nodes by AABB ∩ rect, so
            // we don't have to clip the context — clearing the rect
            // exposes the page behind for any pixels not overdrawn by
            // this frame's primitives.
            //
            // Canvas2D's `clear_rect` accepts f64 px; the wire is
            // Q24.8, so we round-to-nearest via `from_px_q8`. Sub-pixel
            // damage doesn't make sense at the bus boundary.
            for r in damage.rects() {
                let x = from_px_q8(r.x) as f64;
                let y = from_px_q8(r.y) as f64;
                let w = from_px_q8(r.w) as f64;
                let h = from_px_q8(r.h) as f64;
                self.ctx.clear_rect(x, y, w, h);
            }
        }

        // `viewport` is the renderer's idea of the full clip. Canvas2D
        // doesn't need an explicit scissor — `fill_rect` outside the
        // canvas extent is a no-op — but we expose the hook for
        // future overlays.
        let _ = viewport;
        Ok(())
    }

    fn draw_primitive(
        &mut self,
        primitive: &Primitive,
        style: &Style,
        transform: &Transform,
    ) -> Result<(), Self::Error> {
        // Apply the node-local transform via `translate`. v1 honours
        // translation only — rotation / scale fall back to translation
        // per design doc §5.4. v1.1 walks the full affine into
        // `set_transform` (Canvas2D's `a, b, c, d, e, f` matrix maps
        // directly to our `rotation_deg_q8` + `scale_q16`).
        let ox = from_px_q8(transform.x) as f64;
        let oy = from_px_q8(transform.y) as f64;
        self.ctx.save();
        self.ctx.translate(ox, oy).ok();

        // Apply per-node opacity through `globalAlpha`. The Canvas2D
        // surface declares ALPHA so the renderer doesn't pre-collapse
        // `style.opacity` to 0/255 — we receive the full u8.
        if style.opacity != 255 {
            self.ctx.set_global_alpha(f64::from(style.opacity) / 255.0);
        }

        let result = match primitive {
            Primitive::Rect { w, h, radius_q8 } => {
                draw_rect(&self.ctx, *w, *h, *radius_q8, style);
                Ok(())
            }
            Primitive::Line { x2, y2, thickness_q8 } => {
                draw_line(&self.ctx, *x2, *y2, *thickness_q8, style);
                Ok(())
            }
            Primitive::Circle { radius_q16 } => {
                draw_circle(&self.ctx, *radius_q16, style);
                Ok(())
            }
            Primitive::Text {
                content,
                face,
                size_q8,
                weight,
                ..
            } => text::draw_text(&self.ctx, content, face, *size_q8, *weight, style),
            Primitive::Bitmap { w, h, format, data } => {
                // `put_image_data` bypasses the canvas transform — we
                // hand it the resolved destination offset directly.
                bitmap::draw_bitmap(&self.ctx, *w, *h, *format, data, ox, oy)
            }
            Primitive::Path { commands } => canvas_path::draw_path(&self.ctx, commands, style),
        };

        // Restore the saved state so the next primitive starts from a
        // clean transform / alpha / globalCompositeOperation. We do
        // this regardless of `result` — a backend error doesn't leave
        // the next primitive's coordinate system polluted.
        self.ctx.restore();
        result
    }

    fn end_frame(&mut self) -> Result<(), Self::Error> {
        // Restore the per-frame save from begin_frame. Canvas2D
        // presents on the next browser repaint automatically; no
        // explicit commit needed.
        self.ctx.restore();
        Ok(())
    }
}

/// Translate an `Rgba` into a CSS colour string suitable for
/// `set_fill_style_str` / `set_stroke_style_str`.
///
/// Opaque colours use the compact `#rrggbb` form; partially-transparent
/// colours use `rgba(r,g,b,a)` with a normalised `a` in `[0, 1]`.
pub(crate) fn css_color(c: Rgba) -> String {
    if c.is_opaque() {
        format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
    } else {
        // CSS `rgba()` accepts an integer 0..=255 → 0..=1 alpha; we
        // emit at most three decimals (matches what every browser
        // renders identically at 8-bit precision).
        let a = f64::from(c.a) / 255.0;
        format!("rgba({},{},{},{:.3})", c.r, c.g, c.b, a)
    }
}

/// Apply a `Style`'s fill / stroke onto the context, returning whether
/// each path should be filled / stroked. The caller draws the geometry
/// once and conditionally calls `ctx.fill()` / `ctx.stroke()`.
fn apply_style(ctx: &CanvasRenderingContext2d, style: &Style) -> (bool, bool) {
    let has_fill = style.fill.is_some();
    let has_stroke = style.stroke.is_some();
    if let Some(fill) = style.fill {
        ctx.set_fill_style_str(&css_color(fill));
    }
    if let Some(stroke) = style.stroke {
        ctx.set_stroke_style_str(&css_color(stroke));
        // Q8.8 → f64 px. v1's renderer already rounds to int at the
        // wire boundary; we preserve sub-pixel here for the
        // ANTIALIASED capability.
        let w = f64::from(style.stroke_width_q8) / 256.0;
        ctx.set_line_width(w.max(1.0));
    }
    (has_fill, has_stroke)
}

fn draw_rect(
    ctx: &CanvasRenderingContext2d,
    w_q8: i32,
    h_q8: i32,
    radius_q8: u16,
    style: &Style,
) {
    let w = f64::from(from_px_q8(w_q8).max(0));
    let h = f64::from(from_px_q8(h_q8).max(0));
    let (has_fill, has_stroke) = apply_style(ctx, style);

    if radius_q8 == 0 {
        // Sharp corners — fast path via the rectangle primitive
        // helpers.
        if has_fill {
            ctx.fill_rect(0.0, 0.0, w, h);
        }
        if has_stroke {
            ctx.stroke_rect(0.0, 0.0, w, h);
        }
        return;
    }

    // Rounded corners: walk the path manually. Canvas2D's `roundRect`
    // exists in modern browsers, but web-sys's 0.3 binding is hidden
    // behind an unstable feature. The manual path is portable to every
    // Canvas2D-capable browser back to 2018 and matches the geometry
    // Phase A's design doc sketches.
    let r = (f64::from(radius_q8) / 256.0).min(w.min(h) / 2.0);
    ctx.begin_path();
    ctx.move_to(r, 0.0);
    ctx.line_to(w - r, 0.0);
    ctx.quadratic_curve_to(w, 0.0, w, r);
    ctx.line_to(w, h - r);
    ctx.quadratic_curve_to(w, h, w - r, h);
    ctx.line_to(r, h);
    ctx.quadratic_curve_to(0.0, h, 0.0, h - r);
    ctx.line_to(0.0, r);
    ctx.quadratic_curve_to(0.0, 0.0, r, 0.0);
    ctx.close_path();

    if has_fill {
        ctx.fill();
    }
    if has_stroke {
        ctx.stroke();
    }
}

fn draw_line(
    ctx: &CanvasRenderingContext2d,
    x2_q8: i32,
    y2_q8: i32,
    thickness_q8: u16,
    style: &Style,
) {
    let x2 = f64::from(from_px_q8(x2_q8));
    let y2 = f64::from(from_px_q8(y2_q8));
    // For lines, the renderer treats `style.stroke or style.fill` as
    // the line colour. Lines have no "fill" in the rectangle sense; we
    // synthesize a stroke from whichever is set.
    let color = style.stroke.or(style.fill).unwrap_or(Rgba::WHITE);
    ctx.set_stroke_style_str(&css_color(color));
    // Q8.8 thickness → f64 px. Minimum 1px to match the embedded
    // surface's hairline rule.
    let w = (f64::from(thickness_q8) / 256.0).max(1.0);
    ctx.set_line_width(w);
    ctx.begin_path();
    ctx.move_to(0.0, 0.0);
    ctx.line_to(x2, y2);
    ctx.stroke();
}

fn draw_circle(ctx: &CanvasRenderingContext2d, radius_q16: u32, style: &Style) {
    // Q16.16 → f64 px. Keep sub-pixel precision so the ANTIALIASED
    // capability shows.
    let r = f64::from(radius_q16) / 65536.0;
    let (has_fill, has_stroke) = apply_style(ctx, style);
    ctx.begin_path();
    // Arc accepts a Result for `arc()` — we ignore the err because
    // the only failure mode is a non-finite radius, which we've already
    // bounded.
    ctx.arc(0.0, 0.0, r.max(0.0), 0.0, std::f64::consts::TAU).ok();
    if has_fill {
        ctx.fill();
    }
    if has_stroke {
        ctx.stroke();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // These tests run on any host because they don't touch a DOM —
    // they exercise the pure helpers. The `wasm32-unknown-unknown`
    // target is the deployment shape; the helpers are target-agnostic.

    #[test]
    fn css_color_opaque_uses_hex() {
        assert_eq!(css_color(Rgba::new(0x12, 0x34, 0x56, 0xFF)), "#123456");
        assert_eq!(css_color(Rgba::WHITE), "#ffffff");
        assert_eq!(css_color(Rgba::BLACK), "#000000");
    }

    #[test]
    fn css_color_alpha_uses_rgba() {
        let s = css_color(Rgba::new(255, 0, 0, 128));
        // 128/255 ≈ 0.502
        assert!(s.starts_with("rgba(255,0,0,"));
        assert!(s.ends_with(")"));
        // Sanity: parses to the same alpha when rounded back.
        assert!(s.contains("0.502"));
    }

    #[test]
    fn css_color_transparent_uses_rgba_zero() {
        let s = css_color(Rgba::TRANSPARENT);
        assert_eq!(s, "rgba(0,0,0,0.000)");
    }
}
