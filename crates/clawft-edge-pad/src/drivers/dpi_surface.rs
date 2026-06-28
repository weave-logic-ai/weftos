//! `SceneSurface` implementation for the CrowPanel DIS08070H 800×480
//! RGB-parallel TFT, driven by the ESP32-S3 `LCD_CAM` DPI peripheral
//! through the proven `lgfx-bus-rgb-rs` bus.
//!
//! This is **Phase C** of the WeftOS vector-first leaf display
//! (`docs/design/vector-leaf-display.md` §6). The old `LeafSurface`
//! adapter (config-12) is gone; the renderer the kernel talks to is
//! now [`weftos_leaf_renderer::SceneSurface`], and this file
//! implements it.
//!
//! # Substrate: `lgfx-bus-rgb-rs` (untouched, v0.2.1)
//!
//! After eleven hand-rolled configs of the bus path, the faithful
//! 1:1 port of LovyanGFX `Bus_RGB.cpp` was extracted into its own
//! crate — `lgfx-bus-rgb-rs` — and hardware-verified on this exact
//! panel on 2026-05-15. That crate owns:
//!
//! - the PSRAM framebuffer(s) (`alloc_caps(External, ...)`)
//! - one or two circular GDMA descriptor rings + matching FIFO-skip
//!   restart descriptors (LovyanGFX `Bus_RGB.cpp:220-225`)
//! - the VSYNC ISR that does `out_rst` + outlink re-arm every frame
//!   (LovyanGFX `Bus_RGB.cpp:66-93`, with `#[ram]` IRAM placement and
//!   `Priority3`) — extended in v0.2 with a per-VSYNC page-flip
//!   check when `present()` schedules a swap.
//! - the dcache writeback (`Cache_Suspend_DCache_Autoload` + ROM
//!   `rom_Cache_WriteBack_Addr` + resume)
//!
//! This file is a thin adapter: it wraps `lgfx_bus_rgb_rs::BusRgb`
//! and exposes the `SceneSurface` trait line. It is **not** to be
//! touched by Phase D / E work — only `Cargo.toml` and the renderer
//! API are stable downstream contracts.
//!
//! ## v0.2.1 synchronous double-buffer plumbing
//!
//! With `lgfx-bus-rgb-rs` 0.2.1, `bus.framebuffer_addr()` returns the
//! **offscreen** buffer's address — i.e. the one the GDMA is NOT
//! currently scanning. The address changes across `present()` calls
//! (after the synchronous swap), so every `draw_primitive` /
//! `begin_frame` call re-fetches it through `bus.framebuffer_addr()`.
//! `end_frame` calls `bus.present()` which (a) flushes the dcache for
//! the offscreen buffer, (b) schedules a per-VSYNC swap, (c) **blocks
//! until the ISR has performed the swap** — so the next frame's
//! writes are guaranteed to land on a buffer the GDMA is not
//! scanning. This is the fix for the v0.2.0 race in which rapid push
//! events overwrote freshly-drawn buffers before they were ever
//! displayed.
//!
//! The PCA9557 reset dance, GPIO holds, backlight sequencing,
//! two-region heap (Internal first, External second), and
//! `Dpi::new(...DMA_CH2...)` chain all stay in `main.rs` — exactly
//! the boot order that the bus crate's own example
//! `examples/crowpanel_dis08070h.rs` requires for first-flash
//! bringup.
//!
//! # Pixel format
//!
//! The DPI panel is 16-bit RGB565. The framebuffer is stored RGB565
//! (768 KB per buffer) and the surface byte-swaps each pixel before
//! it lands in PSRAM: the CrowPanel pin map routes the high byte of
//! the 16-bit DMA word to data lines 0..7 (factory `Bus_RGB.cpp:157`
//! `rgb565sig_tbl = {8..15, 0..7}`), so a little-endian `u16` store
//! would arrive byte-swapped on the wire. LovyanGFX's `rgb565_t` is
//! pre-swapped for this same reason. We reuse Phase B's
//! [`weftos_leaf_renderer::to_rgb565_be`] — the canonical, unit-
//! tested implementation of the swap. **Do not reimplement.**
//!
//! # Capability declaration
//!
//! [`SceneSurface::capabilities`] returns [`CapabilityMask::empty()`]
//! — the v1 DPI baseline per design doc §11:
//!
//! - no ALPHA (per-pixel `opacity` 1..=254 collapses to 255 upstream),
//! - no SUBPIXEL (integer rasterizer; renderer rounds Q24.8 → px),
//! - no ANTIALIASED (mono-pixel edges),
//! - no VECTOR_FONTS (only `FontFace::Builtin(Mono6x10 | Mono10x20)`),
//! - no BLEND_MODES (every layer composites at `Normal`),
//! - no BITMAP_QOI / BITMAP_PNG (only `Raw8888` and `Raw565`).
//!
//! Producers SHOULD send within these constraints; the renderer
//! degrades gracefully (logs once, draws best-effort) when they
//! exceed them.

