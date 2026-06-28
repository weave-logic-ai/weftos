//! WeftOS Inkpad Actor firmware — ESP32-S3 + Elecrow CrowPanel DIS08070H.
//!
//! Spike scope and acceptance criteria live in
//! `.planning/actors/JOURNALED-ACTOR-INKPAD.md`.
//!
//! Day-2 status:
//! - Boot path + embassy executor: ✅
//! - PSRAM init (64 KiB SRAM + 8 MiB Octal PSRAM heap): ✅
//! - LCD_CAM DPI bringup with solid-color fill: in progress
//! - I²C bus + GT911 touch probe: in progress
//!
//! References:
//! - <https://github.com/infinition/waveshare-watch-rs> (embassy template)
//! - <https://github.com/esp-rs/esp-hal/blob/main/qa-test/src/bin/lcd_dpi.rs>
//! - <https://github.com/Elecrow-RD/CrowPanel-ESP32-Display-Course-File>
//! - <https://github.com/jeyeager65/FluidTouch>

#![no_std]
#![no_main]

extern crate alloc;

mod board;
mod drivers;
mod mesh;
mod net;
mod wifi_secrets;

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::lcd_cam::{
    LcdCam,
    lcd::{
        ClockMode, Phase, Polarity,
        dpi::{Config as DpiConfig, Dpi, Format, FrameTiming},
    },
};
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use esp_println::println;

// Panic / backtrace handlers + global allocator.
use esp_alloc as _;
use esp_backtrace as _;

