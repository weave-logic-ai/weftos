# lgfx-bus-rgb-rs

A faithful no_std Rust port of LovyanGFX's **`Bus_RGB.cpp`** — the
ESP32-S3 RGB-DPI bus driver — to esp-hal 1.0.

> **License:** MIT OR Apache-2.0 OR BSD-3-Clause. The BSD-3-Clause is
> required because this crate is a **derivative work** of
> [LovyanGFX](https://github.com/lovyan03/LovyanGFX) (copyright
> lovyan03 et al.; FreeBSD/BSD-3-Clause licensed). The MIT/Apache-2.0
> are added for compatibility with the Rust ecosystem norm — pick
> whichever fits your project, but **the BSD-3-Clause attribution
> must be carried forward**.

## What it is

`esp-hal` 1.0 exposes the `LCD_CAM` DPI peripheral via
`esp_hal::lcd_cam::lcd::dpi::Dpi`, and is perfectly capable of pushing
pixels through the LCD's parallel-RGB lanes. What it lacks, and what
this crate adds, are the two undocumented pieces that ship in
production boards via the LovyanGFX `Bus_RGB` driver, plus a
tear-free draw model:

1. **A circular GDMA descriptor chain over a PSRAM framebuffer** —
   esp-hal has no stock `DmaTxBuffer` that is large + PSRAM-backed
   + circular. This crate builds one.

2. **A VSYNC ISR that force-stops and re-arms the GDMA every frame**,
   using a separate FIFO-skip "restart descriptor" that compensates
   for the LCD_CAM's downstream async output FIFO holding ~64 bytes
   of pre-fetched data across an `out_rst`.

3. **(v0.2+) Double-buffering with a VSYNC-anchored page flip.** Two
   PSRAM framebuffers, two descriptor rings; `present()` schedules
   an atomic swap on the next VSYNC. Eliminates compose-during-scan
   tearing for callers that rewrite the framebuffer outside of
   LVGL-style upstream layers. Default-on; flip to single-buffer
   with `--no-default-features`.

The FIFO-skip detail is **not** in the public ESP32-S3 TRM. It comes
from ESP-IDF's private `hal/gdma_ll.h` constant `GDMA_LL_L2FIFO_BASE_SIZE = 64`
and is paired with a working fix in exactly one publicly-available
codebase: LovyanGFX. Without it the GDMA's per-frame re-arm produces
a fine pixel-phase shift on most panels.

## Status

