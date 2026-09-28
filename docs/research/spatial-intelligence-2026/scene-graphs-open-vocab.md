# Scene graphs and open-vocab labels — 2026 survey for Graph Views / BVH

**Date:** 2026-09-21  
**Status:** Session research capture (not an ADR)  
**Survey ID:** S4 in [`README.md`](./README.md)  
**Companions:** [`graph-views.md`](../graph-views.md) (F1–F10), [`ruv-worldgraph-vs-weftos.md`](../ruv-worldgraph-vs-weftos.md) (compose, do not duplicate), ADR-056 / 078 / 088 / 093 / 095, `weftos-leaf-types` spatial tags

**Do not re-litigate:** BVH is the geometric index; HNSW is the feature index; join is optional `VectorRef` (ADR-088) + spatial-first / feature-first helpers (ADR-093); fusion ops are Graph Views; world-model SoT after promote is BVH Object leaves + chain, not a CLIP string and not a splat.

**Honesty rule:** open-vocab 3D papers answer “what might this blob be called?” They do **not** replace `LeafId`, chain evidence, or AABB containment. Pixel-dense language fields (LangSplat-class) are appearance-adjacent features. Hierarchical scene graphs are the **shape Graph Views F4/F9 should take** — floor → room → object — not a flat object soup.

---

## 0. What shipped (2023–2026)

