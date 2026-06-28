//! Pin map + panel timings for the Elecrow CrowPanel DIS08070H (7" Basic,
//! ESP32-S3-WROOM-1 N4R8, 800×480 RGB TFT, GT911 touch).
//!
//! This file is a 1:1 transcript of `clawft-edge-pad/src/board.rs` —
//! the pin map is hardware and does not change between MCU drivers.
//! See that file for the full reference-chain commentary. Both ports
//! consume the same constants so the wiring story stays canonical.
//!
//! Sources (re-stated here for self-containment):
//! 1. Elecrow Lesson 2 `gfx_conf.h` (LovyanGFX board profile for
//!    `CrowPanel_70`) — `Code/V3.0/Lesson 2 Draw GUI with LovyanGFX/
//!    4.3inch_5inch_7inch/Draw`.
//! 2. FluidTouch `include/config.h` (HARDWARE_BASIC).
//! 3. Factory ESP-IDF reference, this directory has its sdkconfig +
//!    main.cpp at `.planning/devices/crowpanel-display/...`.

#![allow(dead_code)]

// ── Display geometry ────────────────────────────────────────────────
pub const SCREEN_WIDTH: u16 = 800;
pub const SCREEN_HEIGHT: u16 = 480;

// ── RGB565 parallel data bus (16 lines: 5R + 6G + 5B) ───────────────
//
// The ESP32-S3 drives 16 of the panel's 24 RGB888 data lines. The
// panel's low color bits (R0..R2, G0..G1, B0..B2) are not connected
// to the ESP — this is the "RGB888-low-bit hardware reality" called
// out in the session-learnings doc. Sub-LSB shimmer on the Basic SKU
// is a hardware property; no software-side fix exists.
//
// Pin order matters: `esp_lcd_rgb_panel_config_t::data_gpio_nums[16]`
// is wired index-for-index. Per the factory `Bus_RGB.cpp` pin bit-map
// of `{8..15, 0..7}` plus the source-driver wiring, the indices below
// are the canonical CrowPanel mapping:
//
//   data_gpio_nums[0..5]   = B0..B4   (ESP's 5 blue bits  → panel B3..B7)
//   data_gpio_nums[5..11]  = G0..G5   (ESP's 6 green bits → panel G2..G7)
//   data_gpio_nums[11..16] = R0..R4   (ESP's 5 red bits   → panel R3..R7)
pub const LCD_DATA_B0: i32 = 15;
pub const LCD_DATA_B1: i32 = 7;
pub const LCD_DATA_B2: i32 = 6;
pub const LCD_DATA_B3: i32 = 5;
pub const LCD_DATA_B4: i32 = 4;

pub const LCD_DATA_G0: i32 = 9;
pub const LCD_DATA_G1: i32 = 46;
pub const LCD_DATA_G2: i32 = 3;
pub const LCD_DATA_G3: i32 = 8;
pub const LCD_DATA_G4: i32 = 16;
pub const LCD_DATA_G5: i32 = 1;

pub const LCD_DATA_R0: i32 = 14;
pub const LCD_DATA_R1: i32 = 21;
pub const LCD_DATA_R2: i32 = 47;
pub const LCD_DATA_R3: i32 = 48;
pub const LCD_DATA_R4: i32 = 45;

pub const LCD_DE: i32 = 41;
pub const LCD_VSYNC: i32 = 40;
pub const LCD_HSYNC: i32 = 39;
pub const LCD_PCLK: i32 = 0; // strapping pin — do not externally pull at reset

pub const LCD_BACKLIGHT: i32 = 2; // PWM-capable, manually toggled

// ── Panel timings (factory + Elecrow reference) ─────────────────────
// 15 MHz pclk minimum per the session-learnings doc — the source-
// driver class wants ≥ 15-30 kHz line rate, and 12 MHz puts you at
// ~12.9 kHz, below spec. Factory uses 15 MHz; 20 MHz is the upper
// safe bound (factory ESPHome example).
pub const LCD_PCLK_HZ: u32 = 15_000_000;

pub const LCD_HSYNC_FRONT_PORCH: u32 = 40;
pub const LCD_HSYNC_PULSE_WIDTH: u32 = 48;
pub const LCD_HSYNC_BACK_PORCH: u32 = 40;

pub const LCD_VSYNC_FRONT_PORCH: u32 = 1;
pub const LCD_VSYNC_PULSE_WIDTH: u32 = 31;
pub const LCD_VSYNC_BACK_PORCH: u32 = 13;

// HSYNC + VSYNC are active-low (panel idles them high). In
// esp_lcd_rgb_timing_t these map to `hsync_idle_low: 0` and
// `vsync_idle_low: 0` (idle high = !low). DE idles low →
// `de_idle_high: 0`. PCLK: factory `cfg.pclk_active_neg = 1` +
// `cfg.pclk_idle_high = 0` → data clocked out on the falling edge,
// idle low.
pub const LCD_HSYNC_IDLE_LOW: bool = false;
pub const LCD_VSYNC_IDLE_LOW: bool = false;
pub const LCD_DE_IDLE_HIGH: bool = false;
pub const LCD_PCLK_ACTIVE_NEG: bool = true;
pub const LCD_PCLK_IDLE_HIGH: bool = false;

// ── GT911 capacitive touch (I²C) ────────────────────────────────────
//
// SDA=19, SCL=20 at 400 kHz. Hardware-confirmed primary address is
// 0x5D on DIS08070H (per the journal entry in clawft-edge-pad).
// Reset is routed through the PCA9557 expander on this v3.0 board
// revision — *not* a direct ESP GPIO.
pub const TOUCH_I2C_SDA: i32 = 19;
pub const TOUCH_I2C_SCL: i32 = 20;
pub const TOUCH_I2C_FREQ_HZ: u32 = 400_000;
pub const TOUCH_I2C_ADDR: u8 = 0x5D;
pub const TOUCH_I2C_ADDR_FALLBACK: u8 = 0x14;
pub const TOUCH_RST_PIN: i32 = 38; // legacy reference; v3.0 routes via PCA9557
pub const TOUCH_INT_PIN: Option<i32> = None;

// ── Board enable / control GPIOs (held LOW for program lifetime) ────
// Per factory `LvglWidgets-LVGL-7.0.ino` boot sequence + session-
// learnings step 1: GPIOs 38, 17, 18, 42 → LOW immediately on boot.
// 38 = board enable, 17/18/42 = touch + SD control idles. Holding
// these LOW for the lifetime of the program is a factory invariant.
pub const BOARD_ENABLE_PIN: i32 = 38;
pub const TOUCH_CTRL_PIN: i32 = 17;
pub const SD_CTRL_PIN: i32 = 18;
pub const AUX_CTRL_PIN: i32 = 42;

// ── MicroSD (TF) slot — reserved for offline stroke cache ───────────
pub const SD_MOSI: i32 = 11;
pub const SD_MISO: i32 = 13;
pub const SD_CLK: i32 = 12;
pub const SD_CS: i32 = 10;
