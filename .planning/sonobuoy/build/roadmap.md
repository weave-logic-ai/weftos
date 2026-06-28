# Sonar Buoy — Roadmap

Phased plan from pool prototype to open water. Each phase is a
shippable demonstration, each builds on the last. Companion docs:
[`requirements.md`](requirements.md), [`architecture.md`](architecture.md).

## Phase 0 — Bench validation (pre-pool)

**Goal**: Prove each subsystem works on the bench before any of them
get wet.

- TX subsystem: S2 + DRV8837 + disc, drive a 1.8 kHz tone, measure
  acoustic output with a phone SPL meter.
- RX subsystem: S3 + op-amp + BPF + disc, sample on ADC, FFT in
  software, confirm 1.8 kHz peak when struck or driven externally.
- Self-loopback: TX disc + RX disc on the same bench, ~30 cm apart,
  through air. Send a known frame, verify demod recovers it.
- WiFi/ESP-NOW gossip between two bare S3s, no acoustic involvement.
- WeftOS substrate publish path: S3 sends a fake `AcousticEvent`
  over WiFi to the shore host; verify it lands on chain.

**Exit criterion**: One full TX→acoustic→RX→demod→WiFi→chain hop
works in air on the bench.

## Phase 1 — Single-buoy pool deployment (bare piezo)

**Goal**: One buoy in the pool, talking to itself. Validate the
architecture on real hardware before committing parts orders for
the rest of the fleet.

**RX path**: bare 35 mm piezo discs pressed against the inside of
the dry chamber's PVC wall, feeding the MCP6022 BPF chain
directly. No JFET hydrophones yet, no sidecars. See
[`build-hydrophone-simple.md`](build-hydrophone-simple.md) for the
full build.

- Build one full buoy per [`requirements.md`](requirements.md):
  S2 TX MCU + S3 RX MCU + 2× existing piezo discs + temp probe +
  battery + WiFi antenna on PVC mast + vented ballast.
- Drop into pool. Verify ballast vents, PVC seal, antenna mast
  stays dry under wave slap.
- Send a frame from TX disc, receive on RX disc on the same buoy
  through pool reverb. This is the worst-case demod test.
- Measure self-coupling intensity — characterize how loud our own
  TX looks at our own RX with the TX_ACTIVE blanking applied.
- Measure pool reverb decay time (T60) empirically per P1 §1.2
  measurement procedure. Sabine/Eyring bracket for the reference
  25 × 12 × 2 m smooth-concrete pool gives 200–500 ms at 1.8 kHz
  (α ≈ 0.05–0.10); validate empirically and feed the firmware
  `guard_time_ms` parameter at **≥ 1.5 × measured T60, minimum
  200 ms**.
- Stream `AcousticEvent` records to chain throughout.

**Exit criterion**: Self-loopback frames are reliably decoded in
the pool with reverberation, and events appear on chain. Bare-piezo
SNR is logged so we have a baseline to compare the JFET upgrade
against in Phase 1b. **T60 measured and `guard_time_ms` set to
≥ 1.5 × T60 (minimum 200 ms)**; three independent pool-position
measurements agree to ±20%, or the firmware uses the longest
measured value.

## Phase 1b — Fleet build-out + JFET hydrophone upgrade

**Goal**: Three buoys in the pool, all with JFET hydrophones, all
seeing each other.

**Trigger**: Phase 1 exit criterion met. Parts ordered per the
chosen build's ordering checklist during Phase 1 so they're on
hand when Phase 1 closes. **Pick one** of the two recipes:

- [`build-hydrophone-oil.md`](build-hydrophone-oil.md) — recommended.
  Modular oil-filled sidecars with waterproof connectors. Swappable.
  Supports multi-RX and sensor sidecars later.
- [`build-hydrophone-epoxy.md`](build-hydrophone-epoxy.md) —
  fallback. Permanent epoxy-potted slip caps. Cheapest, smallest.

Build steps (oil path):

- Build six sidecars (3× TX, 3× RX) per
  [`build-hydrophone-oil.md`](build-hydrophone-oil.md). Bench-test
  each in air before oil-fill.
