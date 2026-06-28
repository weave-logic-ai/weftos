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

use embedded_graphics::pixelcolor::raw::RawU16;
use embedded_graphics::pixelcolor::{Rgb565, Rgb888};
use embedded_graphics::prelude::*;

use esp_idf_sys as sys;
use weftos_leaf_display::LeafSurface;
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

/// Wrapper around an `esp_lcd_panel_handle_t` + the framebuffer ptr.
///
/// The framebuffer lives in PSRAM and is owned by the IDF driver
/// (allocated inside `esp_lcd_new_rgb_panel` when `flags.fb_in_psram`
/// is set). We get a borrow to it via
/// `esp_lcd_rgb_panel_get_frame_buffer`, which the LeafSurface impl
/// uses for direct pixel writes (the fast path).
pub struct DpiDisplay {
    panel: sys::esp_lcd_panel_handle_t,
    fb: *mut u16,
    width: u32,
    height: u32,
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
            num_fbs: 1, // single FB initially; double-buffer is a later optimisation
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

        // Grab the framebuffer pointer. We hold this for the program
        // lifetime; the IDF driver owns the allocation.
        let mut fb: *mut c_void = ptr::null_mut();
        let err = unsafe { sys::esp_lcd_rgb_panel_get_frame_buffer(panel, 1, &mut fb) };
        if err != sys::ESP_OK {
            return Err(DisplayError::GetFb(err));
        }

        Ok(Self {
            panel,
            fb: fb as *mut u16,
            width: board::SCREEN_WIDTH as u32,
            height: board::SCREEN_HEIGHT as u32,
        })
    }

    /// Address of the framebuffer (diagnostic).
    pub fn framebuffer_addr(&self) -> usize {
        self.fb as usize
    }

    /// Force a full-screen refresh — uploads the entire framebuffer
    /// via the IDF driver. Used by `LeafSurface::present`.
    fn flush_full(&mut self) -> Result<(), DisplayError> {
        let err = unsafe {
            sys::esp_lcd_panel_draw_bitmap(
                self.panel,
                0,
                0,
                self.width as i32,
                self.height as i32,
                self.fb as *const c_void,
            )
        };
        if err != sys::ESP_OK {
            return Err(DisplayError::DrawBitmap(err));
        }
        Ok(())
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
}

impl LeafSurface for DpiDisplay {
    type Frame<'a> = DpiFrame<'a>;
    type Error = DisplayError;

    fn capability(&self) -> DisplaySinkCap {
        DisplaySinkCap {
            width: self.width,
            height: self.height,
            pixel_format: String::from("rgb565"),
            layers: 4, // matches Compositor::Bg/Widget/Text/Alert
            blend_modes: vec![String::from("normal")],
        }
    }

    fn frame(&mut self) -> DpiFrame<'_> {
        DpiFrame {
            fb: self.fb,
            width: self.width,
            height: self.height,
            _marker: core::marker::PhantomData,
        }
    }

    fn present(&mut self) -> Result<(), DisplayError> {
        self.flush_full()
    }
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