#![allow(dead_code)]

extern crate alloc;

use alloc::vec::Vec;
use core::convert::Infallible;

use embedded_graphics::mono_font::ascii::{FONT_10X20, FONT_6X10};
use embedded_graphics::mono_font::{MonoFont, MonoTextStyle};
use embedded_graphics::pixelcolor::Rgb888;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{
    Circle as EgCircle, Line as EgLine, Primitive as EgPrimitive,
    PrimitiveStyle as EgPrimStyle, PrimitiveStyleBuilder, Rectangle as EgRectangle,
};
use embedded_graphics::text::{Baseline, Text as EgText};
use embedded_graphics::Pixel;

use esp_hal::lcd_cam::lcd::dpi::Dpi;
use esp_hal::DriverMode;
use esp_println::println;

use lgfx_bus_rgb_rs::{BusConfig, BusError, BusRgb, PixelFormat};

use weftos_leaf_renderer::{decode_bitmap, to_rgb565_be, CapabilityMask, SceneSurface};
use weftos_leaf_scene::{
    from_px_q8, BitmapFormat, BuiltinFont, DamageSet, FontFace, Primitive, Rect, Rgba, Style,
    Transform,
};

// ── Panel geometry ───────────────────────────────────────────────────

/// Active pixel width of the CrowPanel DIS08070H.
pub const FB_W: usize = 800;
/// Active pixel height.
pub const FB_H: usize = 480;
const FB_PIXELS: usize = FB_W * FB_H;

/// GDMA channel the `Dpi` was constructed against in `main.rs`
/// (`peripherals.DMA_CH2`). `BusConfig::gdma_channel` must match — the
/// VSYNC ISR re-arms `DMA::regs().ch(GDMA_CHANNEL)` every frame, and
/// esp-hal 1.0 does not expose the channel index off a constructed
/// `Dpi`, so the caller has to repeat it. This is the one documented
/// API leak in `BusConfig`.
const GDMA_CHANNEL: u8 = 2;

// ── Errors ───────────────────────────────────────────────────────────

/// Errors surfaced by [`DpiSurface::new`].
///
/// The draw-side `SceneSurface::Error` is [`Infallible`] — every
/// pixel write is bounds-checked, the cache flush cannot fail, and
/// unsupported primitives are *logged and skipped* rather than
/// propagated (the design doc §6.1 best-effort contract; producers
/// must inspect leaf-side logs to notice).
#[derive(Debug)]
pub enum DpiSurfaceError {
    /// Anything `lgfx_bus_rgb_rs::BusRgb::new` returns:
    ///
    /// - `AlreadyInitialised` — `DpiSurface::new` (and so `BusRgb::new`)
    ///   was called more than once.
    /// - `FramebufferTooLarge` — the framebuffer exceeds the bus crate's
    ///   static descriptor capacity (not hit at 800×480 RGB565).
    /// - `PsramAllocationFailed` — `esp_alloc::HEAP.alloc_caps(External, ...)`
    ///   returned null. Under the two-region heap pattern in `main.rs`,
    ///   PSRAM holds only the framebuffer; on an N8R8 part this means
    ///   the `External` region was not registered. Update `main.rs`.
    /// - `DmaSendFailed` — `Dpi::send` rejected the transfer.
    /// - `VsyncIrqEnableFailed` — the LCD_CAM interrupt vector could
    ///   not be enabled; without the VSYNC ISR the GDMA renders one
    ///   frame and stalls, so this is fatal.
    Bus(BusError),
}

