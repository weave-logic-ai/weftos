# ESP32-C5 vs ESP32-C6 for RuView sensing nodes

Short version: the two are close siblings, and the C5's real addition is
**5 GHz Wi-Fi** plus more headroom. For RuView, the practical difference is
that **only the C6 is supported today**. A C5 node would mean a firmware port.

## Sources

- **RuView, from rUv's repo via the RuvNet Brain:** firmware release v0.8.8
  (`ruview/docs/releases/v0.8.8-esp32.md`) ships images for the ESP32-S3 and
  the ESP32-C6 only. Neither those notes nor the flash guide mentions the C5.
- **Chip specs:** from general knowledge of Espressif's parts, not checked
  against a datasheet here. Entries marked *(verify)* are the least certain.

## The chips

| | ESP32-C6 | ESP32-C5 |
|---|---|---|
| CPU | single RISC-V core, 160 MHz, plus a 20 MHz low-power core | single RISC-V core, 240 MHz, plus a low-power core |
| Wi-Fi | Wi-Fi 6, **2.4 GHz only** | Wi-Fi 6, **dual-band 2.4 + 5 GHz** |
| Channel width | 20 MHz for Wi-Fi 6; 20/40 MHz for 802.11n | similar on both bands *(verify)* |
| Other radios | Bluetooth LE 5, Thread and Zigbee | Bluetooth LE 5, Thread and Zigbee |
| RAM | 512 KB on-chip | about 384 KB on-chip *(verify)* |
| External PSRAM | no | **yes** |
| Maturity | mass production since 2023; solid ESP-IDF support; stable in the Rust `esp-hal` 1.0 | newer, mass production around 2025; needs a recent ESP-IDF *(verify)*; Rust support less mature |
| Cost | slightly cheaper | slightly more |

## What it means for RuView

### 1. The C6 is supported and tested; the C5 is not

- RuView firmware 0.8.8 has a C6 build: a 4 MB flash layout, an image of
  about 1 MB, and 45% spare room for over-the-air updates.
- Two physical C6 boards were tested for about 5 minutes each. Both
  delivered 35–36 CSI packets per second, kept a steady 8 Hz on-device
  processing rate, and lost no samples.
- The C6 runs its processing at **8 Hz**, against the S3's **20 Hz**,
  because its CPU is slower. A C5 at 240 MHz could probably run at 20 Hz.
- No C5 image exists. Supporting it means a port: a new build target and
  flash layout, 5 GHz channel setup, and the same timing qualification
  the C6 got.

### 2. 5 GHz is the C5's real advantage for sensing

- **Finer motion detail.** The wavelength is about 6 cm, against 12.5 cm at
  2.4 GHz. The same movement produces a larger phase change, which helps
  with breathing, gestures and small motion.
- **Cleaner air.** There's much less interference from neighbours,
  Bluetooth and microwaves, and there are more channels. RuView's
  multi-node sensing schedule gets more room.
- **Shorter reach.** 5 GHz loses more strength through walls, so you need
  more nodes per room or tighter spacing.
- **Radar avoidance.** Some 5 GHz channels must avoid radar (DFS). Put
  sensing nodes on fixed channels that don't.

### 3. Mixing chips in one sensing mesh

- Nodes that sense each other must share a channel. C5 nodes on 5 GHz can't
  pair with C6 or S3 nodes, which are 2.4 GHz only.
- A C5 can run at 2.4 GHz to join an existing mesh, but then it gives up
  its main advantage.
- Practical layouts: give C5 nodes their own 5 GHz rooms or clusters, or use
  a C5 to bridge the two bands.

### 4. Memory

The C5 accepts external PSRAM; the C6 doesn't. That's room to buffer CSI,
run heavier on-device processing, or hold a small model.

### 5. More subcarriers isn't the reason to choose either

RuView's processing reads the older 802.11n training symbols: 56
subcarriers at 20 MHz, 114 at 40 MHz. Wi-Fi 6 frames carry more, but it's
unconfirmed whether either chip's CSI callback exposes them. Bench-test
that before relying on it.

### 6. A mistake in RuView's own docs

`ruview/docs/research/architecture/ruvsense-multistatic-fidelity-architecture.md`
says "2.4 GHz + 5 GHz on ESP32-C6". That's wrong. The C6 is 2.4 GHz only,
and the C5 is the dual-band part. The file is in rUv's RuView repo, which
we don't push to.

## Recommendation

- **Today:** buy C6 boards. They run RuView firmware now, with tested
  numbers.
- **For 5 GHz:** choose the C5 if you want 5 GHz's finer detail and cleaner
  air, and you're willing to fund a firmware port and qualify it the way the
  C6 was. That port is also a good first step toward the wideband 5/6 GHz
  work RuView's ADR-292 is aiming at.

*Written 2026-10-03.*