// ESP-IDF-format app descriptor block required by the bootloader.
esp_bootloader_esp_idf::esp_app_desc!();

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    // 1. Boot the SoC at full speed.
    let peripherals =
        esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::_240MHz));

    // 2. Heap regions. Order matters: the Internal (SRAM) region is
    //    registered FIRST, so capability-less `alloc` — WiFi,
    //    embassy-net, mesh, everything — is served from SRAM and never
    //    touches PSRAM. The PSRAM region is registered second, tagged
    //    `External`: only the LCD framebuffer (dpi_surface.rs, via
    //    `alloc_caps`) ever requests it. This keeps the GDMA's
    //    framebuffer read uncontended — the static-grid diagnostic
    //    proved an uncontended GDMA→PSRAM read is rock-steady, while a
    //    contended one tears into shifting blocks. If 160 KiB OOMs at
    //    boot, drop toward 128 KiB or trim WiFi buffer sizing.
    esp_alloc::heap_allocator!(size: 160 * 1024);
    let (psram_ptr, psram_len) =
        esp_hal::psram::psram_raw_parts(&peripherals.PSRAM);
    unsafe {
        esp_alloc::HEAP.add_region(esp_alloc::HeapRegion::new(
            psram_ptr,
            psram_len,
            esp_alloc::MemoryCapability::External.into(),
        ));
    }

    // 3. Hand TIMG0 to the embassy executor.
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0);

    // 3a. Flash-grace window. Once this firmware grabs LCD_CAM + GDMA
    //     and leaks the DMA transfer, the running peripherals block
    //     espflash's auto-reset handshake over CH340 + USBIP — you
    //     then have to physically BOOT+RESET to reflash. This 3 s
    //     do-nothing window at the top of main (before any peripheral
    //     except the bare timer) gives espflash a reliable connect
    //     window on every subsequent flash. Cheap insurance.
    println!("[edge-pad] flash-grace window (3 s)...");
    Timer::after(Duration::from_secs(3)).await;

    println!("[edge-pad] CrowPanel DIS08070H boot");
    println!("[edge-pad] ESP32-S3-WROOM-1 N4R8 — 4MB flash / 8MB Octal PSRAM");
    println!("[edge-pad] heap free: {} bytes", esp_alloc::HEAP.free());

    // 4. Backlight ON via GPIO 2. The PWM backlight controller is
    //    optional refinement; for day-2 we just drive it HIGH so we
    //    can see whatever the LCD emits.
    let _backlight = Output::new(
        peripherals.GPIO2,
        Level::High,
        OutputConfig::default(),
    );
    println!("[edge-pad] backlight ON (GPIO 2)");

    // 5. LCD_CAM DPI bringup → solid red fill.
    //
    // The CrowPanel 7" panel is a "dumb" RGB-in TFT — no driver IC
    // requiring an init command sequence. We just need to start
    // emitting valid RGB signals with the right pixel clock and
    // sync timings and the panel comes alive.
    //
    // Pin map + timings sourced from board.rs (which sources from
    // Elecrow's gfx_conf.h for CrowPanel_70).
    let lcd_cam = LcdCam::new(peripherals.LCD_CAM);

    let dpi_config = DpiConfig::default()
        .with_clock_mode(ClockMode {
            // (IdleLow, ShiftHigh) — confirmed against Elecrow's own
            // CrowPanel ESP32 lesson-4 .ino, which sets LovyanGFX
            //   pclk_idle_high = 0   → idle low  → Polarity::IdleLow
            //   pclk_active_neg = 1  → data latched on falling edge,
            //                          shifted on rising edge → Phase::ShiftHigh
            // The earlier (IdleLow, ShiftLow) gave a readable image
            // (the data eye is forgiving) but sampled on the wrong
            // edge — a candidate contributor to the drift we saw.
            polarity: Polarity::IdleLow,
            phase: Phase::ShiftHigh,
        })
        .with_frequency(Rate::from_hz(board::LCD_PCLK_FREQ_HZ))
        .with_format(Format {
            enable_2byte_mode: true, // 16-bit RGB565
            ..Default::default()
        })
        .with_timing(FrameTiming {
            // Horizontal: 800 active + 40 front + 48 pulse + 40 back = 928 total
            horizontal_active_width: board::SCREEN_WIDTH as usize,
            horizontal_total_width: (board::SCREEN_WIDTH
                + board::LCD_HSYNC_FRONT_PORCH
                + board::LCD_HSYNC_PULSE_WIDTH
                + board::LCD_HSYNC_BACK_PORCH) as usize,
            horizontal_blank_front_porch: board::LCD_HSYNC_FRONT_PORCH as usize,
            hsync_width: board::LCD_HSYNC_PULSE_WIDTH as usize,
            hsync_position: 0,
            // Vertical: 480 active + 1 front + 31 pulse + 13 back = 525 total
            vertical_active_height: board::SCREEN_HEIGHT as usize,
            vertical_total_height: (board::SCREEN_HEIGHT
                + board::LCD_VSYNC_FRONT_PORCH
                + board::LCD_VSYNC_PULSE_WIDTH
                + board::LCD_VSYNC_BACK_PORCH) as usize,
            vertical_blank_front_porch: board::LCD_VSYNC_FRONT_PORCH as usize,
            vsync_width: board::LCD_VSYNC_PULSE_WIDTH as usize,
        })
        // HSYNC and VSYNC are active-low (polarity=0 in LovyanGFX),
        // so they idle high.
        .with_vsync_idle_level(Level::High)
        .with_hsync_idle_level(Level::High)
        // DE idles low (de_idle_high=0 in LovyanGFX).
        .with_de_idle_level(Level::Low)
        .with_disable_black_region(false);

    let dpi = Dpi::new(lcd_cam.lcd, peripherals.DMA_CH2, dpi_config)
        .unwrap()
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

    // Build the LeafSurface. `DpiSurface` seals the §8 pattern — a
    // circular PSRAM descriptor ring + full-framebuffer cache
    // writeback — behind the `weftos_leaf_display::LeafSurface`
    // trait. It allocates the framebuffer, builds the ring, kicks
    // `dpi.send(true, ...)`, and forgets the transfer so the GDMA
    // re-scans forever. The day-2 broken `lcd_rgb::Framebuffer` path
    // is gone; this is the contained, abstraction-backed bus.
    let surface = match drivers::dpi_surface::DpiSurface::new(dpi) {
        Ok(s) => {
            println!(
                "[edge-pad] DpiSurface up — framebuffer @ 0x{:08x} (align%64 = {})",
                s.framebuffer_addr(),
                s.framebuffer_addr() % 64
            );
            s
        }
        Err(e) => {
            println!("[edge-pad] DpiSurface::new FAILED: {:?}", e);
            loop {
                Timer::after(Duration::from_secs(1)).await;
            }
        }
    };

    // `surface` is moved into `mesh::mesh_task` below — it owns the
    // display stack (surface + Compositor), draws a boot screen, then
    // renders every LeafPush received over the mesh.

    // 6. GPIO 38 held LOW. The v3.0 demo does
    //    `pinMode(38, OUTPUT); digitalWrite(38, LOW)` at the top of
    //    setup() and never changes it — a board enable/select line.
    //    Leak the pin so it stays driven low for the program's life.
    {
        let gpio38 = Output::new(peripherals.GPIO38, Level::Low, OutputConfig::default());
        core::mem::forget(gpio38);
        println!("[edge-pad] GPIO 38 held LOW (board enable)");
    }

    // I²C bus. Shared between the PCA9557 board expander and the
    // GT911 touch controller. The touch_task runs the PCA9557 reset
    // sequence on this bus before probing the GT911.
    let i2c = I2c::new(
        peripherals.I2C0,
        I2cConfig::default().with_frequency(Rate::from_hz(board::TOUCH_I2C_FREQ_HZ)),
    )
    .unwrap()
    .with_sda(peripherals.GPIO19)
    .with_scl(peripherals.GPIO20)
    .into_async();

    if let Err(e) = spawner.spawn(touch_task(i2c)) {
        println!("[edge-pad] touch_task spawn failed: {:?}", e);
    }

    // 7. WiFi + embassy-net DHCP stack (task #3). The connection +
    //    net-runner tasks are spawned inside `net::start`; the link
    //    comes up asynchronously.
    let stack = net::start(&spawner, peripherals.WIFI);
    println!("[edge-pad] net: WiFi stack started, DHCP pending");

    // 8. Mesh client (task #4) — owns the display stack (`surface` +
    //    a fresh `Compositor`), connects to the daemon's mesh
    //    transport, subscribes to this leaf's push topic, and renders
    //    every received `LeafPush`. It waits for the link/DHCP itself.
    if let Err(e) = spawner.spawn(mesh::mesh_task(
        stack,
        surface,
        weftos_leaf_display::Compositor::new(),
    )) {
        println!("[edge-pad] mesh_task spawn failed: {:?}", e);
    }

    // 7. Heartbeat — proves embassy is alive even while the LCD DMA
    //    runs autonomously and the touch task is reading.
    let mut tick: u32 = 0;
    loop {
        Timer::after(Duration::from_secs(1)).await;
        tick = tick.wrapping_add(1);
        println!("[edge-pad] tick {}", tick);
    }
}

