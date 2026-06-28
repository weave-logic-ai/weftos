// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-3-Clause
//
// Minimal bringup example for the Elecrow CrowPanel DIS08070H 7" Basic
// (ESP32-S3-WROOM-1 N4R8, 800x480 RGB TFT, GT911 touch).
//
// Brings up the RGB-DPI bus via lgfx-bus-rgb-rs, fills the framebuffer
// with a vertical RGB gradient, calls present(), and loops.
//
// Pin map + timings transcribed from Elecrow's LovyanGFX board profile
// for `CrowPanel_70`:
//   https://github.com/Elecrow-RD/CrowPanel-ESP32-Display-Course-File/
//
// This example covers the bus only. A real Inkpad firmware also has
// to do the PCA9557 panel-reset dance BEFORE this example runs — see
// the JOURNALED-ACTOR-INKPAD spec, or the README of the
// `clawft-edge-pad` crate this port was extracted from.

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::delay::Delay;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::lcd_cam::{
    lcd::{
        dpi::{Config as DpiConfig, Dpi, Format, FrameTiming},
        ClockMode, Phase, Polarity,
    },
    LcdCam,
};
use esp_hal::main;
use esp_hal::time::Rate;
use esp_println::println;

use lgfx_bus_rgb_rs::{BusConfig, BusRgb, PixelFormat};

// ESP-IDF-format app descriptor block required by the bootloader.
esp_bootloader_esp_idf::esp_app_desc!();

// ── Panel geometry ───────────────────────────────────────────────────
const SCREEN_WIDTH: u16 = 800;
const SCREEN_HEIGHT: u16 = 480;

// ── PCLK + porches ───────────────────────────────────────────────────
//
// 15 MHz pclk. The CrowPanel panel class needs ≥ 15 kHz line rate for
// source-driver charge-pumping; 12 MHz puts the line rate below spec
// (~12.9 kHz) and produces a fine horizontal noise. 15 MHz → ~16.2 kHz.
const LCD_PCLK_FREQ_HZ: u32 = 15_000_000;

const LCD_HSYNC_FRONT_PORCH: u16 = 40;
const LCD_HSYNC_PULSE_WIDTH: u16 = 48;
const LCD_HSYNC_BACK_PORCH: u16 = 40;

const LCD_VSYNC_FRONT_PORCH: u16 = 1;
const LCD_VSYNC_PULSE_WIDTH: u16 = 31;
const LCD_VSYNC_BACK_PORCH: u16 = 13;

// ── GDMA channel — must match the DMA_CH<N> passed to Dpi::new ───────
const GDMA_CHANNEL: u8 = 2;

