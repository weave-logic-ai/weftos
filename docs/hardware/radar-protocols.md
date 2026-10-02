# 24 GHz Radar / Sensor UART Protocols — Parser Reference

Reference for writing Rust cog parsers for mmWave radar modules over UART.
Every protocol claim cites a source; anything not verifiable from a public
source is marked **UNCONFIRMED**. Public facts only.

> **Byte-order convention:** all multi-byte numeric fields below are
> **little-endian** (low byte first) unless stated otherwise. `LE` = little-endian.

---

## Quick summary

| Module | What it is | Default baud | Format | Frame format status |
|--------|-----------|-------------|--------|---------------------|
| **Ai-Thinker RD-03E** | 24 GHz FMCW presence + ranging radar (S3KM111L SoC) | **256000** | 8N1 | Simple report frame **confirmed** (community captures); full config protocol partial/UNCONFIRMED |
| **Ai-Thinker RD-03D** | 24 GHz FMCW multi-target tracking radar (variant) | **256000** | 8N1 | **Confirmed** 30-byte X/Y/speed frame |
| **HLK-AS201** | **NOT a radar** — Hi-Link 9/10-axis attitude/IMU sensor | **115200** (4800–921600) | 8N1 | WitMotion-style frame = **fallback** (confirm vs Hi-Link datasheet) |
| Generic VCC/GND/OT1/RX/OT2 board | Presence radar breakout w/ digital triggers | — | — | OT1/OT2 = digital outputs; RX = UART config in (typical) |

---

## 1) Ai-Thinker RD-03E — 24 GHz presence + ranging radar

- **Chip:** S3KM111L single-chip 24 GHz FMCW radar SoC (24.0–24.25 GHz, ≤1 GHz BW).
  Source: <https://openelab.io/products/ai-thinker-rd-03e-24g-millimeter-wave-radar>,
  <https://www.electroniclinic.com/rd-03e-mmwave-human-detection-sensor-with-esp32-distance-measurement-hand-gesture/>
- **UART:** default **256000 bps, 8 data bits, no parity, 1 stop bit (8N1)**, 3.3 V TTL.
  Source: <https://www.electroniclinic.com/rd-03e-mmwave-human-detection-sensor-with-esp32-distance-measurement-hand-gesture/>
- **Range/accuracy:** moving human to 6 m, micro-motion to 3.5 m; ±5 cm from
  30–350 cm, ±5 % from 350–600 cm.
  Source: <https://openelab.io/products/ai-thinker-rd-03e-24g-millimeter-wave-radar>
- **vs RD-03D:** the RD-03E ("E") is the **presence/ranging + gesture** part; the
  **RD-03D ("D") is the multi-target tracking variant** that outputs X/Y/speed per
  target (different frame — see §2). They are **not** wire-compatible.
  Source: <https://www.espboards.dev/sensors/rd03/>

### 1a) Simple report frame (default "ranging" firmware) — CONFIRMED by community captures

The RD-03E streams a short fixed frame continuously. **5 bytes per frame:**

| Offset | Bytes | Field | Meaning |
|-------:|:-----:|-------|---------|
| 0 | 1 | Header | `0xAA` (frame start) |
| 1 | 1 | Distance low | distance in **cm**, LE low byte |
| 2 | 1 | Distance high | distance in cm, LE high byte |
| 3 | 1 | Gesture / state | `0x00` = no target; `0x01` = target/presence; `0x02`–`0x08` = motion/gesture codes (approach, retreat, swipe, etc.) |
| 4 | 1 | Footer | `0x55` (frame end) |

**Distance** = `buf[1] | (buf[2] << 8)` (cm). **Gesture/state** = `buf[3]`.
No checksum — validate on header `0xAA` + footer `0x55`.

**Real example captures:**
```
AA 2D 00 00 55   -> distance = 0x002D = 45 cm, state = 0x00 (no target)
AA 36 00 01 55   -> distance = 0x0036 = 54 cm, state = 0x01 (target present)
```
Source (frame + examples): web decode of RD-03E report stream, confirmed against
the ElectronicLinic parser and community gist
<https://gist.github.com/gtors/1e7a67bf539f2d0f2fcc63ac61f98f88> and
<https://www.electroniclinic.com/rd-03e-mmwave-human-detection-sensor-with-esp32-distance-measurement-hand-gesture/>

