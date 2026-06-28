// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-3-Clause
//
// Derived from LovyanGFX (https://github.com/lovyan03/LovyanGFX) —
// copyright lovyan03 et al., BSD-3-Clause (FreeBSD).
//
// Source: components/LovyanGFX-master/src/lgfx/v1/platforms/esp32s3/Bus_RGB.{cpp,hpp}

//! # lgfx-bus-rgb-rs
//!
//! A faithful no_std Rust port of LovyanGFX's **`Bus_RGB`** — the
//! ESP32-S3 RGB-DPI bus driver — to esp-hal 1.0.
//!
//! ## Why this crate exists
//!
//! esp-hal 1.0 exposes the `LCD_CAM` DPI peripheral
//! (`esp_hal::lcd_cam::lcd::dpi::Dpi`) and can drive an RGB-parallel
//! TFT, but its DMA story for **PSRAM-backed framebuffers** is
//! incomplete: it has no bounce-buffer support
//! ([esp-rs/esp-hal#5262](https://github.com/esp-rs/esp-hal/issues/5262)),
//! no VSYNC-anchored re-arm, and no equivalent of LovyanGFX's
//! FIFO-skip restart descriptor (the workaround for the LCD_CAM's
//! downstream async output FIFO holding ~64 bytes of pre-fetched data
//! across an `out_rst`).
//!
//! Until those land upstream, the working alternatives on hand are:
//!
//! 1. LovyanGFX, via `esp-idf-sys` + the C++ component — heavy, full
//!    IDF dependency tree.
//! 2. ESP-IDF `esp_lcd_panel_rgb` via `esp-idf-hal` — official, has
//!    bounce buffers, but pulls in IDF.
//! 3. **This crate** — faithful 1:1 Rust port of `Bus_RGB.cpp`,
//!    no_std, esp-hal 1.0, no IDF.
//!
//! ## Architecture
//!
//! `Bus_RGB::init()` does five things; esp-hal 1.0 already handles
//! three of them. This crate adds the other two:
//!
//! | `Bus_RGB.cpp` | esp-hal 1.0 covers? | This crate |
//! |---|---|---|
//! | A. Pin muxing + DMA channel allocation (135-167) | yes — `Dpi::with_*` chain | n/a |
//! | B. GDMA `conf0` / `conf1` for burst + external mem (179-191) | yes — `DmaTxBuffer::prepare → Preparation { burst_transfer, accesses_psram }` | n/a |
//! | C. PSRAM framebuffer + **circular descriptor chain** + **FIFO-skip restart descriptor** (195-225) | **no** | **yes** ([`descriptor`]) |
//! | D. LCD_CAM clocks / `lcd_user` / `lcd_misc` / `lcd_ctrl[1,2]` (228-302) | yes — `Dpi::apply_config` | n/a |
//! | E. **VSYNC ISR** + `lc_dma_int_ena` + `lcd_start` (66-94, 304-314) | partial — `Dpi::send` does `lcd_start`; the ISR is ours | **yes** ([`isr`]) |
//!
//! The honest divergence: where esp-hal already does the C++'s work
//! correctly, we use esp-hal. We don't re-implement clock dividers or
//! pin muxing from raw register pokes just to look like a "pure" port,
//! because that would replace a known-good upstream with hand-rolled
//! code that has not been bench-tested. The two LovyanGFX-specific
//! pieces esp-hal does NOT cover (the FIFO-skip restart descriptor and
//! the VSYNC ISR) are translated line-by-line with `Bus_RGB.cpp:LINE`
//! citations in the source.
//!
//! ## Usage
//!
//! See `examples/crowpanel_dis08070h.rs` for a full pin map and
//! timings targeting the Elecrow CrowPanel 7" Basic (DIS08070H).
//! Short version:
//!
//! ```ignore
//! // 1. Register a PSRAM heap region tagged External (Internal first,
//! //    External second — see the example for the two-region setup).
//! // 2. Build the esp-hal Dpi with your pin map.
//! let lcd_cam = LcdCam::new(peripherals.LCD_CAM);
//! let dpi = Dpi::new(lcd_cam.lcd, peripherals.DMA_CH2, dpi_config)
//!     .unwrap()
//!     .with_pclk(peripherals.GPIO0)
//!     /* ...with_vsync/hsync/de/data0..data15... */;
//!
//! // 3. Hand it to BusRgb. After this returns the GDMA is scanning
//! //    and the panel is alive.
//! let cfg = BusConfig {
//!     width: 800,
//!     height: 480,
//!     pixel_format: PixelFormat::Rgb565,
//!     port: 0,
//!     gdma_channel: 2,  // must match DMA_CH2 above
//! };
//! let mut bus = BusRgb::new(dpi, cfg).unwrap();
//!
//! // 4. Draw and present.
//! //    Cast bus.framebuffer_addr() to *mut u16 for RGB565, write
//! //    pixels, then call bus.present() to flush the dcache.
//! bus.present();
//! ```
//!
//! ## Double-buffering (v0.2+, default-on, synchronous since v0.2.1)
//!
//! The `double-buffer` Cargo feature (default-on) extends the bus
//! with two PSRAM framebuffers + two descriptor rings. The VSYNC ISR
//! swaps which ring it re-arms against when [`BusRgb::present`] is
//! called. This eliminates the compose-during-scan tearing that the
//! v0.1 single-buffer path exhibits when the consumer rewrites the
//! framebuffer at non-VSYNC moments.
//!
//! Behavioural deltas vs single-buffer:
//!
//! - [`BusRgb::framebuffer_addr`] returns the OFFSCREEN buffer's
//!   address. The address changes across `present()` calls — the
//!   caller must re-fetch after every `present()`.
//! - [`BusRgb::present`] flushes the offscreen dcache, sets a
//!   per-VSYNC swap flag the ISR consumes, **then blocks until the
//!   ISR has performed the swap** (synchronous, ≤1 frame period
//!   worst-case; 100 ms watchdog). The next `framebuffer_addr()`
//!   after `present()` returns is guaranteed to point at a buffer
//!   the GDMA is not scanning.
//! - [`BusRgb::is_double_buffered`] returns `true`. Consumers that
//!   need different draw strategies for the two modes can branch on
//!   this.
//!
//! The synchronous semantics (v0.2.1) fix a race in v0.2.0 where
//! rapid back-to-back `present()` calls could overwrite a freshly-
//! written buffer before it was ever displayed — the second
//! `present()` would see the not-yet-swapped scanning index and
//! treat the just-written buffer as offscreen again. v0.2.1 blocks
//! until the swap commits, mirroring LovyanGFX's LVGL-side
//! `flush_ready` handshake.
//!
//! Cost: 2× framebuffer PSRAM (1.5 MB total at 800×480 RGB565 vs
//! 768 KB single-buffer). On the CrowPanel N4R8 / N8R8 boards with
//! 8 MB Octal PSRAM this is comfortable; the heap split documented
//! in the example leaves the rest for capability-less allocations
//! that land in internal SRAM anyway.
//!
//! ## Divergence from LovyanGFX
//!
//! LovyanGFX does NOT double-buffer at the bus layer — its
//! `Panel_FrameBufferBase` owns a single framebuffer and LVGL above
//! it handles double-buffering. This crate's double-buffer is a
//! deliberate divergence motivated by callers who do not have an
//! LVGL-style layer above the bus and need tear-free output on the
//! bus itself.
//!
//! To match LovyanGFX bit-for-bit, build with
//! `--no-default-features` (or `default-features = false`).
//!
//! ## Caveats
//!
//! - **RGB565 byte order.** The CrowPanel pin map routes the *high*
//!   byte of the 16-bit DMA word to data lines 0..7 (factory
//!   `Bus_RGB.cpp:157` `rgb565sig_tbl = {8..15, 0..7}`). If you store
//!   little-endian `u16` pixels and the panel is on this pin map, the
//!   bytes arrive swapped — pre-swap each pixel with `.swap_bytes()`
//!   or alter your pin map.
//! - **PCLK pin is GPIO 0** on most CrowPanel revisions. GPIO 0 is
//!   also a strapping pin: external pulls during boot can brick the
//!   board. The LCD_CAM driving it post-boot is fine.
//! - **PSRAM must be registered as `External` capability** in
//!   `esp_alloc::HEAP` before calling `BusRgb::new`. The example
//!   shows the two-region (Internal default + External PSRAM) pattern;
//!   if you skip it, `BusRgb::new` returns `PsramAllocationFailed`.
//!   In double-buffer mode the PSRAM budget is 2 × `fb_bytes()`.

#![no_std]
#![allow(clippy::missing_safety_doc)] // SAFETY contracts are in-line above each unsafe block

extern crate alloc;

pub mod bus;
pub mod config;
pub mod descriptor;
pub mod isr;

pub use bus::{BusError, BusRgb};
pub use config::{BusConfig, PixelFormat};
