//! RGB-DPI display driver — `esp_lcd_panel_rgb` wrapper + `LeafSurface` impl.
//!
//! This replaces the ~1300-line hand-rolled `dpi_surface.rs` from the
//! bare-metal `clawft-edge-pad` port. Bounce buffers, frame sync, the
//! FIFO-skip restart descriptor — all hardware-erratum work that the
//! bare-metal port had to fight through eleven config iterations — is
//! handled inside Espressif's official supported driver.
//!
//! References:
//! - Espressif docs: `esp_lcd_new_rgb_panel`
//!   <https://docs.espressif.com/projects/esp-idf/en/latest/esp32s3/api-reference/peripherals/lcd/rgb_lcd.html>
//! - Factory CrowPanel ESP-IDF reference (LovyanGFX driver, NOT
//!   esp_lcd_panel_rgb — but pin map + timings are canonical):
//!   `.planning/devices/crowpanel-display/CrowPanel-7.0-HMI-ESP32-Display-800x480/example/ESP_IDF/CrowPanel_ESP32_7.0/`
//!
//! Pixel format: the panel takes RGB565 over 16 data lines. Internally
//! we expose a `Rgb888` `DrawTarget` (matches the `LeafSurface`
//! contract used by `weftos-leaf-display::Compositor`) and convert on
//! every pixel write. The conversion is a 3-shift, no-branch fast path
//! and is dwarfed by the cost of any real draw operation.

#![allow(non_upper_case_globals)] // bindgen names

use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use embedded_graphics::mono_font::ascii::{FONT_10X20, FONT_6X10};
use embedded_graphics::mono_font::{MonoFont, MonoTextStyle};
use embedded_graphics::pixelcolor::raw::RawU16;
use embedded_graphics::pixelcolor::{Rgb565, Rgb888};
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Circle, Line, Primitive as EgPrimitive, PrimitiveStyle, PrimitiveStyleBuilder, Rectangle};
use embedded_graphics::text::{Baseline, Text};
use log::warn;

use esp_idf_sys as sys;
use weftos_leaf_renderer::{decode_bitmap, CapabilityMask, SceneSurface};
use weftos_leaf_scene::{
    from_px_q8, BitmapFormat, BuiltinFont, DamageSet, FontFace, Primitive, Rect as SceneRect, Rgba, Style, Transform,
};
use weftos_leaf_types::DisplaySinkCap;

use crate::board;

/// Errors from the LCD driver path.
#[derive(Debug, thiserror::Error)]
pub enum DisplayError {
    #[error("esp_lcd_new_rgb_panel returned {0}")]
    NewPanel(sys::esp_err_t),
    #[error("esp_lcd_panel_reset returned {0}")]
    Reset(sys::esp_err_t),
    #[error("esp_lcd_panel_init returned {0}")]
    Init(sys::esp_err_t),
    #[error("esp_lcd_rgb_panel_get_frame_buffer returned {0}")]
    GetFb(sys::esp_err_t),
    #[error("esp_lcd_panel_draw_bitmap returned {0}")]
    DrawBitmap(sys::esp_err_t),
}

/// Wrapper around an `esp_lcd_panel_handle_t` + two framebuffers.
///
/// Both framebuffers live in PSRAM and are owned by the IDF driver
/// (allocated inside `esp_lcd_new_rgb_panel` when `flags.fb_in_psram`
/// is set, `num_fbs: 2`). The compositor draws into `fbs[back]`;
/// `present` hands that buffer to `esp_lcd_panel_draw_bitmap`, which
/// for a driver-owned buffer is a *flip* (the RGB driver switches the
/// GDMA source at the next frame boundary, `CONFIG_LCD_RGB_RESTART_IN_VSYNC`)
/// plus the PSRAM cache writeback, then waits for VSYNC before the
/// caller may draw into the other buffer. The first hardware flash
/// drew into the single scanned buffer with no sync and the sweep
/// test tore visibly on camera (2026-10-10).
pub struct DpiDisplay {
    panel: sys::esp_lcd_panel_handle_t,
    fbs: [*mut u16; 2],
    back: usize,
    width: u32,
    height: u32,
    vsync_available: bool,
    /// Damage of the frame now on the front buffer (`None` = whole frame).
    /// After a flip the back buffer differs from the front exactly in
    /// those rects, so a partial repaint copies only them (age-2 damage).
    prev_damage: Option<Vec<SceneRect>>,
    /// Damage of the frame being drawn; becomes `prev_damage` at `end_frame`.
    pending_damage: Option<Vec<SceneRect>>,
}

