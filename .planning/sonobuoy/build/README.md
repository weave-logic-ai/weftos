# Sonobuoy Project — Build (Hardware / Mechanical / Cost) Half

**Status**: Pre-implementation, requirements drafting
**Date opened**: 2026-05-10
**Moved here**: 2026-05-11 (from `docs/plans/sonar/` to
`.planning/sonobuoy/build/` to live alongside the research half)
**Owner**: Mathew Beane
**Goal**: Build a small fleet of buoy-form-factor acoustic + WiFi
nodes that gossip on WeftOS, with presence detection and acoustic
trilateration as the headline capabilities — and scale into a
multistatic mesh that grounds the ML architecture documented in the
research half of this corpus.

This directory is the **build / hardware / mechanical / cost half**
of the sonobuoy planning corpus. The research / ML / architecture
half lives one level up at `.planning/sonobuoy/` (`SYNTHESIS.md`,
`RANGING.md`, `GAPS.md`, `k-stemit-sonobuoy-mapping.md`,
`papers/analysis/`).

The two halves correspond to:

- **Up** (`../SYNTHESIS.md`): what the model should *be* — five
  branches (temporal / spatial / physics / classification /
  active-imaging), two deployment profiles (tactical / PAM), the
  4-tier on-buoy / at-shore power split, the federated-learning
  protocol stack, ADRs 053–080+.
- **Down** (this directory): how the buoy gets *built* — PVC body,
  ESP32-S2/S3 split, oil-coupled hydrophones, Airmar P79 + 235 kHz
  D imaging tier, BLDC gimbal in oil, urethane acoustic dome,
  3D-printed components on the Bambu P1S, per-phase BOMs, and the
  commercial-comparison gap analysis.

It is not yet a Plane cycle — it should graduate once the
requirements stabilize and we open WEFT-N work items for the v1
build.

## Cross-reference map: build docs ↔ research-half ADRs

