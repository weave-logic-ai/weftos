# Sonar Buoy — Class B Tethered Subsurface Build (1m / 2m Below Surface Buoy)

**Status**: Planning. Drafted 2026-05-11 by the sonobuoy symposium.
**Class**: B — Tethered Subsurface (per ADR-082).
**Build cost**: ~$15-20 per subsurface unit (parent Class A buoy
amortizes the rest).
**Build time**: ~45 minutes per unit.
**Companion ADRs**: ADR-081 (sensor-framework adoption), ADR-082
(three-class architecture), ADR-083 (calibration service), ADR-084
(acoustic time-sequencing).

## What this is

A **subsurface oil-filled sidecar chamber** hanging on a marine
cable below a parent Class A surface buoy, at fixed depths of
**1 m and 2 m** (and optionally arbitrary depths to 5 m). Each
Class B unit is a receive-only acoustic node — a JFET hydrophone
exactly like the one in `build-hydrophone-oil.md`, but on a
longer cable, with deeper deployment depth, and electrically
tethered to its parent rather than running on local battery.

The parent Class A buoy provides:
- Power to the Class B over the cable (3.3 V or 5 V rail).
- Time-base reference (the parent's clock is the Class B's clock).
- Comms uplink (the Class B's chain events are merged into the
  parent's substrate stream).

Two Class B per Class A at 1 m and 2 m gives an **in-buoy TDoA
bearing** capability per parent buoy at known vertical baseline
(0.6 m at this geometry; 1 m if you choose 1 m and 2 m at the
cable ends). Three Class A surface buoys with this configuration
gives the fleet **9 receive elements at known positions** — a
qualitative upgrade over the original 3-receiver mesh.

## Why this design

Class B is the **direct evolution of the oil-sidecar pattern**
from `build-hydrophone-oil.md`, with two changes:

1. **Longer cable** (1-3 m) instead of the short ballast-section
   pigtail. The Class B chamber hangs below the parent buoy in
   the actual water column, not inside the buoy's flooded
   ballast section.
2. **Deep-deployment seal** — the Class B chamber is at
   1-2 m depth in real water continuously; the seal quality
   matters more than the surface-buoy sidecar's "lives inside
   ballast" environment.

The rest is identical: same 1.5" PVC chassis, same JFET source-
follower circuit, same threaded service cap with O-ring, same M8
IP67 bulkhead connector at the cable side.

## Mechanical layout

```text
   [ Surface — parent Class A buoy ]
   ╔══════════════════════════════╗
   ║   Antenna mast               ║
   ║   ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓ ║ ← water surface
   ║   Sealed top section         ║
   ║   ┌──────────────────────┐   ║
   ║   │ S2/S3 electronics    │   ║
   ║   │ Class A's piezo      │   ║
   ║   │ M8 bulkhead × 2 or 4 │   ║ ← M8 IP67 connectors for
   ║   └──────────┬──┬────────┘   ║   Class B downlinks
   ║              │  │             ║
   ║   Ballast section (vented)   ║
   ║              │  │             ║
   ╚══════════════│══│═════════════╝
                  │  │ ← marine cable (4-conductor:
                  │  │   +V / Signal / GND / I²C-or-Drain)
                  │  │   1 m to upper Class B
                  ▼  │
              ╔══════│══════╗
              ║ Class B #1   ║ ← Class B sidecar at -1 m
              ║ (1 m depth)  ║   Same chassis as oil-sidecar
              ║              ║   (1.5" PVC tube, ~80 mm)
              ║   JFET hydro ║
              ║   in oil     ║
              ╚══════│══════╝
                     │ cable continues
                     │ 1 m to lower Class B
                     ▼
              ╔══════│══════╗
              ║ Class B #2   ║ ← Class B sidecar at -2 m
              ║ (2 m depth)  ║
              ║              ║
              ║   JFET hydro ║
              ║   in oil     ║
              ╚══════│══════╝
                     │
                     │ optional extension cable
                     │ to anchor weight
                     ▼
                     ◆ Anchor (2-3 kg dive weight)
                       Keeps Class B units vertically
                       stable in current
```

### Cable routing

- **Parent → upper Class B**: M8 connector on the surface buoy's
  base bulkhead → cable run through a sealed cable gland on the
  underside of the ballast section → cable terminates at an M8
  connector on the upper Class B's top.
- **Upper Class B → lower Class B**: M8 connector on the upper
  Class B's bottom → cable run with strain relief at both ends →
  M8 connector on the lower Class B's top.
- The pattern chains arbitrarily: 3+ Class B per Class A is
  supported by adding M8 bulkhead connectors at each level.

### Cable type

- **4-conductor M8 cable**, 22 AWG, marine-jacketed (TPE or PUR),
  shielded. Sources: Amazon ~$10 for 5 m; M12-Cables.com for
  higher quality.
- Wire assignments (matching `build-hydrophone-oil.md` M8 pinout):
  - Pin 1: +V (3.3 V)
  - Pin 2: Signal (analog RX from JFET source-follower)
  - Pin 3: GND
  - Pin 4: spare / I²C SCL / 1-Wire data (for future sensor
    sidecars in the same chassis)
- Length: 1-3 m typical (longer cable adds capacitance but the
  JFET source-follower's ~3-10 kΩ output impedance tolerates
  several meters at acoustic frequencies — see
  `hydrophone-transducer-expert` persona §"Cable matters").

## Per-unit BOM

| Qty | Part | ~Price |
|-----|------|--------|
| 1 | 1.5" PVC pipe, ~80 mm length | $0.50 |
| 1 | 1.5" flat test cap (bottom, permanent) | $1 |
| 1 | 1.5" threaded cleanout adapter + plug (top, serviceable) | $3 |
| 1 | Nitrile or silicone O-ring | $0.30 |
| 1 | M8 4-pin IP67 connector pair (panel + cable) | $5 |
| 1 | PG7 cable gland with EPDM seal (cable exit) | $1 |
| 1 | 27 / 35 mm piezo disc | $1 (or $0 if reusing existing) |
| 1 | J201 JFET + 10 MΩ + 10 kΩ resistors | $0.60 |
| ~30 mL | Mineral oil (fragrance-free) | $0.20 |
| ~5 mL | PVC primer + cement | $0.20 amortized |
| 1-3 m | M8 marine cable (per cable run between levels) | $5-15 |
| 1 | Silicone caulk + heat shrink (strain relief) | $0.50 |
|     | **Total per Class B unit** | **~$15-20** |

For a fleet of 3 Class A buoys × 2 Class B per buoy = 6 Class B
units. Total Class B BOM: ~$100. Compared to the cost of building
3 additional Class A buoys for the same number of receive
elements (~$180): roughly half the cost, with the bonus that
Class A buoys are at one depth only.

## Electrical: same JFET source-follower as Class A sidecar

The circuit is identical to `build-hydrophone-oil.md` §"Circuit"
(see also `build-hydrophone-epoxy.md` §"Circuit"). 3-wire JFET
source-follower at 3.3 V:

```
Hydrophone end (oil-immersed):           Parent buoy end:

piezo+ ────── Gate (10 MΩ to GND)
              │                          +V wire (Pin 1) ←── 3.3 V
              │
              ── Drain ←── +V wire
              │
              ── Source ──┬──── Signal wire (Pin 2) ──► 1 µF DC block
                          │                              │
                       R_s = 10 kΩ                       ▼
                          │                          MCP6022 (parent buoy's BPF chain,
                          ▼                          unchanged)
piezo- ───────── GND wire (Pin 3) ────────
```

The signal wire feeds the parent Class A's existing 1.8 kHz BPF
chain via a multiplexer (one BPF per Class B + the parent's own
hydrophone). For 2 Class B + 1 parent hydrophone = 3 inputs, an
ADG704 4:1 mux (or 3 dedicated BPF chains) suffices.

### Multi-channel ADC capture

The parent Class A's ESP32-S3 RX MCU samples each Class B input
via:

- **Option 1** (analog mux + single ADC): ADG704 selects which
  Class B is currently being sampled; round-robin scheduling at
  100 Hz per channel.
- **Option 2** (parallel ADCs): one MCP3201 SAR ADC per Class B,
  SPI multi-drop to the S3. Real-time-simultaneous capture →
  enables true TDoA across the multi-depth aperture.

**Option 2 is the right answer** for the Phase 3 in-buoy bearing
goal — TDoA requires simultaneous capture. The cost is one extra
MCP3201 ($1.50) per Class B, well within the disposability frame.

## Acoustic time-sequencing inheritance (ADR-084)

A Class B node shares its parent Class A's clock via the cable.
There is no separate time-base; every `acoustic.timing` event
emitted from a Class B carries the parent's substrate path with a
sub-path qualifier for the depth tier:

```
substrate/<parent-n-6hex>/sensor/acoustic/timing/B1m   (upper Class B)
substrate/<parent-n-6hex>/sensor/acoustic/timing/B2m   (lower Class B)
```

This nesting per ADR-081 R3.x rules: Class B is logically a
subsensor of its parent Class A.

## Sensitivity per band

A 1.8 kHz hydrophone in mineral oil with JFET source-follower
preamp has effective sensitivity ~-205 dB re 1 V/µPa (per
`hydrophone-transducer-expert` persona §"Sensitivity and noise
floor"). With the BPF gain stage's 60 dB → -145 dB re 1 V/µPa
referred to the ADC input. ADC noise floor ~5 µV → noise-equiv
SPL ~25 dB re 1 µPa/√Hz → integrated across the 300 Hz BPF
bandwidth, ~50 dB re 1 µPa total.

Wenz sea-state-1 noise at 1.8 kHz (per P1 §1.4) is ~55-65 dB re
1 µPa²/Hz × 300 Hz bandwidth ≈ 80-90 dB re 1 µPa total. **The
hydrophone is environment-limited, not preamp-limited** — good.

## Operating modes per Class B

A Class B unit operates in three modes inherited from the parent:

- **RX-only** (default): the Class B is a receive-only hydrophone
  contributing to the parent's TDoA bearing and to the mesh-wide
  joint inference. No TX from Class B.
- **RX + sensor** (optional): if the spare M8 pin carries an I²C
  sensor (depth, temperature, light), the Class B chassis also
  serves as a sensor sidecar in the parent's pipeline.
- **Calibration target** (Phase 1c+): when the parent or another
  Class A fleet member TX-chirps, the Class B records the direct-
  path arrival as a calibration reference. ADR-083 shore service
  consumes this.

## Assembly steps

**1. Build the JFET subassembly** (~20 min)
Same as `build-hydrophone-oil.md` §"Assembly sequence (per RX
sidecar)" steps 1-3. Bench-test the JFET output in air with a
piezo tap before potting.

**2. Install the M8 connector on the chamber top** (~15 min)
Same as oil-sidecar. Wire the M8 pins to short pigtails inside
the chamber.

**3. Fill with mineral oil** (~10 min including bubble settle)
Same as oil-sidecar. 90% fill, 10% headspace.

**4. Thread the service cap** (~5 min)
Same as oil-sidecar.

**5. Pressure test** (~30 min)
Submerge the Class B unit alone in a deep sink for 30 minutes,
inspect for leakage. Especially watch the threaded service cap
under pressure — at 2 m depth the static pressure is ~0.2 atm
gauge, which is well within service-cap rating but worth verifying
empirically.

**6. Attach the marine cable** (~10 min)
Crimp the M8 cable connector on one end. Other end gets crimped
into the parent Class A's bulkhead. Strain-relief at both ends
(silicone over heat-shrink).

**7. Integrate with parent Class A** (~15 min)
Wire the Class A's ESP32-S3 multi-channel ADC path per the
firmware spec (Option 2 above for true TDoA). Update the parent
firmware config to add the new Class B to its known children
roster.

**8. Field test** (~30 min)
Drop the assembled parent + 2 Class B configuration in the pool.
Tap a piezo at known position; verify both Class B units capture
the signal with correct ToF delta matching the known vertical
geometry. This is the Phase 1c in-buoy TDoA bearing exit criterion.

## Phase-by-phase deployment

- **Phase 1c (proposed)**: build 6 Class B units (2 per Class A).
  Pool-validate in-buoy TDoA bearing. Exit: bearing-to-emitter
  estimated within ±5° at known pool emitter positions.
- **Phase 2 (lake)**: deploy unchanged. Add the Class B units to
  the joint-inference solver's known-position roster.
- **Phase 4 (open water)**: longer cables (3-5 m) for greater
  depth diversity. Same hardware.
- **Phase 5+**: Class B variants with imaging-tier transducers
  (50 / 200 / 235 kHz) become possible — extends the gimballed
  multi-band imaging into the water column.

## Variants

- **Class B-mic** (default): JFET hydrophone, as documented above.
- **Class B-temp**: DS18B20 temperature probe in place of the
  hydrophone. For SSP measurement.
- **Class B-pressure**: MS5837 high-accuracy pressure for absolute
  depth confirmation.
- **Class B-imaging** (Phase 5+): adds a 200/235 kHz piezo +
  pulser daughterboard. The chassis is the same; the M8 cable
  carries the higher-voltage pulser drive plus the analog RX
  return.

## Risk register

- **Cable failure** at the M8 connector under fatigue (waves,
  current): use strain reliefs at both connector ends. Replace
  the cable on inspection if visible damage.
- **Connector flooding** if M8 mate isn't fully threaded: pre-
  flight checklist for the diver before drop.
- **Anchor drift** under current: the anchor weight at the bottom
  of the Class B chain keeps the chain vertical. Without an
  anchor, Class B units drift horizontally with current and the
  bearing-baseline becomes uncertain. **Use an anchor for any
  deployment in moving water.**
- **Galvanic corrosion** of dissimilar metals at the M8 connector
  (brass / stainless / aluminum housings) in salt water: brass
  M8 housings are OK in fresh water; salt-water deployments need
  bronze housings (~$15 vs ~$5).

## References

- ADR-082 (Class B architectural role).
- `build/build-hydrophone-oil.md` (parent build doc for the
  sidecar circuit + chassis + M8 connector pattern).
- `build/build-hydrophone-epoxy.md` (JFET source-follower circuit
  reference).
- `hydrophone-transducer-expert` persona for sensitivity / noise
  / cable analysis.
- `.planning/sensors/JOURNALED-NODE-ESP32.md` (R3.x sub-sensor
  pathing).
- P1 §1.4 (Wenz noise floor), §1.5 (DI per aperture).
- P2 §2.4 (consolidated 3D-print matrix — applies to internal
  brackets in the parent Class A that hold the multi-channel
  ADC daughterboard).
