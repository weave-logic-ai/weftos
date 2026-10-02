# Microcontrollers & edge sensor-drivers (Fleet nodes) — WeftOS/WeaveLogic hardware KB

> Scope: the **microcontroller class** of hardware in our stack — the "Fleet"
> nodes that can *drive a sensor and forward its readings* over a radio, as
> opposed to the Seed (a Linux SBC that reads I²C directly). These are the
> boards that run our Rust-on-Espressif firmware substrate
> (`crates/clawft-edge-pad`, no_std; `crates/clawft-edge-pad-idf`, std).
>
> Trust tiers on the firmware/Rust claims below follow the `embedded-rust`
> brain: `in-repo-verified` > `upstream-official` > `ecosystem-canon`. Hardware
> datasheet numbers are cited to Espressif / vendor pages (public facts only).
>
> **Boundary note.** This file is the *language/toolchain/board-selection* view.
> Panel-specific bring-up (RGB-DPI timings, GT911 touch, CrowPanel pin maps)
> belongs to the `esp32-s3-rgb-touch-display` agent; I²S/DMA acoustic capture,
> matched filters and LoRa backhaul belong to `embedded-acoustic-firmware`. Where
> a row straddles, it says which half is ours.

---

## The one decision that shapes every row: Xtensa vs RISC-V

Espressif silicon splits into two CPU families, and the split has real teeth for
Rust beyond toolchain setup:

- **Xtensa** (ESP32, ESP32-S2, **ESP32-S3**): LLVM has no upstream Xtensa
  backend, so Rust needs Espressif's compiler fork via **`espup`** + the `esp`
  toolchain channel. **Atomics in PSRAM are broken on Xtensa** — silent data
  races, not a crash or compile error. Every `Mutex`/`Arc`/channel contains
  atomics, so this constrains *where things are allocated*.
  `[upstream-official: Rust on ESP Book; in-repo-verified: notes/04]`
- **RISC-V** (ESP32-C3, **C6**, C5, C61, H2, P4): officially supported by the
  **upstream** Rust toolchain — plain `rustup target add`, no `espup`. **PSRAM
  atomics work correctly** here. RTIC is available on C3/C6.
  `[upstream-official: Rust on ESP Book; notes/01]`

Composed with dual-core (ESP32/S3): *cross-core sync needs atomics, and atomics
need internal SRAM, so every cross-core shared structure must be SRAM-allocated.*
This is the single most important firmware constraint for our Xtensa Fleet nodes.

**Firmware stack (all rows).** no_std path = `esp-hal 1.0` + `esp-rtos` +
Embassy (Espressif's **officially supported** path since the `esp-idf-*` std
crates went community-maintained in Feb 2025). esp-hal 1.0 stabilized only
`init` + GPIO/UART/SPI/I2C + `time` + `#[main]` + esp-config; everything else
(I²S, ADC, LCD_CAM, RMT, WiFi glue) is behind the `unstable` feature, which must
be **tilde-pinned** (`~1.0`) because breaking `unstable` changes ship in *minor*
releases. `[upstream-official: esp-hal 1.0 release; notes/01, notes/11]`

---

## ESP32-S2 (mini) — the TX sonobuoy node

- **Role for us**: sensor-driver / Fleet node. The **TX node** in the clawft
  sonobuoy stack (pulser drive side). WiFi/ESP-NOW forwarder with no BLE.
- **Core / arch**: single-core **Xtensa LX7** @ 240 MHz. 320 KB SRAM. On the
  common LOLIN/Wemos **S2 Mini (ESP32-S2FN4R2)**: **4 MB flash + 2 MB PSRAM**.
- **Radios**: **2.4 GHz WiFi b/g/n only — NO Bluetooth** on the S2. ESP-NOW is
  available (it rides the WiFi PHY, not BLE). LoRa only as an external SX127x/
  SX126x module over SPI.