| Build doc                                  | Research-half anchor                                                  |
|--------------------------------------------|-----------------------------------------------------------------------|
| `requirements.md` + `build-hydrophone-*`   | `../SYNTHESIS.md` §3 4-tier power hierarchy (tier 2 = on-buoy MCU); the buoy IS the tier-2/3 hardware. |
| `architecture.md` (TWTT ranging, time sync) | `../RANGING.md` — ADR-078 OWTT/JANUS/CSAC+TSHL is the production version of the v1 TWTT protocol described here. |
| `build-buoy-p79.md` (imaging tier)         | `../SYNTHESIS.md` §2.4 active-imaging branch: ADR-063 (5th branch SAS, Hayes-Gough 2009), ADR-064 (Gerg-Monga deep autofocus), ADR-065 (Kiang 2022 multistatic SAS *with stationary sonobuoy* — this buoy is the C-node in Kiang's geometry). |
| `build-buoy-p79.md` (gimbal Path D)        | Active-imaging mechanical-scan complement to ADRs 063-065.            |
| `phase-economics.md` §4 vector-DB data plane | `../SYNTHESIS.md` §5.4 HNSW namespace strategy (`sonobuoy-fish` 1280, `sonobuoy-cetacean` 1024, `sonobuoy-vessel` 768, `sonobuoy-orca-calltype` 512, `sonobuoy-pam-index` 8-32). |
| `phase-economics.md` §5.4 ML classifier layers | `../SYNTHESIS.md` §2.5 classification head: Perch / SurfPerch / BirdNET-2.3 / Bergler orca / DSMIL.  |
| `phase-economics.md` §5.1 sparse-aperture passive | `../SYNTHESIS.md` §2.2 spatial branch (Tzirakis / Grinstein / Chen-Rao), §2.6 PAM deployment profile. |
| `phase-economics.md` §5.5-5.6 bathymetric mosaic + temporal change | `../SYNTHESIS.md` §2.4 active-imaging (Kiang multistatic), §8.3 v3 plan for `clawft-sonobuoy-active`. |
| `commercial-comparison.md` §4 data-plane advantage | `../SYNTHESIS.md` §5 mapping to WeftOS subsystems (ECC / EML / quantum / HNSW / mesh). |
| `commercial-comparison.md` §5.9 ML classification overlay | `../SYNTHESIS.md` §2.5 classification head + `../GAPS.md` G1 (closed via RANGING). |
| `roadmap.md` Phases 1-6                    | `../SYNTHESIS.md` §8.1-8.4 v1/v2/v3/v4 plan. **The two phase ladders must stay aligned.** |
| `build-hydrophone-oil.md` (sidecar / sensor mesh) | `../SYNTHESIS.md` §2.6 PAM deployment profile (HARP-class refurbishable buoys). |
| `build-tethered-subsurface.md` (Class B) | Symposium ADR-082 (three-class architecture); extends `build-hydrophone-oil.md` with longer cable + deeper placement. |
| `build-mininode.md` (Class C acoustic-only) | Symposium ADRs 082 (three-class), 083 (calibration service), 084 (acoustic time-sequencing). |
| `lake-test-protocol.md` (Phase 2 diver-assisted) | Symposium Phase 2 evolved from pool to lake. Exercises ADRs 082-084 end-to-end. |

## Documents

- [`requirements.md`](requirements.md) — hardware spec, mechanical
  form factor, and the bill of materials for the v1 three-buoy pool
  test fleet. Now a **Class A buoy** spec per the three-class
  architecture (ADR-082).
- [`build-tethered-subsurface.md`](build-tethered-subsurface.md) —
  **Class B** subsurface receive node, tethered 1 m / 2 m below a
  parent Class A by marine cable. Extends `build-hydrophone-oil.md`
  with deeper placement; per ADR-082.
- [`build-mininode.md`](build-mininode.md) — **Class C** acoustic-
  only drop-deployable mini-node in a sealed 2" PVC tube. ESP32-S2
  chirp+log firmware spec; sensor logging (depth + IMU); recovery
  log dump over WiFi. Per ADR-082; consumed by ADR-083 calibration
  service; speaks ADR-084 acoustic time-sequencing.
- [`lake-test-protocol.md`](lake-test-protocol.md) — Phase 2
  diver-assisted lake validation methodology. Uses all three
  buoy classes + diver-placed PVC-tube targets. Acceptance
  criteria, failure-mode handbook, diver acoustic safety briefing.
- [`architecture.md`](architecture.md) — modular TX/RX split, on-buoy
  topology, time-sync strategy, on-chain data flow, and the
  shore-side localization service.
- [`build-hydrophone.md`](build-hydrophone.md) — index of the three
  hydrophone packaging options with a decision tree.
- [`build-hydrophone-simple.md`](build-hydrophone-simple.md) —
  Phase 1 single-chamber test build, two ESP32-S3 minis + two
  piezos pressed against the inside of the PVC wall. Fastest
  path to a buoy in the pool.
- [`build-hydrophone-epoxy.md`](build-hydrophone-epoxy.md) —
  Phase 1b option A. Permanent JFET source-follower potted in a
  PVC slip cap. Cheapest, smallest, unserviceable.
- [`build-hydrophone-oil.md`](build-hydrophone-oil.md) —
  Phase 1b option B (recommended). Modular oil-filled sidecar
  chambers clamped to the ballast, connected via waterproof
  M8 connectors. Swappable, expandable, supports multi-RX and
  sensor sidecars.
- [`roadmap.md`](roadmap.md) — phased plan from pool to open water,
  including the LoRa / GPS / multi-RX upgrade paths.
- [`commercial-comparison.md`](commercial-comparison.md) — taxonomy of
  commercial sonar classes (fish-finder / down-scan / side-scan / FLS /
  360 PPI / multibeam / LBL-USBL / PAM), capability matrix against the
  clawft buoy fleet, visualization surfaces enabled by the WeftOS
  substrate mesh, Garmin-familiar-vs-data-rich UI direction, and
  use-case-by-use-case gap analysis.
- [`build-buoy-p79.md`](build-buoy-p79.md) — Phase 5 prototype build
  for a special imaging-tier buoy carrying an Airmar P79 dual-band
  (50 / 200 kHz) plus a single-frequency 235 kHz Airmar D
  (44-053-1-02) alongside the standard 1.8 kHz mesh element. Shared
  pulser + LNA + ADC daughterboard with 3-band selection, two
  oil-filled couplant chambers, bistatic operating mode, new
  `acoustic.depth` / `acoustic.imaging` chain streams, full
  bench-to-pool validation plan. Includes a Phase 5d stretch goal
  for a fully-submerged BLDC gimbal on the 235 kHz channel inside
  a widened 3" or 4" oil-filled chamber — the "PVC wall as
  omnidirectional acoustic window" pattern — and a Phase 5e
  cast-urethane acoustic dome upgrade.
- [`phase-economics.md`](phase-economics.md) — strategic-planning
  doc covering per-phase per-buoy BOM and capability map, total
  fleet-cost trajectory, disposability framing, the
  vector-database-native data-plane advantage that distinguishes
  clawft from commercial sonar, and the compounding capability
  argument (sparse-aperture passive detection, Lagrangian current
  mapping, temporal fish tracking, AI/ML classification layers,
  cm-grade persistent bathymetric mosaics, temporal bottom-change
  detection, 3D + AR visualization).

## One-line summary

Three 2"-PVC vertical buoys with vented ballast, each carrying a
1.8 kHz piezo TX and a 1.8 kHz piezo RX driven by independent
ESP32 minis (S2 for TX, S3 for RX). Acoustic gossip below water,
WiFi/ESP-NOW above water. Buoys publish raw timing and detection
events to a WeftOS substrate stream; a shore-side service consumes
the streams and emits trilateration results.

## What this project is *not*

- Not a port of WeftOS to embedded — WeftOS does not run on ESP32.
  The buoys speak the WeftOS *substrate wire format* and stream
  events onto chain. The shore host runs full WeftOS.
- Not a covert or high-bandwidth link. Effective acoustic data rate
  is ~50–200 bps in the audible band. Frames are small and the
  channel is loud.
- Not a navigation-grade positioning system. Position resolution is
  bounded by the 1.8 kHz wavelength (≈ 82 cm in water) without
  chirp-spread tricks; with chirped pulses we expect ~10–20 cm in a
  pool.