/// Counters bumped from the LCD ISRs; `present` waits on them.
/// `FRAME_FINISH_COUNT` is the bounce-buffer frame boundary — the exact
/// instant `esp_lcd_panel_rgb.c` copies `cur_fb_index` into
/// `bb_fb_index`, i.e. when the old front buffer stops being read.
/// `VSYNC_COUNT` is the fallback when that callback is unavailable.
static FRAME_FINISH_COUNT: AtomicU32 = AtomicU32::new(0);
static VSYNC_COUNT: AtomicU32 = AtomicU32::new(0);
static VSYNC_SEEN: AtomicBool = AtomicBool::new(false);
/// Flip timing, microseconds, for the self-test's rate report.
static FLIP_WAIT_US: AtomicU32 = AtomicU32::new(0);
static FLIP_COUNT: AtomicU32 = AtomicU32::new(0);

/// IRAM-resident callbacks. With `CONFIG_LCD_RGB_ISR_IRAM_SAFE` the
/// driver refuses a callback outside IRAM, hence the explicit section.
/// Each does one relaxed atomic add and nothing else.
#[link_section = ".iram1.lcd_vsync_cb"]
#[inline(never)]
unsafe extern "C" fn lcd_vsync_cb(
    _panel: sys::esp_lcd_panel_handle_t,
    _edata: *const sys::esp_lcd_rgb_panel_event_data_t,
    _user_ctx: *mut c_void,
) -> bool {
    VSYNC_COUNT.fetch_add(1, Ordering::Relaxed);
    VSYNC_SEEN.store(true, Ordering::Relaxed);
    false
}

#[link_section = ".iram1.lcd_frame_finish_cb"]
#[inline(never)]
unsafe extern "C" fn lcd_frame_finish_cb(
    _panel: sys::esp_lcd_panel_handle_t,
    _edata: *const sys::esp_lcd_rgb_panel_event_data_t,
    _user_ctx: *mut c_void,
) -> bool {
    FRAME_FINISH_COUNT.fetch_add(1, Ordering::Relaxed);
    false
}

/// Raw ISR counters `(frame_finish, vsync)` — diagnostic.
pub fn isr_counters() -> (u32, u32) {
    (FRAME_FINISH_COUNT.load(Ordering::Relaxed), VSYNC_COUNT.load(Ordering::Relaxed))
}

/// `(flips, mean wait µs)` since the last call; the self-test logs it.
pub fn take_flip_stats() -> (u32, u32) {
    let n = FLIP_COUNT.swap(0, Ordering::Relaxed);
    let us = FLIP_WAIT_US.swap(0, Ordering::Relaxed);
    (n, if n == 0 { 0 } else { us / n })
}

// SAFETY: the panel handle is opaque and the IDF driver is internally
// thread-safe for `draw_bitmap` (it serialises behind a mutex). We
// only access from one thread anyway (main → display rendering).
unsafe impl Send for DpiDisplay {}

