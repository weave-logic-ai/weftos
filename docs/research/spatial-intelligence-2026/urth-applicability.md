# Urth applicability — S1–S8 synthesis (2026)

**Date:** 2026-09-21  
**Status:** Session research capture (not an ADR; **not Plane-filed**)  
**Parent:** [README.md](./README.md)  
**Product:** **Urth** — sparse-first planetary twin ([ADR-079](../../adr/adr-079-urth-digital-twin.md), [urth-digital-twin.md](../../weftos/urth-digital-twin.md))

This note answers: *given the 2025–2026 spatial-intelligence wave, what actually attaches to Urth — and what would fake the twin?*

**Honesty rule (non-negotiable):** pixel-video “worlds,” Marble-style omni generation, JEPA rollouts, and any other learned fill are **non-metric**. They may be appearance, priors, or soft edges. They must never mint survey truth. Unobserved Urth stays `unobserved`.

---

## 1. What we already decided (do not re-litigate)

| Piece | Decision |
|-------|----------|
| Name / geodesy | Product **Urth**; physical frame WGS84 / ECEF, local ENU |
| Geometric index | BVH (`clawft-bvh`, ADR-056) — not R-tree / kd-tree / CSI occupancy |
| Feature index | HNSW primary; DiskANN **deferred** cold tier |
| Join | Optional `VectorRef` (ADR-088) + spatial-first / feature-first (ADR-093) |
| Appearance vs structure | Dual output (ADR-078). Generative fill is cosmetic, labeled non-metric (ADR-079) |
| Latent WM | LeWM optional; ECC authority R1–R5 (ADR-090) |
| Fusion ops | Graph Views F1–F10 |
| LOD | L0 planetary → L1 admin → L2 city → L3 site → L4 quilt → L5 object. Root `urth` |
| Writes | Capability-governed; chain-audited |

S1–S8 **compose onto** this stack. They do not reopen it.

---

## 2. S1–S8 → Urth seams

| ID | Item | Pri | Urth LOD | Primary WeftOS seam | Stance |
|----|------|-----|----------|---------------------|--------|
| **S1** | World Labs Marble + Atlas; Li taxonomy (renderer / simulator / planner) | A | L4 appearance + L5 collider language | ADR-078 dual output; product copy | **Apply language.** Dual-export (splats + collider mesh) *is* what we already ship as appearance vs structure. Atlas omni-gen is **cosmetic**, labeled non-metric. |
| **S2** | Genie 3, SIMA 2, Cosmos 2.5/3, WBench | B | none as geometry | LeWM priors / synthetic rollouts | **Compose under ADR-090.** Video-as-simulator is a competing *definition* of world model, not Urth. |
| **S3** | VGGT + VGGT-Ω, MASt3R/DUSt3R, SpatialLM | A | L4 pose; L3–L5 layout | splat pipeline before COLMAP; W1 `WM_SURFACE` / `WM_OBJECT` | **Apply as adapters.** Feed-forward SfM can cut overnight COLMAP. SpatialLM proposes indoor partition with `vector: None`. |
| **S4** | HOV-SG, ConceptGraphs, Sparse3DPR, LangSplat | A | L3 floor → L4 room → L5 object | Graph Views F4/F9; `LANGUAGE_FEATURES` | **Apply hierarchy.** Views are not a flat object soup. Open-vocab embeddings live on `VectorRef`, never in BVH payloads. |
| **S5** | CityGaussianV2, Octree-GS, VastGaussian, LODGE | A | L2–L4 appearance LOD | quilt / SOG streaming keyed to `region/urth/…` | **Apply pattern.** Octree-GS is the *appearance* twin of our AABB hierarchy. Shard by region, same as BVH. |
| **S6** | V-JEPA 2 / 2.1 vs LeWM | B | L4–L5 visual codes | `index_ids::VISUAL_FEATURES`; LeWM facade | **Watch → optional embedder.** Dense tokens may fill visual HNSW. Never replace BVH. See [vjepa-and-lewm.md](./vjepa-and-lewm.md). |
| **S7** | rUv WorldGraph, OccWorld, SuperSplat WASM | A (compose) | L4 browser overlay | Graph Views; SuperSplat provenance cards | **Compose.** Overlay + cards in the tab; geometry SoT stays BVH. Crosswalk: [ruv-worldgraph-vs-weftos.md](../ruv-worldgraph-vs-weftos.md). |
| **S8** | Astronex-World 1.0, Matrix-Game 3.0, HY-World 1.5, Yume 1.5 | C | none | camera/PRoPE watch list | **Watch.** Open video-WM explosion. Do not pick a champion this cycle. |

