# Spatial intelligence and world models — 2026 survey for Urth

**Date:** 2026-09-21  
**Status:** Session research capture (not an ADR)  
**Scope:** New models, papers, and products since WeftOS 0.8.0 (2026-07-31) that touch **spatial intelligence**, **world models**, and **large-scale reconstruction** — mapped onto Urth, BVH, HNSW, Graph Views, and LeWM.

**WeftOS already decided (do not re-litigate):**

| Piece | Decision |
|---|---|
| Geometric index | BVH (`clawft-bvh`, ADR-056) — not R-tree / kd-tree |
| Feature index | HNSW primary; DiskANN deferred cold tier |
| Join | Optional `VectorRef` (ADR-088) + spatial-first / feature-first helpers (ADR-093) |
| Appearance vs structure | Dual output (ADR-078). Generative fill is cosmetic, never metric truth (ADR-079) |
| Latent WM | LeWM optional; ECC remains authority (ADR-090) |
| Fusion ops | Graph Views F1–F10 (`docs/research/graph-views.md`) |
| Twin name | **Urth** (ADR-079) |

**Honesty rule:** pixel-video “worlds” are **not** a replacement for BVH leaves + chain. Marble-style persistent 3D is closer to our appearance/structure split than Genie-style interactive video.

---

## Candidate list (this session)

Priority: **A** = apply or steal a pattern now · **B** = research / optional adapter · **C** = watch, do not productize.

| ID | Item | Kind | Date | Why it matters for WeftOS | Pri | Deep dive |
|---|---|---|---|---|---|---|
| S1 | World Labs **Marble** + **Atlas**; Li taxonomy (renderer / simulator / planner) | Product + essay | 2025-11 → 2026-09 | Dual export (splats + collider mesh) is the commercial form of ADR-078. Atlas adds camera-controlled omni generation. Taxonomy is the vocabulary we should use in Urth docs. | A | [world-labs-marble-atlas.md](./world-labs-marble-atlas.md) |
| S2 | DeepMind **Genie 3** + SIMA 2; NVIDIA **Cosmos 2.5 / Cosmos 3**; **WBench** | Interactive video WMs | 2025-08 → 2026-09 | Video-as-simulator is the competing definition of “world model.” Useful for LeWM priors / synthetic rollouts, **not** for Urth metric geometry. | B | [interactive-video-world-models.md](./interactive-video-world-models.md) |
| S3 | **VGGT** (CVPR 2025 best paper) + **VGGT-Ω** (2026-05); **MASt3R** / DUSt3R; **SpatialLM** | Feed-forward reconstruction | 2025–2026 | Pose-free / feed-forward SfM that can replace overnight COLMAP in the splat pipeline. SpatialLM is structured indoor layout from noisy RGB video — closest analog to W1 geometric partition. | A | [feedforward-reconstruction.md](./feedforward-reconstruction.md) |
| S4 | **HOV-SG**, **ConceptGraphs**, **Sparse3DPR**, LangSplat / feature-3DGS | Open-vocab 3D scene graphs | 2024–2026 | Graph Views F4/F9 should look like hierarchical scene graphs (floor → room → object), not a flat object soup. Open-vocab embeddings belong on `VectorRef`, not in BVH payloads. | A | [scene-graphs-open-vocab.md](./scene-graphs-open-vocab.md) |
| S5 | **CityGaussianV2**, **Octree-GS**, **VastGaussian**, LODGE | Large-scale 3DGS + LOD | 2024–2025 | Direct analog of Urth L2–L4 appearance LOD and “shard BVH by region.” Octree-GS is the appearance twin of our AABB hierarchy. | A | [large-scale-3dgs-lod.md](./large-scale-3dgs-lod.md) |
| S6 | **V-JEPA 2 / 2.1** (Meta) vs WeftOS **LeWM** | Latent predictive WM | 2025-06 → 2026-03 | Confirms ADR-090: latent prediction is a sub-layer, not the twin. Dense features could feed HNSW `index_id` visual namespace. | B | [vjepa-and-lewm.md](./vjepa-and-lewm.md) |
| S7 | rUv **WorldGraph** + OccWorld + SuperSplat WASM bridge | Adjacent stack | 2026-06 | Already crosswalked. SuperSplat overlay + provenance cards is the browser pattern for Urth L4. Geometry SoT stays BVH. | A (compose) | [ruv-parallels-and-gaps.md](./ruv-parallels-and-gaps.md) (2026-09-21 two-way) · [July crosswalk](../ruv-worldgraph-vs-weftos.md) |
| S8 | **Astronex-World 1.0**, Matrix-Game 3.0, HY-World 1.5, Yume 1.5 | Open interactive WMs | 2026-08/09 | Show the open-weight video-WM explosion. Watch for camera/PRoPE control; do not pick a champion this cycle. | C | [interactive-video-world-models.md](./interactive-video-world-models.md) |
| S10 | **image-blaster** (Claude skill set: one photo → Marble splat + collider, Hunyuan3D object meshes, SFX; Spark + Rapier viewer) | Open-source workflow (MIT) | 2026-04 → 2026-05 (added 2026-10-04) | All output is generative (cosmetic only). Worth keeping: the lift-or-push rule that splits static scene from movable objects (ADR-078 Object vs surface), and its disk-first indexed artifacts with a provenance JSON beside each file. | B (two A patterns) | [image-blaster.md](./image-blaster.md) |
| S9 | **AntiHunter** DIGI node (ESP32-S3 WiFi/BLE/CSI, Meshtastic) | RF perimeter firmware | 2025–2026 | Land/dock RF intelligence. CSI motion + RSSI trilateration are **features**, not BVH. Do not flash onto acoustic S3 (radio must stay off). | B | [../antihunter-weftos-crosswalk.md](../antihunter-weftos-crosswalk.md) |