impl DpiDisplay {
    /// Build the panel. Must be called AFTER:
    /// 1. The PCA9557 reset dance (panel out of reset, GT911 RST released).
    /// 2. The backlight has been pulled LOW (hides startup garbage).
    ///
    /// See `main.rs` for the full boot-order sequence.
    pub fn new() -> Result<Self, DisplayError> {
        // Data line map — exactly the pin order used by the bare-metal
        // port (`clawft-edge-pad/src/main.rs` `Dpi::with_dataN`).
        let data_gpio_nums: [i32; 16] = [
            board::LCD_DATA_B0, board::LCD_DATA_B1, board::LCD_DATA_B2,
            board::LCD_DATA_B3, board::LCD_DATA_B4,
            board::LCD_DATA_G0, board::LCD_DATA_G1, board::LCD_DATA_G2,
            board::LCD_DATA_G3, board::LCD_DATA_G4, board::LCD_DATA_G5,
            board::LCD_DATA_R0, board::LCD_DATA_R1, board::LCD_DATA_R2,
            board::LCD_DATA_R3, board::LCD_DATA_R4,
        ];

        let timings = sys::esp_lcd_rgb_timing_t {
            pclk_hz: board::LCD_PCLK_HZ,
            h_res: board::SCREEN_WIDTH as u32,
            v_res: board::SCREEN_HEIGHT as u32,
            hsync_pulse_width: board::LCD_HSYNC_PULSE_WIDTH,
            hsync_back_porch: board::LCD_HSYNC_BACK_PORCH,
            hsync_front_porch: board::LCD_HSYNC_FRONT_PORCH,
            vsync_pulse_width: board::LCD_VSYNC_PULSE_WIDTH,
            vsync_back_porch: board::LCD_VSYNC_BACK_PORCH,
            vsync_front_porch: board::LCD_VSYNC_FRONT_PORCH,
            flags: timing_flags(
                board::LCD_HSYNC_IDLE_LOW,
                board::LCD_VSYNC_IDLE_LOW,
                board::LCD_DE_IDLE_HIGH,
                board::LCD_PCLK_ACTIVE_NEG,
                board::LCD_PCLK_IDLE_HIGH,
            ),
        };

        // Build the config struct. The flags + the psram/dma-burst
        // anon union have to be initialised via the bindgen-generated
        // setters / explicit union init; the rest is plain assignment.
        //
        // Names: bindgen renames the IDF type alias `lcd_clock_source_t`
        // to the underlying `soc_periph_lcd_clk_src_t`, so the enum
        // value is `soc_periph_lcd_clk_src_t_LCD_CLK_SRC_DEFAULT`.
        // The `on_frame_vsync` callback in older IDF versions has
        // been replaced in IDF v5 by `esp_lcd_rgb_panel_register_event_callbacks`
        // — not a field on this struct. Leave it; we don't need a VSYNC
        // callback for the synchronous draw_bitmap path.
        let mut config = sys::esp_lcd_rgb_panel_config_t {
            clk_src: sys::soc_periph_lcd_clk_src_t_LCD_CLK_SRC_DEFAULT,
            timings,
            data_width: 16,
            bits_per_pixel: 16,
            num_fbs: 2, // double-buffered; see the struct doc for why
            // 10-line bounce buffer — Espressif's recommended starting
            // size for 800-wide panels. At 16 bpp this is 800 * 10 * 2 =
            // 16 000 bytes in internal SRAM. Session-learnings doc.
            bounce_buffer_size_px: (board::SCREEN_WIDTH as usize) * 10,
            sram_trans_align: 64,
            // `psram_trans_align` lives inside an anonymous union with
            // `dma_burst_size` (the IDF v5.3+ rename). 64-byte align is
            // the value the factory firmware uses and matches the GDMA
            // burst width on the S3.
            __bindgen_anon_1: sys::esp_lcd_rgb_panel_config_t__bindgen_ty_1 {
                psram_trans_align: 64,
            },
            hsync_gpio_num: board::LCD_HSYNC,
            vsync_gpio_num: board::LCD_VSYNC,
            de_gpio_num: board::LCD_DE,
            pclk_gpio_num: board::LCD_PCLK,
            data_gpio_nums,
            disp_gpio_num: -1, // backlight is GPIO 2, controlled separately
            flags: panel_flags_fb_in_psram(),
        };
        // Suppress unused-ptr warning from the `ptr` import elsewhere.
        let _ = ptr::null::<()>();

        let mut panel: sys::esp_lcd_panel_handle_t = ptr::null_mut();
        let err = unsafe { sys::esp_lcd_new_rgb_panel(&mut config, &mut panel) };
        if err != sys::ESP_OK {
            return Err(DisplayError::NewPanel(err));
        }

        // Reset → init. esp_lcd_panel_rgb's reset is a no-op for "dumb"
        // RGB panels (no command IC), but the symmetric API requires it.
        let err = unsafe { sys::esp_lcd_panel_reset(panel) };
        if err != sys::ESP_OK {
            return Err(DisplayError::Reset(err));
        }
        let err = unsafe { sys::esp_lcd_panel_init(panel) };
        if err != sys::ESP_OK {
            return Err(DisplayError::Init(err));
        }

        // Grab both framebuffer pointers. We hold them for the program
        // lifetime; the IDF driver owns the allocations.
        let mut fb0: *mut c_void = ptr::null_mut();
        let mut fb1: *mut c_void = ptr::null_mut();
        let err = unsafe { sys::esp_lcd_rgb_panel_get_frame_buffer(panel, 2, &mut fb0, &mut fb1) };
        if err != sys::ESP_OK || fb0.is_null() || fb1.is_null() {
            return Err(DisplayError::GetFb(err));
        }

        // VSYNC callback so `present` can wait for the flip to land. If
        // the driver refuses it (callback not in IRAM on this toolchain)
        // fall back to a timed wait rather than failing bring-up.
        let callbacks = sys::esp_lcd_rgb_panel_event_callbacks_t {
            on_vsync: Some(lcd_vsync_cb),
            on_bounce_empty: None,
            on_bounce_frame_finish: Some(lcd_frame_finish_cb),
        };
        let err = unsafe { sys::esp_lcd_rgb_panel_register_event_callbacks(panel, &callbacks, ptr::null_mut()) };
        let vsync_available = err == sys::ESP_OK;
        if !vsync_available {
            warn!("[display] esp_lcd_rgb_panel_register_event_callbacks returned {err}; present() will use a timed wait");
        }

        let width = board::SCREEN_WIDTH as u32;
        let height = board::SCREEN_HEIGHT as u32;
        let mut this = Self { panel, fbs: [fb0 as *mut u16, fb1 as *mut u16], back: 1, width, height, vsync_available, prev_damage: None, pending_damage: None };
        // Both buffers start black so a flip never shows allocator junk.
        for i in 0..2 {
            this.back = i;
            this.frame().clear(Rgb888::BLACK)?;
            this.flip()?;
        }
        this.back = 1;
        Ok(this)
    }