> **Note / minor conflict:** one secondary source rendered this as a 6-byte frame
> with a two-byte `0x55 0x55` footer and gesture at offset 1. The 5-byte layout
> above matches the real example captures (`AA 2D 00 00 55`) and the parsing code
> `distance = buf[1] | (buf[2]<<8); gesture = buf[3]`, so treat 5-byte as
> authoritative and tolerate a trailing `0x55` by resyncing on the next `0xAA`.

**Parser guidance (Rust):** scan for `0xAA`, read 4 more bytes, require `buf[4] == 0x55`,
else drop one byte and resync. Frames arrive at the radar's internal rate (~tens of Hz).

### 1b) Full S3KM111L command/config protocol — PARTIAL / UNCONFIRMED

The S3KM111L also supports a richer command/ACK protocol (enter/exit config mode,
read firmware, set thresholds/gate energies) in the style of the Ai-Thinker/Hi-Link
LD2410 family (command header `FD FC FB FA`, footer `04 03 02 01`; report header
`F4 F3 F2 F1`, footer `F8 F7 F6 F5`). **Whether the RD-03E uses exactly these
headers/gate-energy payloads is UNCONFIRMED from public sources** — verify against
the official `rd-03e_v1.0.0_specification.pdf` on docs.ai-thinker.com before relying
on config-mode framing. The simple report frame in §1a is sufficient for
presence + distance and needs no command to start (it streams on power-up).

Official spec (not fetchable as text here; PDF): referenced as
`https://docs.ai-thinker.com/_media/rd-03e_v1.0.0_specification.pdf` — **UNCONFIRMED
contents; obtain and verify.**

---

## 2) Ai-Thinker RD-03D — 24 GHz multi-target tracking radar

Included because it's the tracking sibling of the RD-03E and its frame is fully
decoded. If your part outputs X/Y per target, it's an RD-03D (or RD-03E flashed
with tracking firmware), not the simple RD-03E ranging frame.

- **UART:** **256000 bps, 8N1**, 3.3 V TTL.
  Source: <https://www.espboards.dev/sensors/rd03/>,
  <https://www.electroniclinic.com/rd-03d-mmwave-radar-multi-human-tracking-with-distance-speed-positioning/>
- **Frame:** fixed **30 bytes**, up to **3 targets**.

| Offset | Bytes | Field | Meaning |
|-------:|:-----:|-------|---------|
| 0–3 | 4 | Header | `AA FF 03 00` |
| 4–11 | 8 | Target 1 | X, Y, speed, distance-res (see per-target below) |
| 12–19 | 8 | Target 2 | same layout |
| 20–27 | 8 | Target 3 | same layout |
| 28–29 | 2 | Footer | `55 CC` |

**Per-target 8-byte block (all LE):**

| Sub-offset | Bytes | Field | Encoding |
|-----------:|:-----:|-------|----------|
| +0,+1 | 2 | X coordinate (mm) | 16-bit; **bit 15 = sign** (see below) |
| +2,+3 | 2 | Y coordinate (mm) | 16-bit; bit 15 = sign |
| +4,+5 | 2 | Speed (cm/s) | 16-bit; bit 15 = sign |
| +6,+7 | 2 | Distance resolution (mm) | 16-bit unsigned |

**Sign encoding (CONFIRMED by module behaviour / community correction):** the MSB
(bit 15) is a **sign flag**, magnitude is the low 15 bits. If bit 15 is set → value
is **positive**; if clear → **negative** (per the vendor convention; verify polarity
against your mounting). Decode:
```rust
let raw = u16::from_le_bytes([buf[i], buf[i+1]]);
let mag = (raw & 0x7FFF) as i32;
let val = if raw & 0x8000 != 0 { mag } else { -mag };
```
Source: <https://www.electroniclinic.com/rd-03d-mmwave-radar-multi-human-tracking-with-distance-speed-positioning/>
(article + commenter correction on the bit-15 sign scheme).