Li taxonomy as Urth vocabulary (steal from S1, keep honest):

| Li class | Urth meaning |
|----------|----------------|
| **Renderer** | Appearance: SOG / splat / licensed basemap imagery |
| **Simulator** | LeWM / video WM / OccWorld-class occupancy *priors* |
| **Planner** | Agents querying BVH + Views; optional JEPA-AC style MPC |
| **Twin (ours)** | BVH + chain + LOD regions — not in Li’s product list; **this is Urth** |

---

## 3. Layer-by-layer applicability

### 3.1 LOD (L0–L5)

| LOD | What 2026 papers help | What they must not do |
|-----|----------------------|------------------------|
| **L0–L1** | Nothing new. Open DEM / admin feeds remain ETL. | Planetary mega-train (S2/S6/S8). Generative continents. |
| **L2 city** | S5 appearance LOD (CityGaussian-style) *if* we ever stream city-scale splats — still optional cosmetic over OSM footprints. | Fake building massing as metric. |
| **L3 site** | S3 SpatialLM / OSM buildings as `WM_OBJECT` stubs (conf ~0.4). S4 floor→room hierarchy. E2 OSM ingest. | Indoor capture unanchored to a site region. |
| **L4 quilt** | S3 VGGT path; S5 Octree-GS LOD; S1 dual-export language; S7 SuperSplat overlay. Seams OK. | One seamless planet Gaussian. |
| **L5 object** | S4 instance graphs; S6 visual `VectorRef`; W1 blobs → F9 promote. | Embeddings as identity without AABB + chain. |

Query rule unchanged: same BVH API, filter by LOD and confidence.

### 3.2 BVH (geometry SoT)

- **Shard by region** (`region/urth/…`) — S5’s large-scale 3DGS papers independently rediscover this. Do not build one process-global planet tree (ADR-079 non-goal).
- W1 still exports `bvh_published: false` (`clawft-splat-pipeline` world-model manifest). Live insert is the **oldest** residual; no 2026 paper removes it.
- `spatial_rpc` / `spatial_cli_e2e` remain ignored (WEFT-720). Dual-backend SpatialService façade (ADR-093 helpers exist, daemon join does not) is still the product hole for “similar near me.”
- S1 collider mesh ≡ our structure export, **if and only if** it lands as `WM_*` AABBs / surfaces, not as a sidecar the viewer treats as truth.

### 3.3 HNSW / `VectorRef`

Three reserved namespaces (`weftos-leaf-types`):

| `index_id` | 2026 candidate | View step |
|------------|----------------|-----------|
| `0 ECC_HNSW` | text / ECC memory (unchanged) | F10 agent packs |
| `1 VISUAL_FEATURES` | V-JEPA 2.1 dense (S6), DINO, feature-3DGS | F4 soft edges |
| `2 LANGUAGE_FEATURES` | LangSplat / CLIP / SigLIP (S4) | F4 open-vocab |

DiskANN stays **deferred cold tier** for huge embedding columns (ADR-095 V3). It is not a substitute for View association edges or for BVH.

Temporal HNSW fingerprinting (ADR-056 concept paper §10) is still future. S6 dense *temporal consistency* is interesting **research** for that paper, not a reason to inline tokens.

### 3.4 Graph Views (F1–F10)

Operational fusion is still “construct and maintain a View,” not a mega-tensor.

| Step | 2026 input |
|------|------------|
| F1 Create | Purpose = Urth region / room / rescan job (not “the planet”). |
| F2 Bind geometry | BVH region filter. **Still not operational as a `view.*` RPC.** |
| F3 Bind sensors | Phone / Pi / multi-cam (existing). Video-WM cameras (S2/S8) are **not** sensors. |
| F4 Bind codes | S4 open-vocab + S6 visual dense as soft edges. |
| F5 Bind structure | S3 SpatialLM / W1 partition; S4 scene-graph proposals. |
| F6 Hot fuse | Caps stay; marble/genie tokens do not skip windows. |
| F7 Batch | ADR-095 per-View, not forest-wide. |
| F8 Dual-branch | ADR-087 still **proposed only**. Optional α on View edges. |
| **F9 Promote** | Hierarchical scene-graph components → BVH Object leaves + chain. **The missing product gate.** |
| F10 Serve | Agents query View or promoted leaves — never dump Genie video into the LLM as “the room.” |

