---
title: Journaled Actor — Inkpad (CrowPanel DIS08070H / ESP32-S3)
created: 2026-05-12
status: draft — spike scoped from Hive Mind session 2026-05-12
scope: one Actor, its identity, its owned paths, its emissions, the Inkpad Actor Object Type
depends_on:
  - .planning/sensors/JOURNALED-NODE-ESP32.md (Node ≠ Actor split)
  - docs/adr/adr-025-ed25519-node-identity.md (identity derivation pattern)
  - docs/adr/adr-057-substrate-read-acl.md (deny-by-default read ACL)
related:
  - waveshare-watch-rs (ESP32-S3 no_std esp-hal + embassy reference firmware)
  - jeyeager65/FluidTouch (community PlatformIO project for this exact board — pin map source)
---

# Journaled Actor — Inkpad

## 0. Why this matters — Actors emit *acts*, not measurements

The Node-vs-Actor split from `JOURNALED-NODE-ESP32.md` §0 says
"Sensing is not acting." That asymmetry holds — but it forces a
question the existing journals don't answer: **what shape does an
*Actor*'s substrate emission take?** Nodes emit measurements
(`sensor/mic/pcm_chunk`); the Actor proposal in
`JOURNALED-NODE-ESP32.md` §8.4 only reserves `substrate/actor/<actor-id>/`
without committing to an emission shape.

The Inkpad spike answers this concretely: an Actor emits **acts** —
discrete, signed, intent-bearing events. Ink strokes are the first
non-sensor substrate emission. Future Actor emissions (voice
commands, button presses, gesture envelopes) share the same shape.

Main-thread decisions reflected here (Hive Mind session 2026-05-12):

- The Inkpad is an **Actor**, not a Node. It has an ed25519 keypair
  whose pubkey-hash is its **Actor ID** (`a-<6-hex>`).
- It also runs on physical hardware that emits node-level health, so
  it is *also* a Node (`n-<6-hex>`) with a separate keypair. **Two
  keypairs, two identities, one device.** This is the first concrete
  case of an Actor and a Node sharing hardware.
- Ink strokes publish to `substrate/<actor-id>/ink/strokes/<id>`.
- Per ADR-057, all `substrate/<actor-id>/**` paths are **private by
  default** — only the Actor itself plus `scope:admin` callers may
  read. Sharing a page requires an explicit ACL allow rule.

## 1. Hardware profile

