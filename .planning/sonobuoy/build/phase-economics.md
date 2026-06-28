# Sonar Buoy — Phase Economics and Commercial Parity

**Status**: Strategic planning. Per-phase cost and capability map.
**Date opened**: 2026-05-11
**Owner**: Mathew Beane
**Companion docs**: `roadmap.md` (phase milestones), `commercial-comparison.md`
(taxonomy and gap analysis), `build-buoy-p79.md` (Phase 5 prototype),
build-hydrophone-* (Phase 1/1b packaging variants).

**Research-half anchors** (`.planning/sonobuoy/` one level up):

- `../SYNTHESIS.md` — the 5-branch K-STEMIT architecture, 42-paper
  literature base, ADRs 053-077. This document operationalizes the
  cost/capability side of that synthesis.
- `../SYNTHESIS.md` §3 — **4-tier power hierarchy** (10 µW analog
  gate → 5 mW Cortex-M4 → 50 mW Cortex-M7 → 200 W shore GPU). The
  build phases populate tiers 2-3 on each buoy with ESP32-S3 hardware;
  the shore host carries tier 4 (full K-STEMIT + DEMONet + Perch +
  active imaging).
- `../SYNTHESIS.md` §4 — **federated-learning protocol stack**
  (FedAvg / DGC / Multi-Krum / Split Learning). The fleet-scale §5.4
  ML-classifier capability extensions below are the implementation
  target for this FL stack.
- `../SYNTHESIS.md` §5.4 — **HNSW per-namespace dimension strategy**
  (`sonobuoy-fish` 1280, `sonobuoy-cetacean` 1024, `sonobuoy-vessel`
  768, `sonobuoy-orca-calltype` 512, `sonobuoy-pam-index` 8-32). This
  is the concrete schema underneath §4 (vector-DB-native data plane)
  and §5.4 (ML classification layers) below.
- `../RANGING.md` — production OWTT / JANUS / CSAC + TSHL/D-Sync
  ranging stack (ADR-078). Replaces the v1 TWTT in `architecture.md`
  once Phase 4+ hardware (CSAC, ~$3k/buoy, or Otero-2023
  GNSS-pseudo-range at ~$50/buoy fallback) lands. The ranging upgrade
  is on a separate cost curve from the imaging-tier upgrade analyzed
  here.
- `../GAPS.md` — gap tracker. G1 (sensor-position uncertainty) closed
  by `../RANGING.md`; G2/G3 (PINN 3D, FNO thermocline) closed in
  research with ADRs 079/080; G4-G7 tracked.

This doc answers four questions in one place:

1. **What does each phase cost, per buoy and per fleet?**
2. **What capabilities does each phase unlock?**
3. **Can this project reach commercial-grade sonar capability, and at
   what cost?**
4. **Where does our data model give us an advantage that commercial
   vendors structurally cannot match?**

The throughline of this document: **buoys are semi-disposable**.
We expect to lose them — to weather, to drift, to theft, to operator
error, to learning. The headline economic metric is therefore
**cost-per-buoy**, not total project cost, because the project's
deployable surface area is `(buoys we can afford to lose) × (data
per buoy)`. Every design decision in this doc is filtered through
that lens.

---

## 1. The disposability framing

Why this matters operationally:

| Cost-per-buoy | A lost buoy is... | Implication on design                                       |
|---------------|-------------------|-------------------------------------------------------------|
| ~$50          | annoying          | Build many, deploy aggressively, accept failure              |
| ~$200         | costly            | Worth a recovery beacon, but still deployable in a fleet     |
| ~$1000        | painful           | Tether or GPS-track; deploy only in controlled environments  |
| ~$5000        | a project-scale incident | Treat as capital equipment; deploy with chase-boat protocol |

The architecture has to keep us in the **$50–$300/buoy** band for as
long as possible — that is where deploying a 10-buoy mesh in a
random lake is a $500–$3000 decision rather than a $50k decision.
Crossing into the $1k+ band is acceptable, but each phase that
does so needs an explicit reason.

Disposability also drives material choices: PLA + Plasti-Dip
becomes acceptable for dry-side structural parts because a coated
PLA part outlasts our expected service life and is cheap enough
to throw away. See the per-build-doc "3D-printed components &
coatings" sections for the materials matrix.

---

## 2. Per-phase economics

Each row is rounded; see the build docs for precise BOMs.

### Phase 1 — Single buoy, simple build (architecture validation)

