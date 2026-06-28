# Sonobuoy Project — Research Root

Research and planning folder for the sonobuoy / underwater acoustic sensing
project. Extracted from the phase-2 knowledge-graph paper survey on
2026-04-15 and augmented with additional sonar, fish-ID, and marine
bioacoustics literature.

## Scope

The sonobuoy project is a planned crate in the clawft workspace that
unifies three signal-processing tasks — detection, bearing estimation, and
species ID — into a single learned model operating on distributed
hydrophone-array data. Shared infrastructure with:

- **Quantum cognitive layer** — `quantum_register` graph-to-layout
  mapping reuses for buoy-array geometry graphs.
- **EML learned-function layer** — replaces hardcoded signal-processing
  thresholds.
- **HNSW vector service** — call/signature retrieval for species ID.

## Contents

This corpus has two complementary halves:

- **Research / ML / architecture** (this directory, top level) — paper
  analyses, the 5-branch K-STEMIT architecture, ADRs 053-080+, gap
  tracking, deployment profiles, federated-learning protocol stack.
- **Hardware / mechanical / cost** (`build/` subdirectory, added
  2026-05-11) — the practical implementation side: the buoy form
  factor, hydrophone packaging options, the P79 imaging-tier
  prototype, gimbal and dome upgrade paths, 3D-printing matrix,
  per-phase economics, and the commercial comparison + gap analysis.

| File | Purpose |
|------|---------|
| **`SYNTHESIS.md`** | **Overall research report — start here.** Compares all 42 analyzed papers to WeftOS (ECC, EML, quantum, HNSW), identifies which survey citations were fabricated, and lays out the v1/v2/v3/v4 plan and 25 ADR candidates. |
| `RANGING.md` | Inter-buoy acoustic ranging addendum — OWTT, JANUS, CSAC + TSHL/D-Sync. Closes `SYNTHESIS.md` §10 gap G1 on sensor-position uncertainty. Adds ADR-078. |
| `GAPS.md` | Canonical gap tracker. G1 (ranging) and G2-G3 (PINN 3D, FNO thermocline) already closed; G4-G7 monitored. Successor to `SYNTHESIS.md` §10. |
| `k-stemit-sonobuoy-mapping.md` | Full K-STEMIT → sonobuoy mapping (radar-to-acoustic, learned beamforming, physics priors). Extracted from the phase-2 KG survey. |
| `papers/k-stemit.md` | K-STEMIT reference card (abstract, arXiv link, architecture summary). |
| `papers/survey.md` | Original memory-compiled survey of 18 papers. **14/18 citations were fabricated.** See `SYNTHESIS.md` §1 for the correction table; see `papers/analysis/*.md` for full per-paper verified analyses. |
| `papers/analysis/*.md` | 42 deep-dive analyses (one per paper) covering methodology, results, portable equations, and sonobuoy integration plan. |
| `papers/pdfs/*.pdf` | Downloaded paper PDFs. Gitignored. |
| **`build/`** | **Hardware / mechanical / cost half of the corpus.** See `build/README.md` for the index. Implements the ADRs above in physical form: 2"-PVC buoys, ESP32-S2/S3 split, oil-filled hydrophone packaging, Airmar P79 + 235 kHz D imaging-tier prototype, BLDC gimbal in oil, urethane acoustic dome path, 3D-printing matrix (PLA / PETG / PA-CF / ASA / TPU on Bambu P1S), per-phase economics, and a full commercial-comparison and gap analysis from the practical-build viewpoint. |

## Foundational architecture (from K-STEMIT)

```text
                        adaptive alpha
                             |
                             v
     +-------------+    +---------+    +--------------+
     | GraphSAGE   |--->|  fuse   |--->| detect head  |
     | spatial     |    |         |--->| bearing head |
     +-------------+    |         |--->| species head |
                        |         |
     +-------------+    |         |
     | GLU-gated   |--->|         |
     | temporal    |    |         |
     +-------------+    +---------+

     node features:       inputs:
     - buoy GPS             - hydrophone time series per buoy
     - depth                - spectrogram features
     - SSP                  - TDOA correlations (optional)
     - thermocline
     - sea state
```

## Workflow from here

1. Read `k-stemit-sonobuoy-mapping.md` for the foundational architecture.
2. Read `papers/survey.md` for complementary papers (passive sonar, fish ID,
   marine bioacoustics, graph-based array processing, audio foundation
   models).
3. Draft `ADR-053: Spatio-Temporal Dual-Branch Architecture for Sensor
   Systems` once the paper survey stabilizes.
4. Scaffold a `crates/clawft-sonobuoy/` crate with the dual-branch model
   skeleton, reusing `quantum_register::build_register` for array geometry.