An all-zero 8-byte target block = that target slot is empty.

**Real example frame:**
```
AA FF 03 00  05 01 19 82 00 00 68 01  E3 81 33 88 20 80 68 01  00 00 00 00 00 00 00 00  55 CC
            |---------- target 1 -----||---------- target 2 -----||---- target 3 (empty) ---|
```
Source: <https://www.electroniclinic.com/rd-03d-mmwave-radar-multi-human-tracking-with-distance-speed-positioning/>

Working Rust/C driver references:
<https://github.com/Oz-113/rd-03d_lib>,
<https://github.com/gomgom-40/RD03Radar>

---

## 3) HLK-AS201 — IDENTITY CORRECTION: this is an IMU, not a radar

**The photo (X/Y/Z axis silkscreen, shielded can, "HKJM") identifies this as
Hi-Link's AS201 attitude/IMU sensor, NOT a mmWave radar.** The X/Y/Z axis markings
are the giveaway — radar modules label antenna/range, IMUs label the three sensor
axes. Hi-Link lists **AS201-9** as a *"9-axis Attitude Sensor Gyroscope Module"*
(3-axis accel + 3-axis gyro + 3-axis magnetometer; some variants add a barometer →
"10-axis"). A PCB antenna + shielded can is consistent with a **Bluetooth/BLE
variant** of the attitude sensor, not a radar.
Source: <https://hlktech.net/index.php?id=1380>

> There is also an unrelated **"HLK-AS201 UART-to-WiFi"** serial module in the wild
> (2.4 GHz, TCP/UDP/MQTT) — a PCB antenna could match that too. But the **X/Y/Z axis
> markings rule the WiFi module out**: a serial-to-WiFi bridge has no axes. It is the
> **attitude sensor**.
> WiFi-module ref (for disambiguation only):
> <https://m.indiamart.com/proddetail/hlk-as201-uart-to-wifi-serial-module-2857648791662.html>

### If it is the attitude sensor — UART output

- **UART:** default **115200 bps** (adjustable 4800–921600), **8 data bits,
  1 stop bit, no parity**. Streams accel / angular-rate / magnetometer / Euler
  angles (X/Y/Z) / quaternion / temperature.
  Source: <https://hlktech.net/index.php?id=1380>
- **Exact Hi-Link AS201 frame layout:** **UNCONFIRMED** — the Hi-Link product page
  does not publish the byte-level frame. **Confirm against the AS201 datasheet /
  Hi-Link doc center before trusting offsets.**

### FALLBACK frame format — WitMotion "WIT standard" protocol (clearly labeled fallback)

This class of cheap Chinese 9/10-axis UART IMUs overwhelmingly uses the **WitMotion
WIT standard protocol**, and the AS201's field set (accel/gyro/mag/Euler/quaternion/
temp, 115200 8N1) matches it exactly. Use this as the **closest well-documented
equivalent** if the AS201 output matches on the wire; **verify the `0x55` header +
type byte appear before relying on it.**

**Each packet = 11 bytes, fixed:**

| Offset | Bytes | Field | Meaning |
|-------:|:-----:|-------|---------|
| 0 | 1 | Header | `0x55` |
| 1 | 1 | Type | `0x51` accel · `0x52` angular velocity · `0x53` Euler angles · `0x54` magnetometer (others: `0x50` time, `0x56` pressure/height, `0x59` quaternion) |
| 2–9 | 8 | Payload | four 16-bit **LE signed** values (see per-type) |
| 10 | 1 | Checksum | `SUM = (byte0 + byte1 + ... + byte9) & 0xFF` |

**Payload per type (each value = `i16::from_le_bytes([L, H])`):**

- **`0x51` acceleration:** Ax, Ay, Az, Temp.
  `a = raw / 32768.0 * 16.0` g  (±16 g range); `temp = raw / 100.0` °C.