#[main]
fn main() -> ! {
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::_240MHz));
    let delay = Delay::new();

    // ── Flash-grace window ───────────────────────────────────────────
    // Once LCD_CAM + GDMA are running and the descriptor chain is
    // leaked, the live peripherals block espflash's auto-reset over
    // CH340. A 3 s do-nothing window at the top of main gives espflash
    // a reliable connect window on every subsequent flash.
    println!("[lgfx-bus-rgb] flash-grace window (3 s)...");
    delay.delay_millis(3000);

    // ── Board-enable + I2S idle pins held LOW for program lifetime ───
    //
    // Factory firmware holds GPIO 38 (board enable) + 17/18/42 (I2S
    // idles) LOW at boot 0 and never touches them again. Skipping this
    // leaves them floating during DPI bring-up which can produce
    // residual artifacts; on some board revisions GPIO 38 floating also
    // disables the LCD power gate entirely.
    let g38 = Output::new(peripherals.GPIO38, Level::Low, OutputConfig::default());
    let g17 = Output::new(peripherals.GPIO17, Level::Low, OutputConfig::default());
    let g18 = Output::new(peripherals.GPIO18, Level::Low, OutputConfig::default());
    let g42 = Output::new(peripherals.GPIO42, Level::Low, OutputConfig::default());
    core::mem::forget(g38);
    core::mem::forget(g17);
    core::mem::forget(g18);
    core::mem::forget(g42);

    // Backlight (GPIO 2) starts LOW. We turn it on AFTER the bus is
    // scanning a clean framebuffer to hide startup garbage frames.
    let mut backlight = Output::new(peripherals.GPIO2, Level::Low, OutputConfig::default());

    delay.delay_millis(200);

    // ── Heap setup ───────────────────────────────────────────────────
    //
    // Two-region heap pattern (the "fix B" pattern documented in the
    // module-level doc of `clawft-edge-pad/src/drivers/dpi_surface.rs`):
    //
    // 1. Internal-SRAM region FIRST — capability-less `alloc` (Vec,
    //    Box, anything from the global allocator) is served here.
    // 2. PSRAM region tagged `External` — only `alloc_caps(External, ...)`
    //    requests target it. `BusRgb::new` is the only consumer.
    //
    // This keeps PSRAM bandwidth contention from poisoning the GDMA's
    // framebuffer read — empirically the difference between "rock-
    // steady" and "blinking blocks all over the screen".
    esp_alloc::heap_allocator!(size: 160 * 1024);
    let (psram_ptr, psram_len) = esp_hal::psram::psram_raw_parts(&peripherals.PSRAM);
    unsafe {
        esp_alloc::HEAP.add_region(esp_alloc::HeapRegion::new(
            psram_ptr,
            psram_len,
            esp_alloc::MemoryCapability::External.into(),
        ));
    }

    println!("[lgfx-bus-rgb] CrowPanel DIS08070H bringup example");
    println!(
        "[lgfx-bus-rgb] heap free: {} bytes",
        esp_alloc::HEAP.free()
    );

    // ── LCD_CAM DPI setup ────────────────────────────────────────────
    //
    // `ClockMode { Polarity::IdleLow, Phase::ShiftHigh }` maps to
    // LovyanGFX's `pclk_idle_high = 0` + `pclk_active_neg = 1` (the
    // panel's preferred mode — data shifted on the rising edge, latched
    // on the falling).
    let lcd_cam = LcdCam::new(peripherals.LCD_CAM);

    let dpi_config = DpiConfig::default()
        .with_clock_mode(ClockMode {
            polarity: Polarity::IdleLow,
            phase: Phase::ShiftHigh,
        })
        .with_frequency(Rate::from_hz(LCD_PCLK_FREQ_HZ))
        .with_format(Format {
            enable_2byte_mode: true, // RGB565
            ..Default::default()
        })
        .with_timing(FrameTiming {
            horizontal_active_width: SCREEN_WIDTH as usize,
            horizontal_total_width: (SCREEN_WIDTH
                + LCD_HSYNC_FRONT_PORCH
                + LCD_HSYNC_PULSE_WIDTH
                + LCD_HSYNC_BACK_PORCH) as usize,
            horizontal_blank_front_porch: LCD_HSYNC_FRONT_PORCH as usize,
            hsync_width: LCD_HSYNC_PULSE_WIDTH as usize,
            hsync_position: 0,
            vertical_active_height: SCREEN_HEIGHT as usize,
            vertical_total_height: (SCREEN_HEIGHT
                + LCD_VSYNC_FRONT_PORCH
                + LCD_VSYNC_PULSE_WIDTH
                + LCD_VSYNC_BACK_PORCH) as usize,
            vertical_blank_front_porch: LCD_VSYNC_FRONT_PORCH as usize,
            vsync_width: LCD_VSYNC_PULSE_WIDTH as usize,
        })
        // HSYNC and VSYNC are active-low → idle high.
        .with_vsync_idle_level(Level::High)
        .with_hsync_idle_level(Level::High)
        .with_de_idle_level(Level::Low)
        .with_disable_black_region(false);

    let dpi = Dpi::new(lcd_cam.lcd, peripherals.DMA_CH2, dpi_config)
        .expect("Dpi::new failed")
        .with_vsync(peripherals.GPIO40)
        .with_hsync(peripherals.GPIO39)
        .with_de(peripherals.GPIO41)
        .with_pclk(peripherals.GPIO0)
        // Blue (5 bits) → data0..data4
        .with_data0(peripherals.GPIO15)
        .with_data1(peripherals.GPIO7)
        .with_data2(peripherals.GPIO6)
        .with_data3(peripherals.GPIO5)
        .with_data4(peripherals.GPIO4)
        // Green (6 bits) → data5..data10
        .with_data5(peripherals.GPIO9)
        .with_data6(peripherals.GPIO46)
        .with_data7(peripherals.GPIO3)
        .with_data8(peripherals.GPIO8)
        .with_data9(peripherals.GPIO16)
        .with_data10(peripherals.GPIO1)
        // Red (5 bits) → data11..data15
        .with_data11(peripherals.GPIO14)
        .with_data12(peripherals.GPIO21)
        .with_data13(peripherals.GPIO47)
        .with_data14(peripherals.GPIO48)
        .with_data15(peripherals.GPIO45);

    // ── Hand off to the bus driver ───────────────────────────────────
    let cfg = BusConfig {
        width: SCREEN_WIDTH as usize,
        height: SCREEN_HEIGHT as usize,
        pixel_format: PixelFormat::Rgb565,
        port: 0,
        gdma_channel: GDMA_CHANNEL,
    };
    let mut bus = match BusRgb::new(dpi, cfg) {
        Ok(b) => b,
        Err(e) => {
            println!("[lgfx-bus-rgb] BusRgb::new failed: {:?}", e);
            loop {
                core::hint::spin_loop();
            }
        }
    };
    println!(
        "[lgfx-bus-rgb] bus up — framebuffer @ {:p}, {} bytes",
        bus.framebuffer_addr(),
        bus.framebuffer_len()
    );

    // ── Draw STATIC TEXT as a tearing-vs-static disambiguation ──────
    //
    // This replaces the smooth gradient with high-contrast text, then
    // never touches the framebuffer again. If the text is CLEAN (no
    // glitch) the bus driver is fine and the integrated-edge-pad glitch
    // is compose-during-scan tearing (the firmware rewrites the FB on
    // every push event). If the text is GLITCHY here, the cause is
    // somewhere we haven't found yet (pclk, color order, panel timing).
    //
    // Uses embedded-graphics' FONT_10X20 — the same font the WeftOS
    // compositor uses for kernel.ps rendering. Same draw path as text
    // pushes, just static instead of dynamic.
    use embedded_graphics::draw_target::DrawTarget;
    use embedded_graphics::geometry::{OriginDimensions, Point, Size};
    use embedded_graphics::mono_font::{ascii::FONT_10X20, MonoTextStyle};
    use embedded_graphics::pixelcolor::Rgb888;
    use embedded_graphics::primitives::Rectangle;
    use embedded_graphics::text::Text;
    use embedded_graphics::Drawable;
    use embedded_graphics::Pixel;

    let fb_ptr = bus.framebuffer_addr() as *mut u16;
    let w = cfg.width as u32;
    let h = cfg.height as u32;
    let len = bus.framebuffer_len() / 2;

    // SAFETY: bus.framebuffer_addr() returns a unique, aligned region
    // of bus.framebuffer_len() bytes that we exclusively own.
    let fb_slice: &mut [u16] = unsafe { core::slice::from_raw_parts_mut(fb_ptr, len) };

    struct StaticFb<'a> {
        buf: &'a mut [u16],
        w: u32,
        h: u32,
    }
    impl OriginDimensions for StaticFb<'_> {
        fn size(&self) -> Size {
            Size::new(self.w, self.h)
        }
    }
    impl DrawTarget for StaticFb<'_> {
        type Color = Rgb888;
        type Error = core::convert::Infallible;
        fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
        where
            I: IntoIterator<Item = Pixel<Self::Color>>,
        {
            for Pixel(p, c) in pixels {
                if let (Ok(x), Ok(y)) = (u32::try_from(p.x), u32::try_from(p.y)) {
                    if x < self.w && y < self.h {
                        let raw = rgb565_swapped(
                            embedded_graphics::pixelcolor::RgbColor::r(&c),
                            embedded_graphics::pixelcolor::RgbColor::g(&c),
                            embedded_graphics::pixelcolor::RgbColor::b(&c),
                        );
                        self.buf[(y * self.w + x) as usize] = raw;
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
            use embedded_graphics::pixelcolor::RgbColor;
            let raw = rgb565_swapped(color.r(), color.g(), color.b());
            let x0 = area.top_left.x.max(0) as u32;
            let y0 = area.top_left.y.max(0) as u32;
            let x1 = (area.top_left.x + area.size.width as i32)
                .clamp(0, self.w as i32) as u32;
            let y1 = (area.top_left.y + area.size.height as i32)
                .clamp(0, self.h as i32) as u32;
            for y in y0..y1 {
                let row = (y * self.w) as usize;
                for x in x0..x1 {
                    self.buf[row + x as usize] = raw;
                }
            }
            Ok(())
        }
    }

    let mut frame = StaticFb {
        buf: fb_slice,
        w,
        h,
    };

    // Clear to black first.
    let _ = frame.fill_solid(
        &Rectangle::new(Point::new(0, 0), Size::new(w, h)),
        Rgb888::new(0, 0, 0),
    );

    // Draw representative process-table-style lines.
    let header = MonoTextStyle::new(&FONT_10X20, Rgb888::new(0, 255, 255));
    let running = MonoTextStyle::new(&FONT_10X20, Rgb888::new(0, 255, 0));
    let degraded = MonoTextStyle::new(&FONT_10X20, Rgb888::new(255, 255, 0));
    let normal = MonoTextStyle::new(&FONT_10X20, Rgb888::new(255, 255, 255));

    let mut y = 32_i32;
    let _ = Text::new("PID     AGENT             STATE", Point::new(8, y), header)
        .draw(&mut frame);
    y += 28;
    let rows: [(&str, _); 7] = [
        ("0       kernel            running", running),
        ("1       agent             running", running),
        ("2       cron              running", running),
        ("3       assess            degraded", degraded),
        ("4       container         running", running),
        ("5       chain             running", running),
        ("6       ecc               running", running),
    ];
    for (line, style) in rows {
        let _ = Text::new(line, Point::new(8, y), style).draw(&mut frame);
        y += 28;
    }
    // ── Diagnostic lines to localise the prior "bottom shift" ────────
    //
    // The earlier static-text test reported clean text on rows 1-8 but
    // a horizontal wrap on the two widest lines at y=380/410. This
    // block triangulates whether the trigger is width (long string) or
    // high-Y position. Three test lines, each marked with where it
    // sits on screen.
    let white = MonoTextStyle::new(&FONT_10X20, Rgb888::new(255, 255, 255));

    // (A) SHORT line at HIGH Y — was bottom shifted? If clean here, the
    //     issue is the width of the string, not the y-coordinate.
    let _ = Text::new("Y=380 SHORT", Point::new(8, 380), white).draw(&mut frame);

    // (B) LONG line at MID Y — same width as the prior shifted text,
    //     but well above the previous problem area.
    let _ = Text::new(
        "Y=320 LONG: this line is exactly as wide as the prior shifted",
        Point::new(8, 320),
        white,
    )
    .draw(&mut frame);

    // (C) LONG line at HIGH Y — both factors together, to confirm.
    let _ = Text::new(
        "Y=440 LONG: long line near bottom; was this wrapping right?",
        Point::new(8, 440),
        white,
    )
    .draw(&mut frame);

    bus.present();
    println!("[lgfx-bus-rgb] static text drawn — scanning, no further writes...");

    // ── Backlight ON ─────────────────────────────────────────────────
    // 500 ms after the bus starts scanning is the factory's "hide
    // startup garbage frames" hedge.
    delay.delay_millis(500);
    backlight.set_high();
    core::mem::forget(backlight);
    println!("[lgfx-bus-rgb] backlight ON (GPIO 2 high)");

    loop {
        // Bus and GDMA scan continue forever; the VSYNC ISR re-arms
        // every frame. Nothing more for the main loop to do in this
        // smoke test.
        core::hint::spin_loop();
    }
}

/// RGB888 → byte-swapped RGB565. See the crate's README "RGB565 byte
/// order" note for why the swap is necessary on the CrowPanel pin map.
#[inline]
const fn rgb565_swapped(r: u8, g: u8, b: u8) -> u16 {
    let r5 = (r as u16 >> 3) & 0x1F;
    let g6 = (g as u16 >> 2) & 0x3F;
    let b5 = (b as u16 >> 3) & 0x1F;
    ((r5 << 11) | (g6 << 5) | b5).swap_bytes()
}