- Print three quadrant-clamp mounts for the ballast attachment.
- Retrofit Buoy 1: pull the bare piezos, mount sidecars to its
  ballast section, route pigtails through M8 bulkhead connectors
  on the dry chamber bottom. Compare SNR against Phase 1 baseline.
  Expect ~25 dB improvement.
- Build Buoys 2 and 3 from the start with sidecars. Same S2/S3
  split, same firmware, same TX/RX sidecar chassis.
- **Two-buoy presence milestone** (after Buoy 2 lands): drop Buoys
  1 and 2 in the pool, run the beacon protocol, verify each reports
  the other over acoustic and over WiFi for 10+ minutes.
- **Three-buoy presence**: add Buoy 3, repeat. All three should
  cross-detect.

**Exit criterion**: Three buoys in the pool, each reliably reporting
presence of the other two. All three publishing `AcousticEvent`
records to chain. JFET RX measurably outperforms the Phase 1 bare
piezo on the same buoy.

## Phase 1c — Fleet-density scaling sweep (pre-Phase-2 risk retirement)

**Goal**: Empirically measure how the acoustic mesh degrades as
node count grows from 9 → 14 → 19 → ~30 in the same pool the
Phase 1b fleet validates. Output is a scaling-law dataset that
Phase 2 lake-test composition and ADR-086 carrier-priority
dispatcher consume.

- Build 5–20 Class C mini-nodes per `build-mininode.md` (C-active
  variant; ~$25 each). Recommended budget pass: 10 Class C nodes
  for a ~$430 total Phase 1c fleet cost (see `phase-economics.md`
  §2 Phase 1c row).
- Drop the Phase 1b fleet (3 Class A + 6 Class B) into the pool;
  layer in 5, 10, 15, 20 Class C nodes per the 5-step N-sweep
  in `build-fleet-density.md` §"Experiment description".
- For each step, run the per-band TDM cycle (LF song / 1.8 kHz
  mesh / 35 kHz mesh-comms / 50 / 200 / 235 kHz) for 5 minutes;
  the shore harness logs slot-success / collision / miss rates,
  joint-solver convergence time, and per-band saturation flags.
- Recover all Class C nodes; flush recovery logs; run the
  full-fleet batch joint solver on the consolidated chain
  segment; export per-step scaling-law CSV.

**Exit criterion**: Scaling-law dataset captured per
`build-fleet-density.md` §"Success metrics" — slot-collision
rate vs N at all 5 sweep points; joint-solver convergence time
vs N; per-band schedule saturation curve; single recommended
max-buoys-per-cluster figure committed to
`.planning/sonobuoy/build/experiments/phase-1c-fleet-density/
RESULTS.md`. Above 5% collision rate at N = 19 → revisit
ADR-086. Above 30 s batch solve at N = 19 → revisit Phase 2
fleet composition.

## Phase 2 — Three-buoy TWTT ranging (the localization milestone)

**Goal**: Three buoys, pairwise distances, shore-side trilateration.

- Implement the TWTT exchange protocol (see `architecture.md`).
- Round-robin: buoy 1↔2, 1↔3, 2↔3 each second.
- Stream `TwttRanging` events to chain.
- Build the shore-side localization service: consume the stream,
  compute pairwise distances with sound-speed correction from
  the temperature in each event, run least-squares trilateration,
  emit `BuoyPosition` events.
- Validate against ground truth: place buoys at measured positions
  with a tape measure, compare estimated coordinates.
- Visualize on a WeftOS surface.

**Exit criterion**: Position estimates agree with tape-measured
ground truth to within ~30 cm in the pool. (10–20 cm is the
theoretical floor with chirp-spread; 30 cm is the v1 acceptance bar.)

## Phase 3 — In-buoy bearing (stretch goal for v1)

**Goal**: One buoy with two RX hydrophones at different depths
estimates *bearing* to other emitters from within itself.

- Add a second RX MCU + disc to one buoy at a known vertical
  baseline (e.g., 30 cm below the first RX).
- TDoA across the two RX → vertical bearing to emitter.
- Stream as additional `AcousticEvent` records (same `buoy_id`,
  different `rx_id`).