S4’s lesson: F9 should promote a **hierarchy** (floor / room / object), not a bag of AABBs.

### 3.5 Splat pipeline

Current path: capture → COLMAP → Brush → SOG + W0/W1 export (`bvh_published: false`).

Proposed adapters (research queue, not tickets):

1. **VGGT / MASt3R-SLAM** (S3) as an optional pose-graph **before** COLMAP — same camera-stats doctrine (known poses, not guessed).
2. **SpatialLM** (S3) as a W1 *layout proposer*: emit `WM_SURFACE` / `WM_OBJECT` with `vector: None`, confidence tagged `class_source: spatiallm`.
3. **Octree-GS / CityGaussian** (S5) as appearance LOD keyed to L2–L4 region ids — parallel to BVH shard, **not** a second spatial index.
4. Marble **dual-export language** in docs and UI: “appearance (splat) + structure (collider / WM_*)” — we already decided this; copy should match the commercial form.

Do not wait for a champion train backend. Brush / Instant-NuRec remain appearance; structure is a separate stage (ADR-078).

### 3.6 Live-publish residual

This is the bottleneck that no 2026 paper fixes:

| Residual | Where it lives | Why S1–S8 don’t close it |
|----------|----------------|--------------------------|
| `bvh_published: false` | W1 export tests | Papers produce meshes/splats; they do not insert `clawft-bvh` leaves. |
| `spatial_rpc` dropped | `clawft-weave` | Daemon reattach is ours. |
| Dual SpatialService façade | ADR-093 follow-up | Join helpers are unit-testable; live “near me + similar” is not. |
| F9 promote | graph-views.md | Scene-graph papers propose; chain + capability tokens are WeftOS. |

Until live publish exists, Urth L4 is **files + a JSON export**, not a queryable twin. Treat every S3/S5 demo as **offline appearance** until F9 + publish go green.

### 3.7 Urth E1 / E2 (OSM)

Phased delivery in ADR-079 / urth-digital-twin.md:

| Phase | Goal | 2026 bearing |
|-------|------|----------------|
| **E0** | Doctrine frozen (WEFT-713) | Done. This survey does not reopen naming. |
| **E1** | `urth` → site → room IDs + ECEF↔ENU | **Still later.** No paper replaces geo anchors. |
| **E2** | OSM (+ optional DEM) → coarse `WM_*` for one pilot | S3 SpatialLM is an *indoor* analog, not a Geofabrik replacement. Attribution + license on every leaf still required. |
| E3–E5 | Quilt attach, client glow, multi-writer | S7 SuperSplat overlay is the **browser pattern** for E4, not the SoT. |

Pilot remains **one real captured site + OSM neighborhood**. Cite OSM. Height-missing buildings stay low confidence.

### 3.8 Panopticon (ADR-069) and dual-branch (ADR-087)

- **ADR-069** (proposed): every projection reverse-resolvable by `chain_seq` / `uid`. Spatial lens is in-scope: BVH leaves **must** carry the chain key. New visual `index_id` producers (S6) that insert HNSW with `chain_seq = 0` would **repeat the live defect** this ADR exists to kill. Visual index rows need the same locator as ECC HNSW.
- **ADR-087** (proposed): K-STEMIT dual-branch as View **F8** features. S4/S6 embeddings can be *inputs* to a spatial vs temporal α; they are not the ADR.

Neither is promoted to Accepted by this session.

---

## 4. Do this / don’t do this

