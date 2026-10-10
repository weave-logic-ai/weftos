# clawft-edge-pad-idf

ESP-IDF-on-Rust port of `clawft-edge-pad`. WeftOS Inkpad Actor firmware
for the Elecrow CrowPanel DIS08070H (7" 800×480 RGB TFT + GT911 touch,
ESP32-S3-WROOM-1 N4R8).

## Why a second crate?

The bare-metal `clawft-edge-pad` crate spent eleven config iterations
hand-patching a raw `esp_hal::lcd_cam::dpi` driver against an open
upstream gap (esp-hal #5262 — DPI bounce-buffer support, unmerged).
The factory ESP-IDF reference firmware uses LovyanGFX, which programs
LCD_CAM registers directly; Espressif's official `esp_lcd_panel_rgb`
driver provides bounce buffers, frame sync, and hardware-erratum
compensation out of the box.

This crate moves the firmware onto `esp_lcd_panel_rgb` via
`esp-idf-hal` + `esp-idf-svc`. The sibling `clawft-edge-pad/` crate
stays in tree as the working spike + comparison baseline.

The toolchain pattern (Xtensa target via `xtensa-esp32s3-espidf`,
sdkconfig.defaults, embuild) mirrors `crates/clawft-edge-bench/`,
which is the proven IDF-on-Rust precedent in this repo.

## Crate layout

```
clawft-edge-pad-idf/
├── Cargo.toml                # own [workspace] table -- out-of-workspace
├── .cargo/config.toml        # xtensa-esp32s3-espidf target + runner
├── build.rs                  # embuild::espidf::sysenv::output()
├── rust-toolchain.toml       # channel = "esp"
├── sdkconfig.defaults        # SPIRAM Octal 80M, LCD_RGB_ISR_IRAM_SAFE, ...
└── src/
    ├── main.rs               # entry, boot order, task spawn
    ├── board.rs              # pin map + timings (port from edge-pad)
    ├── display.rs            # esp_lcd_panel_rgb wrapper + LeafSurface impl
    ├── drivers/
    │   ├── mod.rs
    │   ├── pca9557.rs        # blocking sync port
    │   └── gt911.rs          # blocking sync port
    ├── mesh.rs               # std::net mesh client
    ├── net.rs                # esp-idf-svc WiFi bringup
    ├── wifi_secrets.rs       # gitignored
    └── wifi_secrets.rs.example
```

## Building

This crate is **out-of-workspace** (its `Cargo.toml` opens with an
empty `[workspace]` table). Build it from its own directory, not from
the repo root.

Prerequisites (once-per-machine):
- `espup install` (provides the `esp` rustup toolchain).
- `cargo install espflash ldproxy`.

```sh
source ~/export-esp.sh
cd crates/clawft-edge-pad-idf
cargo build --release
```

First build will auto-download and compile ESP-IDF v5.3.x (~10-15 min,
~3 GB on disk under `target/`). Subsequent builds reuse the cached
IDF tree.

## Flashing

The release image is ~1.29 MiB. ESP-IDF's default single-app partition
table gives `factory` only 1 MiB, so the default table is **not
flashable** — the first hardware flash (2026-10-10) failed on exactly
this. `partitions.csv` at the crate root (nvs 24 KiB, phy 4 KiB,
factory 3 MiB, single-app / no OTA on the 4 MB module) is applied at
flash time; the app reads the table from flash at 0x8000, nothing is
compiled in. (`CONFIG_PARTITION_TABLE_CUSTOM_FILENAME` does not work
from this crate: esp-idf-sys resolves it relative to the generated
project under `target/`, not the crate root.)

With a USB-attached host toolchain (`cargo run` uses the runner in
`.cargo/config.toml`, which already passes the table):
```sh
cargo run --release
```

From a build container without USB access (how the Mac bring-up ran —
`espressif/idf-rust` image, device on the host), produce a merged image
in the container and write it with host `esptool`:
```sh
# in the container, crate dir
OUT=target/xtensa-esp32s3-espidf/release/build/esp-idf-sys-*/out/build
espflash save-image --chip esp32s3 --flash-size 4mb --merge \
  --bootloader $OUT/bootloader/bootloader.bin \
  --partition-table partitions.csv \
  target/xtensa-esp32s3-espidf/release/clawft-edge-pad-idf edgepad-merged.bin
# on the host (CrowPanel CH340 enumerates as /dev/cu.usbserial-*)
esptool --port /dev/cu.usbserial-XX --baud 460800 write-flash 0x0 edgepad-merged.bin
esptool --port /dev/cu.usbserial-XX read-flash 0 0x400000 backup.bin   # before the first write
```
The CH340 drops bytes under sustained transfer; if `write-flash` or
`read-flash` fails with "Invalid head of packet", retry at 230400 or
read in 256 KiB chunks. Serial console is 115200.

## Cross-reference to `clawft-edge-pad`

| edge-pad file              | edge-pad-idf file               | Notes |
|----------------------------|--------------------------------|-------|
| `Cargo.toml` (esp-hal)     | `Cargo.toml` (esp-idf-*)       | full re-spec |
| `.cargo/config.toml`       | `.cargo/config.toml`           | `-espidf` target |
| `rust-toolchain.toml`      | `rust-toolchain.toml`          | same `esp` channel |
| n/a                        | `sdkconfig.defaults`           | IDF-only |
| n/a                        | `build.rs`                     | embuild glue |
| `src/main.rs`              | `src/main.rs`                  | std main, FreeRTOS threads |
| `src/board.rs`             | `src/board.rs`                 | 1:1 transcript |
| `src/drivers/pca9557.rs`   | `src/drivers/pca9557.rs`       | async→blocking |
| `src/drivers/gt911.rs`     | `src/drivers/gt911.rs`         | async→blocking |
| `src/drivers/dpi_surface.rs` | `src/display.rs`             | replaced wholesale |
| `src/drivers/lcd_rgb.rs`   | (gone — superseded)            | day-2 broken path |
| `src/mesh.rs`              | `src/mesh.rs`                  | embassy-net→std::net |
| `src/net.rs`               | `src/net.rs`                   | esp-radio→esp-idf-svc |
| `src/wifi_secrets.rs.example` | `src/wifi_secrets.rs.example` | identical template |

## Status

- Build-verify scope only (no flash this cycle).
- WiFi credentials placeholder: copy `wifi_secrets.rs.example` →
  `wifi_secrets.rs` and fill before flashing.
- Mesh target IP `192.168.1.73` matches the bare-metal port; update
  in `mesh.rs` for your daemon.