- Shore service treats each RX as an independent observation in
  the trilateration solve.

**Exit criterion**: Bearing estimates from one buoy improve fleet
position accuracy and degrade gracefully when only one RX is
available.

## Phase 4 — Open-water upgrade

**Goal**: Take the fleet out of the pool.

- **GPS module** per surface buoy (e.g., u-blox NEO-M9). Provides:
  - Sub-µs PPS for clock sync.
  - Absolute lat/lon as anchor for the trilateration solve.
  - Wall-clock time for chain timestamps.
- **LoRa 915 MHz** radio module on the radio MCU. Provides 5–15 km
  surface comms in real water (vs ~100 m for WiFi).
  - Suggested module: SX1276 / SX1262.
  - 915 MHz ISM in US; 868 MHz in EU. License-free.
- **Solar charging**: small panel on the antenna mast, MPPT
  controller (CN3791 or similar), supercap or larger Li-ion.
  Required for any deployment longer than a day.
- Re-validate trilateration with GPS-anchored positions.

**Exit criterion**: A three-buoy fleet operates for 24+ hours in
open water with GPS-anchored position estimates streamed to a
shore host over LoRa.

## Phase 5 — Multi-band acoustic (deferred)

**Goal**: Add a second acoustic band for fine ranging.

- Pair the existing 1.8 kHz piezos with **40 kHz ultrasonic
  transducers** (HC-SR04-style discs).
- 1.8 kHz remains the long-range gossip and beacon channel.
- 40 kHz becomes the short-range high-precision ranging channel
  (cm-level resolution, range-limited to a few meters).
- Both bands publish to the same `AcousticEvent` stream with a
  band tag; shore service fuses them.

**Exit criterion**: Position accuracy improves by a factor of ~5
at short ranges where 40 kHz is in play.

## Phase 6 — Fleet scale (deferred)

**Goal**: More than three buoys in a single mesh.

- Switch acoustic MAC from ALOHA to TDMA (one-second frame, each
  node owns one slot, schedule maintained over the radio link).
- Switch fleet-wide gossip from per-pair WiFi to ESP-NOW broadcast
  groups, or to LoRa multicast.
- Investigate **mesh routing** for relayed acoustic frames: a
  buoy that can hear B but not C may relay B's beacon for C.
- Validate at fleet sizes of 5, 10, and 25 buoys.
- **Introduce 2–3 Tier-1 anchor specializations of Class A** per
  ADR-082 §"Tier-1 anchor specialization" / ADR-087 variant
  matrix (4× Fonseca-Alves rigid-plate transmitters, 2× 4 W
  solar, 12 Ah battery, `--features class_a,tier1_anchor`).
  Each Tier-1 anchor add-on is **+$136 over base Class A**;
  canonical Phase 6 fleet cost is **$6,072 ≈ $6.1k** (10 buoys +
  2 Tier-1 anchors) per `phase-economics.md` §2 Phase 6 and
  Deliverable 2 §3. Older "$5.8k" formulations omit the Tier-1
  add-on.

## Risks and unresolved questions

- **Acoustic self-coupling intensity** is unknown until Phase 1
  measurement. If it saturates the analog front end despite
  TX_ACTIVE blanking, we may need an analog mute switch (FET
  shorting the op-amp input) instead of just an op-amp shutdown.
- **Pool reverb decay** may be longer than 50 ms in some pool
  geometries; measured in Phase 1.
- **Battery life** numbers in `requirements.md` are estimates.
  Measured in Phase 1.
- **TWTT timing precision** depends on UART/GPIO jitter between
  MCUs; if shared symbol-clock GPIO turns out to be necessary,
  add it before Phase 2.
- **Sound-speed accuracy in the pool** is bounded by the
  temperature probe location. If pool stratification matters
  (unlikely for a heated pool, possible for an outdoor unheated
  one), add temperature probes at multiple depths.
- **Marine VHF / AIS bands** are out of scope as a hobby project
  (licensed, expensive transceivers, regulatory exposure). LoRa
  915 MHz is the practical "real water" band and is already in
  Phase 4.