| Item | Venue / date | What it is | What it is **not** |
|------|----------------|------------|---------------------|
| **ConceptGraphs** | ICRA 2024 · [arXiv:2309.16650](https://arxiv.org/abs/2309.16650) · [project](https://concept-graphs.github.io/) · [code](https://github.com/concept-graphs/concept-graphs) | Open-vocab **object** scene graph from posed RGB-D: class-agnostic masks → 3D instances → VLM captions → LLM spatial relations | A floor/room hierarchy; a dense CLIP point map; a metric twin |
| **HOV-SG** | RSS 2024 · [arXiv:2403.17846](https://arxiv.org/abs/2403.17846) · [project](https://hovsg.github.io/) · [code](https://github.com/hovsg/HOV-SG) · [RSS proceedings](https://roboticsproceedings.org/rss20/p077.html) | Hierarchical open-vocab 3D SG: **building → floor → room → object**, CLIP+SAM features at each level, cross-floor Voronoi nav; ~75% smaller than dense OV maps | A replacement for BVH; indoor-only, RGB-D, language-grounded *navigation* paper |
| **Sparse3DPR** | arXiv **2025-11-11** · [arXiv:2511.07813](https://arxiv.org/abs/2511.07813) | Training-free, **sparse RGB** (DUSt3R) → **hierarchical plane-enhanced scene graph (HPSG)** + task-adaptive subgraph for LLM QA. +28.7% EM@1 and 78.2% faster than ConceptGraphs on Space3D-Bench | A WeftOS crate; plane anchors are *reasoning* structure, not the geometric SoT |
| **LangSplat** | CVPR 2024 · [arXiv:2312.16084](https://arxiv.org/abs/2312.16084) · [code](https://github.com/minghanqin/LangSplat) | CLIP language embeddings on **3D Gaussians** + SAM hierarchical semantics (subpart / part / whole); scene-wise autoencoder to shrink CLIP dim | Object identity; metric collider; something to inline into BVH payloads |
| **LangSplat V2** | NeurIPS 2025 · [arXiv:2507.07136](https://arxiv.org/abs/2507.07136) | Sparse coefficient field + CUDA splat of high-dim language at 450+ FPS | Still an appearance/language *field*, not a leaf registry |
| **Feature 3DGS** | CVPR 2024 · [arXiv:2312.03203](https://arxiv.org/abs/2312.03203) · [project](https://feature-3dgs.github.io/) | Distill arbitrary-dim 2D foundation features (SAM, CLIP-LSeg) onto Gaussians via a parallel N-D rasterizer | A scene graph; identity over rescans |
| **Gaussian Grouping** | ECCV 2024 · [arXiv:2312.00732](https://arxiv.org/abs/2312.00732) · [code](https://github.com/lkeab/gaussian-grouping) | Compact **Identity Encoding** on each Gaussian, supervised by SAM 2D masks + 3D consistency → instance/stuff groups for edit | A stable `LeafId`; encodings are grouping features, not chain identity |

Adjacent (cite, do not productize here): Hydra hierarchical SGs (RSS 2022 / IJRR 2024) are the closed-vocab ancestor HOV-SG and Sparse3DPR both contrast against; ConceptFusion (2023) is the dense open-set map HOV-SG abstracts away from.

---

## 1. Hierarchy vs flat object soup

The 2024–2025 literature split is exactly the Graph Views residual: **F4/F9 must not materialize a bag of unlabeled AABBs**.

### 1.1 Three organizations

```text
ConceptGraphs (flat object soup + pairwise relations)
  object ──on── object ──next_to── object
  (LLM captions / CLIP tags on nodes; no floor/room layer)

HOV-SG (indoor architectural hierarchy)
  root ── floor ── room ── object
  CLIP features at every level; height-histogram floors; Voronoi nav

Sparse3DPR HPSG (plane-centered hierarchy, 2025-11)
  V0 scene type (office / room)
    └── V1 dominant planes (floor, wall, ceiling)   ← spatial anchors
          └── V2 object instances
  edges: scene→structure, structure→object, object↔object (on / in / next_to)
```

**ConceptGraphs** (Gu et al., ICRA 2024) is explicit about the failure of dense per-point CLIP maps: they do not scale, and they have **no spatial relations**. The fix is an *object graph*. That is necessary and still **not sufficient** at building scale. HOV-SG says so: ConceptGraphs “focuses on smaller scenes”; large environments and queries *beyond the object level* (“the toilet in the bathroom on floor 2”) need floor and room nodes.

**HOV-SG** (Werby / Huang / Büchner / Valada / Burgard, RSS 2024) builds \(\mathcal{G}=(\mathcal{N},\mathcal{E})\) with \(\mathcal{N}=\mathcal{N}_S \cup \mathcal{N}_F \cup \mathcal{N}_R \cup \mathcal{N}_O\). Floors come from height-histogram peaks; rooms from floor-plan clustering; objects from SAM masks + CLIP, merged hierarchically. Each concept carries an open-vocab feature so a query can be *parsed into three hops* (LLM splits the utterance; the graph narrows floor → room → object). Representation size drops ~75% versus dense OV maps because the dense field is **summarized into nodes**.

**Sparse3DPR** (Feng et al., 2025-11-11) is the paper that names the WeftOS risk in LLM terms. Flat SGs dump every object into the prompt (token-inefficient, noisy). Affordance hierarchies (TB-HSU, AAAI 2025) group by *function* and **break spatial proximity** — worse seed-node retrieval. HPSG instead uses **dominant planes as anchors** (walls / floors / ceilings), then hangs objects on those planes. Ablation on Space3D-Bench: HPSG beats flattened HPSG and affordance-regrouped HPSG; task-adaptive **subgraph** (FAISS seed + 2-hop expand) is the other half of the lift.

That subgraph step is Graph Views **F10**, not a new kernel object: agents query a purpose-scoped View (k-hop / spatial-first), they do not ingest the whole fusion graph.

### 1.2 WeftOS seam (already decided)

| Hierarchy level | WeftOS artifact | Tag / kind |
|-----------------|-----------------|------------|
| Building / site | Urth L3 region View (ADR-079) | region id, not a new leaf tag |
| Floor / room shell | `WM_SURFACE` + `WM_VOLUME` Object leaves; room Graph View | `SpatialLeafTag::WmSurface` `0x5350_0011`, `WmVolume` `0x5350_0012` |
| Object instance | `WM_OBJECT` **Object** leaf, stable `LeafId` | `WmObject` `0x5350_0010` |
| Mask / splat group | `WM_SEGMENT` (link, not identity) | `WmSegment` `0x5350_0013` |
| One-shot observation | **Event** leaf (`SensorRead4D`, train job, human confirm) | ADR-056 `IdentityKind::Event` |

W1 geometric partition already sketches the Sparse3DPR plane step without ML: RANSAC floor/walls/ceiling → `WM_SURFACE` + room shell (`docs/weftos/splat-to-world-model.md` §4.1). Open-vocab is **§4.2 on top**, not instead.

**Do not** mint a parallel “scene-graph crate” that re-stores AABB geometry. The hierarchy lives in **Graph View edges** (`located_in`, `adjacent_to`, parent region) over BVH leaves.

---

## 2. Where embeddings live vs where geometry lives

ADR-056 / 088 / 093 already split the two indexes. The 2024–2025 splat-language papers are the **negative example**: they bolt CLIP onto the primitive that draws pixels.

### 2.1 What the papers do

| System | Geometry | Language / grouping | Memory trick |
|--------|----------|---------------------|--------------|
| Dense OV maps (ConceptFusion, LERF) | points / NeRF | per-point CLIP | does not scale; HOV-SG’s foil |
| ConceptGraphs / HOV-SG | object (and floor/room) nodes | CLIP / VLM **on the node** | one vector per concept, not per point |
| Sparse3DPR | DUSt3R points → planes + instances | captions + SentenceTransformer for **query routing** | subgraph, not a second spatial index |
| LangSplat / V2 | 3DGS | CLIP on **each Gaussian**, SAM 3-level semantics | scene autoencoder (V1) or sparse codes (V2) |
| Feature 3DGS | 3DGS | arbitrary foundation features on each Gaussian | 1×1 decoder / dim compression |
| Gaussian Grouping | 3DGS | compact **Identity Encoding** (not CLIP) per Gaussian, SAM-supervised | grouping id, still on the splat |

LangSplat is honest about the cost: raw CLIP on explicit Gaussians blows memory; they compress. Feature 3DGS is honest about the mismatch: RGB and feature maps differ in resolution and channel count. Gaussian Grouping is the closest to **instance identity**, and even there the encoding is a *rendered grouping feature*, not a durable world-model id.

### 2.2 WeftOS placement (normative for this survey)

```text
appearance (splat.sog / 3DGS)     structure (BVH)           features (HNSW / DiskANN)
  LangSplat-class fields  ──F4──►  Graph View vertices  ──VectorRef──► index_id
  Gaussian Grouping ids   ──F5──►  WM_SEGMENT / OBJECT
                                   AABB / plane / volume
                                   IdentityKind::Object|Event
```

| Question | Index | Handle |
|----------|-------|--------|
| What is in this volume? Same object across rescans? | **BVH** (`clawft-bvh`) | `LeafId` + `IdentityKind` + tag |
| What looks like / is called like this query? | **HNSW** (hot) / DiskANN (cold) | `VectorRef { index_id, vector_id }` |
| Join “near here, similar in CLIP” | ADR-093 `clawft-bvh::vector_join` | spatial-first default; feature-first allowed |
| Transitive “same-as” over many sessions | Graph View edges + optional batch WCC (ADR-095) | `component_id` write-back — **not** ANN |

Reserved namespaces already exist in `weftos-leaf-types::spatial::index_ids`:

| `index_id` | Constant | Put here |
|------------|----------|----------|
| 0 | `ECC_HNSW` | default kernel text / ECC |
| 1 | `VISUAL_FEATURES` | DINO / SAM / grouping encodings, Gaussian-field distillates |
| 2 | `LANGUAGE_FEATURES` | CLIP / SigLIP / HOV-SG node features, LangSplat latents **after** decode |

`WmObjectPayload.vector` is `Option<VectorRef>` and **defaults to `None`** (WEFT-709 / ADR-088). W1 must keep that. Filling `LANGUAGE_FEATURES` is a later producer step, not a payload schema change.

**Rejected (still rejected):** inline CLIP dims in leaf CBOR; a second cosine index inside the BVH; treating LangSplat’s per-Gaussian language as world-model SoT.

### 2.3 Object vs Event leaves

Open-vocab pipelines emit **both**, and mixing them is how identity rots.

| Kind | Examples from this literature | WeftOS |
|------|-------------------------------|--------|
| **Object** | HOV-SG floor/room/object nodes; ConceptGraphs instances; Sparse3DPR \(V_1\) planes and \(V_2\) objects; Gaussian Grouping instance after multi-view consistency | `IdentityKind::Object`, stable `LeafId` across branches |
| **Event** | a SAM mask on frame *t*, a CLIP query, a train job, a human “this is a chair”, a ToF hit | `IdentityKind::Event`, immutable, `parent_leaf` lineage |

Labels and embeddings on Events are **evidence**. Promotion copies a *summary* onto the Object leaf (and a `VectorRef` if we have one); it does not replace the Object.

---

## 3. Promote path F9 → BVH Object leaves + chain

Graph Views already name the loop (`docs/research/graph-views.md` §4b). Scene-graph papers are the **F4 / F5 / F9 / F10** content.

```text
F1  view.create          purpose = room-12-identity / floor-2 / site-X
F2  bind geometry        BVH region / tags [WM_OBJECT, WM_SURFACE, WM_VOLUME]
F3  bind sensors         live RGB-D / ToF / pose / co-observe (windowed)
F4  bind appearance      HNSW/DiskANN soft edges  (CLIP / DINO / grouping)
F5  bind structure       RANSAC planes, SAM→3D clusters, HOV-SG-class hierarchy
F6  hot fuse             co-location, track continuity, human confirm
F7  batch fuse           WCC / rank when the View cliffs (ADR-095)
F8  dual-branch (opt.)   ADR-087 scores as View features
F9  promote              stable components → Object leaves + chain events
F10 serve                k-hop / spatial-first / subgraph — never dump the View into the LLM
```

### 3.1 What “stable” means (steal, don’t copy)

| Paper signal | Promote gate analogue |
|--------------|----------------------|
| ConceptGraphs multi-view association + caption | ≥N views, geometric IoU, optional human Event |
| HOV-SG hierarchical merge of 3D masks | WCC `component_id` + room membership edge |
| Sparse3DPR DBSCAN + cross-view ID + plane inlier ratio | geometric partition first; captions later |
| Gaussian Grouping 3D spatial consistency on encodings | grouping feature on `VISUAL_FEATURES`, not a new tag |

F9 writes:

1. **Object leaf** — `WmObject` / `WmSurface` / `WmVolume` with AABB, optional `label` + `confidence`, `vector: None` until an embedder is chosen.
2. **Chain event** — `bvh.insert_leaf` (ADR-056 §6, ADR-022); no in-memory-only promote.
3. **Evidence Events** — frames, masks (`WmSegment`), CLIP hits, human confirms, with `parent_leaf` → the Object.
4. **View write-back** — `component_id`, provenance (`source_system`, `observed_at`, `confidence`, `chain_seq`).

Appearance (SOG, LangSplat field, Gaussian Grouping edit) **stays a source**. ADR-078 §5b: structure path remains the durable SoT.

### 3.2 Nested Views, not one mega-graph

HOV-SG’s three-level query is a **DAG of Views**, which Graph Views already allow (peer View as source):

| View | Sources | Promote target |
|------|---------|----------------|
| `site-X-floors` | height histogram / IMU gravity / `WM_SURFACE` floors | floor Object leaves |
| `floor-2-rooms` | floor View + wall planes | room volumes |
| `room-12-identity` | room volume + sensors + ANN | object Object leaves |

Caps stay on each View (ADR-095). Urth does **not** get one planet-wide scene graph.

F10 should look like Sparse3DPR’s subgraph extractor: embed the query, FAISS/HNSW top-k seeds on `LANGUAGE_FEATURES`, expand 1–2 hops on **View edges**, then pack. That is agent context policy (MetaHarness `contextBuilder`), not a BVH feature.

---

## 4. Open-vocab labels as features, not as identity

This is the load-bearing doctrine for S4.

**Identity** (WeftOS): `LeafId` + `IdentityKind::Object` + chain lineage + (optional) View `component_id`.  
**Label** (papers): CLIP argmax, RAM++ tag, GPT caption, SAM whole-mask name.  
**Feature** (join): a vector in `LANGUAGE_FEATURES` or `VISUAL_FEATURES` behind `VectorRef`.

`WmObjectPayload` already encodes the split:

```text
label: Option<String>        // human / model utterance — not the key
confidence: Option<f32>
bound: AabbWire              // geometry
vector: Option<VectorRef>    // join key; default None
```

Consequences:

1. **Do not key objects on `"chair"`.** Open-vocab vocabularies churn (CLIP vs SigLIP vs RAM++ vs GPT caption). The same instance can be “office chair”, “task chair”, and “seat” across sessions. HOV-SG and ConceptGraphs *want* that; they retrieve by embedding similarity. WeftOS should too — via HNSW, not by renaming `LeafId`.
2. **Do not put the CLIP vector in the leaf.** ADR-088 Option 3 stays rejected. LangSplat’s whole contribution is “CLIP-on-Gaussian is too fat unless you compress.” BVH payloads must not re-learn that lesson.
3. **Captions are Event evidence or View node features.** ConceptGraphs’ GPT-4 captions and Sparse3DPR’s VLM→LLM captions are useful for F10 packs and for `label` *hints*. They are not promote-gates by themselves (hallucination).
4. **Gaussian Grouping “identity encoding” is a grouping feature.** Map it to `VISUAL_FEATURES` + `WM_SEGMENT`. The durable instance is still the Object leaf after F9.
5. **Human confirm is an Event that can raise confidence**, not a license to skip chain.

rUv WorldGraph makes the same cut in RF language: the graph stores **beliefs with mandatory provenance**, not raw frames, and `SemanticState` is not the room’s id. Compose that pattern onto View edges (`source_system`, `confidence`, `chain_seq`); do not import CSI occupancy as geometry (see §5 compose).

---

## 5. Apply / compose / watch

Priority letters match the survey README (A = steal a pattern now, B = optional adapter, C = watch).

### Apply (A) — this cycle, no new champion model

| Pattern | From | WeftOS action |
|---------|------|----------------|
| Floor → room → object as View DAG | HOV-SG | First fusion ViewSpec should declare hierarchy edges (`located_in` room, room `located_in` floor). Do not ship a flat `WM_OBJECT` soup as “world model done.” |
| Planes before objects | Sparse3DPR HPSG + existing W1 RANSAC | F5 geometric partition: `WM_SURFACE` anchors, then cluster objects onto them. |
| Open-vocab on `VectorRef`, not in AABB | HOV-SG / ConceptGraphs / LangSplat (negative) | Producers may later set `index_id = LANGUAGE_FEATURES`; W1 remains `vector: None`. |
| F9 = stable component → Object leaf + chain | Graph Views + ADR-078 | Promote gate uses geometry + multi-view + optional human Event; CLIP similarity is a **soft edge**, not the commit. |
| F10 = subgraph, not full dump | Sparse3DPR task-adaptive subgraph | Agent packs: ANN seed + k-hop on the named View. |
| Labels ≠ ids | all of the above | `label` optional; `LeafId` authoritative. |

### Compose (A) — rUv WorldGraph, already crosswalked

Full table: [`docs/research/ruv-worldgraph-vs-weftos.md`](../ruv-worldgraph-vs-weftos.md). **Do not duplicate.** For scene graphs only:

| WorldGraph (ADR-139, `wifi-densepose-worldgraph`) | Scene-graph / WeftOS use |
|---------------------------------------------------|--------------------------|
| Typed serde enums (`Room`, `Zone`, `Wall`, `ObjectAnchor`, `Event`, `SemanticState`) | View edge kinds + `weftos-leaf-types` tags — not boxed trait objects |
| Mandatory `SemanticProvenance` | F9/F6 edge fields; CLIP/SAM/human as *why*, not *what* |
| `LocatedIn` / `AdjacentTo` / `Supports` / `Contradicts` / `DerivedFrom` | Graph View relations for HOV-SG-class hierarchy + evidence challenge |
| petgraph at **room** scale, JSON/RVF snapshot | Graph View V1 hot materialization; spill to ADR-095 when it cliffs |
| SuperSplat WASM overlay + provenance cards (ADR-200) | Browser pattern for Urth L4 **over** appearance; geometry SoT stays BVH |
| OccWorld occupancy predictor | Optional View feature / LeWM prior (ADR-090); never replaces promote |

WorldGraph’s sensors are RF/CSI/UWB. WeftOS geometry SoT remains **BVH**, not a CSI grid. Privacy rollup maps to substrate ACL (ADR-057) on View scope — map the principle, not BFLD packet magics.

### Watch (B/C) — do not productize this cycle

| Item | Why watch | Risk if we adopt raw |
|------|-----------|----------------------|
| LangSplat V2 / Online Language Splatting | Fast language fields on GS | Per-Gaussian language in the appearance store; GPU RAM; still not identity |
| Feature 3DGS promptable SAM edits | Nice for Agent Workspace **editing** | Editing appearance ≠ minting Object leaves |
| Gaussian Grouping local edit (remove / inpaint) | Appearance product | Easy to confuse instance encoding with `LeafId` |
| Hydra / Hydra-class live hierarchical SGs | Real-time robotics | Closed-vocab ancestor; WeftOS fusion is Views, not a second live SG daemon |
| TB-HSU affordance hierarchy | Sparse3DPR’s ablation foil | Function-centric trees break spatial seed retrieval; `WM_AFFORDANCE` stays a **volume tag**, not the scene skeleton |
| LLM captions as the graph (ConceptGraphs GPT-4, Sparse3DPR caption refine) | Useful F10 text | Hallucinated identity; cost; offline API keys in capture path |

---

## 6. Exact WeftOS seams (crate / ADR / tag)

| Seam | Where |
|------|--------|
| Broad-phase geometry | `crates/clawft-bvh` — AABB, `IdentityKind::{Object,Event}` |
| Leaf tags | `crates/weftos-leaf-types/src/spatial/tags.rs` — `WmObject` `0x5350_0010`, `WmSurface` `0x5350_0011`, `WmVolume` `0x5350_0012`, `WmSegment` `0x5350_0013`, `WmAffordance` `0x5350_0015` |
| Payloads + `VectorRef` | `primitives.rs`, `vector_ref.rs` (`LANGUAGE_FEATURES = 2`) |
| Dual-index join | `clawft-bvh::vector_join` (ADR-093) |
| Fusion ops | Graph Views F1–F10 — research, not `view.*` RPC yet (ADR-095 §1b) |
| Promote SoT | ADR-078 structure path + ADR-056 chain on every insert |
| Capture → structure | `docs/weftos/splat-to-world-model.md` stages 5b–5d (planes, SAM project, instance AABBs) |
| SAM / CLIP in train backends | `docs/weftos/splat-train-backends.md` §3.5 — world-model stage, **not** Brush replace |

---

## 7. Risks

| Risk | Mitigation |
|------|------------|
| Generative / CLIP label treated as truth | Label is feature; promote requires geometry + provenance; human Event for high trust |
| Per-Gaussian language fields in BVH or SOG as SoT | Keep LangSplat-class output on appearance + `VectorRef`; BVH stays AABB |
| Flat object soup at W1 | Require room/floor View edges or `WM_SURFACE` parent before calling fusion “done” |
| One global scene graph | Purpose-scoped Views + caps; Urth LOD is region Views, not Hydra-at-planet-scale |
| LLM caption / GPT relation edges without audit | Store as View features with `source_system`; chain only on F9 Object/Event writes |
| GPU / SAM+CLIP minutes-per-frame (LangSplat ~2.8 min/fr in later comparisons) | Structure stage async after SOG (ADR-078 non-goal: don’t block appearance); prefer node-level CLIP (HOV-SG) over dense fields |
| Closed weights / API captions in capture | Local CLIP/SigLIP for features; cloud captions optional and provenance-tagged |

---

## 8. Sources

Papers and code (retrieved 2026-09-21):

- Werby, Huang, Büchner, Valada, Burgard. *Hierarchical Open-Vocabulary 3D Scene Graphs for Language-Grounded Robot Navigation*. RSS 2024. [arXiv:2403.17846](https://arxiv.org/abs/2403.17846) · [hovsg.github.io](https://hovsg.github.io/) · [github.com/hovsg/HOV-SG](https://github.com/hovsg/HOV-SG)
- Gu, Kuwajerwala, Morin, Jatavallabhula, et al. *ConceptGraphs: Open-Vocabulary 3D Scene Graphs for Perception and Planning*. ICRA 2024. [arXiv:2309.16650](https://arxiv.org/abs/2309.16650) · [concept-graphs.github.io](https://concept-graphs.github.io/)
- Feng, Wei, Xu, Wang, Li, Wu. *Sparse3DPR: Training-Free 3D Hierarchical Scene Parsing and Task-Adaptive Subgraph Reasoning from Sparse RGB Views*. 11 Nov 2025. [arXiv:2511.07813](https://arxiv.org/abs/2511.07813)
- Qin, Li, Zhou, Wang, Pfister. *LangSplat: 3D Language Gaussian Splatting*. CVPR 2024. [arXiv:2312.16084](https://arxiv.org/abs/2312.16084)
- Li et al. *LangSplatV2: High-dimensional 3D Language Gaussian Splatting with 450+ FPS*. NeurIPS 2025. [arXiv:2507.07136](https://arxiv.org/abs/2507.07136)
- Zhou, Chang, Jiang, et al. *Feature 3DGS: Supercharging 3D Gaussian Splatting to Enable Distilled Feature Fields*. CVPR 2024. [arXiv:2312.03203](https://arxiv.org/abs/2312.03203) · [feature-3dgs.github.io](https://feature-3dgs.github.io/)
- Ye, Danelljan, Yu, Ke. *Gaussian Grouping: Segment and Edit Anything in 3D Scenes*. ECCV 2024. [arXiv:2312.00732](https://arxiv.org/abs/2312.00732)

WeftOS (already decided):

- [`docs/research/graph-views.md`](../graph-views.md) — F1–F10
- [`docs/research/ruv-worldgraph-vs-weftos.md`](../ruv-worldgraph-vs-weftos.md) — WorldGraph compose
- ADR-056, ADR-078, ADR-079, ADR-088, ADR-093, ADR-095
- `crates/weftos-leaf-types/src/spatial/{tags,primitives,vector_ref}.rs`
- `docs/weftos/splat-to-world-model.md`

rUv (grounded via `search_ruvnet`, worldgraph store): `wifi-densepose-worldgraph` typed petgraph, mandatory `SemanticProvenance`, ADR-139 / ADR-200 SuperSplat bridge — compose only.

---

## 9. Recommended tickets (do **not** file)

Plane currently has **no open spatial tickets**; residuals live in docs. Suggested items if product pulls S4 — for a later `scripts/plane.sh` pass, not this session:

1. **Graph View hierarchy materialization** — ViewSpec for floor → room → object (`located_in` / parent region); first shipping fusion View must not be a flat object list. Acceptance: room-scoped View exports vertices tagged `WM_SURFACE` (room shell) and `WM_OBJECT` with parent edges. Depends: Graph Views F1–F5 research hold → `view.*` when product pulls.
2. **F9 promote gate** — WCC/`component_id` + geometric IoU + optional human Event → `IdentityKind::Object` insert + chain; CLIP/ANN is candidate-only. Acceptance: promote writes `WmObjectPayload { vector: None, label: Some(_), confidence: Some(_) }` and at least one evidence Event with `parent_leaf`. Depends: WEFT-709 W1 export, ADR-078.
3. **Plane-anchored F5** — RANSAC / Sparse3DPR-style dominant planes as `WM_SURFACE` Object leaves **before** object clustering (already sketched in splat-to-world-model §4.1). Acceptance: floor/wall/ceiling leaves exist for a W1 fixture scene with `vector: None`.
4. **`LANGUAGE_FEATURES` producer** — optional CLIP/SigLIP node vectors after W1; fill `VectorRef { index_id: 2, vector_id }` on Objects; never inline dims. Acceptance: ADR-093 spatial-first join returns CLIP-similar chairs inside a room AABB. Depends: ADR-088, dual-backend SpatialService façade (survey residual 3).
5. **F10 subgraph pack** — query embed → HNSW seed → 2-hop on View edges (Sparse3DPR pattern) as agent contextBuilder policy; MetaHarness flywheel may mutate caps/thresholds, not ECC. Acceptance: agent pack size bounded by View caps; no full-View dump.
6. **Grouping encodings as `WM_SEGMENT`** — if Gaussian Grouping (or SAM instance ids) land in the splat stage, store as segment links + `VISUAL_FEATURES`, not as `LeafId`.

Non-tickets: LangSplat-in-kernel, Hydra dependency, affordance tree as the primary hierarchy, planet-scale single SG.

---

## 10. Document history

| Date | Change |
|------|--------|
| 2026-09-21 | Initial S4 deep dive: HOV-SG, ConceptGraphs, Sparse3DPR HPSG, LangSplat / Feature-3DGS / Gaussian Grouping, mapped onto Graph Views F9 and `VectorRef` |
