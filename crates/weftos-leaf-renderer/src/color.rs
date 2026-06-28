//! Colour conversion helpers used by every backend.
//!
//! Phase A's [`weftos_leaf_scene::Rgba`] is the canonical wire colour;
//! backends downconvert to their native format here. v1 supports:
//!
//! - `to_rgb888` — 24-bit (RGB888) for the desktop sim + golden tests.
//! - `to_rgb565` — 16-bit packed (R5 G6 B5) for embedded panels.
//! - `to_rgb565_be` — same 16-bit data but byte-swapped, which is what
//!   the CrowPanel DIS08070H pin map needs (data lines wired
//!   high-byte-first relative to LCD_CAM's little-endian DMA order).
//!
//! Phase C's `DpiSurface` consumes [`to_rgb565_be`] verbatim; this is
//! the load-bearing helper for that hardware path.

use weftos_leaf_scene::Rgba;

/// Pack an [`Rgba`] into a 24-bit `(r, g, b)` tuple. Alpha is dropped;
/// callers wanting alpha-blending must composite first.
#[inline]
pub const fn to_rgb888(c: Rgba) -> (u8, u8, u8) {
    (c.r, c.g, c.b)
}

/// Pack an [`Rgba`] into a 16-bit `RGB565` value (LE-host order).
///
/// Bit layout: `RRRRR GGGGGG BBBBB`.
///
/// Alpha is dropped. The conversion is the canonical bit-shift dither
/// (no error diffusion); it matches LovyanGFX `lgfx::convert_to_rgb565`
/// to the bit.
#[inline]
pub const fn to_rgb565(c: Rgba) -> u16 {
    let r = (c.r as u16 >> 3) & 0x1F;
    let g = (c.g as u16 >> 2) & 0x3F;
    let b = (c.b as u16 >> 3) & 0x1F;
    (r << 11) | (g << 5) | b
}

/// Pack an [`Rgba`] into byte-swapped RGB565 — the wire order the
/// CrowPanel DIS08070H 7-inch 800×480 RGB-DPI panel expects.
///
/// LCD_CAM streams pixels little-endian over DMA; the CrowPanel pin
/// map swaps high and low bytes once on the PCB. To get the colours
/// right, the framebuffer needs the bytes pre-swapped. This helper is
/// what `DpiSurface::draw_primitive` will call on every pixel in Phase
/// C. It's here in Phase B so the sim can verify the byte order in a
/// unit test and the DPI backend doesn't have to re-derive it.
#[inline]
pub const fn to_rgb565_be(c: Rgba) -> u16 {
    // Equivalent to `(le << 8) | (le >> 8)` but clippy prefers the
    // intrinsic — and so does the codegen.
    to_rgb565(c).rotate_right(8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb888_drops_alpha() {
        let c = Rgba::new(0x10, 0x20, 0x30, 0x80);
        assert_eq!(to_rgb888(c), (0x10, 0x20, 0x30));
    }

    #[test]
    fn rgb565_red_max() {
        // (0xFF, 0, 0, 0xFF) -> R5=0x1F, G6=0, B5=0
        // = (0x1F << 11) | 0 | 0 = 0xF800
        assert_eq!(to_rgb565(Rgba::RED), 0xF800);
    }

    #[test]
    fn rgb565_green_max() {
        // R5=0, G6=0x3F, B5=0  = 0x07E0
        assert_eq!(to_rgb565(Rgba::GREEN), 0x07E0);
    }

    #[test]
    fn rgb565_blue_max() {
        // R5=0, G6=0, B5=0x1F = 0x001F
        assert_eq!(to_rgb565(Rgba::BLUE), 0x001F);
    }

    #[test]
    fn rgb565_white_is_ffff() {
        assert_eq!(to_rgb565(Rgba::WHITE), 0xFFFF);
    }

    #[test]
    fn rgb565_black_is_zero() {
        assert_eq!(to_rgb565(Rgba::BLACK), 0x0000);
    }

    #[test]
    fn rgb565_be_swaps_bytes() {
        // 0xF800 -> 0x00F8
        assert_eq!(to_rgb565_be(Rgba::RED), 0x00F8);
        // 0x07E0 -> 0xE007
        assert_eq!(to_rgb565_be(Rgba::GREEN), 0xE007);
        // 0x001F -> 0x1F00
        assert_eq!(to_rgb565_be(Rgba::BLUE), 0x1F00);
    }

    #[test]
    fn rgb565_be_white_and_black_are_palindromes() {
        // 0xFFFF and 0x0000 byte-swap to themselves.
        assert_eq!(to_rgb565_be(Rgba::WHITE), 0xFFFF);
        assert_eq!(to_rgb565_be(Rgba::BLACK), 0x0000);
    }
}