`v0.2.0`. Builds cleanly against esp-hal 1.0.0 + esp-alloc 0.9.0 on
the `esp` Rust toolchain (Espressif's Xtensa fork). Hardware-verified
on the Elecrow CrowPanel DIS08070H (7" Basic, 800×480 RGB TFT, GT911
touch — same panel class as Makerfabs MaTouch 7", Sunton 7" boards)
on 2026-05-15. Double-buffering builds clean; field testing on the
swap path is in progress.

The example bringup at `examples/crowpanel_dis08070h.rs` builds with:

```bash
cd crates/lgfx-bus-rgb-rs
source ~/export-esp.sh   # set up xtensa toolchain
cargo build --release --example crowpanel_dis08070h
```

## Cargo features

| Feature | Default | What it does |
|---|---|---|
| `double-buffer` | **on** | Allocates two PSRAM framebuffers + two descriptor rings; `present()` schedules a swap on next VSYNC. 2× PSRAM cost. |

To match LovyanGFX bit-for-bit (single framebuffer, no swap),
disable the default features:

```toml
lgfx-bus-rgb-rs = { version = "0.2", default-features = false }
```

## Double-buffering — the page-flip protocol (v0.2)

This is a deliberate **divergence from LovyanGFX**. LovyanGFX does
not double-buffer at the bus layer — its `Panel_FrameBufferBase`
owns a single framebuffer and LVGL above it handles double-buffering.
For consumers without an LVGL-style layer above the bus, single-
buffering produces visible tearing when the consumer rewrites the
framebuffer at non-VSYNC moments. The page-flip extension fixes
that.

### Allocation

In double-buffer mode, `BusRgb::new`:

1. Allocates **two** PSRAM framebuffers (each `width × height ×
   pixel_bytes`, 64-byte aligned) via
   `esp_alloc::HEAP.alloc_caps(External, ...)`.
2. Builds **two** circular descriptor rings (each backed by its
   framebuffer).
3. Builds **two** FIFO-skip restart descriptors (each pointing into
   its framebuffer + 130 byte offset).
4. Hands the FB-A ring to `Dpi::send` to start scanning.

Memory cost at 800×480 RGB565: 2 × 768 KB = **1.5 MB PSRAM**. On
the 8 MB Octal PSRAM modules this leaves plenty of headroom; the
recommended heap split (Internal default, External-tagged PSRAM
region added separately) keeps non-framebuffer allocations off
PSRAM entirely.

### Runtime — synchronous double-buffering with VSYNC sync (v0.2.1+)

| API | Single-buffer (`--no-default-features`) | Double-buffer (default) |
|---|---|---|
| `framebuffer_addr()` | returns the single buffer's base (stable address) | returns the OFFSCREEN buffer's base (changes after each `present()`) |
| `framebuffer_len()` | unchanged | unchanged |
| `present()` | flush dcache; return | flush offscreen dcache; **block until ISR performs the swap** (≤1 frame period, ~33 ms @ 30 Hz; 100 ms watchdog) |
| `is_double_buffered()` | returns `false` | returns `true` |
| VSYNC ISR behaviour | re-arm same restart descriptor | if `PRESENT_PENDING`, atomic-swap scanning ↔ offscreen, toggle `SCANNING_FB` (Release), then re-arm |

The swap happens exactly at VSYNC, before the GDMA starts walking
the next frame's descriptors — so the GDMA never reads a buffer
the consumer is mid-write to.

`present()` returning means the swap **has already happened**: the
ISR's `SCANNING_FB` toggle was observed via Acquire load, so the
next `framebuffer_addr()` call is guaranteed to return a buffer
the GDMA is *not* scanning. This eliminates the v0.2.0 race in
which rapid back-to-back `present()` calls could overwrite a
freshly-written buffer before it was ever displayed.

### Idiomatic draw loop

```rust
loop {
    let fb = bus.framebuffer_addr() as *mut u16;
    // ... write pixels through `fb` ...
    bus.present();    // synchronous: blocks until swap commits
    // No explicit timer needed — the bus is back-pressured to
    // VSYNC by `present()` itself. Draw whenever you have new
    // content; the wall-clock cadence will track the panel refresh.
}
```

For a burst of N rapid `present()` calls (e.g. ten draws from a
push-event compositor), worst-case latency is N × frame-period —
at 30 Hz refresh that's 10 × 33 ms ≈ 330 ms total. If your draws
were going to be back-pressured by VSYNC anyway, this matches the
natural cadence with no extra cost.

### Watchdog

`present()` has an internal 100 ms watchdog. If the LCD_CAM
interrupt vector is not being serviced (a separate bug), the wait
returns anyway with `PRESENT_PENDING = true` still set, and a
missed frame appears as one stale frame on screen rather than as
a system hang. Persistent watchdog hits indicate priority
inversion or a globally-masked interrupt path; investigate
upstream.

## How it differs from `esp_lcd_panel_rgb`

ESP-IDF's `esp_lcd_panel_rgb` is the official supported driver: it
has bounce buffers, hardware-erratum compensation, frame sync, the
whole works. **If you can use it, you probably should** — pull it in
via `esp-idf-hal` or `esp-idf-sys`.

You'd reach for this crate when:

- You're already on a `no_std` esp-hal stack and don't want the IDF
  toolchain in your build matrix.
- You want a small, auditable bus driver that you can read end-to-end
  in an hour.
- esp-hal upstream issue [#5262 (RGB DPI bounce buffer support)](https://github.com/esp-rs/esp-hal/issues/5262)
  is still open by the time you read this.

## How it relates to LovyanGFX

It is a direct port of `Bus_RGB.cpp`. Every register write and every
descriptor field assignment is cited in the Rust source with the
`Bus_RGB.cpp:LINE` it came from. Where esp-hal 1.0 already does the
same work correctly (clock dividers, pin muxing, the bulk of the
LCD_CAM register init), we delegate to esp-hal and document that
delegation. The result is the smallest *additive* port that produces
a working RGB-DPI bus on top of esp-hal 1.0.

The pieces this crate adds, all from `Bus_RGB.cpp`:

| `Bus_RGB.cpp` | Rust location |
|---|---|
| 66-94 `lcd_default_isr_handler` | `src/isr.rs::lcd_vsync_isr` |
| 195-217 framebuffer + circular descriptor chain | `src/descriptor.rs::build_circular_chain` |
| 220-225 `_dmadesc_restart` FIFO-skip | `src/descriptor.rs::build_restart_descriptor` |
| 179-191 GDMA `conf0` / `conf1` burst flags | `src/bus.rs::RingBuffer::prepare` (delegated to esp-hal via `Preparation`) |
| 304-313 ISR install + `lcd_start` | `src/bus.rs::BusRgb::new` (last block) |

Pieces this crate does NOT re-implement (esp-hal covers them):

| `Bus_RGB.cpp` | esp-hal covers via |
|---|---|
| 135-167 dummy i80 bus for DMA channel + pin muxing | `Dpi::new(..., channel, ...).with_pclk(...).with_vsync(...)..with_data15(...)` |
| 228-302 LCD_CAM clock / `lcd_user` / `lcd_misc` / `lcd_ctrl[1,2]` | `Dpi::apply_config` |

## Usage

```rust
use esp_hal::lcd_cam::{LcdCam, lcd::{ClockMode, Phase, Polarity,
    dpi::{Config as DpiConfig, Dpi, Format, FrameTiming}}};
use esp_hal::time::Rate;
use esp_hal::gpio::Level;
use lgfx_bus_rgb_rs::{BusConfig, BusRgb, PixelFormat};

// 1. Register two heap regions: Internal SRAM first (default),
//    External PSRAM second. Only the framebuffer requests External.
esp_alloc::heap_allocator!(size: 160 * 1024);
let (psram_ptr, psram_len) = esp_hal::psram::psram_raw_parts(&peripherals.PSRAM);
unsafe {
    esp_alloc::HEAP.add_region(esp_alloc::HeapRegion::new(
        psram_ptr, psram_len,
        esp_alloc::MemoryCapability::External.into(),
    ));
}

// 2. Build the esp-hal Dpi with your pin map (omitted for brevity —
//    see examples/crowpanel_dis08070h.rs).
let lcd_cam = LcdCam::new(peripherals.LCD_CAM);
let dpi = Dpi::new(lcd_cam.lcd, peripherals.DMA_CH2, dpi_config)?
    .with_pclk(peripherals.GPIO0)
    .with_vsync(peripherals.GPIO40)
    .with_hsync(peripherals.GPIO39)
    .with_de(peripherals.GPIO41)
    .with_data0(peripherals.GPIO15) /* ...through data15... */;

// 3. Hand off to BusRgb. After this returns the GDMA is scanning.
let cfg = BusConfig {
    width: 800, height: 480,
    pixel_format: PixelFormat::Rgb565,
    port: 0,
    gdma_channel: 2,  // MUST match the DMA_CH<N> above
};
let mut bus = BusRgb::new(dpi, cfg).unwrap();

// 4. Write pixels and present.
let fb = bus.framebuffer_addr() as *mut u16;
// ... write RGB565 pixels ...
bus.present();   // flushes the dcache to PSRAM
```

## Caveats — read before flashing

- **PCLK pin is GPIO 0** on most CrowPanel revisions. GPIO 0 is also
  a strapping pin: external pulls on GPIO 0 during boot can brick the
  board. The LCD_CAM driving it after boot is fine.

- **PSRAM must be registered as `External`** in `esp_alloc::HEAP`
  before calling `BusRgb::new`. The example shows the two-region
  pattern. If you skip it, `BusRgb::new` returns
  `PsramAllocationFailed`.

- **Panel reset must happen BEFORE `BusRgb::new`.** On the CrowPanel
  the reset line is on the PCA9557 I/O expander; the factory firmware
  asserts it before LCD bringup. Starting the DPI scan while the
  panel is held in reset latches the panel in a bad bias state for
  the program lifetime. See the `JOURNALED-ACTOR-INKPAD` spec in the
  parent repo for the reset dance.

- **RGB565 byte order.** The CrowPanel pin map routes the *high* byte
  of the 16-bit DMA word to data lines 0..7 (factory
  `Bus_RGB.cpp:157` `rgb565sig_tbl = {8..15, 0..7}`). Either pre-swap
  each pixel with `.swap_bytes()` or alter the pin map. The example
  does the pixel-side swap.

- **Single-buffered, no double-buffering.** Partial-redraw tearing is
  the accepted tradeoff. To add double-buffering, allocate two PSRAM
  regions yourself and have your own ISR swap `out_link.addr` per
  frame. Out of scope here.

- **`MAX_DESCRIPTORS = 1024`.** Caps the supported framebuffer at
  ~4 MB. If you need more, bump the constant in `src/bus.rs` and
  recompile. (1024 × `MAX_DMA_LEN = 4032 B` = 4 MB.)

## Attribution

Derived from LovyanGFX:

- Original source: <https://github.com/lovyan03/LovyanGFX>
- File: `src/lgfx/v1/platforms/esp32s3/Bus_RGB.{cpp,hpp}`
- Authors: lovyan03; contributors ciniml, mongonta0716, tobozo
- License: BSD-3-Clause (FreeBSD)

Per BSD-3-Clause clause 2, the upstream copyright + license notice
is reproduced in `NOTICE` and at the top of every source file in
this crate.