impl From<BusError> for DpiSurfaceError {
    fn from(e: BusError) -> Self {
        DpiSurfaceError::Bus(e)
    }
}

// ── The SceneSurface implementation ──────────────────────────────────

/// `SceneSurface` for the CrowPanel 800×480 RGB-parallel panel.
///
/// Owns a [`BusRgb`] from `lgfx-bus-rgb-rs`. The bus is the proven
/// substrate: it allocates the PSRAM framebuffer(s), builds the
/// circular descriptor ring + FIFO-skip restart descriptor, leaks
/// the `DpiTransfer`, and installs the per-frame VSYNC re-arm ISR.
/// All this file adds is the `SceneSurface` adapter — vector
/// primitives → byte-swapped RGB565 pixels.
///
/// The public name `DpiSurface` is kept identical to the
/// pre-vector-pipeline adapter so `main.rs`'s boot order does not
/// need to change.
pub struct DpiSurface {
    bus: BusRgb,
    /// Last `set_brightness` value recorded — diagnostic only; the
    /// backlight is GPIO 2 owned by `main.rs`. Kept for parity with
    /// the prior adapter; the trait doesn't currently expose
    /// brightness control.
    backlight_on_us: u32,
    /// Set true on the first unsupported primitive of each kind so we
    /// log once and don't flood the UART during steady-state mesh
    /// traffic.
    warned_path: bool,
    warned_bitmap_unsupported: bool,
    warned_vector_font: bool,
}

impl DpiSurface {
    /// Build the surface and start the GDMA scan.
    ///
    /// `dpi` must already be fully pin-wired and clock-configured by
    /// the caller (`main.rs`'s `Dpi::new(...DMA_CH2..).with_pclk(...)
    /// .with_vsync(...)...with_data15(...)` chain). This function hands
    /// the built `Dpi` to `BusRgb::new`, which carves the PSRAM
    /// framebuffer out of the `External` heap region, builds the
    /// descriptor ring, kicks `Dpi::send(true, ...)`, leaks the
    /// transfer, and arms the VSYNC ISR. After this returns the panel
    /// is alive and the GDMA scans the framebuffer for the program's
    /// entire lifetime.
    ///
    /// Signature preserved verbatim from the prior `LeafSurface`
    /// adapter — `main.rs` boot order is untouched.
    pub fn new<'d, Dm>(dpi: Dpi<'d, Dm>) -> Result<Self, DpiSurfaceError>
    where
        Dm: DriverMode,
    {
        let cfg = BusConfig {
            width: FB_W,
            height: FB_H,
            pixel_format: PixelFormat::Rgb565,
            // ESP32-S3 has exactly one LCD_CAM port (`0`).
            port: 0,
            // Must match `peripherals.DMA_CH2` passed to `Dpi::new` in
            // `main.rs`. The bus crate documents this as the one API
            // leak in `BusConfig` (esp-hal 1.0 does not expose the
            // channel off a constructed `Dpi`).
            gdma_channel: GDMA_CHANNEL,
        };
        let bus = BusRgb::new(dpi, cfg)?;
        Ok(Self {
            bus,
            backlight_on_us: 0,
            warned_path: false,
            warned_bitmap_unsupported: false,
            warned_vector_font: false,
        })
    }

    /// Base address of the (offscreen, in double-buffer mode) PSRAM
    /// framebuffer — for the `align%64` diagnostic line `main.rs`
    /// prints after bringup.
    ///
    /// In double-buffer mode this can change between calls; the
    /// returned value is correct at the moment of the call.
    #[inline]
    pub fn framebuffer_addr(&self) -> usize {
        self.bus.framebuffer_addr() as usize
    }

    /// Whether the underlying bus is double-buffered. Diagnostic;
    /// `main.rs` logs this at boot so the operator can confirm.
    #[inline]
    pub fn is_double_buffered(&self) -> bool {
        self.bus.is_double_buffered()
    }

    /// Diagnostic: record a brightness request. Trait doesn't surface
    /// this yet; backlight is a `main.rs`-owned GPIO. Kept here so a
    /// future LEDC hookup has a documented entry point.
    pub fn set_brightness(&mut self, on_us: u32) {
        self.backlight_on_us = on_us;
    }
}