- **`0x52` angular velocity:** Wx, Wy, Wz, Temp. `w = raw / 32768.0 * 2000.0` °/s.
- **`0x53` Euler angles:** Roll(X), Pitch(Y), Yaw(Z), Version/Temp.
  `angle = raw / 32768.0 * 180.0` degrees.
- **`0x54` magnetometer:** Hx, Hy, Hz, Temp (raw counts).

**Example (acceleration packet):**
```
55 51 AxL AxH AyL AyH AzL AzH TL TH SUM
```
Decode: `ax = i16(AxL,AxH) / 32768 * 16` (g).
Validate: `SUM == (0x55 + 0x51 + AxL + ... + TH) & 0xFF`.

Source (WIT standard protocol, frame + scaling + checksum):
<https://wit-motion.gitbook.io/witmotion-sdk/wit-standard-protocol/wit-standard-communication-protocol>

**Parser guidance (Rust):** scan for `0x55`, read 10 more bytes, verify checksum,
dispatch on `buf[1]` type byte. By default these sensors stream several packet
types back-to-back (accel, then gyro, then angle, …).

---

## 4) Generic 24 GHz board: VCC / GND / OT1 / RX / OT2

A bare 24 GHz presence board with these pins is a **digital-output + UART-config**
style module (common on RCWL / HLK-LD and clone presence boards):

- **VCC / GND** — power (often 3.3 V or 5 V depending on board).
- **OT1 (OUT1)** — **digital presence/motion output** — goes HIGH when a target is
  detected, LOW (after a hold time) when clear. Wire straight to a GPIO for simple
  presence; no UART needed.
- **OT2 (OUT2)** — **second digital output** — typically a second trigger: either a
  micro-motion/occupancy flag vs OT1's motion flag, or a second range/zone gate.
  Board-specific; confirm with a logic probe.
- **RX** — **UART receive on the module = config input.** Send AT/command frames
  here to set sensitivity, hold time, range gates. Many of these boards expose only
  RX (config-in) and drive detection results out via OT1/OT2 rather than a UART TX
  data stream.

**Status: typical/GENERIC interpretation — UNCONFIRMED for any specific board.**
OT-pin polarity, voltage, and whether a UART TX data stream exists vary by part;
verify against that board's own silk/datasheet or a logic probe. Comparable
documented Hi-Link presence parts (LD1115H/LD1125H) instead expose `Vo` (output) +
`URX`/`UTX` and stream ASCII `mov`/`occ` strings.
Source: <https://www.tinytronics.nl/product_files/005045_HLK-LD1115H-24G_Use_Manual.pdf>,
<https://esphome.io/components/sensor/ld2420/>

---

## Sources

- RD-03E / RD-03D overview & pinout — https://www.espboards.dev/sensors/rd03/
- RD-03E decode + examples — https://www.electroniclinic.com/rd-03e-mmwave-human-detection-sensor-with-esp32-distance-measurement-hand-gesture/
- RD-03E community gist — https://gist.github.com/gtors/1e7a67bf539f2d0f2fcc63ac61f98f88
- RD-03D decode + example frame — https://www.electroniclinic.com/rd-03d-mmwave-radar-multi-human-tracking-with-distance-speed-positioning/
- RD-03D driver — https://github.com/Oz-113/rd-03d_lib
- RD-03E spec PDF (UNCONFIRMED contents) — https://docs.ai-thinker.com/_media/rd-03e_v1.0.0_specification.pdf
- HLK-AS201 / AS201-9 attitude sensor — https://hlktech.net/index.php?id=1380
- HLK-AS201 WiFi module (disambiguation) — https://m.indiamart.com/proddetail/hlk-as201-uart-to-wifi-serial-module-2857648791662.html
- WitMotion WIT standard protocol (AS201 fallback) — https://wit-motion.gitbook.io/witmotion-sdk/wit-standard-protocol/wit-standard-communication-protocol
- HLK-LD1115H manual (generic OT/Vo pin reference) — https://www.tinytronics.nl/product_files/005045_HLK-LD1115H-24G_Use_Manual.pdf
