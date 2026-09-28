# rUv spatial stack vs WeftOS / Urth — parallels, gaps, two-way contributions

**Date:** 2026-09-21  
**Status:** Session research capture (not an ADR; **not Plane-filed**)  
**Parent:** [README.md](./README.md) · [urth-applicability.md](./urth-applicability.md) (S7)  
**Prior crosswalk (keep, do not overwrite):** [ruv-worldgraph-vs-weftos.md](../ruv-worldgraph-vs-weftos.md) (2026-07-31)  
**Product:** **Urth** — sparse-first planetary twin (ADR-079)

**Brain corpus:** `ruvnet-brain__search_ruvnet` on 2026-09-21. Newest/oldest store **~11.4 days** old (worldgraph, ruview, skygraph, ruvector). Version/"latest" facts may trail live GitHub/npm; claims below are grounded in that snapshot. Coverage is **thin** where noted; nothing is invented to fill holes.

This note is the **post-survey** (S1–S8) sibling of the July crosswalk. July named the stack rhyme (WorldGraph ↔ Graph Views, OccWorld ↔ LeWM, SkyGraph ↔ domain Views, MetaHarness). September adds SuperSplat overlay, RF-Gaussians, geo frames, DiskANN bench findings, and an explicit **two-way** contribution list. WeftOS canon is not reopened.

---

## 1. Thesis

rUv and WeftOS are **complementary stacks**, not a merge candidate. rUv's spatial work is a **room-scale RF/ambient twin** (typed petgraph beliefs, mandatory semantic provenance, SuperSplat overlay in the tab, RF-aware Gaussians, occupancy *priors*) plus a **sky-domain appliance** (SkyGraph observer frame + VectorDB/GraphDB). WeftOS's spatial work is a **planetary sparse twin** (Urth LOD, BVH geometry SoT, HNSW features, Graph Views F1–F10, chain audit, optional LeWM under ECC). They rhyme on fusion-as-graph, ENU/ECEF, dual appearance/structure, and harness-evolves-not-model. They disagree on what is **geometry truth** (BVH vs occupancy/RF-Gaussian vs splat). Compose adapters, patches, and docs. Do not collapse three Gaussian stories into one, and do not vendor WorldGraph into `clawft-bvh`.

---

## 2. Layer table