    /// Address of the first framebuffer (diagnostic).
    pub fn framebuffer_addr(&self) -> usize {
        self.fbs[0] as usize
    }

    /// Hand the back buffer to the driver, then wait until the panel is
    /// scanning it before the caller may touch the other buffer.
    ///
    /// In bounce-buffer mode `esp_lcd_panel_draw_bitmap` on a driver-owned
    /// buffer only records `cur_fb_index` (no cache writeback: the bounce
    /// ISR reads the framebuffer through the data cache). The bounce ISR
    /// adopts it at the next frame boundary, which is precisely when it
    /// fires `on_bounce_frame_finish` — so waiting for one of those
    /// events after the request is exact. Measured mean wait ~8 ms
    /// (half a frame) versus ~33 ms for the earlier two-VSYNC bound.
    /// Fallback without the callback: two VSYNCs, then a timed wait.
    fn flip(&mut self) -> Result<(), DisplayError> {
        let err = unsafe {
            sys::esp_lcd_panel_draw_bitmap(
                self.panel,
                0,
                0,
                self.width as i32,
                self.height as i32,
                self.fbs[self.back] as *const c_void,
            )
        };
        if err != sys::ESP_OK {
            return Err(DisplayError::DrawBitmap(err));
        }
        let t0 = Instant::now();
        let deadline = t0 + Duration::from_millis(100);
        if self.vsync_available && VSYNC_SEEN.load(Ordering::Relaxed) {
            let ff0 = FRAME_FINISH_COUNT.load(Ordering::Relaxed);
            let vs0 = VSYNC_COUNT.load(Ordering::Relaxed);
            loop {
                let ff = FRAME_FINISH_COUNT.load(Ordering::Relaxed).wrapping_sub(ff0);
                let vs = VSYNC_COUNT.load(Ordering::Relaxed).wrapping_sub(vs0);
                if ff >= 1 || vs >= 2 {
                    break;
                }
                if Instant::now() > deadline {
                    warn!("[display] no frame boundary within 100 ms; continuing");
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        } else {
            // Callback unavailable (or not fired yet): two frame periods.
            std::thread::sleep(Duration::from_millis(34));
        }
        FLIP_WAIT_US.fetch_add(t0.elapsed().as_micros() as u32, Ordering::Relaxed);
        FLIP_COUNT.fetch_add(1, Ordering::Relaxed);
        self.back ^= 1;
        Ok(())
    }

    /// Bring the back buffer up to date with the front before a partial
    /// repaint. After a flip the back buffer is one frame older than the
    /// front and differs only where that frame drew (`prev_damage`), so
    /// only those rects are copied; a whole-frame copy (~768 KB PSRAM→
    /// PSRAM, ~55 ms measured) happens only when the previous frame was
    /// full or unknown.
    fn sync_back_from_front(&mut self) {
        let (w, h) = (self.width as i32, self.height as i32);
        let rects: Vec<(i32, i32, i32, i32)> = match &self.prev_damage {
            None => vec![(0, 0, w, h)],
            Some(rs) => rs
                .iter()
                .map(|r| {
                    let x0 = from_px_q8(r.x).clamp(0, w);
                    let y0 = from_px_q8(r.y).clamp(0, h);
                    let x1 = (from_px_q8(r.x) + from_px_q8(r.w)).clamp(0, w);
                    let y1 = (from_px_q8(r.y) + from_px_q8(r.h)).clamp(0, h);
                    (x0, y0, x1, y1)
                })
                .collect(),
        };
        let (front, back) = (self.fbs[self.back ^ 1], self.fbs[self.back]);
        for (x0, y0, x1, y1) in rects {
            if x1 <= x0 || y1 <= y0 {
                continue;
            }
            for y in y0..y1 {
                let off = (y * w + x0) as usize;
                // SAFETY: both buffers are width*height u16s owned by the
                // driver for the program lifetime; x0..x1 and y are clamped
                // inside them; the front one is only read by DMA.
                unsafe { ptr::copy_nonoverlapping(front.add(off), back.add(off), (x1 - x0) as usize) };
            }
        }
    }

    /// Self-description for the leaf capability advertisement (the
    /// `.announce` publish is not wired yet).
    #[allow(dead_code)]
    pub fn capability(&self) -> DisplaySinkCap {
        DisplaySinkCap {
            width: self.width,
            height: self.height,
            pixel_format: String::from("rgb565"),
            layers: 4, // Layer::Bg/Widget/Text/Alert
            blend_modes: vec![String::from("normal")],
        }
    }

    /// The back buffer as an `embedded-graphics` target. Callers repaint
    /// it fully: after a flip it holds the frame from two presents ago.
    pub fn frame(&mut self) -> DpiFrame<'_> {
        DpiFrame {
            fb: self.fbs[self.back],
            width: self.width,
            height: self.height,
            _marker: core::marker::PhantomData,
        }
    }

    /// Present the back buffer (flip + wait).
    pub fn present(&mut self) -> Result<(), DisplayError> {
        // A direct repaint may have touched anything: the next partial
        // scene frame must copy the whole front buffer.
        self.prev_damage = None;
        self.flip()
    }
}

/// A back-buffer view that implements `DrawTarget<Color = Rgb888>`.
///
/// On `draw_iter` we convert each Rgb888 pixel to Rgb565 and write
/// directly into the framebuffer. `LeafSurface::present` then issues
/// a single `esp_lcd_panel_draw_bitmap` to push the dirty FB.
pub struct DpiFrame<'a> {
    fb: *mut u16,
    width: u32,
    height: u32,
    _marker: core::marker::PhantomData<&'a mut ()>,
}