#[embassy_executor::task]
async fn touch_task(mut i2c: I2c<'static, esp_hal::Async>) {
    use drivers::gt911::Gt911;
    use drivers::pca9557;

    Timer::after(Duration::from_millis(100)).await; // let the bus settle

    // The v3.0 board routes GT911 RST through a PCA9557 I/O expander.
    // Without pulsing it, the GT911 answers I²C but never scans the
    // panel. This runs the expander reset sequence on the shared bus.
    match pca9557::reset_board_peripherals(&mut i2c).await {
        Ok(addr) => println!("[edge-pad] PCA9557: found @ 0x{:02x}, board peripherals reset", addr),
        Err(_) => println!("[edge-pad] PCA9557: NOT FOUND on I²C — GT911 reset skipped"),
    }

    let mut gt911 = match Gt911::new(i2c).await {
        Ok(g) => {
            println!("[edge-pad] GT911: probed OK @ addr 0x{:02x} (factory config left intact)", g.address());
            g
        }
        Err(_) => {
            println!("[edge-pad] GT911: probe FAILED — both 0x14 and 0x5D unresponsive");
            return;
        }
    };

    // Touch read loop. NOTE (2026-05-14): currently non-functional —
    // the GT911 on this board reports `config version = 0xff` and its
    // POINT_INFO register is frozen at 0x80 (buffer-ready, 0 touches)
    // and never updates, even under hard multi-finger touch. The chip
    // answers I²C and the PCA9557 reset gets its scan engine to run
    // ONE cycle, but without a valid touch-panel config blob it will
    // not sustain scanning. BLOCKED on sourcing/writing a complete
    // GT911 config for the CrowPanel 7" panel — see handoff +
    // JOURNALED-ACTOR-INKPAD.md §8. The loop below is the correct
    // handler; it will start producing touches the moment a valid
    // config is committed to the chip.
    let mut poll: u32 = 0;
    let mut last_info: u8 = 0xAA; // sentinel — forces a first log
    loop {
        match gt911.read_frame().await {
            Ok((info, frame)) => {
                if info != last_info {
                    println!("[edge-pad] GT911 POINT_INFO: 0x{:02x}", info);
                    last_info = info;
                }
                if let Some(frame) = frame {
                    for i in 0..frame.touch_count as usize {
                        let p = &frame.points[i];
                        println!(
                            "[edge-pad] touch[{}]: x={} y={} size={} id={}",
                            i, p.x, p.y, p.size, p.id
                        );
                    }
                }
            }
            Err(_) => {
                println!("[edge-pad] GT911: read error");
                Timer::after(Duration::from_millis(500)).await;
            }
        }
        if poll % 250 == 0 {
            println!("[edge-pad] GT911 heartbeat (poll {})", poll);
        }
        poll = poll.wrapping_add(1);
        Timer::after(Duration::from_millis(20)).await;
    }
}