### Sources (session)

- [Airtree world-models primer](https://www.airtree.vc/open-source-vc/world-models-primer) (2026-07-17) — spatial vs behavioural vs dynamics
- [Genie 3](https://deepmind.google/blog/genie-3-a-new-frontier-for-world-models/) (2025-08-05)
- [Cosmos-Predict2.5](https://research.nvidia.com/labs/cosmos-lab/cosmos-predict2.5/) · [arXiv:2511.00062](https://arxiv.org/abs/2511.00062)
- [WBench](https://meituan-longcat.github.io/WBench/) (2026-09)
- [Astronex-World 1.0](https://arxiv.org/abs/2609.20034) (2026-09-17)
- [World Labs Marble](https://www.worldlabs.ai/blog/marble-world-model) · [Atlas](https://www.worldlabs.ai/blog) (2026-09-01)
- [VGGT](https://github.com/facebookresearch/vggt) · [VGGT-Ω](https://vggt-omega.github.io/)
- [SpatialLM](https://github.com/manycore-research/SpatialLM) (NeurIPS 2025)
- [CityGaussianV2](https://github.com/Linketic/CityGaussian) (ICLR 2025)
- [V-JEPA 2](https://ai.meta.com/blog/v-jepa-2-world-model-benchmarks/) · V-JEPA 2.1 (2026-03)
- rUv WorldGraph ADRs 139 / 147 / 200 / 202 (via `search_ruvnet`)

---

## WeftOS research queue (spatial) — still true after this session

Plane has **no open spatial tickets**. Residuals live in docs, not the board:

1. Live BVH publish (`bvh_published: false` on W1 export)
2. Reattach daemon `spatial_rpc` / `spatial_cli_e2e` (ignored)
3. Dual-backend SpatialService façade (ADR-093 helpers exist)
4. Graph Views F1–F10 operational (F2 bind geometry, F9 promote)
5. Urth E1 region hierarchy (ECEF/ENU) and E2 OSM basemap
6. Temporal HNSW fingerprinting (concept paper §10) — still future
7. ADR-069 panopticon (`chain_seq` reverse join)
8. ADR-087 K-STEMIT dual-branch — proposed only
9. DiskANN cold vector tier — deferred
10. Planet-scale BVH shard (geohash / region)

**New residuals this session proposes (not filed unless asked):**

- Optional VGGT / MASt3R-SLAM path in splat pipeline (before COLMAP)
- SpatialLM (or equivalent) as a W1 layout proposer, emitting `WM_SURFACE` / `WM_OBJECT` with `vector: None`
- Octree-GS / CityGaussian-style appearance LOD keyed to Urth L2–L4 region ids
- Scene-graph hierarchy (floor/room/object) as Graph View materialization, promote to BVH Object leaves
- Marble dual-export as the **product language** for appearance vs collider/structure (we already decided this; copy should match)
- V-JEPA 2.1 dense features as a visual `index_id` — never as world-model SoT

---

## How to read the deep dives

Each sibling doc answers:

1. What shipped in 2025–2026 (with URLs)
2. What it is **not**
3. Exact WeftOS seam (crate / ADR / leaf tag)
4. Apply / compose / watch
5. Risks (generative-as-truth, GPU cost, license, closed weights)
