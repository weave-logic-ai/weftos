//! Pin map + panel timings for the Elecrow CrowPanel DIS08070H (7" Basic,
//! ESP32-S3-WROOM-1 N4R8, 800×480 RGB TFT, GT911 touch).
//!
//! Constants transcribed from two known-good references:
//!
//! 1. Elecrow Lesson 2 `gfx_conf.h` (LovyanGFX board profile for
//!    `CrowPanel_70`) — official Elecrow demo, canonical for the v3.0
//!    board revision. RGB pin map, panel timings, and touch I²C config
//!    come from here.
//!    <https://github.com/Elecrow-RD/CrowPanel-ESP32-Display-Course-File/tree/main/CrowPanel_ESP32_Tutorial/Code/V3.0/Lesson%202%20Draw%20GUI%20with%20LovyanGFX/4.3inch_5inch_7inch/Draw>
//!
//! 2. FluidTouch `include/config.h` — community PlatformIO project on
//!    the same family. Mostly agrees with Elecrow; one conflict noted
//!    inline at the GT911 section.
//!    <https://github.com/jeyeager65/FluidTouch/blob/main/include/config.h>

#![allow(dead_code)] // scaffold; populated by day-2 driver bringup

// ─── Display geometry ────────────────────────────────────────────────
pub const SCREEN_WIDTH: u16 = 800;
pub const SCREEN_HEIGHT: u16 = 480;

// ─── RGB parallel data bus (16-bit RGB565: 5R + 6G + 5B) ────────────
//
// Pin assignments are LCD_CAM `dpi` data lines d0..d15. The Elecrow
// board wires R[4:0], G[5:0], B[4:0] in that order to d11..d15,
// d5..d10, d0..d4 respectively — matching the lgfx Bus_RGB convention.
//
// Important: `pin_pclk = GPIO_NUM_0` is the boot-strap pin. On power-
// on it is sampled to decide boot mode. Driving it via LCD_CAM is
// fine *after* the chip has booted; just don't pull it externally
// during reset.
pub const LCD_DATA_B0: u8 = 15;
pub const LCD_DATA_B1: u8 = 7;
pub const LCD_DATA_B2: u8 = 6;
pub const LCD_DATA_B3: u8 = 5;
pub const LCD_DATA_B4: u8 = 4;

pub const LCD_DATA_G0: u8 = 9;
pub const LCD_DATA_G1: u8 = 46;
pub const LCD_DATA_G2: u8 = 3;
pub const LCD_DATA_G3: u8 = 8;
pub const LCD_DATA_G4: u8 = 16;
pub const LCD_DATA_G5: u8 = 1;

pub const LCD_DATA_R0: u8 = 14;
pub const LCD_DATA_R1: u8 = 21;
pub const LCD_DATA_R2: u8 = 47;
pub const LCD_DATA_R3: u8 = 48;
pub const LCD_DATA_R4: u8 = 45;

pub const LCD_DE: u8 = 41;
pub const LCD_VSYNC: u8 = 40;
pub const LCD_HSYNC: u8 = 39;
pub const LCD_PCLK: u8 = 0; // strapping pin — do not externally pull at reset

pub const LCD_BACKLIGHT: u8 = 2; // PWM-capable, day-1 smoke test toggles this

// ─── Panel timings (lgfx Bus_RGB config) ─────────────────────────────
// 15 MHz pixel clock — 15_000_000 Hz / (800+128) / (480+45) ≈ 30.8 Hz
// refresh. Factory `LvglWidgets-LVGL-7.0.ino` uses 15 MHz; the panel
// class needs ≥ 15–30 kHz line rate for source-driver charge-pumping,
// and at 12 MHz we were at ~12.93 kHz line rate — **below spec**, a
// likely contributor to the fine-grained pixel glitch. 15 MHz puts the
// line rate at ~16.2 kHz, in spec.
pub const LCD_PCLK_FREQ_HZ: u32 = 15_000_000;

pub const LCD_HSYNC_FRONT_PORCH: u16 = 40;
pub const LCD_HSYNC_PULSE_WIDTH: u16 = 48;
pub const LCD_HSYNC_BACK_PORCH: u16 = 40;

pub const LCD_VSYNC_FRONT_PORCH: u16 = 1;
pub const LCD_VSYNC_PULSE_WIDTH: u16 = 31;
pub const LCD_VSYNC_BACK_PORCH: u16 = 13;

pub const LCD_HSYNC_POLARITY_ACTIVE_HIGH: bool = false; // polarity=0 in lgfx
pub const LCD_VSYNC_POLARITY_ACTIVE_HIGH: bool = false;
pub const LCD_PCLK_ACTIVE_NEG: bool = true; // sample on falling edge
pub const LCD_DE_IDLE_HIGH: bool = false;
pub const LCD_PCLK_IDLE_HIGH: bool = false;

// ─── GT911 capacitive touch (I²C, shared bus convention) ────────────
//
// **Reference conflict:**
// - Elecrow gfx_conf.h (CrowPanel_70 block):
//     pin_sda = 19, pin_scl = 20, pin_int = -1, pin_rst = -1,
//     i2c_addr = 0x14, freq = 400_000
// - FluidTouch include/config.h (HARDWARE_BASIC):
//     TOUCH_SDA = 19, TOUCH_SCL = 20, TOUCH_RST = 38, TOUCH_INT = -1
//
// SDA/SCL agree; RST and I²C address diverge. Theory: the v3.0 board
// rev removed software-controlled RST (it's now tied to a hardware
// power-up sequence) and changed the boot-time GT911 address strap
// from 0x5D → 0x14. The Elecrow profile is the v3.0 source of truth.
// If touch fails to enumerate, try GPIO 38 driven as RST per
// FluidTouch's older-rev assumption, and probe both 0x14 and 0x5D.
pub const TOUCH_I2C_SDA: u8 = 19;
pub const TOUCH_I2C_SCL: u8 = 20;
pub const TOUCH_I2C_FREQ_HZ: u32 = 400_000;
// Hardware-confirmed 2026-05-14: this board's GT911 enumerates at
// 0x5D and needs a RST pulse on GPIO 38 to start scanning. The
// `gfx_conf.h` 0x14 / pin_rst=-1 values were for a different
// CrowPanel variant; FluidTouch's `config.h` (0x5D + RST=38 for
// HARDWARE_BASIC) is the correct reference for DIS08070H.
pub const TOUCH_I2C_ADDR: u8 = 0x5D; // confirmed on DIS08070H
pub const TOUCH_I2C_ADDR_FALLBACK: u8 = 0x14;
pub const TOUCH_RST_PIN: u8 = 38; // GT911 reset — MUST pulse to start scanning
pub const TOUCH_INT_PIN: Option<u8> = None; // not connected on the Basic SKU

// ─── MicroSD (TF) slot — SPI on the Basic SKU ───────────────────────
// Per FluidTouch include/config.h §HARDWARE_BASIC. Not used in v0 ink
// spike but reserved for offline stroke cache (journal §1).
pub const SD_MOSI: u8 = 11;
pub const SD_MISO: u8 = 13;
pub const SD_CLK: u8 = 12;
pub const SD_CS: u8 = 10;