impl OriginDimensions for DpiFrame<'_> {
    fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }
}

impl DrawTarget for DpiFrame<'_> {
    type Color = Rgb888;
    type Error = DisplayError;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(coord, color) in pixels {
            let (Ok(x), Ok(y)) = (u32::try_from(coord.x), u32::try_from(coord.y)) else {
                continue;
            };
            if x >= self.width || y >= self.height {
                continue;
            }
            let idx = (y * self.width + x) as usize;
            // Rgb888 → Rgb565 — 5/6/5 high-bit downshift.
            let rgb565 = Rgb565::new(
                color.r() >> 3,
                color.g() >> 2,
                color.b() >> 3,
            );
            let pixel: u16 = RawU16::from(rgb565).into_inner();
            // SAFETY: bounds checked above; framebuffer is width*height u16s.
            unsafe { self.fb.add(idx).write_volatile(pixel); }
        }
        Ok(())
    }

    /// Row-wise fill. `clear` and every solid rectangle go through here;
    /// the per-pixel default was the whole cost of a full repaint, which
    /// double buffering now needs on every frame.
    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        let Some(bottom_right) = area.bottom_right() else { return Ok(()) };
        let x0 = area.top_left.x.clamp(0, self.width as i32) as u32;
        let y0 = area.top_left.y.clamp(0, self.height as i32) as u32;
        let x1 = (bottom_right.x + 1).clamp(0, self.width as i32) as u32;
        let y1 = (bottom_right.y + 1).clamp(0, self.height as i32) as u32;
        if x1 <= x0 || y1 <= y0 {
            return Ok(());
        }
        let rgb565 = Rgb565::new(color.r() >> 3, color.g() >> 2, color.b() >> 3);
        let pixel: u16 = RawU16::from(rgb565).into_inner();
        for y in y0..y1 {
            // SAFETY: x0..x1 and y are inside the width*height buffer.
            let row = unsafe { core::slice::from_raw_parts_mut(self.fb.add((y * self.width + x0) as usize), (x1 - x0) as usize) };
            row.fill(pixel);
        }
        Ok(())
    }
}

