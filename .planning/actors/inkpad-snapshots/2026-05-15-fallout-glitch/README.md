# Fallout-glitch mode — CrowPanel DPI snapshot (2026-05-15)

> "table is pretty crisp but ... pleasant glitch"
> — user verdict on this exact firmware state, after the 10-config DPI
> bring-up that ended at config #10 + clock-mode `ShiftHigh` + fix-B
> heap split.

A snapshot of the three load-bearing firmware files that produce the
**frame-locked, fine-grained-pixel-glitch** look on the Elecrow
CrowPanel DIS08070H — saved here because the look has a real Fallout
Pip-Boy / dying-CRT / Star-Trek-warp aesthetic and we may want to
**deliberately switch into it** as an effect mode for the Inkpad Actor,
not just as a workaround.

## What the look is

A clean, readable image (text crisp, frame-locked, NOT moving) overlaid
with a fine-grained per-pixel sparkle / color shimmer that recurs frame
to frame. Subjectively: like an old phosphor CRT with slightly noisy
beam current, or a Pip-Boy display with a buddy-icon ghost. Functional
to read; texturally lo-fi.

## Hardware root cause (best-guess)

Per the V3.0 schematic at
`.planning/devices/crowpanel-display/CrowPanel-7.0-HMI-ESP32-Display-800x480/Eagle_SCH&PCB/V3.0/`,
the LI0704122Z panel is **true RGB888** (R0..R7, G0..G7, B0..B7 — 24
data lines) but the ESP32-S3 drives only **16 data lines** (RGB565).
Elecrow's pin mapping connects ESP's 5 blue bits to panel B3..B7, ESP's
6 green bits to panel G2..G7, ESP's 5 red bits to panel R3..R7. The
panel's **low color bits (R0..R2, G0..G1, B0..B2) are unconnected to
the ESP** — tied or floating per the on-board 0R / 0R-NC jumpers.

If those unused panel inputs aren't perfectly tied to GND on this PCB
revision they pick up EMI/crosstalk from the active high-bit lines at
12 MHz PCLK → the panel reads junk in the LSBs of every pixel → fine-
grained color sparkle. **Not software-fixable on this hardware.** So
either we live with it, or we **own it** as an effect.

## The recipe (firmware state that produces this look)

Snapshot taken from `crates/clawft-edge-pad/` on 2026-05-15. Three
files in this directory are byte-for-byte copies of the firmware that
produced the look.

Key configuration knobs:

| Component | Setting |
|---|---|
| DPI driver | Config #10: parked GDMA descriptor chain, `next` null on last, `suc_eof` on last only, VSYNC ISR (`LCD_CAM` @ Priority2) re-arms idle channel via `out_conf0.out_rst` pulse + `outlink_addr = &FB_DESC[0]` + `outlink_start` |
| Framebuffer | 800×480×2 = 768000 bytes in PSRAM, allocated via `esp_alloc::HEAP.alloc_caps(MemoryCapability::External)` |
| Heap split (fix B) | `heap_allocator!(160 KiB SRAM)` registered FIRST, then PSRAM region tagged `External`. Capability-less `alloc` → SRAM (WiFi/mesh/embassy live in SRAM, never contend with the GDMA framebuffer read) |
| Clock mode | `Polarity::IdleLow + Phase::ShiftHigh` (matches Elecrow's `pclk_active_neg = 1`) |
| PCLK | 12 MHz (Elecrow's own .ino uses 15 MHz — we run slower) |
| HSYNC | fp 40 / pulse 48 / bp 40 / active 800 |
| VSYNC | fp 1 / pulse 31 / bp 13 / active 480 |
| Polarity | HSYNC + VSYNC active-low (idle high); DE idles low |

The exact source is in this directory:
- `dpi_surface.rs` — config #10 driver
- `main.rs` — fix-B heap + ShiftHigh clock + full app wiring
- `board.rs` — pin map + timings

## How to restore this state

1. `cp .planning/actors/inkpad-snapshots/2026-05-15-fallout-glitch/dpi_surface.rs crates/clawft-edge-pad/src/drivers/dpi_surface.rs`
2. `cp .planning/actors/inkpad-snapshots/2026-05-15-fallout-glitch/main.rs crates/clawft-edge-pad/src/main.rs`
3. `cp .planning/actors/inkpad-snapshots/2026-05-15-fallout-glitch/board.rs crates/clawft-edge-pad/src/board.rs`
4. `cd crates/clawft-edge-pad && source ~/export-esp.sh && cargo build --release`
5. `espflash flash --port /dev/ttyUSB0 target/xtensa-esp32s3-none-elf/release/clawft-edge-pad`

## Future — making this an effect mode

If we move to the manufacturer's ESP-IDF / LovyanGFX stack (which the
factory uses and where the display is clean), the path to **opt-in**
this look is:

- **Software effect**: per-frame dither in the LSBs of each RGB565
  pixel — cheap, deterministic, reproduces the look on any backend.
- **Hardware mode**: if running on this exact CrowPanel revision, the
  look is "free" — just match the descriptor/clock/heap recipe above.
  On a clean-low-bits panel revision (or a different display), use the
  software effect.

So: this is durably preserved both as an aesthetic recipe AND as a
working firmware artifact. Even if we never use it as a deliberate
effect, the snapshot doubles as the recovery path for config #10's
spike-final state — `dpi_surface.rs` is otherwise untracked in git.