| Layer | rUv artifact | WeftOS counterpart | Parallel? | Gap owner |
|-------|--------------|--------------------|-----------|-----------|
| **Current-state twin** | WorldGraph (`wifi-densepose-worldgraph`, ADR-139): typed `petgraph` `StableDiGraph` of rooms/sensors/tracks/beliefs — **not frames** | BVH + chain (ADR-056/078) + Graph Views F1–F10 | **Yes, operationally.** Both are purpose-scoped belief graphs over a scene. Different SoT. | Shared compose. rUv: room petgraph. WeftOS: BVH leaves + View edges. |
| **Appearance (RGB splat)** | SuperSplat (PlayCanvas/WebGPU) as **overlay host**; WorldGraph is the semantic layer on top (ADR-200/202) | SOG / splat pipeline (ADR-078 dual output); appearance never geometry SoT | **Yes, dual-output.** rUv already *renders* the split. WeftOS *decides* it (ADR-078) but live BVH publish is still false. | WeftOS: ship overlay. rUv: splat is not their twin. |
| **Appearance (RF field viz)** | RuView Observatory (ADR-047): Three.js room + CSI overlays (wireframe pose, signal field, not RGB-splat SoT). `ui/components/gaussian-splats.js` is CSI screen-space discs. | Spark / Agent Workspace as **projections** of BVH + SOG | **Weak.** Both have a demo room. Different sensors. | Neither should treat CSI discs as 3DGS. |
| **RF-Gaussian spatial memory** | `RfGaussian` / `GaussianMap` (ruview ADR-275, `v2/crates/ruview-unified/src/gaussian/`): anisotropic Gaussians carrying geometry + occupancy + 16-d semantic + RF reflectivity + provenance; fusion by precision updates; `SceneGraph::activate` bounded subgraph | **No counterpart.** WeftOS 3DGS is appearance; BVH is AABB structure. Occupancy is a *feature*, not a primitive. **Land RF node:** [AntiHunter](https://github.com/lukeswitz/AntiHunter) ESP32 CSI motion (WiDetect) + RSSI trilateration — still features, see `docs/research/antihunter-weftos-crosswalk.md`. | **No — three-way, do not collapse.** See §7. | WeftOS: optional RF adapter later. rUv: do not claim this is 3DGS. |
| **Geometry index** | Spatial-hash grid over `GaussianMap` (1 m pitch); WorldGraph zone bounds in ENU rectangles | **BVH** (`clawft-bvh`, ADR-056) — AABB hierarchy, spatial kNN, ray, frustum | **No.** Hash cells ≠ BVH. Occupancy grid ≠ AABB SoT. | WeftOS owns geometric SoT. rUv has no BVH. |
| **Feature ANN** | RuVector HNSW (RAM, incremental); DiskANN/Vamana **Implemented** (ADR-144, `ruvector-diskann` v2.1.0, batch SSD); SkyGraph uses `ruvector-core` VectorDB + `ruvector-graph` GraphDB | HNSW primary (`VectorBackend`); DiskANN **deferred** cold tier after WEFT-660/661 + serial-build economics | **Yes, same crates, different posture.** | WeftOS: keep deferred; contribute bench. rUv: serial Vamana + hybrid metric bugs. |
| **Geo frames** | WorldGraph: installation **ENU** (ADR-044) + PlayCanvas map (ADR-201: `X=East, Y=Up, Z=-North`). `wifi-densepose-geo`: WGS84/UTM/ENU, `register.rs` “maps local sensor coordinates to WGS84.” SkyGraph: **WGS-84 → ECEF → ENU → az/el/range** (`ObserverFrame`, ADR-199 §10). | Urth: WGS84/ECEF global identity, ENU local capture, `T_ecef_local` on region (ADR-079). E1 region hierarchy **still later**. | **Yes, same geodesy.** SkyGraph's observer pipeline is the most complete *implemented* math. | WeftOS: E1. rUv WorldGraph: installation-scale, not planetary LOD. |
| **Provenance** | Mandatory `SemanticProvenance` on every `SemanticState` (evidence + model + calibration + privacy). Click-to-audit `ProvenanceCard` (ADR-202 §4). Opaque content-address handles to ADR-137/141. | Graph View edge fields (`source_system`, `chain_seq`, confidence). ExoChain (ADR-022). ADR-069 panopticon **still Proposed**. | **Rhyme, not parity.** rUv *enforces* belief provenance in the twin schema. WeftOS *audits* chain on promote. | WeftOS: ProvenanceCard UX + mandatory belief fields on Views. rUv: no chain_seq reverse join. |
| **Privacy** | `PrivacyRollup` on the graph container; `privacy_limited_by` edges; person nodes are anonymous track ids (no identity, no video). ADR-141 referenced as opaque handles. | Substrate ACL (ADR-057); capability tokens (ADR-066). RF-home BFLD is **not** copied. | **Principle only.** Structural anonymity vs capability-governed writes. | Map principles. Do not port RF packet magics. |
| **Predictive WM** | OccWorld (ADR-147): PersonTrack ENU → `OccupancyGrid3D` (200×200×16, class 17 free / 10 person) → Python subprocess → `TrajectoryPrior` into Kalman. Overlay as fading polyline (ADR-202 §3). **Not the twin.** | LeWM optional; ECC authority R1–R5 (ADR-090). Video-WMs (S2/S8) are non-metric priors. | **Yes.** Both: predictor injects *priors*, never SoT. | Compose occupancy priors onto Views. Do not let OccWorld mint BVH leaves. |
| **Domain twins** | SkyGraph (skygraph, ADR-199): tracks/weather/anomalies as a spatial graph over one observer (Oakville node). | Urth L0–L5 planetary LOD; region Views. No sky appliance. | **Template, not a dep.** Domain View pattern. | WeftOS: could host a sky *region View*. rUv: no planetary LOD. |
| **Browser overlay** | SuperSplat WASM bridge (ADR-200): thin `wasm_bindgen`, host-tested `core`/`enu`/`overlay`, four apps (ADR-202). No backend. | Browser WASM agent path (ADR-083) exists; **L4 quilt overlay does not.** S7 names this as the browser pattern. | **Steal the pattern.** | WeftOS. |
| **Authoring** | Configurator (ADR-202 §2): drag boxes/markers in SuperSplat → ENU → `exportRvf()`. | Capture pipeline (phone/Pi/multi-cam) + W0/W1 export. No in-splat authoring. | **Partial.** rUv has a 3-D authoring loop. WeftOS has capture. | WeftOS: L4 configurator over quilt. rUv: not planetary. |
| **Harness** | MetaHarness flywheel / Darwin (rUv ADR-150 surfaces; July crosswalk §6) | ADR-096 (draft): score/genome/flywheel; no silent promote; not a `weft` link dep | **Yes, already adopted as doctrine.** | Keep flywheel on ViewSpec/promote gates. |

**Thin coverage (do not over-claim):**

- `GeoRegistration` type body was **not** retrieved. ADR-200 states `WorldGraph::from_json` needs a `GeoRegistration`; `wifi-densepose-geo/src/register.rs` is documented only as “maps local sensor coordinates to WGS84.” Treat as a named seam, not a spec we have.
- `PrivacyRollup` internals: exported from `worldgraph/wifi-densepose-worldgraph/src/graph.rs`; module body not in the snapshot chunks.
- `ruvector-graph` GraphDB internals: SkyGraph README names it; a direct `ruvector GraphDB` query returned npm package.json noise, not crate source. Cite SkyGraph's usage, not GraphDB internals.
- ADR-139 / ADR-141 / ADR-147 **ADR markdown** itself was not returned for worldgraph (crate docs + SuperSplat ADRs were). Claims about those ADRs come from crate lead comments that *cite* them.

---

## 3. Parallels (what already rhymes)

1. **Fusion is a maintained graph, not a mega-tensor.** WorldGraph stores fused *beliefs* downstream of fusion (ADR-137 cited in `wifi-densepose-worldgraph/src/lib.rs`). Graph Views F1–F10 are the same job with WeftOS names. July already said this; still true after S1–S8.

2. **Appearance and structure are two outputs.** SuperSplat is the photoreal host; WorldGraph is the semantic overlay; OccWorld is a trajectory prior. ADR-078 dual output is the WeftOS form (SOG + `WM_*`). S1 Marble “splats + collider” is the commercial vocabulary for the same split.

3. **Predictive models are not the twin.** OccWorld converts PersonTrack ENU into a 16-voxel-high occupancy tensor and returns `TrajectoryPrior` for a Kalman tracker (`worldgraph/wifi-densepose-worldmodel`). LeWM is optional impulse/observation only (ADR-090 R1–R5). Neither mint geometry.

4. **ENU is the local metric frame; WGS-84/ECEF is the planet.** WorldGraph ADR-201 pins ENU→PlayCanvas. SkyGraph `ObserverFrame` is the textbook WGS-84→ECEF→ENU→az/el pipeline (`skygraph/core/src/coords.rs`). Urth ADR-079 stores `T_ecef_local` on the region. Same geodesy, different scale.

5. **Typed serde enums beat boxed trait objects.** WorldGraph nodes/edges are schema-versioned RVF/JSON (`to_json` / `from_json` snapshot — not public `node_weights()`). Graph Views want typed edge tables with provenance fields. Steal the *discipline* (snapshot is the read path; tests pin field names).

6. **WASM overlay: logic host-tested, glue thin.** ADR-200: `bridge.rs` is `wasm32`-only marshal; `core`/`enu`/`overlay` run under native `cargo test`. Matches WeftOS ADR-083 “don’t invent a second browser runtime.”

7. **HNSW for live, DiskANN for cold.** RuVector ADR-144: HNSW incremental RAM; DiskANN batch SSD after all inserts. WeftOS independently landed the same split (and then deferred DiskANN). SkyGraph still uses a **flat** VectorDB at ≤10⁴ tracks and says HNSW is the next step.

8. **Bounded activation, not dump-the-graph.** RuView `SceneGraph::activate(relevant_kinds, seeds, max_nodes)` reports truncation. Graph Views F10: agents query the View or promoted leaves — never dump raw multi-sensor graph into the LLM. Same lesson.

9. **Harness evolves; model stays frozen.** MetaHarness slogan already in ADR-096. Fusion ViewSpecs, soft-edge thresholds, and F9 promote gates are the churn surface. Do not Darwin “WM overrides ECC.”

10. **Open basemap as ingest, not as SoT.** `wifi-densepose-geo` already lists Sentinel-2, SRTM, OSM buildings/roads (`worldgraph/wifi-densepose-geo/src/lib.rs`). Urth E2 is OSM/DEM → coarse `WM_*` with attribution. Same feed classes; WeftOS has the LOD doctrine they lack.

---

## 4. Gaps on the WeftOS side (LEARN FROM)

What to steal or compose. Not crate merges.

### 4.1 SuperSplat overlay + ProvenanceCard (highest leverage)

Urth L4 today is files + JSON export (`bvh_published: false`). rUv already ships, in-tab, no backend:

| App (ADR-202) | What WeftOS should copy |
|---------------|-------------------------|
| Avatars | Anonymous moving capsules over the quilt (tracks as Event leaves, not identity) |
| Configurator | Drag AABB over SOG → ENU → export a View / `WM_*` payload |
| OccWorld overlay | Fade predicted trajectories / LeWM rollouts as **labeled non-metric** polylines |
| Audit | Click a leaf → `ProvenanceCard` (chain_seq, source_system, confidence, license) |

Wire contract to copy *as a pattern*, not as a dep: `node.kind` (snake_case tag), `bounds_enu`, `EnuPoint {east_m, north_m, up_m}`. ADR-200's caution is load-bearing — a sketch API that doesn't match serde renders nothing. Host-test the mapping; keep `wasm_bindgen` thin.

### 4.2 Mandatory belief provenance

WorldGraph requires `SemanticProvenance` on every `SemanticState` (signal evidence + model + calibration + privacy decision). Graph Views *recommend* `source_system` / `chain_seq` / confidence. WeftOS should make those fields **mandatory on View edges that can F9-promote**, and surface them as a card in the viewer. ADR-069 (panopticon) is the reverse-join half; still Proposed.

### 4.3 Structural privacy

Person nodes are `person #<track_id>`, no identity, no video (ADR-202 §1). Urth capture *is* RGB. WeftOS cannot copy “never video.” We *can* copy: identity lives behind a gate; anonymous occupancy tracks are a first-class Event kind; audit cards show the privacy decision. Do not port BFLD / CSI packet magics (July §2.3 still holds; this snapshot did not retrieve BFLD source).

### 4.4 SkyGraph ObserverFrame as E1 math

Urth E1 (ECEF↔ENU region hierarchy) is still later. SkyGraph already implements and benches the pipeline (~130 ns/target, 10k batch, tests pin az/el). **Compose the formulas**, or vendor a tiny `ObserverFrame` helper — do not take the ADS-B appliance. Pin handedness the way ADR-201 pins `X=East, Y=Up, Z=-North`.

### 4.5 Configurator / `exportRvf()`

Non-technical authoring of rooms and sensors in the splat is something WeftOS does not have. A quilt configurator that writes `WM_SURFACE` / `WM_OBJECT` with `vector: None` and capability-gated region ids is the Urth-shaped version. RVF/JSON snapshot as the interchange, not live petgraph mutation from JS.

### 4.6 Task-gated subgraph reads

`SceneGraph::activate` (ruview ADR-275 §5) is the JITOMA lesson in code: bounded BFS, truncation flagged. Graph Views F10 should grow an explicit `max_nodes` / truncation flag before agent packs. Cheap, high-signal.

### 4.7 RF-Gaussian as an *optional sensor adapter*, not a new SoT

If WeftOS ever attaches WiFi CSI / UWB (ADR-087 dual-branch is still Proposed, sonobuoy/EML not Urth-critical), `RfGaussian` fusion-by-precision and `observe_link` inverse update are the right *feature* path: occupancy and channel residuals as View edge features, then F9 only when an AABB + chain exist. **Never** replace BVH with a spatial-hash Gaussian map.

### 4.8 DiskANN: learn the *when*, not the *now*

RuVector implemented Vamana (two-pass α=1.0/1.2, optional PQ, mmap load). WeftOS already wired `ruvector-diskann` and then deferred it. The lesson is confirmed: HNSW for streaming ECC; DiskANN for cold/static. Do not re-enable hybrid until WEFT-661 metric mix is fixed. Watch upstream for parallel/incremental build (our bench: serial Vamana, 128 s at 10K/384d, one core).

---

## 5. Gaps on the rUv side (CONTRIBUTE BACK)

What WeftOS can offer as patches, ADRs, adapters, docs — **not** a crate merge.

### 5.1 Geometric SoT they do not have

WorldGraph zone bounds are ENU rectangles; `GaussianMap` is a 1 m spatial hash; OccWorld is a 200×200×16 occupancy tensor. None of these answer ray/frustum/AABB-overlap at object scale with chain identity. `clawft-bvh` `SpatialBackend` + optional `VectorRef` join (ADR-088/093) is a **sibling index** WorldGraph could query: “which object AABBs overlap this PersonTrack?” Geometry stays BVH; beliefs stay petgraph.

### 5.2 Planetary LOD and sparse-first honesty

Urth L0–L5 + `unobserved` cells + generative-fill-is-cosmetic (ADR-079) is a doctrine SkyGraph/WorldGraph never needed at room/sky scale. If rUv ever leaves the installation, they will re-litigate “one petgraph of the planet.” Contribute the LOD note and the shard-by-`region/…` rule. Product name **Urth**; physical frame WGS84/ECEF.

### 5.3 Chain + capability as promote gate

F9 (promote stable View components → BVH Object leaves + chain events) plus capability tokens on region writes is the product gate rUv's overlay does not have. Their configurator `exportRvf()` is an authoring snapshot. WeftOS can document “how a SuperSplat-authored room becomes a chain-backed leaf” as an adapter ADR, without them taking ExoChain.

### 5.4 Dual-index join, not inline embeddings

ADR-088 `VectorRef { index_id, vector_id }` on spatial payloads, W1 default `vector: None`. RuView `RfGaussian` **inlines** a 16-d semantic embedding. That is fine at room scale; it is a migration tax at Urth volume. Contribute the optional-handle pattern: Gaussian/WorldGraph node carries a join key, RuVector owns the vector.

### 5.5 DiskANN bench findings (WEFT-660 / WEFT-661)

WeftOS ran `ruvector-diskann` v2.1.0 as a real cold tier (`docs/brain/vector-backend-bench-2026-07.md`):

| Finding | Upstream value |
|---------|----------------|
| Vamana `build()` serial (98.6% one core); ~128 s at 10K×384d; 100K killed after ~91 min | Rayon-over-nodes is the cheap ask (ADR-144 already parallelizes **medoid**, not the node loop) |
| **WEFT-660** `DiskAnnBackend::search` returns `SearchResult.id = 0` (only `.key` usable) | Correctness bug in the adapter or the crate — file/patch |
| **WEFT-661** Hybrid merge compares cosine (hot) vs sqL2 (cold) raw → recall@10 = 0.113 | Metric-normalization in any hybrid example |
| Query path is genuinely good (0.994 recall, p50 363 µs) | Don't throw DiskANN away; don't claim billion-scale on a serial build |

Contribute as an issue + bench harness notes, not a fork.

### 5.6 Three-way Gaussian vocabulary

rUv currently uses “Gaussian” for (a) SuperSplat 3DGS appearance, (b) CSI screen-space discs in Observatory, (c) `RfGaussian` RF/occupancy memory. WeftOS can contribute a short glossary (see §7) so their ADRs stop sounding like they replaced 3DGS with CSI. Honest naming helps both trees.

### 5.7 Live publish residual is ours — but the overlay contract is theirs to reuse

Until `bvh_published` is true, we cannot give them a live SpatialBackend. We *can* give them the W0/W1 export schema and a “snapshot → overlay” adapter that speaks `east_m/north_m/up_m` and `bounds_enu`. That is a docs + fixture contribution today.

### 5.8 Graph Views F9 hierarchy vs flat object soup

S4 (HOV-SG / ConceptGraphs) plus Graph Views: promote floor → room → object, not a bag of AABBs. WorldGraph already has Room / Zone / ObjectAnchor kinds. Contribute the promote-gate language (caps, confidence, chain) so their scene graph does not silently become identity.

---

## 6. Concrete contribution sketches (named, two-way, small)

### C1. `QuiltOverlay` (WeftOS learns)

**Steal:** SuperSplat bridge layering (ADR-200) + four apps (ADR-202) + ENU↔Y-up map (ADR-201).  
**Urth shape:** L4 region quilt as the splat host; BVH AABBs / View nodes as overlay primitives; click → ProvenanceCard with `chain_seq` + license.  
**Do not:** load `wifi-densepose-worldgraph` as a WeftOS crate. Reimplement the *thin* WASM overlay against our snapshot JSON. Host-test field names.

### C2. `SpatialBackend` sibling query (WeftOS contributes)

**Offer:** `clawft-bvh` as an optional geometry index WorldGraph / GaussianMap can call: AABB overlap, ray, frustum, spatial kNN, then `VectorRef` join (ADR-093 helpers).  
**Form:** adapter crate or a documented FFI/JSON query, not a workspace member of worldgraph.  
**Blocked on:** live BVH publish (WeftOS R1). Until then, fixture payloads only.

### C3. `ObserverFrame` ↔ Urth E1 (both)

**Steal:** SkyGraph `geodetic_to_ecef` / `ecef_to_enu` / az/el/range (`skygraph/core/src/coords.rs`), tests included.  
**Contribute:** region-scoped `T_ecef_local` and LOD parent ids so a SkyGraph observer is an Urth L3 *site*, not a parallel planet. One observer ≠ one world.

### C4. Three Gaussians, kept distinct (glossary + adapters)

| Name | Whose | Answers | Must not answer |
|------|-------|---------|-----------------|
| **3DGS / SOG** | SuperSplat (rUv host) / Brush+SOG (WeftOS) | How it *looks* | Where an object *is* |
| **RfGaussian map** | ruview ADR-275 | RF gain, occupancy extinction, semantic cosine at room scale | Survey geometry; planetary LOD |
| **BVH AABB** | WeftOS ADR-056 | Where / when / shape, chain-backed | Appearance; cosine similarity |

Adapters: RfGaussian occupancy → View soft occupancy feature. 3DGS → appearance LOD keyed to `region/urth/…`. BVH ← F9 only.

### C5. DiskANN upstream (WeftOS contributes)

File against `ruvector-diskann`: serial node-loop build, WEFT-660 id=0, WEFT-661 hybrid cosine/sqL2. Attach `docs/brain/vector-backend-bench-2026-07.md` numbers. Ask for rayon-over-nodes (ADR-144 already did parallel medoid). WeftOS keeps DiskANN **deferred** until (a) or (b)+(c) land.

### C6. ProvenanceCard schema (both)

rUv: `core::provenance_for` from node fields + `derived_from` / `contradicts`.  
WeftOS: same card shape with `chain_seq`, `source_system`, confidence, OSM license, `class_source`.  
Interchange: a small JSON card, not a shared crate. Panopticon (ADR-069) is the reverse lookup we still owe.

### C7. Occupancy priors on Views (WeftOS learns, under ADR-090)

OccWorld-class free-space / person occupancy as **F4/F8 features** or Event leaves. Trajectory overlay in the quilt viewer, labeled non-metric. Kalman/LeWM may consume. F9 still requires AABB + chain. R1–R5 unchanged.

### C8. OSM/DEM ingest comparison (docs)

`wifi-densepose-geo` already fetches Sentinel-2, SRTM, OSM. Urth E2 is the same feeds with attribution-on-every-leaf and low confidence for height-missing buildings. Cross-link the two ingest lists; do not dual-implement tile clients if theirs is reusable as a library. License still ours to enforce.

---

## 7. Do-not list

- **Do not merge WorldGraph into BVH** (or BVH into petgraph). Beliefs ≠ frames ≠ AABBs.
- **Do not treat CSI occupancy / RfGaussian occupancy as geometry SoT.** Occupancy is a prior or a feature.
- **Do not silent-crate-merge** worldgraph, ruview, skygraph, or ruvector into the WeftOS workspace.
- **Do not collapse 3DGS, CSI discs, and RfGaussians** into “we use Gaussians now.”
- **Do not let OccWorld / LeWM / Genie / Marble Atlas mint survey truth.** Unobserved Urth stays `unobserved`.
- **Do not replace HNSW live path with DiskANN** until serial build + WEFT-660/661 are fixed. Do not claim “we have DiskANN so graph analytics are done.”
- **Do not inline embeddings in BVH leaves** (ADR-088 Option 3 still rejected) even if `RfGaussian` inlines 16-d semantics.
- **Do not skip `GeoRegistration` / ENU sign tests.** ADR-200 exists because a sketch API compiled and rendered nothing.
- **Do not brand Urth against third-party Earth maps.** WGS84/ECEF under the hood; product name **Urth**.
- **Do not Darwin a “WM overrides ECC” policy** (ADR-090, ADR-096).
- **Do not wait for rUv to ship live BVH publish.** That residual is ours (`bvh_published: false`).
- **Do not copy RF-home privacy packet magics** (BFLD). Copy structural anonymity and audit cards.

---

## 8. Sources

### rUv (brain, ~11.4 d snapshot)

| Path | Used for |
|------|----------|
| `worldgraph/wifi-densepose-worldgraph/src/lib.rs` | ADR-139 twin: petgraph beliefs, mandatory SemanticProvenance, PrivacyRollup export, RVF JSON |
| `worldgraph/wifi-densepose-worldgraph/src/model.rs` | serde enums, opaque ADR-137/141 handles |
| `worldgraph/wifi-densepose-worldgraph/src/graph.rs` | provenance + privacy rollup (doc only; body thin) |
| `worldgraph/wifi-densepose-worldgraph/Cargo.toml` | crate description, geo dep |
| `worldgraph/Cargo.toml` | workspace members: geo, worldgraph, worldmodel, wasm |
| `worldgraph/docs/adr/ADR-200-wasm-bridge.md` | SuperSplat WASM bridge, real serde (`kind`, `bounds_enu`, `east_m/north_m/up_m`), GeoRegistration on `from_json` |
| `worldgraph/docs/adr/ADR-201-coordinate-frame.md` | ENU ⇄ PlayCanvas `X=East, Y=Up, Z=-North` |
| `worldgraph/docs/adr/ADR-202-spatial-applications.md` | four apps: avatars, configurator `exportRvf()`, OccWorld overlay, ProvenanceCard |
| `worldgraph/worldgraph-wasm/src/{lib,enu,overlay}.rs` | host-tested core; ProvenanceCard; trajectory overlay |
| `worldgraph/INTEGRATION.md` | wiring guide; tests without browser |
| `worldgraph/supersplat-bridge/src/{index,enu,usecases/audit}.ts` | TS mirror + click-to-audit |
| `worldgraph/wifi-densepose-geo/src/{lib,coord,register}.rs` | WGS84/UTM/ENU, OSM/SRTM/Sentinel, geo-registration (doc-thin) |
| `worldgraph/wifi-densepose-worldmodel/src/{lib,occupancy,bridge}.rs` | ADR-147 OccWorld thin client, 200×200×16 grid, Unix-socket JSON |
| `ruview/docs/adr/ADR-047-psychohistory-observatory-visualization.md` | Observatory: Three.js + CSI, not RGB-splat SoT |
| `ruview/ui/observatory/js/main.js` | room CSI viz |
| `ruview/ui/components/gaussian-splats.js` | CSI screen-space discs |
| `ruview/docs/adr/ADR-275-rf-aware-gaussian-spatial-memory.md` | RfGaussian / GaussianMap / channel_gain / SceneGraph.activate |
| `ruview/v2/crates/ruview-unified/src/gaussian/{mod,map}.rs` | implementation of ADR-275 |
| `skygraph/core/README.md` | ADR-199 appliance; VectorDB + GraphDB; ObserverFrame |
| `skygraph/core/src/{lib,coords}.rs` | WGS-84→ECEF→ENU math |
| `skygraph/core/BENCHMARKS.md` | projection ~130 ns; flat VectorDB until 10⁴ |
| `skygraph/wasm/src/lib.rs` | browser projector; heavy stores stay native |
| `ruvector/docs/adr/ADR-144-diskann-vamana-implementation.md` | DiskANN Implemented; HNSW vs batch SSD table |
| `ruvector/docs/adr/ADR-146-diskann-vamana-implementation.md` | duplicate path of ADR-144 in snapshot |
| `ruvector/docs/adr/ADR-258-hnsw-delete-repair.md` | HNSW tombstone vs repair (agent-memory, not spatial) |
| `ruvector/npm/packages/ruvector/src/core/diskann-wrapper.ts` | DiskANN wrapper |
| `ruvector/crates/ruvector-hnsw-repair/Cargo.toml` | repair crate |

**Not retrieved (thin):** GeoRegistration struct, PrivacyRollup body, ADR-139/141/147 markdown, `ruvector-graph` GraphDB source, BFLD.

### WeftOS (read, not rewritten)

- [ruv-worldgraph-vs-weftos.md](../ruv-worldgraph-vs-weftos.md) (2026-07-31)
- [graph-views.md](../graph-views.md) (F1–F10)
- [README.md](./README.md), [urth-applicability.md](./urth-applicability.md)
- ADR-056, ADR-078, ADR-079, ADR-088, ADR-090, ADR-093, ADR-096
- [vector-backend-bench-2026-07.md](../../brain/vector-backend-bench-2026-07.md) (WEFT-660/661)
- [diskann-and-large-scale-indexes.md](../diskann-and-large-scale-indexes.md)
- [urth-digital-twin.md](../../weftos/urth-digital-twin.md)

---

## 9. Proposed next (not Plane-filed)

Plane still has **no open spatial tickets** as of this session. This list is research queue only.

| # | Item | Direction | Depends on |
|---|------|-----------|------------|
| N1 | **QuiltOverlay spike** — SuperSplat-style WASM overlay of W1 AABBs + ProvenanceCard on one captured L4 quilt | Learn | Host-tested ENU map; not live publish |
| N2 | **Mandatory View-edge provenance** fields on anything that can F9 | Learn | Graph Views ops (still a doc) |
| N3 | **ObserverFrame helper** for Urth E1 (WGS84→ECEF→ENU), tests pinned to SkyGraph formulas | Learn + E1 | ADR-079 E1 still later |
| N4 | **Upstream DiskANN issue** (serial build, WEFT-660 id=0, WEFT-661 hybrid metric) with bench citation | Contribute | None |
| N5 | **SpatialBackend adapter sketch** (JSON/FFI) WorldGraph could call — fixtures only until `bvh_published` | Contribute | R1 live publish |
| N6 | **Three-Gaussian glossary** PR-able as a short note they can paste | Contribute | None (this doc §6 C4) |
| N7 | Occupancy-prior View feature (OccWorld-class) under ADR-090, labeled non-metric | Learn | F4/F8; not F9 |
| N8 | Configurator: drag AABB on quilt → `WM_*` with `vector: None` | Learn | N1 |
| N9 | Do **not** file RF-Gaussian as Urth geometry. Optional ADR-087 adapter only if CSI/UWB actually attaches | Watch | ADR-087 still Proposed |

**Still true, still ours, still blocking overlay-as-product:** live BVH publish, `spatial_rpc` reattach, dual SpatialService façade, F2 bind geometry + F9 promote, Urth E1/E2.

July 2026-07-31 recommended adoption order (vocabulary → schema → MetaHarness → first fusion View → optional priors → batch plane) still stands. This note adds **browser overlay + provenance card** as the first *visible* compose after that order, and **DiskANN bench upstream** as the first *give-back* that does not wait on F9.