/// The vector-leaf backend: `weftos-leaf-renderer` walks the
/// `SceneStore` and calls these three hooks. Ported from the bare-metal
/// `clawft-edge-pad/src/drivers/dpi_surface.rs` (v1: built-in mono
/// fonts, Raw565/Raw8888 bitmaps; vector fonts, compressed bitmaps and
/// paths are skipped with one warning each).
impl SceneSurface for DpiDisplay {
    type Error = DisplayError;

    fn capabilities(&self) -> CapabilityMask {
        CapabilityMask::empty()
    }

    fn begin_frame(&mut self, damage: &DamageSet, viewport: SceneRect) -> Result<(), Self::Error> {
        if !damage.is_full() {
            self.sync_back_from_front();
        }
        self.pending_damage = if damage.is_full() { None } else { Some(damage.rects().to_vec()) };
        let mut f = self.frame();
        if damage.is_full() {
            if viewport.is_empty() {
                f.clear(Rgb888::BLACK)?;
            } else {
                scene_rect_to_eg(viewport).into_styled(PrimitiveStyle::with_fill(Rgb888::BLACK)).draw(&mut f)?;
            }
        } else {
            for r in damage.rects() {
                scene_rect_to_eg(*r).into_styled(PrimitiveStyle::with_fill(Rgb888::BLACK)).draw(&mut f)?;
            }
        }
        Ok(())
    }

