# Sonar Buoy — Architecture Notes

This document captures the design rationale and the data-plane shape
agreed during the 2026-05-10 planning conversation. Companion docs:
[`requirements.md`](requirements.md), [`roadmap.md`](roadmap.md).

## Relationship to the research-half corpus

This is the build / hardware architecture. It is the *implementation*
side of several ADRs documented in the research half of this corpus
(`.planning/sonobuoy/` one level up):

- **Time sync & ranging** — the TWTT protocol described in §"Time
  synchronization" below is the v1 / pool-scale prototype of the
  production **OWTT + JANUS + CSAC + TSHL/D-Sync** stack specified in
  `../RANGING.md` (ADR-078). v1 TWTT covers Phase 1b through Phase 3
  of the build roadmap; production replacement with JANUS + CSAC
  lands in `../RANGING.md`'s recommended v2 protocol. Closes gap G1
  in `../GAPS.md`.
- **4-tier power hierarchy** — the on-buoy ESP32-S2/S3 MCU split
  here is the *hardware* of `../SYNTHESIS.md` §3 ("4-tier
  on-buoy/at-shore power hierarchy"). The S3 plays tier-2
  (~50 mW, MCUNet-distilled triggers) and partially tier-3
  (Cortex-M7-equivalent classification confirmation); the shore
  host plays tier 4 (full K-STEMIT + DEMONet + Perch + active
  imaging).
- **Deployment profile** — the v1 fleet maps to the
  `sonobuoy-tactical` profile (`../SYNTHESIS.md` §2.6); future
  refurbishable / long-deployment variants map to `sonobuoy-pam`
  (HARP-class, ~250 mW, year-scale duty).
- **Active-imaging branch** — the P79 + 235 kHz D imaging tier
  documented in `build-buoy-p79.md` is the hardware that grounds
  `../SYNTHESIS.md` §2.4 (the 5th, phase-coherent branch added in
  round 2): ADR-063 (SAS 5th branch), ADR-064 (Gerg-Monga deep
  autofocus), ADR-065 (Kiang 2022 multistatic SAS *with stationary
  sonobuoy* — clawft buoys are the C-node in Kiang's geometry).

## Why WeftOS does not run on the buoy

WeftOS is a tokio-on-native (or wasm-bindgen-on-browser) Rust system.
It assumes a desktop-class std environment with a multi-MB binary.
ESP32 chips do not have the SRAM, the flash, the OS scheduler, or
the right async executor model for tokio.

The buoys therefore run **embassy-rs on `esp-hal`** (no_std, async,
small) and **speak the WeftOS substrate wire format** to publish
events. The shore host runs full WeftOS and is the trusted compute
surface for any non-trivial math (trilateration, sound-speed
correction, fleet-wide state).

This is the same pattern already used for other sensor streams in
the project: edge devices stream raw events on chain, services
consume and enrich.

## Why WiFi is dead underwater (the constraint that shapes everything)

2.4 GHz is the resonance band water was *deliberately picked for in
microwave ovens*. Practical link ranges:

| Medium | Skin depth @ 2.4 GHz | Practical range |
|---|---|---|
| Air | — | 30–100 m |
| Distilled water | ~1.5 m | 1–3 m |
| Pool / fresh water | 5–15 cm | 20–50 cm |
| Sea water | ~5 mm | 1–3 cm (effectively dead) |

Consequence: **submerged comms must be acoustic**; WiFi only works
when antennas are above the waterline. The buoy form factor (sealed
top with antenna mast, ballast below) reflects this reality.

## Modular MCU split (one MCU per piezo)

Two independent MCUs per acoustic chain:

- **TX = ESP32-S2 mini.** Runs only a modulator state machine and an
  H-bridge driver. Wakes from light sleep when there's a frame to
  send. ~$3, single core, plenty.
- **RX = ESP32-S3 mini.** Runs the analog front-end ADC capture, the
  matched filter / Goertzel bank, the demod, the temperature probe,
  and the chain-publish path. Both cores busy: PHY on core 0,
  protocol on core 1.

### Why split, not unified

- **WiFi ISRs and ground-bounce.** A WiFi-enabled ESP32 jitters
  audio sampling and couples noise into a µV-level RX preamp.
  Physical separation eliminates this.
- **Failure isolation.** TX wedge does not blind the listener; RX
  panic does not drop beacons.
- **Independent firmware lifecycle.** The demod can be iterated
  without touching TX firmware at all.
- **Modular fleet.** TX-only beacons (deep sleep, sip battery),
  RX-only listening posts, full nodes = TX board + RX board.
- **Multiple RX per buoy.** Trivially supported — just add more
  RX-MCU+disc stacks on the same UART backbone. Each RX publishes
  its own events, all timestamped. With 2–3 RX per buoy at known
  vertical baselines, in-buoy TDoA gives bearing-to-emitter from
  a single buoy.
- **The S2 finally has a real job** (revisiting the original
  question that started this planning session).

### What the split does *not* fix: acoustic self-coupling

Two MCUs is digital independence; the acoustic coupling between
the TX disc and the RX disc on the same buoy is unchanged. The RX
will still hear its own buoy's TX as the loudest signal in the
water. Mitigations:

- **TX_ACTIVE GPIO** between TX and RX MCUs. RX uses it to gate
  the analog front end during local TX, and to timestamp
  start-of-symbol for self-loopback calibration.
- **Half-duplex protocol** at the link layer. Guard time
  between TX events for pool reverberation to settle: **plan for
  100-300 ms** as a working starting point; the 50 ms originally
  proposed here is too aggressive. Underwater pool T60 at 1.8 kHz
  is typically in the 200-500 ms range for a smooth-concrete
  pool, even though water absorption is negligible — coherent
  wall echoes dominate the decay tail. **Phase 1 empirical
  measurement supersedes the estimate** (this is in the
  `roadmap.md` Phase 1 exit criteria). Firmware should treat the
  guard time as a tunable parameter, not a hard-coded constant.
- **Physical separation** of TX and RX discs in the mast (≥ 10 cm)
  with foam decoupling between them.

## Modulation and link layer

- **Center frequency**: 1.8 kHz (the disc resonance).
- **Channel bandwidth**: ~300 Hz (bounded by Q ≈ 6 of the disc).
- **Modulation v1**: 4-tone MFSK (e.g., 1650/1750/1850/1950 Hz),
  50 baud → 100 bps gross. Demod via four parallel Goertzel
  detectors on the S3.
- **Modulation v2**: chirp-spread (1.5–2.1 kHz sweeps), ~50 bps,
  better SNR margin and better ranging precision (matched-filter
  cross-correlation gives sub-cycle timing). Switch when v1 hits
  its range floor.
- **Frame format** (v1, ~80 bits ≈ 0.8 s at 100 bps):
  ```
  [preamble 16b][node_id 8b][seq 8b][type 4b][payload 0–32b][crc16]
  ```
  Compact wire format only. No JSON, no full WeftOS envelopes.
  Payload tags pack into the 4-bit type field: `BEACON`,
  `RANGE_REQ`, `RANGE_RESP`, `TELEMETRY`.

### Channel access

- **v1 (3 nodes)**: pure ALOHA with random backoff is fine.
- **v2 (>3 nodes)**: tiny TDMA — one-second frames, each node owns
  one slot, schedule maintained over WiFi.

## Time synchronization (the unsung hard problem)

For TDoA localization across separate buoys we need either
synchronized clocks to ~10 µs (≈ 1.5 cm position resolution), or
a protocol that doesn't need shared time.

**v1 answer: two-way travel time (TWTT) ranging.**

```
buoy A                                buoy B
  │                                     │
  │ TX ping (timestamp_a_tx)            │
  │ ──────────acoustic───────────────▶  │ RX (timestamp_b_rx)
  │                                     │
  │                                     │ TX response (timestamp_b_tx)
  │ ◀──────────acoustic───────────────  │ RX (timestamp_a_rx)
  │                                     │
  │ ─────── WiFi: report b_rx, b_tx ──▶ │
  │                                     │
  shore service consumes both events,
  computes one-way delay = ((a_rx − a_tx) − (b_tx − b_rx)) / 2,
  and distance = delay × c(temperature).
```

No shared clock needed. With three buoys we get three pairwise
distances → unique triangle (up to a flip ambiguity that a fourth
detection or a known anchor resolves).

**v2 answer (open water)**: GPS module per surface buoy. Free PPS
for sub-µs sync *and* absolute lat/lon for the localization solver
to anchor against. Useless in the pool (no sky view); essential
for real water.

## On-chain data flow

The buoys publish to the WeftOS substrate. **Schemas below are
illustrative v1 sketches**, not the final wire format. The schemas
shown use `buoy_id: u8` for readability and use ad-hoc stream names
(`acoustic.event` etc.) for the same reason.

### Decision (added 2026-05-11, draft ADR-081)

The **production wire format adopts the WeftOS sensor-framework
contracts** at `.planning/sensors/JOURNALED-NODE-ESP32.md`,
`JOURNALED-SENSOR-MIC.md`, `HEALTHCHECK-CONTRACT.md`,
`PIPELINE-PRIMITIVE-JOURNAL.md`, and `PIPELINE-PRIMITIVE-SPIKE.md`
rather than rolling its own scheme. Specifically:

- **Identity**: each buoy publishes under
  `substrate/<n-6hex>/sensor/acoustic/...` with **ed25519 path
  identity + BLAKE3 fingerprint + write-gate** per
  JOURNALED-NODE-ESP32. The `buoy_id: u8` shown in the v1 schemas
  below is *not* the canonical identity — it remains only as a
  transit-time integer convenience inside a single chain envelope.
- **Hydrophone is a JOURNALED-SENSOR-MIC special case**: the
  `summary` + `pcm_chunk` sibling-split pattern is the right
  inheritance; the `Sensitivity::Capture` tier per ADR-012 is
  declared for raw PCM exposure.
- **Healthcheck is a sibling stream**, not a field on every event:
  battery / water-temp / SNR / packet-loss publish to
  `health/sensor/<name>` per HEALTHCHECK-CONTRACT.md §3, with the
  required `status` enum + `observed_rate_hz` + `tick`.
- **Derived/mesh streams** (`acoustic.position` from shore-side
  trilateration) publish under
  `substrate/_derived/acoustic/position/<source-buoy>` per the
  R3.2 tier rule, not as a flat top-level path.
- **Pipeline shape**: per-buoy pulser→ADC→matched-filter is the
  R2 Source/Stage/Sink template from PIPELINE-PRIMITIVE-SPIKE.md.
- **Explorer surface**: the buoy fleet appears in
  EXPLORER-MANAGEMENT-SURFACE.md as an Explorer Inventory entry
  with Actions (`buoy.set_mode`, `buoy.calibrate`, etc.) and
  Health rollups.

Driving rationale: rolling our own identity / health / pipeline
breaks composability with every other WeftOS sensor (whisper,
camera, etc.) and reintroduces 256-node ceilings + non-cryptographic
identity that the framework explicitly solved.

The schema rewrites that follow from this decision land in the
upcoming P3 / P4 panel runs of the symposium at
`.planning/symposiums/sonobuoy/`. The v1 illustrative schemas are
preserved below so the reader can see what they *will look like*
after the rewrite, plus a few breadcrumbs.

### Stream: `acoustic.event` (v1 illustrative; final path: `substrate/<n-6hex>/sensor/acoustic/event`)

One record per acoustic detection (frame received or matched-filter
peak above threshold).

```rust
AcousticEvent {
    // buoy_id below is the transit-time integer; the canonical
    // identity is the substrate path (<n-6hex>) per ADR-081.
    buoy_id: u8,
    rx_id: u8,                        // which RX on this buoy
    timestamp_local_us: u64,          // monotonic clock of this MCU
    timestamp_wallclock_ms: u64,      // best-effort NTP/GPS time
    detection: {
        peak_correlation: f32,        // matched-filter peak
        leading_edge_us: u64,         // µs from local timestamp to first arrival
        snr_db: f32,
    },
    decoded_payload: Option<Frame>,   // None if just a tone, Some if framed
    // battery_mv, water_temp_c migrate to the sibling health stream
    // per HEALTHCHECK-CONTRACT.md §3 (ADR-081 follow-up).
    water_temp_c: f32,                // for sound-speed correction
    battery_mv: u16,
    self_tx_active: bool,             // was this our own TX bleeding through?
}
```

### Stream: `acoustic.twtt`

One record per completed two-way ranging exchange. Initiator
publishes after receiving the response.

```rust
TwttRanging {
    initiator_buoy: u8,
    target_buoy: u8,
    initiator_tx_local_us: u64,
    target_rx_local_us: u64,          // reported back over WiFi
    target_tx_local_us: u64,
    initiator_rx_local_us: u64,
    water_temp_c: f32,                // initiator-side; target reports its own in its own event
}
```

## Shore-side localization service

Runs on the WeftOS host (laptop, mini-PC, whatever). Consumes the
two streams above and emits a third:

### Stream: `acoustic.position`

```rust
BuoyPosition {
    buoy_id: u8,
    timestamp_ms: u64,
    estimate: {
        x_m: f32, y_m: f32, z_m: f32,    // pool-frame coordinates
        covariance: [f32; 9],            // 3×3 row-major
    },
    method: "twtt-trilateration" | "tdoa" | "anchored-gps",
    sound_speed_c_m_per_s: f32,          // value used in the solve
    n_constraints: u8,
}
```

The service:

1. Aggregates `TwttRanging` events into a moving window of pairwise
   distances.
2. Computes sound speed from `water_temp_c` (UNESCO Mackenzie
   formula or similar; for pool water, c ≈ 1402 + 4.6·T − 0.055·T²
   m/s is fine).
3. Runs least-squares trilateration. With 3 buoys in 2D, that's a
   closed-form solve; for 3D or N>3, a Levenberg–Marquardt loop.
4. Emits `BuoyPosition` events to chain.
5. Feeds existing WeftOS surfaces.

The S3 is *absolutely not* the place to do this. Its job is to
publish raw events with accurate timestamps; the math is
shore-side.

## Pool acoustics will be brutal (in useful ways)

Pool walls reflect almost everything. Expect 5–20 echoes per ping.
This is actually the right crucible: surviving the pool means
open-water will feel easy. But:

- Use **leading-edge detection on the matched-filter output**, not
  peak-of-envelope. Reflected paths can be louder than the direct
  path; only the first arrival is geometrically meaningful.
- 1.8 kHz wavelength in water is 82 cm → raw resolution floor.
  Chirp-spread + cross-correlation pulls this to ~10–20 cm.
- Pool round-trip times are ~7 ms across the longest dimension.
  Inter-pulse spacing should target **≥ 200 ms initially**, with
  the actual value pinned down by Phase 1 T60 measurement. The
  earlier 50 ms estimate in this section was too aggressive —
  Sabine/Eyring on a typical 25 × 12 × 2 m smooth-concrete pool
  with α ≈ 0.02 gives a coherent-reverb decay tail of 200-500 ms
  at 1.8 kHz before scattering dominates. Validate empirically
  before locking the firmware constant.
- Don't trust raw ToF in the pool for absolute distance until the
  demod's first-arrival picker has been validated against a known
  separation.

## Acoustic-physics budget references

Quantitative dB-budget grounding for the build corpus lives in
panel P1 of the sonobuoy symposium
(`.planning/symposiums/sonobuoy/panels/P1-acoustic-physics.md`),
which instantiates the sonar equation at every clawft band. The
paper analyses behind P1's table (with one-line usage):

- **`papers/analysis/urick-sonar-equation.md`** (Urick 1983
  *Principles of Underwater Sound*) — the SE = SL − TL + TS −
  (NL − DI) − DT framework that anchors every range / SNR claim
  in this doc and in `phase-economics.md`. P1 §1.1 instantiates
  it for all seven clawft bands.
- **`papers/analysis/wenz-ambient-noise.md`** (Wenz 1962 +
  Dahl-Dall'Osto 2025 retrospective) — the noise-floor input.
  At 1.8 kHz the SS0→SS3 swing is 20 dB and **fixed-threshold
  detectors will fail outright**; at 200+ kHz the swing is 3 dB
  and the imaging tier is sea-state-insensitive (P1 §1.4).
- **Francois & Garrison 1982** (*JASA* 72:896 + 72:1879) —
  formula written out in P1 §1.1.2. Fresh-water absorption at
  1.8 kHz is 1×10⁻⁴ dB/km (negligible); at 235 kHz it is
  20 dB/km fresh / 65 dB/km seawater (caps the imaging-band
  open-water range around 200–300 m).
- **`papers/analysis/kraken-propagation.md`** (Porter 1991) —
  range-independent normal-mode propagation, default solver for
  the long-range 1.8 kHz PAM band.
- **`papers/analysis/bellhop-ray-tracing.md`** (Porter & Bucker
  1987, Porter 2011) — range-dependent shallow-water ray
  propagation. Default solver for clawft pool / coastal regime
  at the imaging bands.
- **`papers/analysis/capon-mvdr.md`** + **`papers/analysis/
  schmidt-music.md`** — MVDR / MUSIC for Phase 3 in-buoy bearing
  and Phase 6 shore-side multi-node beamforming, beyond the λ/L
  aperture-limited resolution.
- **`papers/analysis/bucker-mfp.md`** — matched-field processing
  consumes the in-situ SSP (`RANGING.md` §3) for passive source
  localization across the mesh.
- **`papers/analysis/lbl-acoustic-nav.md`** (Hunt et al. 1974
  WHOI) — historical foundation for the LBL geometry that
  clawft v1 TWTT inherits.
- **`papers/analysis/cooperative-buoy-positioning.md`** (Otero et
  al. 2023, *JMSE* 11:682) — closest paper analog to clawft
  Phase 4 (drifting GPS-anchored buoy mesh).

### Propagation regime selection

For deciding which solver applies at which band in which
deployment:

- **Pool / lake / harbor, all bands**: BELLHOP ray tracing.
  Range-dependent, shallow, multipath-dominated; rays handle
  the wall-floor-surface bounce structure correctly. 1.8 kHz in
  a 2 m pool is well above the mixed-layer cutoff
  `f_c ≈ c/(4·H_ML)` ≈ 185 Hz, so no surface-duct trapping —
  free-field plus discrete reflections.
- **Open water, ≥ 1 km, ≤ 1 kHz** (long-range passive
  monitoring): KRAKEN normal modes. Range-independent
  assumption is acceptable when the SSP is depth-only.
- **Open water, 1–10 km, ≥ 10 kHz** (active imaging):
  BELLHOP-3D. Azimuth matters; rays diverge in the surface duct
  pattern.
- **FNO surrogate** (Zheng 2025 / ADR-080 ThermoFno): inference-
  time approximation for online use on the shore host;
  benchmark against KRAKEN / BELLHOP before consuming. Falls
  out of scope for any safety-critical path.