// ── Framebuffer view: a `DrawTarget<Color = Rgb888>` over PSRAM ──────

/// Internal scratch wrapper: lets us hand the offscreen framebuffer to
/// `embedded-graphics` primitive rasterizers as a `Rgb888` draw
/// target. Every pixel write is narrowed `Rgb888 → Rgba → byte-swapped
/// RGB565` on the way into PSRAM via Phase B's
/// [`to_rgb565_be`] helper.
///
/// Borrows the framebuffer slice for the duration of one drawing
/// operation. Not exposed publicly — the only consumer is
/// `DpiSurface::draw_primitive`.
struct FbView<'a> {
    buf: &'a mut [u16],
}

impl<'a> FbView<'a> {
    /// Build a view over the current offscreen framebuffer.
    ///
    /// SAFETY: `bus.framebuffer_addr()` returns the base of a region
    /// of `FB_PIXELS * 2` bytes that the bus owns for the program's
    /// lifetime; we hold `&mut DpiSurface` so we own exclusive
    /// Rust-side write access for the borrow's duration.
    #[inline]
    fn from_bus(bus: &'a mut BusRgb) -> Self {
        let ptr = bus.framebuffer_addr() as *mut u16;
        let buf = unsafe { core::slice::from_raw_parts_mut(ptr, FB_PIXELS) };
        Self { buf }
    }

    /// Fill a pixel-coordinate (post-Q24.8-resolve) rect with a
    /// pre-swapped RGB565 word. Used by `begin_frame` damage clears.
    fn fill_px_rect(&mut self, x: i32, y: i32, w: i32, h: i32, raw: u16) {
        let x0 = x.max(0) as usize;
        let y0 = y.max(0) as usize;
        let x1 = (x + w).max(0) as usize;
        let y1 = (y + h).max(0) as usize;
        let x0 = x0.min(FB_W);
        let y0 = y0.min(FB_H);
        let x1 = x1.min(FB_W);
        let y1 = y1.min(FB_H);
        for yy in y0..y1 {
            let row = yy * FB_W;
            self.buf[row + x0..row + x1].fill(raw);
        }
    }
}

impl OriginDimensions for FbView<'_> {
    fn size(&self) -> Size {
        Size::new(FB_W as u32, FB_H as u32)
    }
}

impl DrawTarget for FbView<'_> {
    type Color = Rgb888;
    type Error = Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(coord, color) in pixels {
            let (Ok(x), Ok(y)) =
                (usize::try_from(coord.x), usize::try_from(coord.y))
            else {
                continue;
            };
            if x >= FB_W || y >= FB_H {
                continue;
            }
            self.buf[y * FB_W + x] = rgb888_to_swapped_565(color);
        }
        Ok(())
    }

    fn fill_solid(
        &mut self,
        area: &EgRectangle,
        color: Self::Color,
    ) -> Result<(), Self::Error> {
        let raw = rgb888_to_swapped_565(color);
        let area = area.intersection(&self.bounding_box());
        for y in area.rows() {
            let yu = y as usize;
            if yu >= FB_H {
                continue;
            }
            let row_off = yu * FB_W;
            for x in area.columns() {
                let xu = x as usize;
                if xu >= FB_W {
                    continue;
                }
                self.buf[row_off + xu] = raw;
            }
        }
        Ok(())
    }

    fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
        let raw = rgb888_to_swapped_565(color);
        self.buf.fill(raw);
        Ok(())
    }
}

// ── Colour conversions ───────────────────────────────────────────────

