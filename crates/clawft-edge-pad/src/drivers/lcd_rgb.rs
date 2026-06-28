//! RGB parallel TFT driver for the CrowPanel 7" panel.
//!
//! Day-2 lit the panel with a 16-pixel `dma_loop_buffer!` solid fill.
//! This module replaces that with a real **800×480 RGB565 framebuffer
//! in PSRAM**, GDMA-scanned continuously by the `LCD_CAM` DPI
//! peripheral, with an `embedded-graphics` `DrawTarget` so text and
//! primitives can be drawn into it.
//!
//! Single-buffered: the CPU writes the framebuffer while GDMA reads
//! it. That aliasing is deliberate `unsafe` — tearing is accepted for
//! the spike. A double-buffer + re-`send` is the production fix.
//!
//! Cache note: `DmaTxBuf::send` does a one-time cache writeback. CPU
//! writes *before* `send` are flushed; writes *after* `send` may sit
//! in cache and not reach PSRAM. Draw the initial frame before
//! handing the buffer to `Dpi::send`. (Live updates after that are a
//! task-2+ concern — likely a periodic writeback or double-buffer.)
//!
//! API surface confirmed against esp-hal 1.0 source:
//! - `esp-hal/src/dma/buffers.rs` — `DmaTxBuf::new`, PSRAM support
//! - `esp-hal/src/dma/mod.rs` — `DmaDescriptor`, descriptor counting
//! - `esp-hal/src/lcd_cam/lcd/dpi.rs` — `Dpi::send`

#![allow(dead_code)]

use alloc::vec;

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;
use embedded_graphics::Pixel;
use esp_hal::dma::{DmaDescriptor, DmaTxBuf};

// ── RGB565 helpers ───────────────────────────────────────────────────

/// Encode an (R8, G8, B8) tuple into 16-bit RGB565.
#[inline]
pub const fn rgb565(r: u8, g: u8, b: u8) -> u16 {
    let r5 = (r as u16 >> 3) & 0x1F;
    let g6 = (g as u16 >> 2) & 0x3F;
    let b5 = (b as u16 >> 3) & 0x1F;
    (r5 << 11) | (g6 << 5) | b5
}

pub const COLOR_RED: u16 = rgb565(0xFF, 0x00, 0x00);
pub const COLOR_GREEN: u16 = rgb565(0x00, 0xFF, 0x00);
pub const COLOR_BLUE: u16 = rgb565(0x00, 0x00, 0xFF);
pub const COLOR_WHITE: u16 = rgb565(0xFF, 0xFF, 0xFF);
pub const COLOR_BLACK: u16 = 0;

// ── Framebuffer ──────────────────────────────────────────────────────

pub const FB_W: usize = 800;
pub const FB_H: usize = 480;
const FB_PIXELS: usize = FB_W * FB_H;
const FB_BYTES: usize = FB_PIXELS * 2;

// GDMA descriptors must live in internal DRAM (a `static` lands in
// `.bss`). PSRAM max chunk ≈ 4095 B/descriptor → ~188 needed for
// 768 KB; 256 is cheap over-allocation (256 × ~12 B ≈ 3 KB DRAM).
const FB_DESCRIPTORS: usize = 256;

static mut FB_DESC: [DmaDescriptor; FB_DESCRIPTORS] =
    [DmaDescriptor::EMPTY; FB_DESCRIPTORS];

/// Owns the 800×480 RGB565 framebuffer in PSRAM.
///
/// [`Framebuffer::new`] returns the `Framebuffer` (CPU draw handle)
/// plus a [`DmaTxBuf`] to hand to `Dpi::send` — both alias the same
/// PSRAM region intentionally.
pub struct Framebuffer {
    ptr: *mut u16,
    len: usize,
}