- **Per-buoy BOM**: ~$45 (`build-hydrophone-simple.md`).
- **Fleet at this phase**: 1 unit → ~$45.
- **Capabilities at end of phase**:
  - One full TX → acoustic → RX → demod → WiFi → chain hop in pool.
  - Baseline SNR measurement against which Phase 1b is judged.
  - Architecture risk retired before fleet parts are ordered.
- **Gap to commercial**: vast — this is a pre-functional research
  unit, not a product.
- **Data products**: bare `acoustic.event` stream, single node.

### Phase 1b — Three-buoy fleet, hydrophone upgrade

- **Per-buoy BOM**: ~$50 (epoxy hydrophone) or ~$60 (oil sidecar).
  Recommended path is the oil sidecar.
- **Fleet at this phase**: 3 units → ~$180.
- **Capabilities at end of phase**:
  - Three-buoy presence — every node sees every other node.
  - ~25 dB SNR improvement over Phase 1.
  - Modular sidecar architecture (TX / RX / sensor swappable on
    the dock).
- **Gap to commercial**: still vast for *imaging* products; we are
  in the LBL/PAM family (`commercial-comparison.md` §2.9, §2.10)
  and not yet competitive even there.
- **Data products**: `acoustic.event` × 3 nodes, with SNR,
  leading-edge timing, water temperature, battery voltage,
  decoded MFSK payload.

### Phase 1c — Fleet-density experiment (Class C scaling sweep)

- **Per-buoy BOM additions**: none on the Phase 1b parent buoys.
  Adds 5–20 Class C mini-nodes ($25 each per `build-mininode.md`),
  dive lines + weights ($5/line × 10 = $50), underwater slate
  ($15), shore-laptop battery pack ($30), GFI extension ($20),
  logbook ($5).
- **Incremental BOM**: ~$250 for the small-budget pass (5–10
  Class C nodes) up to ~$620 for the full 20-node sweep
  (`build-fleet-density.md` §"Required materials").
- **Fleet at this phase**: 9 → 29 nodes (3 Class A + 6 Class B +
  0–20 Class C). Total cost at the recommended 10-Class-C build:
  ~$430 (existing $180 Phase 1b fleet + 10 × $25 Class C + ~$120
  ancillaries).
- **Capabilities**: empirical scaling law for slot-collision rate,
  mesh refresh-rate degradation, joint-solver convergence time,
  per-band saturation point — all measured vs N. Output is the
  `scaling-law dataset` consumed by Phase 2 lake-test composition
  decision and ADR-086 carrier-priority dispatcher.
- **Gap to commercial**: not a commercial-parity phase; this is
  pure architecture risk retirement for the multi-class mesh.
- **Data products**: `acoustic.timing` events at 10–120 ev/s
  aggregate (P4 §4.1.3); per-step scaling-law CSV; chain-replay
  bundle manifest (ADR-088/089 pattern).

### Phase 2 — TWTT ranging + shore-side trilateration

- **Per-buoy BOM**: no change (~$60).
- **Fleet at this phase**: 3 units → ~$180. **Shore host**: existing
  laptop / mini-PC running WeftOS, ~$0 incremental.
- **Capabilities**: pairwise distances to ~30 cm in the pool,
  trilateration emitting `BuoyPosition` events on chain.
- **Gap to commercial**: a pool-grade equivalent of a Sonardyne
  Ranger LBL at ~0.5% of commercial cost. Precision is
  meters-class until chirp-spread is added.
- **Data products**: `acoustic.event`, `acoustic.twtt`,
  `acoustic.position`. **Mesh-scale spatial product begins here.**

### Phase 3 — In-buoy bearing (stretch goal)

- **Per-buoy BOM**: +$8 per additional RX sidecar; one buoy at +1
  RX → ~$68.
- **Fleet at this phase**: 3 units, one with bearing → ~$190.
- **Capabilities**: bearing-to-emitter from a single buoy via
  in-buoy TDoA; fleet position accuracy improves accordingly.
- **Gap to commercial**: this is a sparse-aperture single-node USBL.
  Commercial USBLs (Sonardyne, iXblue) are ~$5k–$50k. We are at
  ~$70 with one degree of freedom.
- **Data products**: `acoustic.event` × multiple `rx_id` per buoy,
  enabling shore-side bearing computation.

### Phase 4 — Open water (GPS, LoRa, solar)

- **Per-buoy BOM additions**:
  - GPS module (u-blox NEO-M9): ~$15
  - LoRa radio (SX1276/SX1262): ~$10
  - Small solar panel + MPPT (CN3791): ~$20
  - Larger battery / supercap budget: ~$10
  - Total: **+~$55 per buoy → ~$115 per buoy**