    fn draw_primitive(&mut self, primitive: &Primitive, style: &Style, transform: &Transform) -> Result<(), Self::Error> {
        let ox = from_px_q8(transform.x);
        let oy = from_px_q8(transform.y);
        let (fb_w, fb_h) = (self.width as i32, self.height as i32);
        let mut f = self.frame();
        match primitive {
            Primitive::Rect { w, h, radius_q8: _ } => {
                let (w_px, h_px) = (from_px_q8(*w).max(0), from_px_q8(*h).max(0));
                if w_px == 0 || h_px == 0 {
                    return Ok(());
                }
                Rectangle::new(Point::new(ox, oy), Size::new(w_px as u32, h_px as u32))
                    .into_styled(build_eg_style(style))
                    .draw(&mut f)?;
            }
            Primitive::Line { x2, y2, thickness_q8 } => {
                let thickness = ((*thickness_q8 as u32 + 128) >> 8).max(1);
                let color = style.stroke.or(style.fill).unwrap_or(Rgba::WHITE);
                Line::new(Point::new(ox, oy), Point::new(ox + from_px_q8(*x2), oy + from_px_q8(*y2)))
                    .into_styled(PrimitiveStyle::with_stroke(rgba_to_rgb888(color), thickness))
                    .draw(&mut f)?;
            }
            Primitive::Circle { radius_q16 } => {
                let r_px = from_px_q8((*radius_q16 >> 8) as i32).max(0) as u32;
                Circle::new(Point::new(ox - r_px as i32, oy - r_px as i32), r_px.saturating_mul(2))
                    .into_styled(build_eg_style(style))
                    .draw(&mut f)?;
            }
            Primitive::Text { content, face, .. } => {
                let mono: &MonoFont<'static> = match face {
                    FontFace::Builtin(BuiltinFont::Mono6x10) => &FONT_6X10,
                    FontFace::Builtin(BuiltinFont::Mono10x20) => &FONT_10X20,
                    FontFace::Vector { .. } | FontFace::Inline { .. } => {
                        if !WARNED_VECTOR_FONT.swap(true, Ordering::Relaxed) {
                            warn!("[display] vector / inline FontFace not supported in v1 — skipping");
                        }
                        return Ok(());
                    }
                };
                let color = style.fill.unwrap_or(Rgba::WHITE);
                Text::with_baseline(content, Point::new(ox, oy), MonoTextStyle::new(mono, rgba_to_rgb888(color)), Baseline::Top)
                    .draw(&mut f)?;
            }
            Primitive::Bitmap { w, h, format, data } => match format {
                BitmapFormat::Raw565 => {
                    let (w_px, h_px) = (from_px_q8(*w).max(0) as usize, from_px_q8(*h).max(0) as usize);
                    if data.len() != w_px * h_px * 2 {
                        if !WARNED_BITMAP.swap(true, Ordering::Relaxed) {
                            warn!("[display] Raw565 size mismatch: got {} expected {} — skipping", data.len(), w_px * h_px * 2);
                        }
                        return Ok(());
                    }
                    for row in 0..h_px {
                        let dst_y = oy + row as i32;
                        if dst_y < 0 || dst_y >= fb_h {
                            continue;
                        }
                        for col in 0..w_px {
                            let dst_x = ox + col as i32;
                            if dst_x < 0 || dst_x >= fb_w {
                                continue;
                            }
                            let i = (row * w_px + col) * 2;
                            let raw = u16::from_le_bytes([data[i], data[i + 1]]);
                            // SAFETY: dst_x/dst_y bounds-checked above.
                            unsafe { f.fb.add((dst_y * fb_w + dst_x) as usize).write_volatile(raw) };
                        }
                    }
                }
                BitmapFormat::Raw8888 => {
                    let decoded = match decode_bitmap(*w, *h, *format, data) {
                        Ok(d) => d,
                        Err(e) => {
                            if !WARNED_BITMAP.swap(true, Ordering::Relaxed) {
                                warn!("[display] Raw8888 decode failed: {e:?} — skipping");
                            }
                            return Ok(());
                        }
                    };
                    let mut pixels = Vec::with_capacity((decoded.w * decoded.h) as usize);
                    for y in 0..decoded.h {
                        for x in 0..decoded.w {
                            let p = decoded.pixel(x, y);
                            if p.a != 0 {
                                pixels.push(Pixel(Point::new(ox + x as i32, oy + y as i32), rgba_to_rgb888(p)));
                            }
                        }
                    }
                    f.draw_iter(pixels)?;
                }
                BitmapFormat::Qoi | BitmapFormat::Png | BitmapFormat::Rle | BitmapFormat::WebP => {
                    if !WARNED_BITMAP.swap(true, Ordering::Relaxed) {
                        warn!("[display] BitmapFormat {format:?} not supported in v1 — skipping");
                    }
                }
            },
            Primitive::Path { .. } => {
                if !WARNED_PATH.swap(true, Ordering::Relaxed) {
                    warn!("[display] Primitive::Path not supported in v1 — skipping");
                }
            }
        }
        Ok(())
    }

    fn end_frame(&mut self) -> Result<(), Self::Error> {
        self.flip()?;
        self.prev_damage = self.pending_damage.take();
        Ok(())
    }
}