- **Sensor I/O**: I²C, SPI, up to 18 ADC-capable pins, DAC ×2, I²S. Native
  **USB-OTG**, but **no dedicated USB-Serial/JTAG peripheral** — so `probe-rs`
  HIL is not available the S3/C-series way; flashing is over USB-OTG/CDC or an
  external UART bridge. `[upstream-official: ESP32-S2 datasheet]`
- **Rust/firmware notes**: Xtensa → needs `espup`; **PSRAM atomics landmine
  applies** (keep sync primitives in SRAM; only bulk buffers in PSRAM). Single
  core, so the Embedded-Rust-Book single-handler non-reentrancy guarantee *does*
  hold here (unlike S3). 2 MB PSRAM is small — fine for acoustic sample buffers,
  not for framebuffers. 4 MB flash makes dual-slot OTA a tight/hardware call
  (see OTA note below). `[in-repo-verified: notes/04; upstream-official: notes/01]`
- **Source**: <https://documentation.espressif.com/esp32-s2-mini-2_esp32-s2-mini-2u_datasheet_en.pdf> ·
  board: <https://www.wemos.cc/en/latest/s2/s2_mini.html>

---

## ESP32-S3 (WROOM-1, incl. N8R8) — the RX node & general Fleet workhorse

- **Role for us**: sensor-driver / Fleet node. The **RX node** in the sonobuoy
  stack (capture + matched-filter side) and the default board for any Fleet node
  that needs BLE, PSRAM, or dual-core headroom. Also the SoC under the Inkpad
  display actor (next row).
- **Core / arch**: **dual-core Xtensa LX7** @ 240 MHz, 512 KB SRAM.
  Module SKUs: **N8R8 = 8 MB flash + 8 MB octal PSRAM**; **N4R8 = 4 MB flash +
  8 MB PSRAM**. `[upstream-official: ESP32-S3-WROOM-1 datasheet]`
- **Radios**: 2.4 GHz WiFi b/g/n + **BLE 5 (LE)**. ESP-NOW on the WiFi PHY.
  WiFi+BLE coexistence (`coex`) is a supported feature in our firmware. LoRa via
  external SX126x/SX127x over SPI.
- **Sensor I/O**: I²C, SPI (incl. octal/QSPI), **I²S** (used for INMP441 MEMS mic
  + external SAR ADC capture on the RX node), ADC, RMT, LCD_CAM, **native
  USB-Serial/JTAG** → `probe-rs` HIL *is* possible on a board that exposes the
  USB-JTAG pins (a CH340-bridged board instead needs an external esp-prog).
  `[in-repo-verified: notes/12 §4.6; notes/07]`
- **Rust/firmware notes**: this is our best-characterized board.
  - **Dual-core** → `static mut` in an interrupt handler is **not** automatically
    safe; the single-handler non-reentrancy guarantee does not cover us.
    `[upstream-official: Embedded Rust Book; notes/11]`
  - **PSRAM atomics landmine** → use the measured **SRAM-first allocator split**:
    `esp_alloc::heap_allocator!` for internal SRAM first (≈160 KiB held WiFi +
    embassy-net + mesh on our workload), then `HEAP.add_region(... External ...)`
    for PSRAM, and request `External` *only* for big buffers. The documented
    `esp_alloc::psram_allocator!` macro **panics** on our N4R8/AP_3v3 board —
    measured workaround wins. `[in-repo-verified: notes/04]`
  - Crypto: hardware SHA/AES/HMAC acceleration; carries an **ed25519 keypair**
    (node identity, ADR-025/057) — `sha2`/`ed25519-dalek` no_std.
    `[in-repo-verified: .planning/sensors/JOURNALED-NODE-ESP32.md]`
- **Source**: <https://www.espressif.com/sites/default/files/documentation/esp32-s3-wroom-1_wroom-1u_datasheet_en.pdf>

---

## Elecrow CrowPanel ESP32-S3 7" (DIS08070H) — the Inkpad display actor

- **Role for us**: **display actor** node (not primarily a sensor-driver).
  Runs the Inkpad Actor firmware; the working substrate for both firmware crates.