- **Fleet at this phase**: 3 units → ~$345.
- **Capabilities**:
  - GPS-anchored position (absolute lat/lon).
  - PPS sub-µs clock sync.
  - 5–15 km LoRa surface comms.
  - 24h+ autonomous deployment.
- **Gap to commercial**: now competitive with low-end commercial
  oceanographic moorings at ~1% of their cost (commercial: $5k–$50k
  per buoy with similar specs).
- **Data products**: `nav.gps`, plus everything above with absolute
  geo-coordinates.

### Phase 5a — Imaging tier prototype (P79 + 235 kHz D, surplus parts)

- **Per-buoy BOM additions** for one experimental buoy:
  - P79 transducer (surplus): ~$35
  - 235 kHz D transducer (surplus): ~$35
  - Pulser + LNA + ADC daughterboard: ~$45
  - Two oil-filled couplant chambers: ~$18
  - Power for pulser rail: ~$8
  - Total: **+~$140 over a Phase 4 buoy → ~$255 for the prototype**
- **Fleet at this phase**: 3 (Phase 4 buoys) + 1 (prototype) → ~$600.
- **Capabilities**: depth, downward imaging at 50/200/235 kHz,
  bistatic mode E, dual-band frequency diversity. Three new chain
  streams.
- **Gap to commercial**: same hardware *band* as $200–$2000 commercial
  fishfinders, with worse calibration and proven worse beam
  characteristics; **but multistatic geometry across the mesh that
  commercial can't do**.
- **Data products**: `acoustic.depth`, `acoustic.imaging`,
  `acoustic.event` extended to high-band detections.

### Phase 5b — Modern transducer baseline (production validation)

- **Per-buoy BOM additions**: replace surplus transducers with a
  modern Airmar (B45 / P19 / B260) at ~$100–$300 → **buoy now
  ~$320–$520**.
- **Fleet at this phase**: ~3 production-ready + 1 prototype → ~$1500.
- **Capabilities**: same as 5a, with clean modern transducer specs
  and known beam characteristics.
- **Gap to commercial**: at this band, we're now competitive with
  high-end consumer/prosumer fishfinders (e.g. Garmin GT54UHD,
  ~$500) on hardware, with mesh + chain advantages they lack.

### Phase 5c — Fleet imaging-tier rollout

- **Per-buoy BOM**: same as 5b ~$320–$520 each.
- **Fleet at this phase**: 3 buoys with modern imaging tier → ~$1500.

### Phase 5d — Gimbal upgrade (single-element scanning imager)

- **Per-buoy BOM additions**: ~$44 for BLDC gimbal + encoder + driver
  + larger oil chamber → **buoy now ~$365–$565**.
- **Fleet at this phase**: 3 gimballed buoys → ~$1700.
- **Tier-1 anchor add-on (NEW first-class line, Phase 5d+)**: 1
  Tier-1 anchor lands in Phase 5d per ADR-082 phase ladder. The
  Tier-1 specialization adds **+$136 over base Class A** for the
  4× Fonseca-Alves rigid-plate transmitters, TPA3116D2 class-D
  amp, 4-channel relay, 4× LM317 current-source loops, **2× 4 W
  solar panels** (extras above the base 1× 4 W in Phase 4), 2
  extra 18650 cells (12 Ah parallel bank, extras above base 2×),
  and scaled-up MPPT. See Deliverable 1 §3.4.1 for the BOM table
  and ADR-082 §"Tier-1 anchor specialization" for the
  architectural commitment. **Phase 5d fleet cost with 1 Tier-1
  anchor**: 3 buoys × $450 + $136 Tier-1 add-on = **~$1836**.
- **Capabilities**:
  - **Forward-looking sonar (FLS) mode**.
  - **Mechanical-scan 360° PPI** per buoy.
  - Adaptive pointing under shore-service control.
- **Gap to commercial**: now in family with Tritech Micron-DST,
  Imagenex 881A, BlueView P900 — all $3k–$30k commercial mechanical
  scanners. We are at ~$500/node with mesh.

### Phase 5e — Acoustic dome (production-quality imaging)

- **Per-buoy BOM additions**: ~$40 for cast urethane dome (or ~$150
  for off-the-shelf hydrophone dome) → **buoy now ~$400–$600 (DIY)
  or ~$500–$700 (commercial dome)**.
- **Fleet at this phase**: 3 dome-equipped buoys → ~$1800.
- **Capabilities**: clean omnidirectional acoustic transmission, no
  PVC-wall refraction, biofouling-resistant. Imaging quality now
  limited by transducer + DSP, not packaging.

### Phase 5f — Threaded "lighthouse" dome (production form factor)