| Do this | Don’t do this |
|---------|----------------|
| Call the product **Urth**. Keep WGS84/ECEF under the hood. | Brand against third-party “Earth” maps; imply we host planetary tiles in v1. |
| Keep BVH as geometry SoT; shard by `region/urth/…`. | Replace BVH with HNSW, DiskANN, Octree-GS, WorldGraph petgraph, or JEPA tokens. |
| Dual-export language: appearance (splat/SOG) **and** structure (collider / `WM_*`). | Ship pretty rooms with no AABB; treat SOG as “what is where.” |
| Label Genie / Cosmos / Marble Atlas / JEPA rollouts **non-metric**. | Fill unobserved cells with generative cities and call it the twin. |
| W1 default `vector: None`; later `VectorRef` to `VISUAL_FEATURES` or `LANGUAGE_FEATURES`. | Inline embeddings in leaves; invent AABBs from a feature cluster. |
| Scene-graph **hierarchy** (floor → room → object) inside a purpose-scoped View; F9 promote to Object leaves. | Flat object soup; promote soft ANN edges without geometry + chain. |
| Optional VGGT/MASt3R **before** COLMAP when camera stats exist. | Unanchored indoor capture; overnight COLMAP as the only religion *or* as a sacred cow. |
| SpatialLM (or equal) as a **proposer** of `WM_SURFACE` / `WM_OBJECT`. | Let a layout LLM mint high-confidence buildings without OSM/capture evidence. |
| Octree-GS-style appearance LOD keyed to L2–L4 region ids. | One planetary Gaussian train; one process-global BVH. |
| SuperSplat WASM overlay + provenance cards in the browser (S7). | Let the overlay become the SoT; skip license/provenance on basemap leaves. |
| LeWM / V-JEPA as optional sub-layer; ECC R1–R5 on every WM write. | WM `ReasoningOverride`, causal short-circuit, or hard dependency on cluster fusion. |
| OSM attribution + license on every E2 leaf; confidence explicit. | Ingest tiles without provenance; treat footprints as surveyed height. |
| Capability tokens on region writes; conflict = branch / confidence contest. | Silent overwrite of a site quilt. |
| Pilot **one** real site. MetaHarness receipts if an embedder/SfM path proves lift. | Silent champion swap of COLMAP, DINO, or V-JEPA (ADR-096). |
| Watch S8 camera/PRoPE control. | Pick Astronex / Yume / Matrix-Game as *the* Urth renderer this cycle. |
| Keep DiskANN deferred; HNSW live. | Claim “we have DiskANN so graph analytics / spatial queries are done.” |
| When adding a visual index, key rows by `chain_seq` (ADR-069 direction). | Repeat ECC-brain HNSW `chain_seq = 0`. |

---

## 5. Proposed research queue (not Plane-filed)

Plane has **no open spatial tickets**. This list is a **merge of old residuals and new session items** for world-builder / lead to file later if asked. Order is dependency-ish, not a sprint plan.

### 5.1 Old residuals (still true)

| # | Item | Why it still blocks Urth |
|---|------|---------------------------|
| R1 | **Live BVH publish** (`bvh_published: false` on W1) | Without it, L4 is an export file. Every S3/S5 demo stays offline. |
| R2 | Reattach daemon **`spatial_rpc` / `spatial_cli_e2e`** (ignored, WEFT-720) | Agents cannot query the twin over weave. |
| R3 | **Dual-backend SpatialService façade** (ADR-093 helpers exist) | “Near here, visually similar” is the product join; not wired live. |
| R4 | Graph Views **F1–F10 operational** — especially **F2 bind geometry** and **F9 promote** | Fusion remains a doc. S4 hierarchy has nowhere to land. |
| R5 | Urth **E1** region hierarchy (ECEF/ENU) and **E2** OSM/DEM basemap for one pilot | No globe scaffolding; quilts cannot attach to `urth/…`. |
| R6 | Temporal HNSW fingerprinting (concept paper §10) | Future; do not confuse with S6 dense features. |
| R7 | **ADR-069 panopticon** (`chain_seq` reverse join), still Proposed | Spatial + visual lenses otherwise become one-way. |
| R8 | **ADR-087** K-STEMIT dual-branch, still Proposed | F8 remains empty; sonobuoy/EML, not Urth-critical. |
| R9 | **DiskANN** cold vector tier — deferred | Keep deferred until HNSW + live publish exist. |
| R10 | Planet-scale BVH **shard** (geohash / region) | Required before any L2 appearance LOD (S5) is more than a site demo. |

### 5.2 New items this session (compose onto the residuals)

