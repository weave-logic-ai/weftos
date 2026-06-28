//! Bitmap codec dispatch — see
//! [vector-leaf-display.md §6 Renderer Trait](../../../docs/design/vector-leaf-display.md).
//!
//! v1 ships **Raw8888** and **Raw565** decoders. Every other format
//! returns [`BitmapError::Unsupported`] — the renderer skips the
//! affected node and the backend never sees malformed pixel data.
//!
//! v1.1 adds QOI (no `std`, allocator-only, fits embedded) and PNG
//! (gated behind `std` because most PNG crates pull in `Vec`/`io`).
//! The dispatch shape doesn't change; producers just stop seeing
//! `Unsupported` for those two variants.

use alloc::vec::Vec;

use weftos_leaf_scene::{BitmapFormat, Rgba};

/// One decoded bitmap.
///
/// `pixels` is in row-major, top-left origin order, in straight (non-
/// premultiplied) RGBA at 8 bits per channel. Width × height is fixed
/// by `w`/`h`; `pixels.len() == w * h * 4` for a well-formed bitmap.
///
/// The decoder normalises every supported source format into this
/// representation so backends can blit / composite from one shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedBitmap {
    pub w: u32,
    pub h: u32,
    pub pixels: Vec<u8>,
}

impl DecodedBitmap {
    /// Convenience accessor: one pixel as `Rgba`. Returns
    /// `Rgba::TRANSPARENT` for out-of-bounds requests.
    pub fn pixel(&self, x: u32, y: u32) -> Rgba {
        if x >= self.w || y >= self.h {
            return Rgba::TRANSPARENT;
        }
        let i = ((y * self.w + x) * 4) as usize;
        Rgba::new(
            self.pixels[i],
            self.pixels[i + 1],
            self.pixels[i + 2],
            self.pixels[i + 3],
        )
    }
}

/// Why a decode failed.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum BitmapError {
    /// Format is on the wire but not implemented in v1. The renderer
    /// skips the affected node; the producer can resend in `Raw8888`
    /// or `Raw565` (or wait for v1.1).
    Unsupported(BitmapFormat),
    /// `data.len()` did not match the expected `w * h * bytes_per_pixel`.
    SizeMismatch {
        expected: usize,
        got: usize,
    },
    /// `w` or `h` was <= 0 (wire is signed Q24.8; producers should not
    /// emit negative sizes). Renderer skips.
    InvalidExtent {
        w: i32,
        h: i32,
    },
}

/// Decode a wire-format bitmap into [`DecodedBitmap`].
///
/// `w` and `h` are Q24.8 pixel sizes; the decoder rounds to nearest
/// integer pixel via shift (matching the renderer's `from_px_q8` rule
/// for the AABB-clipped happy path).
///
/// v1 supports `Raw8888` and `Raw565`; everything else returns
/// `Unsupported`.
pub fn decode_bitmap(
    w_q8: i32,
    h_q8: i32,
    format: BitmapFormat,
    data: &[u8],
) -> Result<DecodedBitmap, BitmapError> {
    let w_px = (w_q8 + 128) >> 8;
    let h_px = (h_q8 + 128) >> 8;
    if w_px <= 0 || h_px <= 0 {
        return Err(BitmapError::InvalidExtent { w: w_q8, h: h_q8 });
    }
    let w = w_px as u32;
    let h = h_px as u32;
    match format {
        BitmapFormat::Raw8888 => decode_raw8888(w, h, data),
        BitmapFormat::Raw565 => decode_raw565(w, h, data),
        // v1.1 lands these. The renderer is responsible for skipping
        // the node; the codec just declares it can't help.
        f @ (BitmapFormat::Qoi
        | BitmapFormat::Png
        | BitmapFormat::Rle
        | BitmapFormat::WebP) => Err(BitmapError::Unsupported(f)),
    }
}

fn decode_raw8888(w: u32, h: u32, data: &[u8]) -> Result<DecodedBitmap, BitmapError> {
    let expected = (w as usize) * (h as usize) * 4;
    if data.len() != expected {
        return Err(BitmapError::SizeMismatch {
            expected,
            got: data.len(),
        });
    }
    // Copy: Vec<u8> is the canonical owned representation; downstream
    // backends slice it without an extra realloc.
    Ok(DecodedBitmap {
        w,
        h,
        pixels: data.to_vec(),
    })
}