/// Narrow an `embedded-graphics` `Rgb888` to wire-format RGB565 via
/// Phase B's canonical [`to_rgb565_be`]. The detour through
/// `Rgba` is intentional: it keeps **one** definition of the swap
/// logic (in the renderer crate) and lets the sim verify it via
/// `cargo test` on the host.
#[inline]
fn rgb888_to_swapped_565(c: Rgb888) -> u16 {
    to_rgb565_be(Rgba::new(c.r(), c.g(), c.b(), 0xFF))
}

/// Project a scene `Rgba` to `embedded-graphics`' `Rgb888`. Alpha is
/// dropped here — the DPI surface declares `!ALPHA`, so the renderer
/// has already collapsed `opacity` 1..=254 to 255 upstream. Fully
/// transparent (`a == 0`) is also filtered upstream
/// (`Style::is_invisible`).
#[inline]
fn rgba_to_rgb888(c: Rgba) -> Rgb888 {
    Rgb888::new(c.r, c.g, c.b)
}

// ── The trait impl ──────────────────────────────────────────────────

impl SceneSurface for DpiSurface {
    type Error = Infallible;

    fn capabilities(&self) -> CapabilityMask {
        // Phase C / design doc §11 baseline: this panel has no native
        // ALPHA, no SUBPIXEL, no ANTIALIASED, no VECTOR_FONTS, no
        // BITMAP_QOI / BITMAP_PNG, no BLEND_MODES. The renderer
        // honours these via opacity-collapse, integer rounding,
        // mono-pixel rasterization, and degrade-to-Normal blending.
        CapabilityMask::empty()
    }

    fn begin_frame(&mut self, damage: &DamageSet, viewport: Rect) -> Result<(), Self::Error> {
        // Clear damage to opaque black using Phase B's canonical
        // byte-swapped RGB565 encoder. This is the *whole point* of
        // the damage-driven pipeline — we DO NOT touch unaffected
        // regions. Producer-side scene snapshot ops emit
        // `DamageSet::full()` for a full repaint, which is the only
        // path that clears the entire FB.
        let raw_black = to_rgb565_be(Rgba::BLACK);
        let mut fb = FbView::from_bus(&mut self.bus);
        if damage.is_full() {
            // Clear within the viewport rather than the whole FB — if
            // the producer has a viewport smaller than the panel, the
            // surround stays whatever the prior frame painted (the
            // borrow-checker doesn't help us here; we trust the
            // producer to use the full panel extent or live with the
            // surround).
            let vx = from_px_q8(viewport.x);
            let vy = from_px_q8(viewport.y);
            let vw = from_px_q8(viewport.w);
            let vh = from_px_q8(viewport.h);
            if viewport.is_empty() {
                // No viewport set yet (boot path before any
                // `set_viewport` call). Clear the whole FB so the
                // panel doesn't show garbage on first paint.
                fb.fill_px_rect(0, 0, FB_W as i32, FB_H as i32, raw_black);
            } else {
                fb.fill_px_rect(vx, vy, vw, vh, raw_black);
            }
        } else {
            for r in damage.rects() {
                let rx = from_px_q8(r.x);
                let ry = from_px_q8(r.y);
                let rw = from_px_q8(r.w);
                let rh = from_px_q8(r.h);
                fb.fill_px_rect(rx, ry, rw, rh, raw_black);
            }
        }
        Ok(())
    }

