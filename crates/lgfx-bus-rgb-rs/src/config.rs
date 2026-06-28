// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-3-Clause
//
// Derived from LovyanGFX (https://github.com/lovyan03/LovyanGFX) —
// copyright lovyan03 et al., BSD-3-Clause (FreeBSD).
//
// Source: components/LovyanGFX-master/src/lgfx/v1/platforms/esp32s3/Bus_RGB.hpp

//! Configuration mirror of LovyanGFX `Bus_RGB::config_t` (Bus_RGB.hpp:47-96).
//!
//! Fields are intentionally a near-1:1 copy of the C++ struct so the
//! port stays auditable side-by-side with the reference source.

/// Pixel format of the RGB-DPI bus.
///
/// LovyanGFX `Bus_RGB.cpp:152` derives `pixel_bytes` from
/// `_cfg.panel->getWriteDepth() & bit_mask`. Here we expose the choice
/// directly since the consumer-side `panel` abstraction does not apply
/// — esp-hal's `Format::enable_2byte_mode` is the underlying control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// 8-bit RGB332 — one byte per pixel, esp-hal `enable_2byte_mode = false`.
    Rgb332 = 1,
    /// 16-bit RGB565 — two bytes per pixel, esp-hal `enable_2byte_mode = true`.
    /// The common case; what the CrowPanel DIS08070H wants.
    Rgb565 = 2,
}

impl PixelFormat {
    /// Bytes per pixel — used to size the framebuffer and to compute
    /// the FIFO-skip restart offset.
    ///
    /// Mirrors Bus_RGB.cpp:152
    /// `uint8_t pixel_bytes = (_cfg.panel->getWriteDepth() & bit_mask) >> 3;`
    #[inline]
    pub const fn bytes_per_pixel(self) -> usize {
        self as usize
    }
}

/// Configuration for [`crate::BusRgb`].
///
/// Mirror of LovyanGFX `Bus_RGB::config_t` (Bus_RGB.hpp:47-96), minus
/// the fields esp-hal 1.0 covers itself (the `Dpi::new` clock /
/// timing / polarity / pin chain inside `lcd_cam/lcd/dpi.rs`). This
/// crate's `BusRgb::new` consumes a built `Dpi` and only adds the
/// LovyanGFX-specific machinery that esp-hal does not provide:
///
/// 1. The PSRAM framebuffer + circular descriptor ring (Bus_RGB.cpp:195-217)
/// 2. The FIFO-skip restart descriptor (Bus_RGB.cpp:220-225)
/// 3. The VSYNC ISR with `out_rst` + outlink re-arm (Bus_RGB.cpp:66-93)
///
/// So this struct is the smaller subset of `config_t` that those three
/// pieces consume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BusConfig {
    /// Active pixel width.
    ///
    /// Mirrors `_cfg.panel->width()` (Bus_RGB.cpp:230).
    pub width: usize,

    /// Active pixel height.
    ///
    /// Mirrors `_cfg.panel->height()` (Bus_RGB.cpp:236).
    pub height: usize,

    /// Bytes per pixel.
    ///
    /// LovyanGFX takes this off the bound panel object; we take it
    /// directly. Bus_RGB.cpp:152.
    pub pixel_format: PixelFormat,

    /// LCD_CAM port. ESP32-S3 has exactly one (`0`) per Bus_RGB.hpp:51
    /// comment. Kept for parity; the value is ignored on S3.
    pub port: u8,

    /// GDMA out-channel index the `Dpi` was constructed against.
    ///
    /// LovyanGFX discovers this at runtime with `search_dma_out_ch`
    /// (Bus_RGB.cpp:170); under esp-hal 1.0 the channel is chosen by
    /// the caller when they build the `Dpi` (e.g.
    /// `Dpi::new(.., peripherals.DMA_CH2, ..)` → `2`). esp-hal does
    /// not expose the channel index off a constructed `Dpi`, so the
    /// caller must repeat it here. This is the one and only place the
    /// LovyanGFX-to-esp-hal API mismatch leaks through.
    pub gdma_channel: u8,
}

impl BusConfig {
    /// Total framebuffer size in bytes (`width * height * pixel_bytes`).
    ///
    /// Mirrors Bus_RGB.cpp:195
    /// `size_t fb_len = (_cfg.panel->width() * pixel_bytes) * _cfg.panel->height();`
    #[inline]
    pub const fn fb_bytes(&self) -> usize {
        self.width * self.height * self.pixel_format.bytes_per_pixel()
    }

    /// Row stride in bytes (`width * pixel_bytes`).
    #[inline]
    pub const fn stride_bytes(&self) -> usize {
        self.width * self.pixel_format.bytes_per_pixel()
    }
}