impl Framebuffer {
    /// Allocate the framebuffer in PSRAM and build the `DmaTxBuf`
    /// over it. Call once, after `psram_allocator!`.
    pub fn new() -> (Self, DmaTxBuf) {
        // `vec!` lands in PSRAM (PSRAM is the global allocator).
        // `.leak()` makes it `&'static mut` — never freed, which is
        // correct: the DMA scans it for the program's lifetime.
        let buf: &'static mut [u16] = vec![0u16; FB_PIXELS].leak();
        let ptr = buf.as_mut_ptr();

        // Byte view for DmaTxBuf; raw ptr retained for CPU drawing.
        let bytes: &'static mut [u8] =
            unsafe { core::slice::from_raw_parts_mut(ptr as *mut u8, FB_BYTES) };
        let desc: &'static mut [DmaDescriptor] =
            unsafe { &mut *core::ptr::addr_of_mut!(FB_DESC) };

        let dma_buf = DmaTxBuf::new(desc, bytes).expect("framebuffer DmaTxBuf");
        (Self { ptr, len: FB_PIXELS }, dma_buf)
    }

    /// Mutable pixel view. Aliases the live DMA scan — tearing-
    /// tolerant spike use only.
    #[inline]
    pub fn pixels(&mut self) -> &mut [u16] {
        unsafe { core::slice::from_raw_parts_mut(self.ptr, self.len) }
    }

    #[inline]
    pub fn fill(&mut self, color: u16) {
        self.pixels().fill(color);
    }

    #[inline]
    pub fn pixel(&mut self, x: usize, y: usize, color: u16) {
        if x < FB_W && y < FB_H {
            unsafe { self.ptr.add(y * FB_W + x).write(color) };
        }
    }

    /// Base address of the framebuffer — for alignment diagnostics.
    #[inline]
    pub fn base_addr(&self) -> usize {
        self.ptr as usize
    }

    /// Flush the CPU data cache for the whole framebuffer so the GDMA
    /// reads current pixels from PSRAM.
    ///
    /// The ESP32-S3 dcache is write-back: CPU pixel writes sit in
    /// cache and do NOT reach PSRAM until flushed. `DmaTxBuf::send`
    /// does a one-time writeback, but (a) it can race the tail of a
    /// large buffer and (b) any draw *after* `send` never flushes at
    /// all. Call this after every batch of CPU draws.
    ///
    /// Mirrors esp-hal's internal `soc::esp32s3::cache_writeback_addr`
    /// (`#[doc(hidden)]`, private `soc` module — not reachable from
    /// here, so we bind the ROM symbols directly). The autoload
    /// suspend/resume around the writeback is load-bearing — it stops
    /// autoloaded cachelines from being written back mid-flush.
    pub fn flush(&self) {
        unsafe extern "C" {
            fn rom_Cache_WriteBack_Addr(addr: u32, size: u32);
            fn Cache_Suspend_DCache_Autoload() -> u32;
            fn Cache_Resume_DCache_Autoload(value: u32);
        }
        unsafe {
            let autoload = Cache_Suspend_DCache_Autoload();
            rom_Cache_WriteBack_Addr(self.ptr as u32, (self.len * 2) as u32);
            Cache_Resume_DCache_Autoload(autoload);
        }
    }
}

// ── embedded-graphics DrawTarget ─────────────────────────────────────

impl OriginDimensions for Framebuffer {
    fn size(&self) -> Size {
        Size::new(FB_W as u32, FB_H as u32)
    }
}

impl DrawTarget for Framebuffer {
    type Color = Rgb565;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(coord, color) in pixels {
            if let (Ok(x), Ok(y)) =
                (usize::try_from(coord.x), usize::try_from(coord.y))
            {
                if x < FB_W && y < FB_H {
                    unsafe {
                        self.ptr.add(y * FB_W + x).write(color.into_storage())
                    };
                }
            }
        }
        Ok(())
    }

    fn fill_solid(
        &mut self,
        area: &Rectangle,
        color: Self::Color,
    ) -> Result<(), Self::Error> {
        let raw = color.into_storage();
        let area = area.intersection(&self.bounding_box());
        for y in area.rows() {
            for x in area.columns() {
                unsafe {
                    self.ptr
                        .add(y as usize * FB_W + x as usize)
                        .write(raw)
                };
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rgb565_pure_red() {
        assert_eq!(rgb565(0xFF, 0x00, 0x00), 0xF800);
    }
    #[test]
    fn rgb565_pure_green() {
        assert_eq!(rgb565(0x00, 0xFF, 0x00), 0x07E0);
    }
    #[test]
    fn rgb565_pure_blue() {
        assert_eq!(rgb565(0x00, 0x00, 0xFF), 0x001F);
    }
}