- **Per-buoy BOM**: same as 5e + serviceable threaded dome mount,
  ~$60–$100 → **buoy ~$460–$700**.
- **Form factor**: production-ready, serviceable, calibrated.

### Phase 6 — Fleet scale (5–25 buoys, TDMA, mesh routing)

- **Per-buoy BOM**: same as 5c-f; cost driven by transducer choice.
- **Fleet at this phase**: 5–25 buoys → **$2500–$15000** at modern-
  transducer cost (base).
- **Tier-1 anchor add-ons (Phase 6 ladder)**: 2–3 Tier-1 anchors
  per ADR-082 §"Phase ladder". Each Tier-1 specialization adds
  **+$136 over base Class A** (4× Fonseca-Alves transmitters,
  TPA3116D2 class-D + 4-channel relay, 4× LM317 loops, 2× 4 W
  solar, +2× 18650, larger MPPT). With 2 Tier-1 anchors the
  Phase 6 add-on is **+$272**; with 3 Tier-1 anchors it is
  **+$408**.
- **Canonical Phase 6 fleet cost** (10 buoys + 2 Tier-1 anchors):
  10 × $580 base + 2 × $136 Tier-1 add-on = **$6,072 ≈ $6.1k**.
  This is the canonical Phase 6 fleet number; older "$5.8k"
  formulations omitted the Tier-1 add-on and are superseded by
  Deliverable 2 §3.
- **Capabilities**: fleet-scale sparse-aperture imaging; tow-line
  geometry for synthetic-aperture-equivalent imaging at our bands;
  TDMA-coordinated multistatic operations.
- **Gap to commercial**: this is the phase where the **mesh
  advantage compounds**. Commercial vendors have nothing equivalent
  at any price — they sell single-perspective devices, not
  distributed meshes.

### Phase 7 — High-band upgrade (commercial-parity imaging resolution)

- **Per-buoy BOM additions**:
  - High-frequency transducer (500 kHz+, Imagenex / commercial
    side-scan element): $1k–$5k
  - FPGA coprocessor (Lattice iCE40 UP5K daughterboard): ~$40
  - HV pulser IC (HV7361 / MAX14808): ~$20
  - AFE5832-class receive chip: ~$30
  - Machined / cast Delrin pressure case: ~$100–$300
  - Total: **+$1.5k–$6k per buoy → $2k–$7k per buoy**
- **Fleet at this phase**: 3–5 buoys → **$6k–$35k**.
- **Capabilities**:
  - **Sub-centimeter range resolution** at the high band.
  - **SAS-class cross-range resolution** via mesh sparse aperture or
    moving tow-line.
  - **Side-scan imagery, FLS, 360° PPI** all at production-grade
    resolution.
- **Gap to commercial**: at this point we are in the same hardware
  band as $30k–$300k commercial survey gear, at ~5–20% the per-node
  cost, with the mesh and chain advantages on top.

---

## 3. Total project cost trajectory

Headline numbers, cumulative through each phase, assuming the
recommended path (oil sidecar Phase 1b, modern transducers by 5b,
gimbal in 5d, dome in 5e, fleet of 3 throughout 5c–6, scale to 10
in late 6, high-band only on 2 of 10 for 7):

| Phase  | Per-buoy        | Fleet size      | Fleet cost     | Cumulative project |
|--------|-----------------|-----------------|----------------|--------------------|
| 1      | $45             | 1               | $45            | $45                |
| 1b     | $60             | 3               | $180           | $225               |
| 1c     | $60 + 10×$25    | 13 (+ ancillary)| ~$430          | ~$655              |
| 2      | $60             | 3               | $180           | ~$655 (no add)     |
| 3      | $68             | 3               | $205           | ~$680              |
| 4      | $115            | 3               | $345           | ~$820              |
| 5a     | $115/255        | 3 + 1 proto     | $600           | ~$860              |
| 5b     | $400            | 3 prod + proto  | $1500          | $1700              |
| 5c     | $400            | 3               | $1500          | $1700 (replaces)   |
| 5d     | $450 + 1× $136 Tier-1 | 3 + 1 Tier-1 | ~$1836      | ~$2036             |
| 5e     | $500            | 3 + 1 Tier-1    | ~$1936         | ~$2136             |
| 5f     | $580            | 3 + 1 Tier-1    | ~$2036         | ~$2236             |
| 6      | $580 + 2× $136 Tier-1 | 10 + 2 Tier-1 | **$6,072 ≈ $6.1k** | **~$6.2k** |
| 7      | $2500           | 2 of 10 upgraded| +$5000         | ~$11.2k            |