fn decode_raw565(w: u32, h: u32, data: &[u8]) -> Result<DecodedBitmap, BitmapError> {
    let expected = (w as usize) * (h as usize) * 2;
    if data.len() != expected {
        return Err(BitmapError::SizeMismatch {
            expected,
            got: data.len(),
        });
    }
    // Expand RGB565 -> RGBA8888. Replicate the top bits into the
    // low ones so 0x1F (5-bit max) -> 0xFF and 0x00 -> 0x00. Mirrors
    // LovyanGFX `lgfx::lgfx_565_to_888`.
    let mut pixels = Vec::with_capacity((w as usize) * (h as usize) * 4);
    for chunk in data.chunks_exact(2) {
        // Wire byte order: little-endian (the host serialises u16
        // little-endian in raw bitmaps; the CrowPanel byte-swap is a
        // strictly leaf-side concern in `color::to_rgb565_be`).
        let v = u16::from_le_bytes([chunk[0], chunk[1]]);
        let r5 = ((v >> 11) & 0x1F) as u8;
        let g6 = ((v >> 5) & 0x3F) as u8;
        let b5 = (v & 0x1F) as u8;
        // 5→8: replicate top bits.
        let r = (r5 << 3) | (r5 >> 2);
        let g = (g6 << 2) | (g6 >> 4);
        let b = (b5 << 3) | (b5 >> 2);
        pixels.extend_from_slice(&[r, g, b, 0xFF]);
    }
    Ok(DecodedBitmap { w, h, pixels })
}

#[cfg(test)]
mod tests {
    use super::*;
    use weftos_leaf_scene::px;

    #[test]
    fn raw8888_roundtrips() {
        // 2×1 image: red, green.
        let data = alloc::vec![
            0xFF, 0x00, 0x00, 0xFF, // red
            0x00, 0xFF, 0x00, 0xFF, // green
        ];
        let d = decode_bitmap(px(2), px(1), BitmapFormat::Raw8888, &data).unwrap();
        assert_eq!(d.w, 2);
        assert_eq!(d.h, 1);
        assert_eq!(d.pixel(0, 0), Rgba::RED);
        assert_eq!(d.pixel(1, 0), Rgba::GREEN);
    }

    #[test]
    fn raw8888_size_mismatch_errors() {
        let data = alloc::vec![0u8; 3];
        let r = decode_bitmap(px(2), px(1), BitmapFormat::Raw8888, &data);
        match r {
            Err(BitmapError::SizeMismatch { expected, got }) => {
                assert_eq!(expected, 8);
                assert_eq!(got, 3);
            }
            other => panic!("expected SizeMismatch, got {other:?}"),
        }
    }

    #[test]
    fn raw565_roundtrips_red() {
        // 1×1: 0xF800 LE = [0x00, 0xF8] -> red.
        let data = alloc::vec![0x00, 0xF8];
        let d = decode_bitmap(px(1), px(1), BitmapFormat::Raw565, &data).unwrap();
        let p = d.pixel(0, 0);
        // 5-bit red 0x1F -> 0xFF (replicated bits).
        assert_eq!(p.r, 0xFF);
        assert_eq!(p.g, 0x00);
        assert_eq!(p.b, 0x00);
        assert_eq!(p.a, 0xFF);
    }

    #[test]
    fn raw565_two_pixels_blue_then_white() {
        // px0 = 0x001F (blue), px1 = 0xFFFF (white).
        let data = alloc::vec![0x1F, 0x00, 0xFF, 0xFF];
        let d = decode_bitmap(px(2), px(1), BitmapFormat::Raw565, &data).unwrap();
        let blue = d.pixel(0, 0);
        let white = d.pixel(1, 0);
        assert_eq!(blue.b, 0xFF);
        assert_eq!(blue.r, 0);
        assert_eq!(white, Rgba::WHITE);
    }

    #[test]
    fn qoi_png_rle_webp_return_unsupported() {
        let data = alloc::vec![0u8; 100];
        for fmt in [
            BitmapFormat::Qoi,
            BitmapFormat::Png,
            BitmapFormat::Rle,
            BitmapFormat::WebP,
        ] {
            let r = decode_bitmap(px(10), px(10), fmt, &data);
            match r {
                Err(BitmapError::Unsupported(got)) => assert_eq!(got, fmt),
                other => panic!("{fmt:?}: expected Unsupported, got {other:?}"),
            }
        }
    }

    #[test]
    fn negative_extent_errors() {
        let data = alloc::vec![0u8; 4];
        let r = decode_bitmap(-px(1), px(1), BitmapFormat::Raw8888, &data);
        matches!(r, Err(BitmapError::InvalidExtent { .. }));
    }

    #[test]
    fn pixel_oob_returns_transparent() {
        let d = DecodedBitmap {
            w: 1,
            h: 1,
            pixels: alloc::vec![0xFF, 0xFF, 0xFF, 0xFF],
        };
        assert_eq!(d.pixel(10, 10), Rgba::TRANSPARENT);
    }
}
