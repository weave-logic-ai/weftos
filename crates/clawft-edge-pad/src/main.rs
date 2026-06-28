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

    // CONFIG-11 fix 1: synchronous panel-reset block, BEFORE
    // `DpiSurface::new`. Earlier firmware ran the PCA9557 reset dance
    // inside `touch_task`, which spawned AFTER `DpiSurface::new` had
    // already started DPI scanning — so the panel was reset mid-flight
    // every boot and latched whatever bias state was active at the
    // LOW→HIGH transition for the program lifetime. Factory
    // `LvglWidgets-LVGL-7.0.ino:1244-1378` always asserts reset BEFORE
    // `lcd.begin()`; we now follow that order.
    //
    // 4a. Drive board enable / control GPIOs LOW and leak them — they
    //     must stay LOW for the program's lifetime per factory. GPIO 38
    //     is the board enable (was previously toggled in touch_task as
    //     a GT911 RST line — that was wrong; GT911 RST is exclusively
    //     via PCA9557 IO1). GPIOs 17, 18, 42 are touch-screen / SD-card
    //     control lines that factory drives LOW at boot.
    {
        let g38 = Output::new(peripherals.GPIO38, Level::Low, OutputConfig::default());
        let g17 = Output::new(peripherals.GPIO17, Level::Low, OutputConfig::default());
        let g18 = Output::new(peripherals.GPIO18, Level::Low, OutputConfig::default());
        let g42 = Output::new(peripherals.GPIO42, Level::Low, OutputConfig::default());
        core::mem::forget(g38);
        core::mem::forget(g17);
        core::mem::forget(g18);
        core::mem::forget(g42);
        println!("[edge-pad] GPIOs 38/17/18/42 held LOW (board enable + ctrl)");
    }
    Timer::after(Duration::from_millis(200)).await;

    // 4b. Build the I²C bus (GPIO 19 = SDA, 20 = SCL) early so the
    //     PCA9557 reset can run synchronously. This is the same `i2c`
    //     handle that gets handed off to `touch_task` at the end of
    //     main — we just initialise it here first.
    let mut i2c = I2c::new(
        peripherals.I2C0,
        I2cConfig::default().with_frequency(Rate::from_hz(board::TOUCH_I2C_FREQ_HZ)),
    )
    .unwrap()
    .with_sda(peripherals.GPIO19)
    .with_scl(peripherals.GPIO20)
    .into_async();

    // 4c. PCA9557 reset dance — SYNCHRONOUSLY. The expander drives:
    //     IO0 = LCD panel reset, IO1 = GT911 RST. We assert both LOW
    //     (resets held), wait 20 ms, release IO0 (panel out of reset),
    //     wait 100 ms (panel internal LDO + scan-engine boot), then
    //     release IO1 to Hi-Z so the board pull-up takes GT911 RST
    //     high (chip boots its scan engine). After this returns the
    //     panel is in a known-good state, scanning will be quiet, and
    //     the DPI controller can safely start.
    match drivers::pca9557::reset_board_peripherals(&mut i2c).await {
        Ok(addr) => println!(
            "[edge-pad] PCA9557 reset OK @ 0x{:02x} — panel out of reset",
            addr
        ),
        Err(_) => println!(
            "[edge-pad] PCA9557 NOT FOUND on I²C — proceeding without panel reset (display may be unstable)"
        ),
    }
    Timer::after(Duration::from_millis(200)).await;

    // 4d. Backlight OFF for now — turn it on AFTER DPI is scanning a
    //     clean framebuffer to hide the startup garbage frames.
    let mut backlight = Output::new(
        peripherals.GPIO2,
        Level::Low,
        OutputConfig::default(),
    );
    println!("[edge-pad] backlight OFF (GPIO 2 low) — will enable after DPI quiet");

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

    // Build the SceneSurface. `DpiSurface` seals the §8 pattern — a
    // circular PSRAM descriptor ring + full-framebuffer cache
    // writeback — behind the
    // `weftos_leaf_renderer::SceneSurface` trait (Phase C of the
    // vector-first leaf display; see
    // `docs/design/vector-leaf-display.md` §6 and
    // `src/drivers/dpi_surface.rs`). It allocates the framebuffer,
    // builds the ring, kicks `dpi.send(true, ...)`, and forgets the
    // transfer so the GDMA re-scans forever. The day-2 broken
    // `lcd_rgb::Framebuffer` path and the config-12 LeafSurface
    // adapter are both gone; this is the contained, vector-pipeline
    // bus.
    let mut surface = match drivers::dpi_surface::DpiSurface::new(dpi) {
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

    // Phase C boot screen — render a vector scene through
    // `weftos-leaf-renderer` directly into the just-built
    // `DpiSurface`. The producer (Phase E) will replace this initial
    // scene with mesh-driven SceneEnvelopes, but the boot scene
    // stays on the panel until then so the operator always has a
    // visible "leaf alive, waiting" indicator.
    //
    // Scope:
    //  - one Bg layer Rect filling the viewport (opaque dark gray),
    //  - two Text nodes on the Text layer: a title line and a status
    //    line ("subscribed -- waiting for pushes" style).
    //
    // The store is allocated on the heap (SceneStore::new boxes its
    // own BTreeMap) — capability-less alloc lands in Internal SRAM
    // per the heap-region split above. After rendering we drop the
    // store; the panel keeps the painted frame across the swap (the
    // bus owns both buffers) and the mesh task simply doesn't
    // overwrite it.
    {
        use weftos_leaf_renderer::render_damage;
        use weftos_leaf_scene::{
            px, BuiltinFont, DamageSet, FontFace, KerningHint, Layer, Node, NodeId, Primitive,
            Rect as SceneRect, Rgba, SceneOp, SceneStore, Style, Transform,
        };

        const DISPLAY_ID: u8 = 0;
        let mut store = SceneStore::new();
        store.set_viewport(
            DISPLAY_ID,
            SceneRect::from_px(0, 0, board::SCREEN_WIDTH as i32, board::SCREEN_HEIGHT as i32),
        );

        // Background: dark slate, full-viewport rect. Without this
        // the surface clears to opaque black; the slate gives the
        // operator a "yes the FB writes are landing" signal that's
        // distinct from a black DPI signal stuck mid-handshake.
        store.apply_op(
            DISPLAY_ID,
            &SceneOp::Insert(Node {
                id: NodeId::from_parts(DISPLAY_ID, 0x00_0001),
                layer: Layer::Bg,
                transform: Transform::IDENTITY,
                primitive: Primitive::Rect {
                    w: px(board::SCREEN_WIDTH as i32),
                    h: px(board::SCREEN_HEIGHT as i32),
                    radius_q8: 0,
                },
                style: Style::filled(Rgba::opaque(0x10, 0x18, 0x28)),
                input: None,
            }),
        );

        // Title line. Mono10x20 for readability at 800×480.
        store.apply_op(
            DISPLAY_ID,
            &SceneOp::Insert(Node {
                id: NodeId::from_parts(DISPLAY_ID, 0x00_0002),
                layer: Layer::Text,
                transform: Transform::translate(px(40), px(50)),
                primitive: Primitive::Text {
                    content: alloc::string::String::from("clawft-edge-pad :: vector leaf"),
                    face: FontFace::Builtin(BuiltinFont::Mono10x20),
                    size_q8: 20 << 8,
                    weight: 400,
                    kerning: KerningHint::Auto,
                },
                style: Style::filled(Rgba::WHITE),
                input: None,
            }),
        );

        // Status line. Cyan + Mono6x10 for the longer hint string.
        store.apply_op(
            DISPLAY_ID,
            &SceneOp::Insert(Node {
                id: NodeId::from_parts(DISPLAY_ID, 0x00_0003),
                layer: Layer::Text,
                transform: Transform::translate(px(40), px(90)),
                primitive: Primitive::Text {
                    content: alloc::string::String::from(
                        "subscribed -- waiting for pushes (Phase E pending)",
                    ),
                    face: FontFace::Builtin(BuiltinFont::Mono6x10),
                    size_q8: 10 << 8,
                    weight: 400,
                    kerning: KerningHint::Auto,
                },
                style: Style::filled(Rgba::new(0x00, 0xFF, 0xFF, 0xFF)),
                input: None,
            }),
        );

        // First-frame contract: full repaint. The damage walker
        // skips the AABB-intersection check on `is_full()` so every
        // node draws.
        let damage = DamageSet::full();
        match render_damage(&store, DISPLAY_ID, &damage, &mut surface) {
            Ok(stats) => println!(
                "[edge-pad] boot scene rendered: drawn={} skipped_inv={} skipped_off={} skipped_unsup={}",
                stats.drawn,
                stats.skipped_invisible,
                stats.skipped_offscreen,
                stats.skipped_unsupported,
            ),
            Err(e) => println!("[edge-pad] boot scene render error: {:?}", e),
        }
        // store dropped here; the DpiSurface (and the BusRgb inside
        // it) hold the painted frame in PSRAM until the next render
        // pass (Phase E).
    }

    // CONFIG-11 fix 1: 500 ms quiet window with the backlight OFF
    // while the DPI peripheral settles + draws its first few (garbage)
    // frames, then drive the backlight HIGH. This hides the startup
    // garbage AND the boot-scene paint transition from the user.
    Timer::after(Duration::from_millis(500)).await;
    backlight.set_high();
    println!("[edge-pad] backlight ON (GPIO 2 high) — DPI quiet, scanning framebuffer");

    // 6. Allocate the shared SceneStore.
    //
    // Phase E plumbing: the mesh task (display ingest) writes the
    // store; the input task (touch publish) reads it for hit-tests.
    // Both tasks need `&'static Mutex<SceneStore>`, which we get from
    // `mesh::shared_store()` (StaticCell-backed). The store starts
    // empty; the boot scene rendered above doesn't live in the shared
    // store — it lives in the framebuffer only. The first
    // `SceneEnvelope` arriving from the host (typically a
    // `Replace(Scene)` snapshot from `weaver leaf scene ps`) writes
    // the authoritative state.
    let scene_store = mesh::shared_store();

    // 7. WiFi + embassy-net DHCP stack (task #3). The connection +
    //    net-runner tasks are spawned inside `net::start`; the link
    //    comes up asynchronously.
    let stack = net::start(&spawner, peripherals.WIFI);
    println!("[edge-pad] net: WiFi stack started, DHCP pending");

    // 8. Mesh client task #1 — display ingest (Phase E).
    //
    //    Decodes `SceneEnvelope`s off the mesh push topic, applies
    //    them through `SceneStore::apply`, and renders the resulting
    //    `DamageSet` through `render_damage` to the `DpiSurface`. The
    //    surface ownership stays here for the program's lifetime; the
    //    panel keeps showing the last rendered frame across mesh
    //    reconnects.
    if let Err(e) = spawner.spawn(mesh::mesh_task(stack, scene_store, surface)) {
        println!("[edge-pad] mesh_task spawn failed: {:?}", e);
    }

    // 9. Mesh client task #2 — input publish (Phase E).
    //
    //    Polls the GT911 (via the new `weftos-leaf-touch-gt911`
    //    driver), hit-tests every event against `scene_store`, and
    //    publishes `InputEnvelope`s on `mesh.leaf.<pk>.input`. The
    //    I²C bus was built + PCA9557-reset synchronously above; the
    //    task settles for 100 ms before opening the GT911 session.
    //
    //    NOTE (2026-05-14, still current): the GT911 on this board
    //    reports `config version = 0xff` and its POINT_INFO register
    //    is frozen at 0x80 (buffer-ready, 0 touches). Until a valid
    //    config blob is committed (tracked in
    //    `.planning/actors/JOURNALED-ACTOR-INKPAD.md` §8), this task
    //    will probe + heartbeat but produce no touch events. The
    //    plumbing through `hit_test_event` → `InputEnvelope` → mesh
    //    publish is verified in `weftos-leaf-touch-gt911`'s unit
    //    tests; the firmware path lights up the moment the chip
    //    starts emitting frames.
    if let Err(e) = spawner.spawn(mesh::input_task(stack, scene_store, i2c)) {
        println!("[edge-pad] input_task spawn failed: {:?}", e);
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

// `touch_task` has moved into `mesh::input_task` (Phase E). The new
// task owns the GT911 driver from `weftos-leaf-touch-gt911`, hit-tests
// every TouchEvent against the shared `SceneStore`, and publishes
// `InputEnvelope`s on `mesh.leaf.<pk>.input`. See `mesh.rs`.