- **Board**: Elecrow CrowPanel DIS08070H ("ESP32 Display 800×480,
  7 Inch HMI Basic")
- **Module**: ESP32-S3-WROOM-1 (variant suffix N4R8 — `MC` prefix on
  the metal shield is a factory date code, not load-bearing)
- **SoC**: ESP32-S3, Xtensa **LX7** dual-core @ 240 MHz
- **Flash**: 4 MB (Q I/O, 80 MHz) — single-app partition, **no OTA
  dual-slot**
- **PSRAM**: 8 MB **Octal**, capable of 120 MHz
- **Display**: 800×480 TN RGB TFT (not IPS — the *Advance* SKU has
  IPS)
- **Touch**: GT911 capacitive over I²C
- **Backlight**: PWM on GPIO2 (Basic SKU); the Advance uses an
  STC8H1K28 I²C backlight controller at 0x30
- **USB**: USB-UART bridge (CP210x or CH340) for flashing — S3's
  native USB-OTG is *not* exposed on the Basic
- **Battery**: external connector only (no internal LiPo on Basic)
- **MicroSD (TF) slot**: present on the mainboard, SPI-attached.
  Not used in the v0 spike (strokes publish straight to substrate
  over WiFi), but reserved for: offline stroke cache when WiFi is
  unavailable, page-image snapshots, downloaded model / font
  blobs. If wired later, `embedded-sdmmc` is the canonical no_std
  driver.
- **Audio**: not used in this spike
- **Stock firmware**: Elecrow LVGL demo (Arduino + LovyanGFX) — back
  up to `elecrow7basic_stock.bin` before flashing Rust per the
  Windows-side probe sequence

> **Operational gotcha 1 (learned 2026-05-13):** Running
> `esptool read-flash` in parallel with a `cargo build --release`
> over the same WSL2 / `usbipd-win` / CH340 path causes byte loss
> in the serial stream (4096-byte transfers consistently return
> ~3900-4080 bytes), corrupting the backup. The CH340 itself is
> fine — small reads (4 KB, 64 KB, 1 MB) succeed at full
> 90 kbit/s. The failure mode is CPU contention starving the
> Windows-side USB driver or USBIP service of cycles, not
> a fundamental USBIP issue. **Run sustained flash reads with
> nothing else heavy running on the host.**

> **Operational gotcha 2 (resolved 2026-05-13):** First-boot
> attempt with `esp_alloc::psram_allocator!` panicked in
> `linked_list_allocator-0.10.6/src/hole.rs:331` (assertion
> `hole_size >= size_of::<Hole>()`). **Root cause:** esp-hal 1.0
> defaults `ESP_HAL_CONFIG_PSRAM_MODE` to `"quad"`, but the N4R8
> module on this board has Octal PSRAM. `init_psram` silently
> failed, the macro registered a 0-byte region, the allocator
> tripped on it. **Fix:** add `ESP_HAL_CONFIG_PSRAM_MODE = "octal"`
> to `crates/clawft-edge-pad/.cargo/config.toml`. Verified at
> boot: `heap free: 8454144 bytes` = 64 KiB SRAM + 8 MiB PSRAM.
> Diagnosis credit: the `embedded-acoustic-firmware` agent
> walked the esp-hal source. See it for the 120 MHz `PsramConfig`
> 3-arg form if higher PSRAM bandwidth is later needed.

### 1.1 Pin map (initial; cross-check against FluidTouch & Lesson 2)

To be filled in during the spike. Sources of truth:
- FluidTouch `include/` — LovyanGFX board profile constants
- Elecrow tutorial `Lesson 2 Draw GUI with LovyanGFX` — RGB bus pin
  declarations and timing
- `Elecrow-RD/gt911_for_crowpanel` — I²C / INT / RST for touch

Open the file `crates/clawft-edge-pad/src/board.rs` (TBD) once those
constants are transcribed to `esp_hal::lcd_cam::lcd::dpi::Config`.

## 2. Actor identity

### 2.1 Keypair

- **Algorithm**: ed25519 — matches Node identity (ADR-025) and chain
  signing.
- **Storage**: NVS plain (provisioning-grade per
  `JOURNALED-NODE-ESP32.md` §1.1). Upgrade to NVS-encrypted on first
  non-spike deployment.
- **Generation**: host-side CLI at first flash burns the private
  half into NVS, records the public half under
  `substrate/<mesh-id>/cluster/actors/<actor-id>` (signed by the
  daemon's own Actor identity).

### 2.2 Actor ID derivation

- BLAKE3 hash of pubkey → first 3 bytes → 6 hex chars → prefix `a-`.
- Example: `a-7b2c9e`.

This mirrors the Node ID scheme (`n-<6-hex>`) so the identity-string
parsers in the daemon stay symmetric: `a-` ⇒ Actor, `n-` ⇒ Node. The
ACL ADR (ADR-057) already accepts both prefixes in its identity
strings.

### 2.3 Why a separate keypair from the Node identity

The device is *both* a Node (emits `health`, `meta`) and an Actor
(emits ink). Two keypairs let the read-gate distinguish "this
publish is from the Node hardware" from "this publish is the user
acting through the device." Concretely:

- A `substrate.publish` on `substrate/<node-id>/health` is signed by
  the Node key.
- A `substrate.publish` on `substrate/<actor-id>/ink/strokes/<id>`
  is signed by the Actor key.
- The write-gate verifies *which* key signed the envelope and checks
  the appropriate identity directory entry.

This also lets a future device support multiple Actor identities
(e.g., user A and user B sharing one Inkpad) by holding multiple
Actor keypairs while keeping a single Node keypair.

## 3. Paths this Actor owns

All paths below are prefixed with `substrate/<actor-id>/`.

### 3.1 Ink subtree

`substrate/<actor-id>/ink/pages/<page-id>` — page metadata
(title, surface dimensions, tool palette, created-at).

`substrate/<actor-id>/ink/strokes/<stroke-id>` — one stroke per
substrate entry. Atomic: a stroke is published *once* on
touch-up, not incrementally during the touch event. Sub-stroke
streaming is reserved for a later optimization (see §7.2).

### 3.2 Signature subtree

`substrate/<actor-id>/signature/<action-id>` — a captured signature
stroke bound to a specific Action envelope. The Action payload
includes the action target, the stroke bytes, and is signed by the
Actor's ed25519 key.

### 3.3 Health subtree (Node-side, *not* Actor-side)

The device's `substrate/<node-id>/health` and
`substrate/<node-id>/meta` emit from the Node identity per the
existing ESP32 journal. The Actor side has no health emissions —
Actors don't emit measurements.

## 4. Wire format (v0 — to be revised by spike)

```jsonc
{
  "stroke_id":  "s-<8-hex>",
  "actor_id":   "a-<6-hex>",
  "page_id":    "p-<6-hex>",
  "started_ms": 1714000000000,
  "ended_ms":   1714000004821,
  "surface":    { "w": 800, "h": 480, "dpi": 132 },
  "tool":       { "kind": "pen", "color": "#000000", "width_px": 3 },
  "points": [
    { "x": 312, "y": 188, "t": 0,    "p": 1.0 },
    { "dx": 2,  "dy": 1,  "dt": 16,  "p": 1.0 },
    { "dx": 2,  "dy": 0,  "dt": 16,  "p": 1.0 },
    "..."
  ]
}
```

Notes:

- First point is absolute `(x, y, t)`; subsequent are delta-encoded
  `(dx, dy, dt)`. Compact wire size, simple decoder.
- `p` (pressure) is `1.0` on capacitive touch (no pressure sensing).
  Field is present and float-typed so EMR-digitizer hardware can
  fill it in later without a schema bump.
- `t` and `dt` are milliseconds since `started_ms` (`t` is monotonic
  within the stroke, not wall-clock).
- `surface` carries the device-local pixel dimensions so a
  consumer rendering on a different surface (the watch, a host
  egui Workshop) can scale correctly.
- The full envelope is signed by the Actor ed25519 key; the
  signature lives in the JSON-RPC envelope, not in the value body
  (per `JOURNALED-NODE-ESP32.md` §6).

## 5. ACL clauses honoring ADR-057

The Inkpad is the first Actor to exercise ADR-057 in anger. Specific
clauses seeded at provisioning:

| Path glob | Allow | Deny | Notes |
|---|---|---|---|
| `substrate/<actor-id>/ink/pages/**` | `actor:<actor-id>`, `scope:admin` | — | Page metadata is private by default |
| `substrate/<actor-id>/ink/strokes/**` | `actor:<actor-id>`, `scope:admin` | — | **Default-private; this is the privacy test** |
| `substrate/<actor-id>/signature/**` | `actor:<actor-id>`, `scope:admin` | — | Signatures NEVER auto-share |

Per ADR-057, the device also gets the `publish_public` helper so a
specific page can be opted into broader read access. Implementation:

```rust
substrate.publish(format!("substrate/{actor_id}/ink/pages/{page_id}"), page_meta)?;
substrate.acl_grant(
    format!("substrate/{actor_id}/ink/pages/{page_id}"),
    Allow::Public,
)?;
```

ACL writes are themselves signed by the Actor key (the rule
`substrate/<mesh-id>/acl/**` accepts writes from the path-owning
Actor; see ADR-057 §"ACL data model" for who can write each rule).

## 6. Code cross-references

- **Firmware crate**: `crates/clawft-edge-pad/` (out-of-workspace —
  mirrors `clawft-edge-bench/`'s isolation pattern so the Xtensa
  toolchain doesn't infect the main workspace build).
  - `Cargo.toml` — esp-hal 1.0 + embassy + embedded-graphics stack
    (matches `infinition/waveshare-watch-rs` shape)
  - `rust-toolchain.toml` — `esp` channel (espup-managed)
  - `.cargo/config.toml` — Xtensa target + `espflash` runner
  - `src/main.rs` — embassy entry; backlight smoke test on GPIO2
  - `src/board.rs` (TBD) — pin map transcribed from FluidTouch +
    Elecrow Lesson 2
  - `src/drivers/lcd_rgb.rs` (TBD) — `esp_hal::lcd_cam` RGB dpi
    config + framebuffer-in-PSRAM rendering
  - `src/drivers/gt911.rs` (TBD) — touch driver (port the
    Elecrow C++ `TAMC_GT911` to embedded-hal-async I²C, or use
    `gt911-async` if it fits)
  - `src/ink/` (TBD) — stroke buffer, delta encoding, wire-format
    serialization, ed25519 signing
- **Daemon-side ACL plumbing**: per ADR-057 §"MUST-HAVE acceptance
  criteria" — the substrate ACL table type, default seeding, and
  `acl_denied` error wiring all need to land for this spike to
  prove enforcement.
- **Host viewer**: the egui shell needs an `InkpadViewer` to render
  strokes back from substrate paths. Reserved; not part of the
  initial 5-day spike.

## 7. Open questions

1. **Two keypairs on one device — provisioning UX.** The Inkpad
   needs both a Node keypair and an Actor keypair burned at
   provisioning. Does the host-side provisioning CLI burn both in
   one operation, or does the Actor identity get added as a
   separate "claim this device as your input" step? Spike
   recommendation: burn both at first-flash for now; revisit when
   multi-user Inkpads come up.
2. **Sub-stroke streaming.** v0 publishes one stroke per
   `substrate.publish` call on touch-up. Pen-up-pen-down loops
   (rapid taps, scribbles) hammer the substrate. If profiling
   shows this is a problem, batch strokes into pages and publish on
   a timer or on `page-end`. Reserved.
3. **Stroke compression at the wire.** Delta-encoded JSON is
   readable but verbose. CBOR (already used per ADR-030 for chain
   payloads) would cut wire size by ~3-5×. Defer until a single
   raw-JSON publish is shown to work — measurement before
   optimization.
4. **Rendering on subscribe.** When the device subscribes to its
   own ink path (echo test), it has to render a JSON-decoded stroke
   to the framebuffer. Same code path that LOCAL strokes render
   through, just sourced from substrate. Worth one test write-up.
5. **ADR-057 enforcement test as a spike deliverable.** A second
   unauthenticated CLI subscriber MUST get `acl_denied` on the
   inkpad's stroke path. This is the first real test of the ACL
   gate against a non-toy emission — wire it into the spike's
   acceptance criteria.
6. **EMR / pressure path.** Capacitive touch has no pressure or
   tilt. The wire format reserves `p` but ignores it. If/when an
   EMR sandwich follows this spike, the same Actor identity and
   substrate paths apply — only the driver layer changes.

## 8. Acceptance criteria for the 5-day spike

- [x] `crates/clawft-edge-pad/` boots embassy + esp-hal on hardware;
      backlight ON via GPIO2 confirms toolchain works. *(landed
      2026-05-13.)*
- [x] PSRAM init working — 8,454,144 bytes total heap (64 KiB
      SRAM + 8 MiB Octal PSRAM) confirmed at boot. *(landed
      2026-05-13 via `.cargo/config.toml` env fix; see §1
      gotcha 2 for root cause.)*
- [x] Pin map transcribed to `src/board.rs` from the two
      reference projects (FluidTouch + Elecrow `gfx_conf.h` for
      `CrowPanel_70`). RGB data bus, sync lines, backlight, GT911
      I²C, microSD SPI. Reference conflict on GT911 RST/address
      flagged inline. *(landed 2026-05-13.)*
- [x] GT911 async driver scaffold at `src/drivers/gt911.rs`
      ported from `Elecrow-RD/gt911_for_crowpanel` — init,
      address probe (0x14 / 0x5D), `read_frame` with point list,
      flag-clear handshake. Compiles. Not yet wired to a real
      I²C bus instance. *(landed 2026-05-13.)*
- [x] LCD RGB scaffold at `src/drivers/lcd_rgb.rs` with the
      `Dpi` config plan inline as TODO. RGB565 pixel encoder
      with unit tests for pure R/G/B. *(landed 2026-05-13.)*
- [x] New `esp32-s3-rgb-touch-display` expert agent at
      `~/.claude/agents/esp32-s3-rgb-touch-display/`. Hardware-
      specific equivalent of `embedded-acoustic-firmware`.
      *(landed 2026-05-13.)*
- [x] LCD RGB DPI **wired up** — `Dpi::new` via esp-hal `lcd_cam`,
      `dma_loop_buffer!` + `send(next_frame_en=true)`. **Solid red
      fill confirmed on the physical panel 2026-05-14.** Pin map,
      panel timings, 16-bit RGB565 format, and clock polarity all
      verified. Key gotchas found: `next_frame_en` must be `true`
      (false → one frame then black); `core::mem::forget(transfer)`
      keeps DMA alive but blocks espflash auto-reset — fixed with a
      3 s flash-grace window at the top of `main()`.
- [x] GT911 touch — **WORKING.** Multi-point coordinates,
      smooth finger-drag tracking, proper 800×480 space, confirmed
      on hardware 2026-05-14. The path that got here:
      - The v3.0 board routes GT911 RST through a **PCA9557 I²C I/O
        expander @ 0x18** (pin IO1), not a direct GPIO. Driver at
        `src/drivers/pca9557.rs` runs the reset sequence
        transcribed from the Elecrow v3.0 demo's `setup()`.
      - GT911 enumerates at I²C **0x5D** (not 0x14 — `gfx_conf.h`'s
        0x14 was a different variant).
      - **The `config version` byte reading 0xFF was a red
        herring** — a full 185-byte register dump showed a
        complete, valid factory config (170/185 bytes non-0xFF,
        correct 800×480 resolution + channel map). 0xFF is a valid
        *max-priority* version, not "blank".
      - **The actual fix: do NOT poke `CONFIG_FRESH`.** An earlier
        `commit_config()` that wrote 1 to 0x8100 was telling the
        chip to re-validate its config and disrupting the scan
        engine. Removing it — leaving the factory config 100%
        untouched — plus the PCA9557 RST release = the chip scans
        on its own.
      - Also learned: the GT911 is single-buffered — it won't
        post a new scan until the host clears `POINT_INFO`. A
        "passive read-only" diagnostic that never clears the flag
        will see it frozen, which looks like a dead chip but
        isn't. `read_frame` clears correctly.
- [ ] GT911 driver reads (x, y, event) tuples at expected rate
      (typical 100-200 Hz).
- [ ] Local stroke render: drawing on the panel produces visible
      ink with measured tip latency reported.
- [ ] Actor ed25519 keypair generated + persisted in NVS; Actor ID
      computed.
- [ ] First stroke publishes to `substrate/<actor-id>/ink/strokes/<id>`
      over WiFi + JSON-RPC.
- [ ] Echo-subscribe back: same device's own stroke renders a second
      time in a different color from the substrate path.
- [ ] ADR-057 enforcement: a second CLI subscriber without the
      `actor:<actor-id>` identity gets `acl_denied`.
- [ ] Spike write-up posted at `.planning/actors/SPIKE-INKPAD-FINDINGS.md`
      with: tip latency, wire-format revisions, ACL surprises,
      whether the device feels like a usable Actor.

## §9 — "Fallout glitch" aesthetic (2026-05-15, post-spike)

The 10-config DPI bring-up landed at: config #10 (parked DMA chain +
VSYNC-ISR re-arm) + fix-B heap split (SRAM heap + PSRAM tagged
External) + clock-mode `Phase::ShiftHigh`. Result: frame-locked,
readable, but with a **fine-grained per-pixel color sparkle** the user
described as a "very pleasant glitch" with Pip-Boy / dying-CRT energy.

Root cause is almost certainly the panel hardware: the LI0704122Z is
true RGB888 but the ESP only drives 16 of 24 data lines; the 8
unconnected panel inputs (R0–R2, G0–G1, B0–B2) on this PCB revision
likely pick up crosstalk EMI from the active high-bit lines at 12 MHz
→ pixel-LSB color noise. Not software-fixable on this board.

**This is captured, not discarded.** A complete snapshot of the three
load-bearing firmware files + a recipe/restoration README lives at
`.planning/actors/inkpad-snapshots/2026-05-15-fallout-glitch/`. If we
move the Inkpad to the manufacturer's ESP-IDF / LovyanGFX stack (the
production path), we can either re-apply the snapshot to get the look
back, or implement it as a software per-frame LSB-dither effect that
runs on any backend. Treat this as a **deliberate effect mode**, not
just a workaround.