| # | Item | From | Depends on | Note |
|---|------|------|------------|------|
| N1 | Optional **VGGT / MASt3R-SLAM** path in splat pipeline (before COLMAP) | S3 | camera-stats capture; not R1 | Adapter, not a replacement religion. MetaHarness eval vs COLMAP on the pilot site. |
| N2 | **SpatialLM** (or equal) as W1 layout proposer → `WM_SURFACE` / `WM_OBJECT`, `vector: None` | S3 | W1 export; R1 to become live | Indoor only; OSM still owns L2–L3 footprints. |
| N3 | **Octree-GS / CityGaussian-style** appearance LOD keyed to Urth L2–L4 region ids | S5 | E1 region ids; R10 shard | Appearance twin of AABB hierarchy. Seams OK at L4. |
| N4 | **Scene-graph hierarchy** (floor/room/object) as Graph View materialization; F9 → BVH Object leaves | S4 | R4 (F2/F9) | Open-vocab on `LANGUAGE_FEATURES`, not in payloads. |
| N5 | **Marble dual-export** as **product language** for appearance vs collider/structure | S1 | copy / docs / UI | Already decided in ADR-078; align naming with the commercial form. No new crate. |
| N6 | **V-JEPA 2.1 dense features** as visual `index_id` (`VISUAL_FEATURES`) | S6 | ADR-088/093; R3 to query; R7 if indexed | Never world-model SoT. Frozen encoder, pooled over existing AABBs. License check (paper NC-ND vs MIT weights). |

### 5.3 Suggested filing clusters (if asked)

Not filed. If Plane work is requested, keep clusters small:

1. **Twin publish** — R1 + R2 + R3 (without this, N1–N6 are research toys).
2. **Urth scaffold** — R5 E1/E2 + R10 shard policy (pilot site only).
3. **Fusion gate** — R4 F2/F9 + N4 hierarchy + N5 copy.
4. **Optional adapters** — N1 VGGT, N2 SpatialLM, N3 appearance LOD, N6 visual index (each its own ticket, each with a “does not replace BVH” acceptance line).
5. **Lenses** — R7 panopticon compliance for spatial + visual; R8/R9/R6 stay later.

Acceptance crumbs for any adapter ticket:

- Pilot **one** geo-anchored site.
- `vector: None` valid forever.
- Generative / latent outputs tagged **non-metric**.
- OSM / imagery license on basemap leaves.
- Tests via `scripts/build.sh` (no MetaHarness as a `weft` link dep).
- No silent promote of SfM or embedder champions.

---

## 6. Risks that survive the survey

| Risk | Still true because |
|------|--------------------|
| Fake density | S1 Atlas + S2/S8 video WMs make *pretty empty* cheap. Doctrine is the only brake. |
| GPU cost | VGGT, Octree-GS, ViT-G 2B are not the phone path. Edge stays capture + known poses. |
| License | OSM attribution; Marble/Genie closed; V-JEPA 2.1 paper NC-ND; imagery tiles. Provenance on leaves. |
| Scope explosion | L0–L2 stay thin. Pilot site first. |
| Index confusion | Four “graphs” (BVH, HNSW, View edges, DiskANN Vamana) — vocabulary trap documented in diskann note. |
| Authority leak | Latent WMs (S2/S6) want to act. ADR-090 facade is the tripwire. |

---

## 7. How to read the siblings

| Deep dive | Use when |
|-----------|----------|
| [world-labs-marble-atlas.md](./world-labs-marble-atlas.md) | Dual-export copy, Li taxonomy, Atlas non-metric |
| [interactive-video-world-models.md](./interactive-video-world-models.md) | S2 + S8; LeWM priors only |
| [feedforward-reconstruction.md](./feedforward-reconstruction.md) | VGGT / SpatialLM adapters |
| [scene-graphs-open-vocab.md](./scene-graphs-open-vocab.md) | F4/F9 hierarchy |
| [large-scale-3dgs-lod.md](./large-scale-3dgs-lod.md) | L2–L4 appearance LOD |
| [vjepa-and-lewm.md](./vjepa-and-lewm.md) | Visual `index_id`, ADR-090 |
| [ruv-worldgraph-vs-weftos.md](../ruv-worldgraph-vs-weftos.md) | S7 compose; SuperSplat overlay |

---

## 8. Bottom line

Urth does not need a new world-model religion. 2026 gave us **better adapters** (feed-forward pose, indoor layout proposers, appearance LOD, hierarchical scene graphs, dense visual codes) and a **commercial vocabulary** for the dual export we already required.

What Urth still needs is unglamorous and in-tree: **live BVH publish**, **spatial RPC**, **F9 promote**, **E1/E2**, and honest empty space.

Generative worlds stay labeled **non-metric**. Metric beats generative. BVH stays the twin.
)
