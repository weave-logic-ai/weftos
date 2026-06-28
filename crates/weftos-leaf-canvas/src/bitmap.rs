//! `Primitive::Bitmap` → Canvas2D pixel blit.
//!
//! v1 supports `Raw8888` + `Raw565` via the renderer's shared
//! `decode_bitmap` helper (Phase B), then projects the decoded RGBA
//! into a `web_sys::ImageData` for a single `put_image_data` call.
//! Canvas2D handles per-pixel alpha for free; the surface declares
//! `ALPHA` so the renderer hands us straight (non-premultiplied) RGBA
//! and we don't have to second-guess the wire.
//!
//! `BitmapFormat::Png` is declared via `CapabilityMask::BITMAP_PNG`
//! and a v1.1 follow-up wires `createImageBitmap` (an async Promise)
//! into a pre-frame cache. The renderer's `decode_bitmap` returns
//! `Unsupported(Png)` today, which we propagate as
//! [`CanvasError::BitmapDecode`]; the node is skipped with a logged
//! warning. The capability flag ships today so producers don't need a
//! second wire bump.
//!
//! `put_image_data` ignores the canvas transform — it always blits in
//! raw canvas-space pixels. The caller must therefore pass the node's
//! resolved `(dx, dy)` translation explicitly; the
//! [`SceneSurface`][weftos_leaf_renderer::SceneSurface] dispatcher
//! (see `canvas_surface.rs`) extracts those from the merged
//! `Transform` and hands them to us.

use wasm_bindgen::Clamped;
use web_sys::{CanvasRenderingContext2d, ImageData};

use weftos_leaf_renderer::{decode_bitmap, BitmapError};
use weftos_leaf_scene::BitmapFormat;

use crate::canvas_surface::CanvasError;

/// Draw a `Primitive::Bitmap` blitted at `(dx, dy)` in canvas pixels.
///
/// `put_image_data` is intentionally transform-agnostic: it writes raw
/// pixels straight to the canvas backing store, bypassing
/// `globalAlpha`, `globalCompositeOperation`, and the active transform.
/// That makes it the fastest path for pre-decoded raw bitmaps, but the
/// caller has to supply the destination offset.
///
/// On Raw8888 / Raw565 the renderer's `decode_bitmap` returns a
/// well-formed straight RGBA buffer; we hand it to `ImageData::new_*`
/// and blit. PNG / QOI / WebP currently land on the
/// `Unsupported(format)` path.
pub fn draw_bitmap(
    ctx: &CanvasRenderingContext2d,
    w_q8: i32,
    h_q8: i32,
    format: BitmapFormat,
    data: &[u8],
    dx: f64,
    dy: f64,
) -> Result<(), CanvasError> {
    let decoded = decode_bitmap(w_q8, h_q8, format, data).map_err(CanvasError::BitmapDecode)?;

    // `ImageData::new_with_u8_clamped_array_and_sh` expects a
    // `Clamped<&mut [u8]>` slice in straight RGBA order, plus width /
    // height in CSS pixels. The web-sys binding takes `&mut [u8]`
    // because the underlying JS API can read the buffer back out; we
    // construct from an owned copy so the borrow doesn't escape.
    let mut buf = decoded.pixels.clone();
    let image = ImageData::new_with_u8_clamped_array_and_sh(
        Clamped(buf.as_mut_slice()),
        decoded.w,
        decoded.h,
    )
    .map_err(|_| {
        // `ImageData::new_*` only fails when the buffer length doesn't
        // match `w * h * 4` — which means the renderer's decoder
        // produced a malformed buffer. That's a renderer bug, not a
        // wire bug; surface it via SizeMismatch so the catch site can
        // distinguish it from "format unsupported".
        CanvasError::BitmapDecode(BitmapError::SizeMismatch {
            expected: (decoded.w as usize) * (decoded.h as usize) * 4,
            got: decoded.pixels.len(),
        })
    })?;

    ctx.put_image_data(&image, dx, dy).map_err(|_| {
        CanvasError::BitmapDecode(BitmapError::SizeMismatch {
            expected: (decoded.w as usize) * (decoded.h as usize) * 4,
            got: decoded.pixels.len(),
        })
    })?;
    Ok(())
}
