//! WeftOS Inkpad Actor firmware — ESP-IDF port.
//!
//! Sibling of `crates/clawft-edge-pad/` (the bare-metal esp-hal spike).
//! This port uses `esp-idf-hal` + `esp-idf-svc` + Espressif's official
//! `esp_lcd_panel_rgb` driver, replacing the eleven-config-iteration
//! hand-rolled DPI driver in the bare-metal port. Bounce buffers,
//! frame sync, FIFO-skip restart, and hardware-erratum compensation
//! are all handled inside Espressif's supported driver.
//!
//! Boot order is canonical (factory `LvglWidgets-LVGL-7.0.ino`
//! lines 1244-1378, restated in the session-learnings doc):
//!
//!  1. GPIOs 38, 17, 18, 42 → LOW (board enable + control idles).
//!  2. 200 ms.
//!  3. I²C up (GPIO 19 SDA / GPIO 20 SCL @ 400 kHz).
//!  4. PCA9557 reset dance, **synchronously**:
//!       all output → 20 ms LOW → IO0 HIGH → 100 ms → IO1 input → 100 ms.
//!  5. 200 ms.
//!  6. Backlight (GPIO 2) → LOW.
//!  7. `esp_lcd_new_rgb_panel` + `esp_lcd_panel_init`.
//!  8. 500 ms (hides startup garbage frames).
//!  9. Backlight → HIGH.
//! 10. Start WiFi, then mesh task (on its own thread) + touch task.
//!
//! References:
//! - `.planning/actors/JOURNALED-ACTOR-INKPAD.md` — Actor contract.
//! - `~/.claude/agents/esp32-s3-rgb-touch-display/...md` — session
//!   learnings, especially "Boot order — panel reset BEFORE DPI".

mod board;
mod display;
mod drivers;
mod mesh;
mod net;
mod wifi_secrets;

use std::thread;
use std::time::Duration;

use esp_idf_hal::gpio::{PinDriver, Pull};
use esp_idf_hal::i2c::{I2cConfig, I2cDriver};
use esp_idf_hal::peripherals::Peripherals;
use esp_idf_hal::units::Hertz;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use log::{error, info};

use weftos_leaf_display::Compositor;