- **Core / arch**: **ESP32-S3-WROOM-1-N4R8** — dual-core Xtensa LX7 @ 240 MHz,
  512 KB SRAM, **4 MB flash + 8 MB PSRAM**. `[vendor: Elecrow wiki]`
- **Radios**: 2.4 GHz WiFi b/g/n + BLE 5 (inherits the S3). Substrate publish
  goes over WiFi (JSON-RPC) today.
- **Sensor I/O**: 800×480 RGB-parallel TFT (TN panel) + **GT911 capacitive
  touch over I²C**; TF-card, speaker, battery interfaces. Touch and the RGB bus
  are the display agent's domain; the GT911 driver
  (`crates/weftos-leaf-touch-gt911/`) is HAL-agnostic (`embedded-hal` traits).
- **Rust/firmware notes**: the clearest std-vs-no_std case study in the repo.
  **esp-hal 1.0 has no RGB-DPI bounce buffer** (upstream esp-hal #5262), which is
  why `clawft-edge-pad-idf` (std/esp-idf-svc) exists to use Espressif's official
  `esp_lcd_panel_rgb`, and why the no_std port hand-ported LovyanGFX into
  `crates/lgfx-bus-rgb-rs/`. PSRAM framebuffer → same SRAM-first allocator split;
  PSRAM **bandwidth contention looks like a rendering bug** (write-once static-grid
  diagnostic separates it from logic). **4 MB flash → documented single-app, no
  OTA.** `[in-repo-verified: notes/01, notes/04, notes/08, notes/12]`
- **Source**: <https://www.elecrow.com/wiki/esp32-display-702727-intelligent-touch-screen-wi-fi26ble-800480-hmi-display.html>

---

## ESP32-C3 / ESP32-C6 (RISC-V) — the alternatives worth keeping on the table

RISC-V siblings. Carry these when a Fleet node wants **no `espup`**, **no PSRAM
atomics landmine**, lower power, or (C6) a mesh radio.

### ESP32-C3
- **Role for us**: low-cost single-sensor Fleet node; a clean toolchain story.
- **Core / arch**: single-core **RISC-V** @ 160 MHz, 400 KB SRAM, typically
  4 MB flash, **no PSRAM**.
- **Radios**: 2.4 GHz WiFi b/g/n + **BLE 5** + ESP-NOW.
- **Sensor I/O**: I²C, SPI, ADC, **USB-Serial/JTAG** (probe-rs HIL works).
- **Rust/firmware notes**: officially supported upstream Rust target
  (`riscv32imc-unknown-none-elf`, **no espup**); PSRAM atomics landmine is moot
  (no PSRAM); **RTIC available**. Single core. Good default when the design fits
  in SRAM and wants the simplest toolchain. `[upstream-official: notes/01]`
- **Source**: <https://www.espressif.com/sites/default/files/documentation/esp32-c3_datasheet_en.pdf>

### ESP32-C6
- **Role for us**: the forward-looking mesh/low-power Fleet node — it adds an
  **802.15.4** radio (Thread / Zigbee) alongside WiFi 6, plus a low-power core.
- **Core / arch**: **RISC-V** HP core @ 160 MHz + LP core @ 20 MHz, 512 KB
  HP SRAM, typically 4–8 MB flash, no PSRAM on standard modules.
- **Radios**: **WiFi 6 (802.11ax)** 2.4 GHz + **BLE 5** + **802.15.4
  (Thread/Zigbee)** + ESP-NOW. The only Espressif part here with a native mesh
  radio, which matters for a battery-constrained sensor Fleet.
- **Sensor I/O**: I²C, SPI, ADC, I²S, **USB-Serial/JTAG**; the LP core + LP-I²C
  can poll a sensor while the HP core sleeps (`esp-lp-hal`, RISC-V LP target).
- **Rust/firmware notes**: upstream Rust target
  (`riscv32imac-unknown-none-elf`, note **imac** vs C3's **imc** — atomics in the
  ISA), no espup, PSRAM landmine moot, RTIC available. **Not yet in-repo** — no
  WeftOS firmware runs on C6 today, so treat the esp-radio 802.15.4 maturity as
  *unverified for us*. `[upstream-official: notes/01; NOT covered in-repo]`
- **Source**: <https://www.espressif.com/sites/default/files/documentation/esp32-c6_datasheet_en.pdf>

---

## Cross-cutting constraints for any Fleet sensor-driver node

1. **OTA is a hardware decision, not a firmware one.** It needs the ESP-IDF
   second-stage bootloader, a partition table with two app slots + `otadata`,
   and flash big enough for *two* app images. On a **4 MB** module (S2 mini,
   CrowPanel N4R8) dual-slot OTA roughly halves app space — our 4 MB boards are
   documented **single-app, no OTA**. An 8 MB (N8R8) module is the OTA-capable
   choice. OTA is exposed via `esp-bootloader-esp-idf` 0.5; the confirm/rollback
   API is **not yet verified by us**. `[in-repo-verified / upstream-official: notes/08]`
2. **The Xtensa PSRAM atomics landmine (S2/S3) is silent.** Enforce the SRAM-first
   allocator split structurally; never let a `Mutex`/`Arc`/channel land in PSRAM.
   RISC-V (C3/C6) is immune. `[in-repo-verified: notes/04]`
3. **HIL needs USB-Serial/JTAG.** S3/C3/C6 have it; **S2 does not** (USB-OTG only);
   a CH340-bridged board needs an external esp-prog. Decide this at planning time.
   `[upstream-official / in-repo-verified: notes/07, notes/12]`
4. **Design drivers against `embedded-hal` traits, not concrete esp-hal types** —
   so firmware is host-testable and a driver ports std↔no_std for free (the GT911
   driver is the in-repo model). `[in-repo-verified: notes/09, notes/12]`
5. **`unstable` esp-hal/esp-radio must be tilde-pinned** while enabled; our crates
   now do (`~1.0` / `~0.17`, WEFT-667). Every `esp-*` pin is currently one minor
   behind upstream as a coherent, radio-coupled set — bump as one atomic wave, not
   piecemeal. `[in-repo-verified: notes/12 §2–3]`
6. **Identity is uniform.** Every node signs its emissions with an ed25519 key;
   `node_id = hex(SHA-256(pubkey)[..16])` (ADR-099/103). Any OTA image-signing
   design should reuse this trust root, not add a second one.
   `[in-repo-verified: JOURNALED-NODE-ESP32.md, notes/08]`

---

## Quick selection table

| Board / SoC | Arch | Cores | SRAM | Flash / PSRAM | Radios | USB-JTAG (HIL) | Our role |
|---|---|---|---|---|---|---|---|
| ESP32-S2 mini | Xtensa LX7 | 1 | 320 KB | 4 MB / 2 MB | WiFi + ESP-NOW (no BLE) | ✗ (OTG only) | Sonobuoy TX |
| ESP32-S3 WROOM-1 N8R8 | Xtensa LX7 | 2 | 512 KB | 8 MB / 8 MB | WiFi + BLE5 + ESP-NOW | ✓ | Sonobuoy RX / workhorse |
| CrowPanel ESP32-S3 (N4R8) | Xtensa LX7 | 2 | 512 KB | 4 MB / 8 MB | WiFi + BLE5 | ✓ | Inkpad display actor (no OTA) |
| ESP32-C3 | RISC-V | 1 | 400 KB | 4 MB / — | WiFi + BLE5 + ESP-NOW | ✓ | Low-cost sensor node |
| ESP32-C6 | RISC-V | 1 HP + 1 LP | 512 KB | 4–8 MB / — | WiFi6 + BLE5 + 802.15.4 | ✓ | Mesh / low-power (not yet in-repo) |

*Xtensa rows need `espup`; RISC-V rows use the upstream Rust toolchain. PSRAM
atomics are broken on the Xtensa rows only.*
