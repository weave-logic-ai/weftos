# World Labs spatial intelligence — Marble, Atlas, Li taxonomy

**Status:** Session research capture (not an ADR)  
**Date:** 2026-09-21  
**Author:** wm-labs (researcher), Ruflo team `spatial-intel-2026`  
**Parent:** [`README.md`](./README.md) (S1)  
**Companions:** ADR-056, ADR-078, ADR-079, ADR-088, ADR-090, ADR-093;  
[`docs/research/graph-views.md`](../graph-views.md);  
[`docs/weftos/splat-to-world-model.md`](../../weftos/splat-to-world-model.md)

**Honesty rule (parent):** pixel-video “worlds” are **not** a replacement for BVH leaves + chain. Marble-style persistent 3D (splats + collider) is the commercial rhyme of ADR-078. Atlas is an omni *generator / reconstructor*, not a twin.

WeftOS already decided the stack below. This note **maps** World Labs onto that vocabulary. It does not reopen BVH vs R-tree, dual output, sparse-first Urth, optional `VectorRef`, spatial-first join, LeWM-optional / ECC-authority, or Graph Views F1–F10.

---

## 0. Thesis

World Labs is shipping the same *split* WeftOS named in ADR-078, in commercial packaging:

| World Labs artifact | What it is | WeftOS counterpart |
|---|---|---|
| Gaussian splat (SPZ / PLY) | Highest-fidelity **appearance** | `splat.sog` / PLY, Spark / harness viewer |
| Collider mesh (GLB, 100–200k tris) | Coarse **state** for physics | `WM_SURFACE` / `WM_VOLUME` / AABB leaves in `clawft-bvh` |
| High-quality visual mesh (GLB) | Appearance fallback for DCC tools | Optional; **not** world-model SoT |
| Spark (THREE.js, MIT) | Browser renderer | Appearance projection (ADR-078 §5) |
| Atlas (2026-09-01) | Omni generator + sparse reconstructor | Optional *cosmetic / prior* source; never metric truth |
| Li taxonomy | renderer / simulator / planner | appearance / BVH structure / agents + Graph Views + optional LeWM |

The rhyme is dual export. The anti-rhyme is treating generated rooms, Atlas-filled backsides, or the closed World API as source of truth. ADR-079 already forbids that: generative fill is **cosmetic, never metric**.

---

## 1. What shipped