fn main() -> anyhow::Result<()> {
    // ESP-IDF boilerplate — must run first.
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    info!("[edge-pad-idf] clawft-edge-pad-idf v{} boot",
        env!("CARGO_PKG_VERSION"));
    info!("[edge-pad-idf] CrowPanel DIS08070H — ESP32-S3 N4R8 (4 MB flash / 8 MB Octal PSRAM)");

    // Flash-grace window. Once esp_lcd_panel_rgb grabs LCD_CAM + GDMA,
    // espflash auto-reset over CH340 + USBIP is unreliable; a 3 s do-
    // nothing window gives the host a chance to interrupt with a
    // reflash if needed. Cheap insurance, same pattern as bare-metal.
    info!("[edge-pad-idf] flash-grace window (3 s)...");
    thread::sleep(Duration::from_secs(3));

    let peripherals = Peripherals::take()?;
    let sysloop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;

    // ── Step 1+2: board-enable + control GPIOs LOW, then settle. ────
    //
    // GPIO 38 = board enable. Factory holds it LOW from boot 0 for the
    // lifetime of the program. 17/18/42 are touch + SD control idles.
    //
    // We `mem::forget` the pin drivers so they stay LOW for the
    // program lifetime — dropping a PinDriver would reset to Hi-Z.
    let g38 = PinDriver::output(peripherals.pins.gpio38)?;
    let g17 = PinDriver::output(peripherals.pins.gpio17)?;
    let g18 = PinDriver::output(peripherals.pins.gpio18)?;
    let g42 = PinDriver::output(peripherals.pins.gpio42)?;
    let mut g38 = g38;
    let mut g17 = g17;
    let mut g18 = g18;
    let mut g42 = g42;
    g38.set_low()?;
    g17.set_low()?;
    g18.set_low()?;
    g42.set_low()?;
    std::mem::forget(g38);
    std::mem::forget(g17);
    std::mem::forget(g18);
    std::mem::forget(g42);
    info!("[edge-pad-idf] GPIOs 38/17/18/42 held LOW (board enable + ctrl)");

    thread::sleep(Duration::from_millis(200));

    // ── Step 3: I²C bus (GPIO 19 SDA / GPIO 20 SCL @ 400 kHz). ──────
    let i2c_cfg = I2cConfig::new()
        .baudrate(Hertz(board::TOUCH_I2C_FREQ_HZ))
        .scl_enable_pullup(true)
        .sda_enable_pullup(true);
    let mut i2c = I2cDriver::new(
        peripherals.i2c0,
        peripherals.pins.gpio19, // SDA
        peripherals.pins.gpio20, // SCL
        &i2c_cfg,
    )?;

    // ── Step 4: PCA9557 reset dance, synchronously. ─────────────────
    //
    // Same as the bare-metal port: this MUST complete before
    // esp_lcd_new_rgb_panel(). Doing it after a panel-init leaks the
    // mid-flight reset into a permanent latched-bias artifact (see
    // session-learnings doc).
    match drivers::pca9557::reset_board_peripherals(&mut i2c) {
        Ok(addr) => info!(
            "[edge-pad-idf] PCA9557 reset OK @ 0x{:02x} — panel out of reset",
            addr
        ),
        Err(e) => error!(
            "[edge-pad-idf] PCA9557 NOT FOUND on I²C ({e:?}) — proceeding without panel reset (display will be unstable)"
        ),
    }

    thread::sleep(Duration::from_millis(200));

    // ── Step 6: backlight OFF (will turn on AFTER DPI quiets). ──────
    let mut backlight = PinDriver::output(peripherals.pins.gpio2)?;
    backlight.set_low()?;
    info!("[edge-pad-idf] backlight OFF (GPIO 2 low) — will enable after DPI quiet");

    // ── Step 7: esp_lcd_panel_rgb bringup. ──────────────────────────
    //
    // `DpiDisplay::new` calls `esp_lcd_new_rgb_panel` + reset + init
    // and grabs a pointer to the PSRAM-resident framebuffer. The IDF
    // driver kicks the GDMA chain inside `esp_lcd_panel_init`; from
    // here on the panel scans whatever's in the framebuffer.
    let surface = match display::DpiDisplay::new() {
        Ok(s) => {
            info!(
                "[edge-pad-idf] DpiDisplay up — framebuffer @ 0x{:08x}",
                s.framebuffer_addr()
            );
            s
        }
        Err(e) => {
            error!("[edge-pad-idf] DpiDisplay::new FAILED: {e:?}");
            // Park forever so the watchdog doesn't reboot us in a tight loop.
            loop {
                thread::sleep(Duration::from_secs(5));
            }
        }
    };

    // ── Step 8: 500 ms quiet window with backlight OFF. ─────────────
    thread::sleep(Duration::from_millis(500));

    // ── Step 9: backlight HIGH. The panel is now scanning a clean
    // (cleared-to-black) framebuffer; turning on the backlight here
    // hides the startup-garbage frames the user would otherwise see.
    backlight.set_high()?;
    std::mem::forget(backlight); // leak so it stays HIGH for the program lifetime
    info!("[edge-pad-idf] backlight ON (GPIO 2 high) — DPI quiet");

    // ── Step 10a: WiFi. ─────────────────────────────────────────────
    let _wifi = match net::connect_wifi(peripherals.modem, sysloop, nvs) {
        Ok(w) => Some(w),
        Err(e) => {
            error!(
                "[edge-pad-idf] WiFi connect failed: {e:?} — continuing offline; mesh task will keep retrying its TCP connect (will never succeed without link)"
            );
            None
        }
    };

    // ── Step 10b: touch task on a dedicated thread. ────────────────
    //
    // The bare-metal port spawned this as an embassy task; here we
    // dedicate a FreeRTOS-backed std thread. 100 Hz polling matches
    // the bare-metal port (20 ms sleeps).
    thread::Builder::new()
        .name("touch".into())
        .stack_size(4096)
        .spawn(move || touch_loop(i2c))?;

    // ── Step 10c: mesh client on the main thread. ──────────────────
    //
    // Owns the display stack (surface + compositor); blocks forever
    // running the subscribe + receive + render loop.
    let compositor = Compositor::new();
    mesh::run(surface, compositor)
}

/// Touch-input loop. Blocks on the GT911 I²C bus at ~50 Hz; ports the
/// `touch_task` in the bare-metal `main.rs`.
fn touch_loop(i2c: I2cDriver<'static>) {
    use drivers::gt911::Gt911;

    // Small bus settle window — the PCA9557 reset already gave the
    // GT911 time to boot its scan engine; this is just I²C-line idle.
    thread::sleep(Duration::from_millis(100));

    let mut gt911 = match Gt911::new(i2c) {
        Ok(g) => {
            info!(
                "[edge-pad-idf] GT911: probed OK @ addr 0x{:02x} (factory config left intact)",
                g.address()
            );
            g
        }
        Err(e) => {
            error!(
                "[edge-pad-idf] GT911: probe FAILED ({e:?}) — both 0x14 and 0x5D unresponsive"
            );
            return;
        }
    };

    let mut poll: u32 = 0;
    let mut last_info: u8 = 0xAA;
    loop {
        match gt911.read_frame() {
            Ok((info, frame)) => {
                if info != last_info {
                    info!("[edge-pad-idf] GT911 POINT_INFO: 0x{info:02x}");
                    last_info = info;
                }
                if let Some(frame) = frame {
                    for i in 0..frame.touch_count as usize {
                        let p = &frame.points[i];
                        info!(
                            "[edge-pad-idf] touch[{i}]: x={} y={} size={} id={}",
                            p.x, p.y, p.size, p.id
                        );
                    }
                }
            }
            Err(e) => {
                error!("[edge-pad-idf] GT911: read error {e:?}");
                thread::sleep(Duration::from_millis(500));
            }
        }
        if poll % 250 == 0 {
            info!("[edge-pad-idf] GT911 heartbeat (poll {poll})");
        }
        poll = poll.wrapping_add(1);
        thread::sleep(Duration::from_millis(20));
    }
}

// Suppress unused-import for `Pull`; it's not strictly used in this
// boot path but referenced by the GPIO config docs.
#[allow(dead_code)]
fn _pin_pull_marker(_p: Pull) {}