static WARNED_VECTOR_FONT: AtomicBool = AtomicBool::new(false);
static WARNED_BITMAP: AtomicBool = AtomicBool::new(false);
static WARNED_PATH: AtomicBool = AtomicBool::new(false);

fn scene_rect_to_eg(r: SceneRect) -> Rectangle {
    Rectangle::new(
        Point::new(from_px_q8(r.x), from_px_q8(r.y)),
        Size::new(from_px_q8(r.w).max(0) as u32, from_px_q8(r.h).max(0) as u32),
    )
}

fn rgba_to_rgb888(c: Rgba) -> Rgb888 {
    Rgb888::new(c.r, c.g, c.b)
}

fn build_eg_style(style: &Style) -> PrimitiveStyle<Rgb888> {
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

// ── Helpers for the bit-field structs ────────────────────────────────
//
// Bindgen exposes the bit-field struct as opaque storage with `set_*`
// methods. We can't initialise it field-by-field in a literal, so we
// build it via the API.

fn timing_flags(
    hsync_idle_low: bool,
    vsync_idle_low: bool,
    de_idle_high: bool,
    pclk_active_neg: bool,
    pclk_idle_high: bool,
) -> sys::esp_lcd_rgb_timing_t__bindgen_ty_1 {
    let mut f: sys::esp_lcd_rgb_timing_t__bindgen_ty_1 =
        unsafe { core::mem::zeroed() };
    f.set_hsync_idle_low(hsync_idle_low as u32);
    f.set_vsync_idle_low(vsync_idle_low as u32);
    f.set_de_idle_high(de_idle_high as u32);
    f.set_pclk_active_neg(pclk_active_neg as u32);
    f.set_pclk_idle_high(pclk_idle_high as u32);
    f
}

fn panel_flags_fb_in_psram() -> sys::esp_lcd_rgb_panel_config_t__bindgen_ty_2 {
    let mut f: sys::esp_lcd_rgb_panel_config_t__bindgen_ty_2 =
        unsafe { core::mem::zeroed() };
    f.set_fb_in_psram(1);
    f
}
