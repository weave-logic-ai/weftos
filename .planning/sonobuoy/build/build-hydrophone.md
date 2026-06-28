# Sonar Buoy — Hydrophone Build Index

Three build options for the buoy's transducer packaging, ordered by
phase and serviceability. Pick by what you're trying to learn at this
stage of the project.

| Build | When | Cost/unit | Serviceable | SNR | See |
|-------|------|-----------|-------------|-----|-----|
| Simple | Phase 1 — single-buoy architecture validation | ~$0 (uses parts on hand) | Trivial (open top cap) | Baseline | [`build-hydrophone-simple.md`](build-hydrophone-simple.md) |
| Epoxy | Phase 1b — fleet build-out, no service needed | ~$3 + epoxy | No (destroy to access) | +20 dB vs baseline | [`build-hydrophone-epoxy.md`](build-hydrophone-epoxy.md) |
| Oil sidecar | Phase 1b+ — modular, swappable, deeper-mounted, expandable | ~$8 | Yes — drain, swap, refill | +25 dB vs baseline, plus near-perfect acoustic match | [`build-hydrophone-oil.md`](build-hydrophone-oil.md) |

## Decision tree

- **Building your first buoy?** → simple. One chamber, no oil, no
  epoxy. Validate the architecture before committing parts orders.
- **Building out the fleet and never need to service?** → epoxy.
  Cheapest, smallest, durable. Slip-cap potted units hard-mounted
  to the buoy. Once it's built, it's built forever.
- **Building out the fleet and want modularity, swappability,
  multiple receivers per buoy, sidecar sensors, or deeper acoustic
  conditions via a drop tether?** → oil sidecar. The recommended
  Phase 1b+ design. Two cable-glanded ends and a threaded service
  cap mean you can replace a dead JFET in 10 minutes instead of
  rebuilding from scratch.

All three share the same circuit (JFET source follower at 3.3 V) and
the same downstream signal chain in the buoy (MCP6022 BPF). They
differ only in *packaging*. You can mix builds across a fleet: e.g.,
keep Buoy 1 as the simple test unit and build Buoys 2 and 3 as oil
sidecars for the localization milestone.

## 3D-printed parts across all three builds

The cross-cutting **consolidated 3D-print matrix is the single
source of truth** at panel P2 §2.4
(`.planning/symposiums/sonobuoy/panels/P2-build-mechanical.md`).
It lists every printed part across every phase (1, 1b epoxy,
1b oil, 5a, 5d, 5e, 5f) with material, print settings, coating,
heat-set inserts, and source build doc. Six per-doc inconsistencies
identified and resolved there (quadrant clamp material, internal
piezo holder material, M8 backing plate material, insert coverage,
P1S enclosure usage, PA-CF annealing temperature).

The per-doc "3D-printed components (Bambu P1S)" sections in
`build-hydrophone-simple.md`, `build-hydrophone-epoxy.md`,
`build-hydrophone-oil.md`, and `build-buoy-p79.md` remain as
build-specific reference; the cross-cutting truth lives in the
P2 matrix.

Common ground that applies across all builds:

- **Disposability framing.** Buoys are semi-disposable (see
  `phase-economics.md`); print cost is in the noise compared to
  transducers and MCUs. PLA + Plasti-Dip is acceptable for
  anything inside the dry chamber. Per-buoy print cost is ~$8
  of filament for a fully-loaded Phase 5d build.
- **Material picks**: PLA for dry-side internal, PETG for
  oil-immersed non-load-bearing, PETG-CF for slightly-stiffer
  oil-immersed parts, PA-CF for oil-immersed structural (gimbal
  frame, pitch arm, BLDC mount), ASA for UV-exposed exterior,
  TPU 95A for cable boots and gaskets.
- **Heat-set M3 brass inserts** at every threaded interface — far
  more durable than printed threads, install in 5 seconds with a
  soldering iron.
- **Plasti-Dip or marine epoxy spray** as the standard coating for
  any printed part that may see water. Two coats of Plasti-Dip
  brings even PLA to "service life exceeds buoy expected lifetime"
  durability.
- **PA-CF annealing**: 100 °C × 1 h in the P1S enclosure for any
  oil-immersed structural PA-CF part. Below 100 °C the nylon-CF
  matrix doesn't fully anneal; above 110 °C the print warps.

## Companion docs

- [`requirements.md`](requirements.md) — overall hardware spec
- [`architecture.md`](architecture.md) — modular MCU split, on-chain
  data flow, time sync
- [`roadmap.md`](roadmap.md) — phased plan and exit criteria