**Top-line outcome**: a fleet of 10 buoys + 2 Tier-1 anchors with
production-quality imaging across 50/200/235 kHz, gimballed
scanning, acoustic domes, GPS+LoRa+solar autonomy, mesh routing,
and TWO of them upgraded to commercial-parity high-frequency
imaging, lands around **$11.2k total project cost** — less than a
single commercial multibeam echosounder transducer head. The
canonical Phase 6 number is **$6,072 ≈ $6.1k** (10 buoys + 2
Tier-1 anchors); older "$5.8k" formulations omit the Tier-1
add-on.

---

## 4. The vector-database-native data-plane advantage

This is the strategic argument for why this project can plausibly
exceed commercial-grade sonar at the system level, even when
individual nodes are cheaper / less capable than commercial
equivalents.

### How commercial sonar handles data

Commercial fishfinders, side-scan, FLS, multibeam — almost all of
them — treat data as a *display pipeline*:

```
transducer → DSP → screen pixel buffer → operator's eyes
                ↘ proprietary log file (sometimes)
```

The screen is the product. The log file, when it exists, is a
fossilized rendering — a stored echogram, side-scan strip, or
3D point cloud in a vendor-specific format. To do anything with
that data — re-analyze it, search it, train a model on it,
correlate it with anything else — you fight vendor SDKs, you
re-render, you lose information at every step. Cross-deployment
queries ("show me every bottom return on every trip that looks
like this one") are operationally impossible.

### How clawft handles data

Every detection, every TWTT exchange, every position estimate,
every imaging window is a **typed event on the WeftOS substrate
chain**:

```
transducer → DSP on buoy → chain event (signed, timestamped, schema-typed)
                        ↘ subscriber: shore-side reconstruction
                        ↘ subscriber: vector database ingester
                        ↘ subscriber: real-time UI surface
                        ↘ subscriber: anyone (open data plane)
```

Every event ingests into a vector database (AgentDB with HNSW
indexing per the project's `CLAUDE.md`). The vector representation
is the union of:

- **Spatial features**: node position, beam pointing, range bin,
  signal envelope shape.
- **Temporal features**: pulse repetition phase, time-of-day,
  inter-event timing.
- **Spectral features**: band, dominant frequencies, chirp
  parameters, Goertzel feature vectors.
- **Environmental features**: water temperature, sound speed,
  battery state, fleet geometry.
- **Provenance**: which node, which firmware version, which
  calibration epoch, which signed identity.

### What that unlocks that commercial cannot

| Query type                                                | Commercial sonar                | clawft + vector DB                  |
|-----------------------------------------------------------|---------------------------------|-------------------------------------|
| "Show me the screen right now."                           | Yes — this is the product.      | Yes — derived view.                 |
| "Show me the screen from last Tuesday at 14:32."          | Maybe, if you remembered to record. | Yes — chain replay over any window. |
| "Find every acoustic event that looks like this one."     | No.                             | **Yes — vector similarity search**. |
| "Classify this return as fish / structure / bottom / boat-noise / unknown." | Limited per-vendor classifier.  | **Yes — train any model on the vectors**. |
| "What's anomalous about today's data vs. the historical baseline?" | No.                             | **Yes — anomaly detection on vector embeddings**. |
| "Correlate sonar returns with weather data, water temperature, fleet geometry." | No.                             | **Yes — cross-modal joins are first-class**. |
| "Produce a side-scan view of this area from the mesh data we already have." | No (would have needed a side-scan rig). | **Yes — multistatic reconstruction is just another subscriber to the same chain**. |
| "Reproduce this view exactly six months from now."        | No — vendor file format drift, proprietary processing. | **Yes — events are signed; the same query yields the same result**. |
| "Let another researcher / institution share the data."    | Vendor-locked.                  | **Yes — open chain stream, signed and portable**. |
| "Train an AI model on this data."                         | Wrestle the vendor SDK.         | **Yes — vectors are the canonical training format already**. |

### The product implication

A commercial sonar is **a measurement instrument**. clawft is
**a measurement instrument that emits a queryable, replayable,
ML-ready data substrate**. Every buoy in the fleet contributes to
a shared vector database that grows in value with every deployment.

The commercial market sells fixed-form-factor instruments
optimized for a single screen. We sell — or release, or
self-host — a **distributed acoustic memory** that any subscriber
can project into any visualization, retrieve by content, train
models against, and cross-correlate with any other data source.

The closest commercial analogue is not in marine electronics at
all — it's in the security / observability industry, where
products like Splunk, Datadog, or Honeycomb make money by being
the queryable layer over data that originally came from many
single-perspective devices (logs, metrics, traces). They beat
single-device dashboards by being the *aggregation and query*
layer. The same architectural shift is available, and largely
unclaimed, in marine acoustics.

---

## 5. Capability extensions: what the mesh + data plane enable in practice

This section gets concrete about the *product*, separately from the
hardware. Each capability below is a direct consequence of the
mesh + chain + vector-DB architecture combined with the phase
hardware already in the plan — not new hardware on top.

### 5.1 Sparse-aperture passive detection (the mesh IS the antenna)

A 1.8 kHz omnidirectional element per buoy is the worst possible
*single-node* sonar. But the **mesh is the antenna**. A tow-line of
~20 nodes spread over 100–200 m gives angular resolution of λ/L ≈
0.5–1° at 1.8 kHz — SOSUS-spacing-class.

What that detects, in **passive mode with no P79 or other
imaging-tier upgrade**:

- **Schools of vocal fish** — croaker, drum, snapper, hake, and
  any species that vocalize in the audible band. With a chorusing
  source level SL ≈ 120 dB re 1 µPa @ 1 m (Ramcharitar et al.
  2006), 1.8 kHz seawater absorption of 0.06 dB/km, and Wenz
  sea-state-1 noise floor NL_det = 50 dB at B = 100 Hz, the
  Urick passive sonar equation `SE = SL − TL − (NL − DI) − DT`
  with DT = 10 dB yields **SE = 0 at ~1 km for a single buoy
  (DI = 0)**, **~3.2 km for a 10-buoy mesh at 10 m spacing**
  (incoherent DI = 10 dB), and **~14 km for a 20-buoy tow-line
  at 100 m baseline** (coherent λ/L beamforming, DI = 23 dB).
  Density × aperture is the load-bearing variable, not transducer
  quality. Worked budget at panel P1 §1.1.7
  (`.planning/symposiums/sonobuoy/panels/P1-acoustic-physics.md`)
  citing Urick 1983 §2 + `papers/analysis/urick-sonar-equation.md`.
- **Vessel traffic** — engine and prop signatures dominate the
  50 Hz–5 kHz band; per-vessel classification by harmonic content
  is a standard PAM technique.
- **Marine mammals** — baleen whales (20 Hz–1 kHz) and odontocetes
  (kHz-and-up) are detectable by any hydrophone array; species
  ID is well-studied.

The implication: **Phase 1b (cheap 1.8 kHz fleet) is already a
viable passive acoustic monitoring (PAM) instrument as soon as
enough nodes are in the water**. No further hardware needed —
density is the upgrade.

### 5.2 Lagrangian ocean current mapping (drift is data)

GPS-equipped buoys (Phase 4+) free-floating in open water are, by
definition, Lagrangian drifters. NOAA and Argo run a worldwide
fleet of these to measure ocean currents.

A clawft fleet of 10 GPS buoys deployed in a region produces, as
a free side-effect of the deployment:

- Per-buoy drift tracks (chain-recorded `nav.gps`)
- Velocity-field interpolation across the deployment area
- Eddy / convergence / divergence detection from the field structure
- Long-term records of current variability across deployments

This is a measurement product **completely separate from acoustic**
— same hardware, same chain, additional capability.

### 5.3 Temporal fish tracking (vector-DB joins)

When a vocal target is detected at position `(x, y, t1)` and a
similar vector signature appears at `(x', y', t2)`, the velocity
vector and trajectory falls out of a temporal join over the vector
DB. With enough detection density you reconstruct:

- School trajectories within a deployment
- Daily migration cycles (diel vertical migration in particular)
- Feeding-aggregation patterns
- Response to environmental change

This is how oceanographers reconstruct marine animal movement from
passive arrays. Doing it at hobby / small-research scale requires
the vector-DB data plane — which §4 already argues we have and
commercial does not.

### 5.4 AI / ML classification layers on the chain

The same vector store that supports similarity search also supports
model training. Practical classifier targets, all of which add a
new chain stream:

- **`acoustic.species`** — biological sound classifier
- **`acoustic.vessel`** — small boat / large ship / fishing /
  military, plus per-vessel signature (each ship has a unique
  acoustic fingerprint, as the navy well knows)
- **`acoustic.weather`** — wind, rain, ice, surf, biological
  ambient
- **`acoustic.anomaly`** — gunshots, dredging, drilling, hull
  pings from other vessels, blasts
- **`acoustic.bottom_type`** (active sonar) — mud / sand / gravel
  / rock from backscatter envelope shape
- **`acoustic.structure`** (active imaging) — wreck / pipeline /
  aquaculture cage / artificial reef / lost gear

Each classifier becomes another layer in the visualization stack
(§5.7), another column in the vector DB, another loss-function
target for the next model.

### 5.5 Centimeter-grade persistent bathymetric mosaics

With P79 + gimbal (Phase 5d) + GPS (Phase 4):

- Each ping has known `(buoy_pos, beam_pointing, time, range,
  return)` → a 3D depth sample
- Across a deployment, samples accumulate into a 3D point cloud
- Across deployments, point clouds compile into a persistent
  bathymetric mosaic
- Resolution: limited by the chirp range bin (~15 cm at 5 kHz BW
  @ 200 kHz; ~5 cm at higher chirp BW; <1 cm at the Phase 7
  high-band)
- Stitching: standard structure-from-motion / GICP / ICP, plenty
  of open-source tooling (Open3D, PCL, CloudCompare)

The mosaic isn't a one-shot product; it's a **growing dataset**
that improves every time the fleet revisits the area. Commercial
mosaic products from multibeam survey are vendor-locked and
typically not re-fusable across visits without per-vendor
processing. Ours is a vector-DB query.

### 5.6 Temporal bottom-change detection

With persistent bathymetric mosaics dated by deployment epoch,
differencing any two epochs surfaces:

- **Sediment migration** (sandbar movement, channel evolution)
- **Scour around structures** (bridge piers, wind turbine
  foundations, mooring anchors)
- **Dredging activity** (before/after, depth and volume)
- **Anchor drag tracks** (a single anchor pass leaves a recoverable
  cm-scale signature in fine sediment)
- **Vegetation growth and biofouling**
- **New objects** (lost gear, wrecks, debris, ordnance)

This is the same idea as InSAR for terrain change on land, applied
underwater. **Commercial single-vessel sonar cannot do this at the
required accuracy without survey-grade gear and per-visit
calibration.** A mesh that repeatedly visits the same area
naturally produces this product.

### 5.7 3D + AR visualization surfaces

With the data already in 3D vector form (positions, times,
intensities, ML labels), rendering becomes a software project:

- **3D world view** in a WeftOS surface (egui + WebGPU / wgpu)
- **WebXR / AR fly-through** on a tablet or headset — operator
  sees the seabed under their boat with last-week's fish-track
  trails and current weather overlays
- **Chart overlay** on Google Earth tiles, NOAA charts, or paper
  via georeferenced PNG export
- **Time-lapse video** of any layer over any deployment window
- **Difference-overlay** between any two epochs (5.6)

The hard part — getting the data into a usable form — is already
solved by chain + vector DB. Visualization is just another
subscriber, and a designer's job, not an acoustics engineer's job.

This is the surface where "more interesting than commercial"
becomes legible to a non-technical user. You don't beat Garmin by
drawing a prettier echogram; you beat them by giving the operator
**an AR view of the seabed under their boat with persistent
fish-track trails, temporal change detection, ML-labeled contacts,
and current weather fused in** — all sourced from a mesh that
costs less per node than the Garmin does.

### 5.8 The compounding capability table

Every capability above lands on hardware already in the phase
ladder. No new BOM lines are required for any of them once the
phase that introduces the prerequisite hardware has been built.

| Capability                              | Required phase                   | New hardware? | New software? |
|-----------------------------------------|----------------------------------|---------------|---------------|
| Sparse-aperture passive detection       | 1b + fleet density               | No            | Shore beamformer |
| Lagrangian current mapping              | 4                                | No            | Shore drift integrator |
| Temporal fish / target tracking         | 1b + 2 + vector DB               | No            | Vector-DB join queries |
| ML species / vessel / noise classifiers | any phase + training pipeline    | No (GPU on shore is already in dev) | ML pipeline |
| cm-grade bathymetric mosaic             | 5d                               | No            | Mosaic stitcher subscriber |
| Temporal bottom-change detection        | 5d + repeated deployments        | No            | Differencing visualizer |
| 3D + AR visualization                   | any phase with enough data       | No            | WebGPU / WebXR surface |

**No commercial product compounds like this.** Buying ten Garmin
LiveScopes does not give you ten times the data product. Buying
ten clawft buoys does — and the data product grows every time you
deploy them, in every area you deploy them, for the lifetime of
the chain. **Density × duration × subscribers = the moat.**

---

## 6. Path to commercial parity (and beyond)

### Capability-to-cost map for matching commercial classes

| Commercial class (`commercial-comparison.md` §2) | Match needed at clawft phase | Cost per node at match | Commercial equivalent cost | Ratio  |
|--------------------------------------------------|------------------------------|-----------------------|----------------------------|--------|
| Single-beam depth (§2.1)                         | 5b                            | ~$400                  | $100–$500                  | 1×–4×  |
| CHIRP depth (§2.2)                               | 5b                            | ~$400                  | $300–$1500                 | 0.3×–4× |
| Down-imaging / side-imaging consumer (§2.3/2.4)  | 7 (high-band on tow-line)     | ~$2500                 | $1000–$10000               | 0.25×–2.5× |
| Forward-looking sonar (§2.5)                     | 5d (gimballed) + 7 (high band) | ~$2500                | $1500–$8000                | 0.3×–1.6× |
| 360° imaging sonar (§2.6)                        | 5d (gimballed)                | ~$500                  | $3000–$15000               | **0.03×–0.17×** |
| Multibeam survey grade (§2.7)                    | not pursued                   | —                      | $50k–$1M+                  | —      |
| LBL/USBL positioning (§2.9)                      | 2–3 (already in scope)        | ~$70                   | $5k–$300k                  | **0.0002×–0.014×** |
| PAM (§2.10)                                      | 1b (already in scope)         | ~$60                   | $1k–$30k                   | **0.002×–0.06×** |

Bolded ratios are **structural cost advantages of 5–500×**. These
are the categories where the project's mesh + chain + cheap-node
architecture is dominant on cost, not just competitive.

### Where commercial still wins, honestly

- **Calibration to scientific traceability** — IHO survey
  certification, dB re 1 µPa hydrophone calibration. Not in scope.
- **Imaging at the dense-array bands (1 MHz, 5 MHz)** — needs
  AFE5832-class chips, FPGAs, machined cases, regulatory
  approvals. Doable but a different engineering project.
- **Sealed deep-water packaging** (>100 m depth) — commercial gear
  is rated to thousands of meters; ours to ~10 m without significant
  rework.
- **Service support** — Garmin replaces your unit; clawft doesn't.

These are real, durable advantages of the commercial industry. The
project can choose to address them later or to stay deliberately
outside them.

### Where the mesh + chain architecture compounds with scale

Each additional buoy in a clawft fleet:

1. Adds another vantage point to multistatic geometry → all the
   imaging modes (FLS, 360°, side-scan, bathymetry) improve.
2. Adds another hydrophone to the PAM coverage → wider area,
   better triangulation of acoustic sources.
3. Adds another contributor to the vector database → richer
   training data for downstream models.
4. Adds redundancy → fleet survives individual losses.

No commercial product has this scaling characteristic. They are
single-perspective instruments. We are a coverage mesh whose data
product compounds with N.

---

## 7. Decision points and next moves

### When to spend money

1. **Don't skip Phase 1 (simple) to save $40.** The architecture-
   risk-retirement value of that one buoy in the pool exceeds the
   savings by an order of magnitude.
2. **Do graduate to oil sidecars at 1b.** $10/buoy delta vs. epoxy,
   but every modular advantage propagates to all later phases.
3. **Phase 5b (modern transducer) is the only step that costs real
   money before phase 7.** Budget ~$300–$1000 per buoy for the
   imaging tier transducer specifically. The rest of the project
   stays cheap.
4. **Phase 7 (high-band) is optional and project-defining.** If
   the data-plane / vector-DB advantages of the lower-band fleet
   prove out, Phase 7 may be unnecessary — the value of imaging
   resolution is partly compensated by mesh density and vector-DB
   queryability at lower bands.

### When to invest in software vs. hardware

Every dollar spent on shore-side software (vector DB ingester,
reconstruction service, visualization surfaces, ML training
pipeline) is a multiplier over **every buoy in every phase**.
Hardware spending only benefits the buoy it's spent on.

For most of the phases above, the right marginal dollar goes to
the shore-side data plane and the vector database integration,
not to the next hardware upgrade.

### Where the project's strategic advantage actually lives

Not in the transducers — those are commodity. Not in the buoys —
those are PVC and ESP32s. Not in the firmware — that's well-trodden.

In the **substrate-chain-native, vector-indexed, signed, replayable,
multi-subscriber data plane** that turns every buoy in every
deployment into a contributor to a single growing knowledge base.
That is what no commercial vendor has, and what is structurally
difficult for them to build because their business models depend
on locked-in vendor formats and on selling screens.

The path to commercial parity, then commercial superiority, is:

1. Build the cheap mesh.
2. Build the data plane underneath.
3. Let the data plane outpace the hardware.

The hardware doesn't have to be best-in-class. The data has to be.