    fn draw_primitive(
        &mut self,
        primitive: &Primitive,
        style: &Style,
        transform: &Transform,
    ) -> Result<(), Self::Error> {
        // v1 honours translation only; rotation/scale fall back to
        // translation per design doc §5.4.
        let ox = from_px_q8(transform.x);
        let oy = from_px_q8(transform.y);

        // Fresh framebuffer view per primitive — `bus.framebuffer_addr()`
        // is stable for the lifetime of the `&mut self` borrow.
        let mut fb = FbView::from_bus(&mut self.bus);

        match primitive {
            Primitive::Rect { w, h, radius_q8: _ } => {
                // v1 ignores `radius_q8` — sharp corners. Rounded
                // rects land in v1.1 with the SUBPIXEL/AA path.
                let w_px = from_px_q8(*w).max(0);
                let h_px = from_px_q8(*h).max(0);
                if w_px == 0 || h_px == 0 {
                    return Ok(());
                }
                let rect = EgRectangle::new(
                    Point::new(ox, oy),
                    Size::new(w_px as u32, h_px as u32),
                );
                let eg_style = build_eg_style(style);
                let _ = rect.into_styled(eg_style).draw(&mut fb);
            }
            Primitive::Line {
                x2,
                y2,
                thickness_q8,
            } => {
                let x2_px = from_px_q8(*x2);
                let y2_px = from_px_q8(*y2);
                let thickness = ((*thickness_q8 as u32 + 128) >> 8).max(1);
                let color = style.stroke.or(style.fill).unwrap_or(Rgba::WHITE);
                let eg_style =
                    EgPrimStyle::with_stroke(rgba_to_rgb888(color), thickness);
                let _ = EgLine::new(
                    Point::new(ox, oy),
                    Point::new(ox + x2_px, oy + y2_px),
                )
                .into_styled(eg_style)
                .draw(&mut fb);
            }
            Primitive::Circle { radius_q16 } => {
                // Q16.16 → Q24.8 → px.
                let r_q8: i32 = (*radius_q16 >> 8) as i32;
                let r_px = from_px_q8(r_q8).max(0) as u32;
                let diameter = r_px.saturating_mul(2);
                let top_left = Point::new(ox - r_px as i32, oy - r_px as i32);
                let eg_style = build_eg_style(style);
                let _ = EgCircle::new(top_left, diameter)
                    .into_styled(eg_style)
                    .draw(&mut fb);
            }
            Primitive::Text { content, face, .. } => {
                // v1: built-in mono fonts only. Phase B's GlyphCache
                // is currently unused on the DPI path because
                // `embedded-graphics::MonoTextStyle` rasterizes
                // directly from its compile-time-embedded bitmap
                // tables (no allocation, no LRU eviction). v1.1's
                // vector-font path will plumb the cache through.
                let mono: &MonoFont<'static> = match face {
                    FontFace::Builtin(BuiltinFont::Mono6x10) => &FONT_6X10,
                    FontFace::Builtin(BuiltinFont::Mono10x20) => &FONT_10X20,
                    FontFace::Vector { .. } | FontFace::Inline { .. } => {
                        if !self.warned_vector_font {
                            println!(
                                "[dpi-surface] vector / inline FontFace not supported in v1 — skipping"
                            );
                            self.warned_vector_font = true;
                        }
                        return Ok(());
                    }
                };
                let color = style.fill.unwrap_or(Rgba::WHITE);
                let text_style: MonoTextStyle<'_, Rgb888> =
                    MonoTextStyle::new(mono, rgba_to_rgb888(color));
                let _ = EgText::with_baseline(
                    content,
                    Point::new(ox, oy),
                    text_style,
                    Baseline::Top,
                )
                .draw(&mut fb);
            }
            Primitive::Bitmap {
                w,
                h,
                format,
                data,
            } => match format {
                BitmapFormat::Raw565 => {
                    // Fast path: source words are RGB565 LE in `data`;
                    // we copy them across with `.swap_bytes()` to land
                    // them in the panel's expected high-byte-first
                    // wire order. Bounds-checked against the
                    // framebuffer extent.
                    let w_px = from_px_q8(*w).max(0) as usize;
                    let h_px = from_px_q8(*h).max(0) as usize;
                    let expected = w_px * h_px * 2;
                    if data.len() != expected {
                        if !self.warned_bitmap_unsupported {
                            println!(
                                "[dpi-surface] Raw565 size mismatch: got {} expected {} — skipping",
                                data.len(),
                                expected
                            );
                            self.warned_bitmap_unsupported = true;
                        }
                        return Ok(());
                    }
                    for row in 0..h_px {
                        let dst_y = oy + row as i32;
                        if dst_y < 0 || (dst_y as usize) >= FB_H {
                            continue;
                        }
                        let dst_row_off = (dst_y as usize) * FB_W;
                        let src_row_off = row * w_px * 2;
                        for col in 0..w_px {
                            let dst_x = ox + col as i32;
                            if dst_x < 0 || (dst_x as usize) >= FB_W {
                                continue;
                            }
                            let lo = data[src_row_off + col * 2];
                            let hi = data[src_row_off + col * 2 + 1];
                            // Source is host-LE u16; wire wants swapped.
                            let le_word = u16::from_le_bytes([lo, hi]);
                            fb.buf[dst_row_off + dst_x as usize] = le_word.swap_bytes();
                        }
                    }
                }
                BitmapFormat::Raw8888 => {
                    // Convert per-pixel via Phase B's decoder, then
                    // narrow each RGBA to swapped RGB565. The decoder
                    // also validates the size envelope, so a
                    // malformed bitmap (which the renderer doesn't
                    // filter — `aabb()` only checks `w/h`) skips
                    // here without scribbling on the FB.
                    let decoded = match decode_bitmap(*w, *h, *format, data) {
                        Ok(d) => d,
                        Err(e) => {
                            if !self.warned_bitmap_unsupported {
                                println!(
                                    "[dpi-surface] Raw8888 decode failed: {:?} — skipping",
                                    e
                                );
                                self.warned_bitmap_unsupported = true;
                            }
                            return Ok(());
                        }
                    };
                    let mut pixels: Vec<Pixel<Rgb888>> =
                        Vec::with_capacity((decoded.w * decoded.h) as usize);
                    for y in 0..decoded.h {
                        for x in 0..decoded.w {
                            let p = decoded.pixel(x, y);
                            if p.a == 0 {
                                continue; // fully transparent — skip
                            }
                            pixels.push(Pixel(
                                Point::new(ox + x as i32, oy + y as i32),
                                rgba_to_rgb888(p),
                            ));
                        }
                    }
                    let _ = fb.draw_iter(pixels);
                }
                BitmapFormat::Qoi
                | BitmapFormat::Png
                | BitmapFormat::Rle
                | BitmapFormat::WebP => {
                    if !self.warned_bitmap_unsupported {
                        println!(
                            "[dpi-surface] BitmapFormat {:?} not supported in v1 — skipping",
                            format
                        );
                        self.warned_bitmap_unsupported = true;
                    }
                }
            },
            Primitive::Path { .. } => {
                // v1.1: lyon-style path rasterization. v1 logs once
                // and skips.
                if !self.warned_path {
                    println!("[dpi-surface] Primitive::Path not supported in v1 — skipping");
                    self.warned_path = true;
                }
            }
        }
        Ok(())
    }

    fn end_frame(&mut self) -> Result<(), Self::Error> {
        // v0.2.1: `bus.present()` is **synchronous** in double-buffer
        // mode — it flushes the offscreen buffer's dcache, schedules
        // a swap, and blocks until the next VSYNC ISR performs the
        // swap (worst-case one frame period, ~33 ms at 30 Hz; 100 ms
        // internal watchdog). When this returns, the next
        // `begin_frame` / `draw_primitive` call writes to a buffer
        // the GDMA is not scanning.
        //
        // In single-buffer mode (`--no-default-features` on the bus
        // crate) this is cache writeback only, no wait.
        self.bus.present();
        Ok(())
    }
}

/// Build an embedded-graphics `PrimitiveStyle` from a scene `Style`.
/// Fill and stroke colours are converted Rgba → Rgb888; the
/// `FbView::draw_iter` / `fill_solid` adapter then byte-swaps them
/// into wire-format RGB565 at the framebuffer boundary.
fn build_eg_style(style: &Style) -> EgPrimStyle<Rgb888> {
    let mut b = PrimitiveStyleBuilder::new();
    if let Some(fill) = style.fill {
        b = b.fill_color(rgba_to_rgb888(fill));
    }
    if let Some(stroke) = style.stroke {
        let w = ((style.stroke_width_q8 as u32 + 128) >> 8).max(1);
        b = b.stroke_color(rgba_to_rgb888(stroke)).stroke_width(w);
    }
    b.build()
}