Dates below are from World Labs posts, docs, and Fei-Fei Li’s Substack. Atlas published **no paper, no model card, no weights**. Reconstruction “wins” vs VGGT-Ω / π³ / Depth Anything 3 / MapAnything are **World Labs’ own reruns** ([Atlas blog](https://www.worldlabs.ai/blog/atlas); see also [Implicator, 2026-09-01](https://www.implicator.ai/world-labs-atlas-withholds-paper-price-partners/)). Do not cite them as independent literature.

### 1.1 Timeline

| Date | What | Source |
|---|---|---|
| 2024 (early) | World Labs founded (Li, Justin Johnson, Christoph Lassner, Ben Mildenhall) | [From Words to Worlds](https://drfeifei.substack.com/p/from-words-to-worlds-spatial-intelligence) |
| 2024 | Large-world-model research preview; Lofi Worlds | [Spark 2.0 post](https://www.worldlabs.ai/blog/spark-2.0) |
| 2025-06-03 | **Forge** MIT 3DGS renderer for THREE.js / WebGL2 | [Radiance Fields](https://radiancefields.com/forge-dev-released-by-world-labs) |
| 2025-06-19 | Forge renamed **Spark** (name collision); npm `@sparkjsdev/spark` | [Radiance Fields](https://radiancefields.com/forge-renamed-to-spark); [sparkjs.dev](https://sparkjs.dev) |
| 2025-10-16 | **RTFM** research preview — learned *renderer* (no explicit 3D) | [RTFM](https://www.worldlabs.ai/blog/rtfm) |
| 2025-11-10 | Li manifesto **From Words to Worlds** | [Substack](https://drfeifei.substack.com/p/from-words-to-worlds-spatial-intelligence); [World Labs](https://www.worldlabs.ai/blog) |
| 2025-11-12 | **Marble** generally available | [Marble post](https://www.worldlabs.ai/blog/marble-world-model) |
| 2026-01-21 | **World API** (`api.worldlabs.ai/marble/v1/…`) | [World API](https://www.worldlabs.ai/blog/announcing-the-world-api) |
| 2026-04-02 | Marble **1.1 / 1.1 Plus** | [Release notes](https://docs.worldlabs.ai/marble/release-notes) |
| 2026-04-14 | **Spark 2.0** — LoD splat tree, streamable **`.RAD`**, GPU page table | [Spark 2.0](https://www.worldlabs.ai/blog/spark-2.0) |
| 2026-06-03 | **A Functional Taxonomy of World Models** (renderer / simulator / planner) | [Substack](https://drfeifei.substack.com/p/a-functional-taxonomy-of-world-models); [World Labs](https://www.worldlabs.ai/blog/taxonomy-of-world-models) |
| 2026-07-21 | **SceniX** acquired (robotics sim; Yunzhu Li, Changxi Zheng) | [SceniX](https://www.worldlabs.ai/blog/scenix) |
| 2026-07-28 | R2S2R robotics post + a16z Casado conversation | [R2S2R](https://www.worldlabs.ai/blog/real-to-sim-to-real) |
| 2026-09-01 | **Atlas** omni world model; early access waitlist; “will power future Marble”; **not in Marble yet** | [Atlas](https://www.worldlabs.ai/blog/atlas) |

### 1.2 Marble — product and formats

Marble is World Labs’ first commercial multimodal world model. Public claim (2025-11-12): create persistent, navigable 3D worlds from **text, images, video, or coarse 3D layouts**; then **edit, expand, compose**, and export ([Marble](https://www.worldlabs.ai/blog/marble-world-model)).

**Inputs**

- Text prompt; single image; multi-image (stitch + invent transitions); video (paid); 360 panorama.
- **Chisel:** coarse boxes / planes / imported assets for *structure*, text prompt for *style*. World Labs’ own words: “Chisel decouples **structure** from **style**.”
- **Expand:** grow a selected region (also used to “clean” artifacted corners).
- **Composer:** lay out multiple worlds relative to each other.

**Exports** (paid; Free generates but does not export — [Gaussian splat export](https://docs.worldlabs.ai/marble/export/gaussian-splat); [specs](https://docs.worldlabs.ai/marble/export/specs); [mesh](https://docs.worldlabs.ai/marble/export/mesh)):

| Class | Format | Spec (as of docs 2026-05 / 2026-09) | Plan |
|---|---|---|---|
| Appearance | Splats **SPZ** ~2M / ~500k | Native, compressed (Niantic SPZ lineage) | Standard |
| Appearance | Splats **PLY** ~2M / ~500k | Broader DCC compatibility | Standard |
| Appearance | 360 panorama PNG | Equirectangular 2560×1280 | Standard |
| Appearance | Video | Pixel-accurate camera path; optional “enhance” (adds motion/detail) | Standard; enhanced on Pro |
| **Simulator-shaped** | **Collider mesh GLB** | 100–200k triangles; ~3–4 MB; **physics only** | Standard |
| Appearance-mesh | HQ mesh GLB | ~600k textured **and** ~1M vertex-colored; up to 1 h offline; ~100–200 MB | Pro |

World Labs is explicit: **do not use collider meshes for visual rendering**; Gaussian splat is the highest-fidelity representation; HQ mesh is derived and will have holes, floaters, and bad geometry on thin / reflective / uncovered regions ([mesh FAQ](https://docs.worldlabs.ai/marble/export/mesh)).

Default coordinates are **OpenCV** (`+x` left, `+y` down, `+z` forward). DCC tools often need Y/Z flip to OpenGL ([specs FAQ](https://docs.worldlabs.ai/marble/export/specs)).

World API `GET /marble/v1/worlds/{id}` returns the same split as URLs: `assets.splats.spz_urls`, `assets.mesh.collider_mesh_url`, `assets.mesh.hq_mesh_url`, plus `semantics_metadata.ground_plane_offset` and `metric_scale_factor` ([Get a world](https://docs.worldlabs.ai/api/reference/worlds/get)). That last pair is a yardstick analog of WeftOS `SPLAT_YARDSTICK` — useful on import, **not** a substitute for surveyed scale.

Interactive demos combine **Spark splat + Rapier physics + collider GLB** (e.g. [spark-physics](https://github.com/bmild/spark-physics), cited from Marble mesh docs).

### 1.3 Spark — THREE.js splat renderer

Spark is the **open** piece. MIT license. THREE.js + WebGL2. Repo: [github.com/sparkjsdev/spark](https://github.com/sparkjsdev/spark). Docs: [sparkjs.dev](https://sparkjs.dev).

Shipped capabilities (2025–2026):

- Mix splat objects with mesh objects; global back-to-front sort (not per-object paste-over).
- Formats: `.PLY` (incl. compressed), `.SPZ`, `.SPLAT`, `.KSPLAT`, `.SOG` (WeftOS already emits SOG).
- User-programmable GPU splat pipeline (GLSL or Dyno shader graph).
- **Spark 2.0 (2026-04-14):** continuous **LoD splat tree**, progressive streaming, virtual memory over a fixed GPU page table (64k-splat pages). New streamable container **`.RAD`** (JSON header + gzipped column-major 64k chunks). LoD traversal in **Rust → Wasm** on a Worker. Tiny-LoD (on-demand) vs Bhatt-LoD (offline). Claimed demos: 6M–100M+ splats with a ~0.5–2.5M on-device budget ([Spark 2.0](https://www.worldlabs.ai/blog/spark-2.0)).

Spark is a **renderer**. It does not mint identity, affordances, or chain evidence.

### 1.4 Atlas — 2026-09-01 omni world model

Atlas is described as a **multimodal autoregressive diffusion transformer** pretrained from scratch on text, images, video, and 3D. Inputs are grounded at 3D positions into a **spatial context**. Tasks claimed ([Atlas](https://www.worldlabs.ai/blog/atlas)):

1. **Camera-controlled generation** — native camera geometry (not text “pan left”); up to **1 minute at 1440p** from 1–6 images.
2. **Spatial reconstruction** — 1 to 100+ images; joint novel views + **point clouds / 3D Gaussian splats**. “Two or three images typically enough”; more images → less imagination.
3. **Space-time simulation** — bullet-time reframe from 3–5 phones; Real-to-Sim RGB+depth for robot cameras; manipulation variation.
4. **Image / 360 generation** — secondary; not the product thesis.

Important product facts:

- Atlas **will power future Marble**; **existing Marble worlds are untouched**; Atlas is **not in Marble yet**.
- Early access via Typeform; **no public API, price, paper, or weights**.
- Atlas **fills unseen regions from world knowledge**. Their own cottage example: one photo invents the rest of the scene; three photos make it “accurate.” That is generative completion, not a survey.

### 1.5 Essays (doctrine, not papers)

**From Words to Worlds** (Li, 2025-11-10). Spatial intelligence is “the frontier beyond language.” LLMs are “wordsmiths in the dark.” World models need three capabilities: **generative** (geometrically/physically consistent worlds, including an *explicit observable state*), **multimodal**, **interactive** (next state from action; eventually next action). Research bets: a geometry/physics-grounded training objective; extract 3D from 2D video at scale; 3D/4D-aware architectures beyond 1D tokenization. Marble is “first step,” not the unified model. ([Substack](https://drfeifei.substack.com/p/from-words-to-worlds-spatial-intelligence))

**A Functional Taxonomy of World Models** (Li + World Labs, 2026-06-03). POMDP loop. Three *functions*, not three companies:

- **Renderer** → observations (pixels). Visual fidelity. Sora / Genie 3 / RTFM. “What a viewer would see, not what is.”
- **Simulator** → **state** (geometry, physics, dynamics) that humans *and programs* can compute on. Linchpin. Scarce 3D data; sim-to-real gap; **generative geometry can look correct while being self-intersecting or wrong-scale**.
- **Planner** → actions from observation + goal. Inverse of renderer. VLAs / World Action Models. Nascent.

Marble is cited as the first World Labs move into simulator territory *because it emits Gaussian splats **and** collision meshes from one model*. Endpoint they want: one foundation model that switches output modality. ([Substack](https://drfeifei.substack.com/p/a-functional-taxonomy-of-world-models); [World Labs](https://www.worldlabs.ai/blog/taxonomy-of-world-models))

### 1.6 SceniX (2026-07-21)

World Labs acquired SceniX, a robotics-simulation company (founders Yunzhu Li, Changxi Zheng; Columbia). Official line: robotics is “where spatial intelligence becomes physical”; next breakthroughs from “spatial intelligence, world models, learning-based simulation, and a closed loop with real-world learning.” Terms undisclosed. Follow-up (2026-07-28) frames **R2S2R** (real-to-sim-to-real) with Marble-class worlds as the sim substrate ([SceniX](https://www.worldlabs.ai/blog/scenix); [R2S2R](https://www.worldlabs.ai/blog/real-to-sim-to-real)).

For WeftOS this is a **watch** on the planner/sim loop, not a crate to vendor.

---

## 2. Taxonomy vs WeftOS vocabulary

Li’s three functions already have names in this repo. Do not invent a fourth.

```text
  observation (pixels)          state (geometry)           action
  RENDERER                      SIMULATOR                  PLANNER
  ─────────                     ──────────                 ────────
  splat.sog / Spark             clawft-bvh + WM_*          agents query View
  HQ mesh / video enhance       collider analog            Graph Views F10
  Atlas novel-view / fill       ADR-078 structure stage    optional LeWM impulse
                                ADR-056 AABB index         ECC remains authority
```

| Li function | Contract | World Labs today | WeftOS (already decided) |
|---|---|---|---|
| **Renderer** | Pixels for eyes; visual fidelity | Marble splat, Spark, RTFM, Atlas video/novel-view, HQ mesh, enhanced video | Appearance train (`clawft-splat-pipeline` / Brush) → `splat.sog`; Spark / harness / Agent Workspace backdrop. ADR-078 §1 left column. |
| **Simulator** | **State** programs can compute on | Collider GLB; Atlas point cloud / splat-as-geometry; SceniX / R2S2R | **BVH geometric index** (ADR-056). Structure extract → `WM_OBJECT` / `WM_SURFACE` / `WM_VOLUME` (ADR-078). SoT is **BVH + chain**, not the SOG, not a closed API. BVH is **not** a physics engine (ADR-056 / ADR-078 non-goal). |
| **Planner** | Actions from observation + goal | Atlas does **not** emit actions (early-access gen/recon/sim). SceniX is the bet. | Agents + **Graph Views F1–F10** (create → bind geometry/sensors → fuse → **F9 promote** → **F10 serve**). **LeWM optional**; ECC authority R1–R5 (ADR-090). |

**Chisel** (structure boxes vs style prompt) is the authoring UI of the same split: coarse layout ≈ simulator-shaped constraint; text ≈ renderer.

**Atlas “spatial context”** is closer to a *conditioned generator* than to a Graph View. A View has `view_id`, caps, provenance, promote-to-BVH. Atlas context is a latent window that invents hallways between two unrelated photos. Do not conflate.

**Join hygiene (ADR-088 / ADR-093):** open-vocab / visual embeddings from any World Labs (or other) appearance model belong on optional `VectorRef` (`index_id` visual namespace), **never inline** in BVH payloads. Spatial-first join remains default: AABB query → decode handles → rank in HNSW.

**LeWM vs Atlas:** Atlas is a giant pixel/3D generator. LeWM is a **latent predictive** substrate that may emit Impulse/Observation only. ADR-090 R2: the runtime must work with WM absent. Atlas-as-dependency would violate that even if weights were public.

---

## 3. Dual export vs ADR-078

This is the commercial rhyme. We already chose it. Copy should match; architecture should not move.

ADR-078 §1:

> Every successful reconstruction job **should** produce appearance (`splat.sog` / ply) **and** structure (`world_model` export + optional BVH publish).

Marble §export:

> Gaussian splats are the highest-fidelity representation. Collider meshes are low-fidelity meshes intended for coarse physics. High-quality meshes try to match splat visual fidelity.

| Concern | Marble | WeftOS ADR-078 / 056 |
|---|---|---|
| Appearance SoT for *looking* | Splat (SPZ/PLY); Spark | SOG/PLY; Spark/harness |
| Structure SoT for *agents* | Collider GLB (coarse) + implied layout | **BVH leaves + chain**. AABB/object identity. |
| Visual mesh | Derived, lossy, Pro-gated | Optional DCC; never replaces BVH |
| “Done” | Appearance can ship before HQ mesh (async, ≤1 h) | Appearance may ship first; world-model **done** includes structure |
| Physics | Rapier + collider; “do not render colliders” | BVH is **not** a physics engine; volumes/affordances are queryable geometry |
| Viewer | Spark / Marble web | Projection only (ADR-078 §5) |
| Fusion / identity | Not a product object | Graph Views; F9 promote into BVH |

**What to steal (language, not crates):**

1. Say “splat + collider/structure” in Urth / splat docs the way Marble says “splats + collider mesh + visual mesh.”
2. Keep collider-class geometry **off** the beauty path. World Labs already warns operators who try to render colliders.
3. Async structure (W1 geometric partition after train) matches Marble’s offline HQ mesh — except our structure artifact is `WM_*` records, not a 200 MB GLB.

**What is *not* the same:** Marble’s collider is a triangle soup for game engines. WeftOS structure is **tagged AABBs with chain evidence** so agents can ask “what is in this room?” and governance can audit why. Importing a Marble collider as a `WM_VOLUME` *candidate* is compose; promoting it without evidence is a reversal of ADR-078.

---

## 4. What NOT to copy

| Temptation | Why it fails here | Canon |
|---|---|---|
| **Generative rooms as metric truth** | Marble expand / Atlas backside-fill / multi-image “hallways” invent geometry. Li’s taxonomy warns: generative simulators can look right with self-intersections and **wrong scale**. | ADR-079: empty space is `unknown` / `unobserved`. Generative fill is **optional cosmetic, always labeled non-metric**. |
| **Closed World API as SoT** | Worlds live behind `WLT-Api-Key`, credit meters, and World Labs URLs. No local ECC, no chain, no substrate ACL. Atlas has **no** public API. | Twin + fusion = Graph Views + BVH + chain. Foreign APIs are **imports** with provenance (`source_system`). |
| **One foundation model that renders, simulates, and plans** | Li’s 2026-06 endpoint. WeftOS split is load-bearing: ECC must run without a WM (ADR-090 R2); BVH must not grow a second similarity index (ADR-088); fusion is purpose-scoped Views, not a global tensor. | Keep renderer / simulator / planner as **projections of our stack**, not one crate. |
| **Collider as visual mesh / visual mesh as collider** | World Labs FAQ forbids both directions. HQ mesh is appearance-derived. | Appearance vs structure dual output. |
| **Atlas reconstruction as COLMAP replacement without a paper** | Self-reported vs VGGT-Ω / π³. Sibling S3 (VGGT / MASt3R) is the open path. | Watch Atlas; apply VGGT-class feed-forward SfM if anything. |
| **Inline embeddings from splat semantics** | API exposes `metric_scale_factor` and (elsewhere) community semantics. Do not stuff CLIP/DINO into leaf CBOR. | ADR-088 `vector: None` default; ADR-093 spatial-first join. |
| **RTFM / Genie-style interactive video as the twin** | Li classifies RTFM as a **renderer**. Parent README already parked Genie 3 / Cosmos in S2. | LeWM priors / synthetic rollouts only. |
| **Planet-scale generative Urth** | Marble compose/expand is room-to-building, not L0–L2. Spark LoD is appearance paging. | Urth sparse-first; OSM/DEM basemap; densify only where captured. |

---

## 5. Apply / compose / watch

Priority: **A** apply or steal a pattern now · **B** compose behind an adapter · **C** watch, do not productize.

| Pri | Item | Seam (crate / ADR / tag) | Notes |
|---|---|---|---|
| **A** | Dual-export **product language** | ADR-078; `docs/weftos/splat-to-world-model.md`; splat harness copy | Match Marble: appearance = splat; structure = collider-class / `WM_*`. Already decided; docs/UI should say it. |
| **A** | Spark (MIT) as L4 appearance viewer | `clawft-splat-pipeline` artifacts; browser harness; `clawft-wasm` www | We already name Spark in splat-to-world-model. Spark 2.0 LoD + `.SOG` is the Urth L4 backdrop path. Geometry SoT stays BVH overlays. |
| **A** | Taxonomy vocabulary in Urth docs | ADR-079; Graph Views F1–F10 | Renderer = appearance. Simulator = BVH structure. Planner = agents + Views + optional LeWM. Stops “world model” meaning three incompatible things in one PR. |
| **B** | Optional Marble/Atlas **import** | Graph View foreign import; `source_system=worldlabs`; leaf tags `WM_*` with `vector: None` | Ingest splat as appearance; collider GLB as *candidate* surfaces/volumes. Stamp **non-metric** unless a yardstick / survey exists. Never live-query the World API as the twin. |
| **B** | Collider GLB → W1 proposer | `clawft-bvh` / WEFT-709 geometric partition | Triangle soup → AABB / plane slabs → `WM_SURFACE` / `WM_VOLUME` **proposals**. Human/agent confirm before F9 promote. |
| **B** | Spark 2.0 LoD / `.RAD` ideas | Urth L2–L4 appearance LOD (sibling S5 CityGaussian / Octree-GS) | Continuous splat tree + 64k paging is the appearance twin of AABB hierarchy. Do not replace BVH with a splat tree. |
| **B** | `metric_scale_factor` / ground plane | `SPLAT_YARDSTICK`; gravity align in structure stage | Compose as evidence Event, not as silent world scale. |
| **C** | Atlas weights / API | — | No paper, no card, early access. Revisit if they open reconstruction **without** generative fill, or publish a protocol. |
| **C** | SceniX / R2S2R | ADR-090 planner surface | Robotics closed loop is their product. WeftOS planner is Graph View serve + ECC. Do not grow a robot-sim runtime this cycle. |
| **C** | Unified renderer-simulator-planner FM | ADR-090 R1–R5 | Explicitly the World Labs long arc. Explicitly not ours. |

Crate reminder (do not add World Labs as a link requirement): `clawft-bvh`, `weftos-leaf-types`, `clawft-splat-pipeline` / `clawft-splatd`, optional `weftos-worldmodel*` behind ADR-090. Spark is a **JS viewer** dependency of the harness, not a Rust world-model crate.

---

## 6. Recommended Plane tickets (acceptance criteria only)

Not filed. Parent README: Plane currently has **no open spatial tickets**. If lead files, keep them in `0.8.x` only if they unblock publish; otherwise later cycle.

**T1 — Dual-export language (docs / harness copy)**  
- AC: splat harness + `splat-to-world-model` + Urth intro use the triad *appearance splat / structure (`WM_*` or collider-class) / visual mesh optional*.  
- AC: one sentence states BVH+chain is SoT for “what is where,” matching ADR-078 §5.  
- Non-goal: new code.

**T2 — Non-metric generative-fill tag**  
- AC: leaf / View edge provenance has a boolean or enum `metric_truth` ∈ {surveyed, reconstructed, **generative_cosmetic**}.  
- AC: Marble/Atlas/expand-style imports default to `generative_cosmetic`.  
- AC: agent queries that need occupancy / navigation reject or down-rank cosmetic leaves (test fixture, no live API).  
- Canon: ADR-079 §2.

**T3 — Optional collider-GLB ingest (W1 proposer)**  
- AC: given a GLB in the 100–200k-tri class, emit `WM_SURFACE` / `WM_VOLUME` records with `vector: None`, `source_system` set, AABBs in the session ENU frame.  
- AC: OpenCV→ENU/OpenGL axis conversion is tested.  
- AC: **no** BVH publish until F9-equivalent promote; export-only path is enough for W1.  
- Non-goal: using the GLB as a visual mesh.

**T4 — Spark LoD appearance overlay (browser)**  
- AC: harness (or `clawft-wasm` www) can load a SOG/SPZ with Spark and overlay BVH AABB wireframes from a `world_model` export.  
- AC: overlay is a projection; toggling it off does not change exported structure.  
- Non-goal: `.RAD` encoder, 100M-splat paging, or replacing the current viewer if Spark 2.0 is too heavy this cycle — then ticket is “document gap,” not ship.

**T5 — Graph View F2 bind of imported structure**  
- AC: a purpose-scoped View can attach (a) local BVH region and (b) imported World Labs collider proposals as a second spatial source with caps.  
- AC: F9 promote writes WeftOS `WM_OBJECT` leaves + chain events, not World Labs URLs.  
- Canon: Graph Views F2/F5/F9.

**Do not file:** World API as SpatialBackend; Atlas in the splat train loop; LeWM depending on Marble; inline appearance embeddings; planet-scale generative fill.

---

## 7. Sources

Primary (fetched 2026-09-21):

- Fei-Fei Li, *From Words to Worlds: Spatial Intelligence is AI’s Next Frontier*, 2025-11-10 — <https://drfeifei.substack.com/p/from-words-to-worlds-spatial-intelligence>
- Fei-Fei Li / World Labs, *A Functional Taxonomy of World Models*, 2026-06-03 — <https://drfeifei.substack.com/p/a-functional-taxonomy-of-world-models> · <https://www.worldlabs.ai/blog/taxonomy-of-world-models>
- World Labs, *Marble: A Multimodal World Model*, 2025-11-12 — <https://www.worldlabs.ai/blog/marble-world-model>
- World Labs, *Announcing the World API*, 2026-01-21 — <https://www.worldlabs.ai/blog/announcing-the-world-api>
- World Labs, *Streaming 3DGS worlds on the web* (Spark 2.0), 2026-04-14 — <https://www.worldlabs.ai/blog/spark-2.0>
- World Labs, *Atlas: A World Model for Spatial Intelligence*, 2026-09-01 — <https://www.worldlabs.ai/blog/atlas>
- World Labs, *World Labs Acquires SceniX*, 2026-07-21 — <https://www.worldlabs.ai/blog/scenix>
- World Labs, *Building Worlds That Train Robots* (R2S2R), 2026-07-28 — <https://www.worldlabs.ai/blog/real-to-sim-to-real>
- World Labs, *RTFM: A Real-Time Frame Model*, 2025-10-16 — <https://www.worldlabs.ai/blog/rtfm>
- Marble docs: [mesh export](https://docs.worldlabs.ai/marble/export/mesh) · [file specs](https://docs.worldlabs.ai/marble/export/specs) · [splat export](https://docs.worldlabs.ai/marble/export/gaussian-splat) · [release notes](https://docs.worldlabs.ai/marble/release-notes) · [Get world API](https://docs.worldlabs.ai/api/reference/worlds/get) · [interactive examples](https://docs.worldlabs.ai/api/interactive-world-examples)
- Spark: <https://sparkjs.dev> · <https://github.com/sparkjsdev/spark> · physics demo <https://github.com/bmild/spark-physics>

Secondary (context, not used as paper claims):

- Radiance Fields World Labs platform page — <https://radiancefields.com/platforms/world-labs>
- Radiance Fields Atlas announcement — <https://radiancefields.com/world-labs-announces-new-world-model-atlas>
- Implicator on withheld paper/price/partners — <https://www.implicator.ai/world-labs-atlas-withholds-paper-price-partners/>
- a16z excerpt of *From Words to Worlds* — <https://www.a16z.news/p/from-words-to-worlds-spatial-intelligence>

WeftOS canon cited: ADR-056, ADR-078, ADR-079, ADR-088, ADR-090, ADR-093; `docs/research/graph-views.md` F1–F10; `docs/weftos/splat-to-world-model.md`.

No academic paper is cited for Atlas or Marble because none was published with those launches.

---

## 8. Handoff to team lead (spatial-intel-2026)

1. Deep dive is at `docs/research/spatial-intelligence-2026/world-labs-marble-atlas.md`; parent S1 row already points here.  
2. Live-sourced: Marble GA 2025-11-12, World API 2026-01-21, Spark 2.0 2026-04-14, Li taxonomy 2026-06-03, SceniX 2026-07-21, Atlas 2026-09-01; **no Atlas paper**.  
3. Map, do not reverse: renderer = splat/Spark; simulator = collider-class / BVH `WM_*`; planner = Graph Views + agents + optional LeWM.  
4. Dual export is the commercial rhyme of ADR-078 — steal the language, keep BVH+chain as SoT.  
5. Do **not** copy generative expand/Atlas-fill as metric truth (ADR-079) or the closed World API as SpatialBackend.  
6. Apply now: taxonomy words + Spark as appearance projection. Compose later: tagged collider-GLB ingest. Watch: Atlas weights, SceniX R2S2R, unified FM.  
7. Five Plane tickets sketched (T1–T5) as acceptance criteria only — **not created**; file only if you want them on the board.  
8. Crate seams: `clawft-bvh`, `weftos-leaf-types`, splat pipeline/harness; Spark is JS viewer, not a Rust WM dependency.  
9. Residual vs board: still no live BVH publish / Graph Views F2+F9 ops; this note does not unblock those.  
10. I am stopping here (researcher). Architect/implementer can claim T1 copy or T3 ingest if you promote them.
