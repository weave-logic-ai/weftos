# Splat train backends & reconstruction model collection

**Status:** Living (2026-07-30)  
**Product:** Pluggable **image+pose → Gaussians** engines behind splatd / quilt / multi-cam.

**Related:**

| Doc | Role |
|-----|------|
| [splat-freeform-quilt.md](./splat-freeform-quilt.md) | Accumulating layers; wants *fast* backends |
| [splat-multi-camera-rig.md](./splat-multi-camera-rig.md) | Known poses; ideal for feed-forward |
| [splat-to-world-model.md](./splat-to-world-model.md) | Structure after appearance |
| [splat-pipeline-design.md](./splat-pipeline-design.md) | Host stages |
| `~/llm/docs/models/deep-dives/3d-reconstruction-survey-2026-07.md` | Lab deep dive (Shasta + HF cards) |
| `~/llm/docs/models/registry/reconstruction.yaml` | Machine-readable model registry |

---

## 1. Why collect backends

We already run **COLMAP + Brush**. That is not the only way to get Gaussians:

| Family | Idea | Latency | Needs poses? |
|--------|------|---------|--------------|
| **Classical optimize** | Per-scene gradient train (3DGS / Brush / gsplat) | Minutes–hours | SfM or provided |
| **Feed-forward recon** | One neural net pass → Gaussians | Seconds–minutes | Usually **yes** (or predicts them) |
| **Hybrid** | Predict depth/pose then light refine | Mixed | Partial |
| **Generative** | Image → plausible 3D | Fast | No real metric |

**Policy (aligned with llm deep dive):**  
For **measurement / world model**, only **reconstruction** (real geometry from real views).  
**Generative** = mockups only, always labeled hallucinated.

Collecting models lets splatd choose:

```toml
[train]
backend = "brush"              # default hero quality
# backend = "instant-nurec"    # fast, pose-rich multi-view (domain-limited)
# backend = "gsplat-ns"        # nerfstudio/gsplat path
```

---

## 2. Architecture: pluggable train stage

```
session (images + camera stats)
        │
        ▼
┌───────────────────┐
│  Pose gate         │  known | colmap | reject
└─────────┬─────────┘
          │
          ▼
┌───────────────────┐
│  TrainBackend      │  trait / subprocess
│  · brush           │
│  · gsplat/ns       │
│  · instant-nurec   │
│  · … registry      │
└─────────┬─────────┘
          │ PLY / SOG
          ▼
  compress + package + structure (ADR-078)
```

### Backend interface (normative sketch)

| Input | |
|-------|--|
| Image list | Paths + timestamps |
| Cameras | Intrinsics + `T_world_cam` (or request SfM) |
| Config | Resolution, max gaussians, device |
| Output | `splat.ply` (+ optional semantics / depth) |
| Metrics | Time, peak VRAM, quality proxies |

Registry entry (see `reconstruction.yaml`) drives install, license, domain, hardware.

---

## 3. Families of approaches (deep dive)

### 3.1 Classical per-scene optimization

| Tool | Notes | WeftOS role |
|------|--------|-------------|
| **Brush** | Rust/wgpu; Metal on Mac; COLMAP dataset | **Default** host train |
| **gsplat** + Nerfstudio Splatfacto | CUDA-first research stack; rich methods | Linux/GPU runner option |
| **Original 3DGS (Inria)** | Reference | Research parity |
| **OpenSplat** | C++ open train | Watch |
| **PostShot / commercial** | Fast artist GUI | Not first-party |

**Strengths:** quality on arbitrary rooms when poses/overlap are good.  
**Weaknesses:** slow; depends on COLMAP when poses unknown.

### 3.2 Feed-forward / large reconstruction models (LRM-style)

| Model / line | Input contract | Output | Domain | Notes |
|--------------|----------------|--------|--------|--------|
| **nvidia/instant-nurec** | NCoreV4: multi-view + **poses** + intrinsics (~90 imgs @ 504×280) | 3DGS PLY + coarse semantics | **AV / driving** | Fast; **not** generic phone walk without NCore packaging; OOD indoors |
| **GS-LRM** (research) | Multi-view → Gaussians | 3DGS | General research | Lineage of large recon models |
| **Depth-Anything-v3** family | Depth/encoder backbone | Depth, not full 3DGS alone | General | Used *inside* Instant-NuRec |
| **DUSt3R / MASt3R / VGGT-class** | Unposed or loosely posed pairs/sets | Pointmaps / poses | General | **Pose recovery** helper before train |
| **apple/Sharp** | **Single image** → 3DGS views | Novel views | General (Apple) | **Not** multi-view room recon; detail/hero shots |

**Strengths:** low latency when contract matches.  
**Weaknesses:** domain lock, resolution caps, GPU hunger, license review.

### 3.3 Pose / geometry helpers (not full train, but pipeline)

| Tool | Role in WeftOS |
|------|----------------|
| **COLMAP / GLOMAP** | SfM default |
| **DUSt3R / MASt3R** | Bootstrap poses when free SfM fails |
| **ARCore / OpenVINS** | Edge poses for free-form quilt |
| **ToF / stereo / LiDAR** | Scale + structure (not appearance train) |

### 3.4 Generative image→3D (mockup only)

| Model | Use |
|-------|-----|
| TRELLIS.2, HY-World gen modes, Lyra-class | Illustrative missing props — **never** metric world model |
| World Labs Marble + Hunyuan3D via [image-blaster](../research/spatial-intelligence-2026/image-blaster.md) | One-photo mockup worlds and object meshes — same rule, `generative_cosmetic` |

### 3.5 Semantic / structure models (world model, not splat train)

| Model class | Role |
|-------------|------|
| SAM / SAM2 / open-vocab detectors | 2D masks → project to 3D objects |
| Depth foundation models | Dense depth for free-space volumes |
| CLIP / grounding | Class labels on WM_OBJECT |

These pair with ADR-078 structure stage, not with Brush replace.

---

## 4. Mapping to product modes

| Mode | Prefer backend | Why |
|------|----------------|-----|
| Indoor room, unknown poses | COLMAP + **Brush** | General, Metal-capable |
| Fixed multi-cam, locked poses | Brush **or** Instant-NuRec (if domain OK) | Poses free; feed-forward optional |
| Free-form quilt contribution | Instant-NuRec / fast gsplat for **preview layer**; Brush for **bake** | Latency vs quality |
| Hero export / demo | Brush (or gsplat high-iter) | Quality |
| Single object detail | apple/Sharp trial | Not full scene |
| Outdoor AV-like corridor | Instant-NuRec candidate | Trained distribution |

---

## 5. Instant-NuRec specifically (re-evaluated for WeftOS)

From HF + llm survey:

| Claim | Reality for us |
|-------|----------------|
| Images → 3DGS | Yes, but **NCore multi-cam log**, not arbitrary album |
| Fast | Yes (~minutes) |
| Known poses | **Required** — matches multi-cam + free-form contracts |
| Indoor rooms | **OOD risk** — validate before default |
| World model | Coarse road/FG/BG semantics only |
| Mac M-series | No custom CUDA but **AV pipeline + VRAM**; practical default remains Brush on Metal |

**Verdict:** keep as **optional backend** for pose-rich contributions and experiments; **not** replace Brush for general indoor; **do** collect NCore packaging path for multi-cam logs that look like “rig × time.”

---

## 6. Model collection process

### 6.1 Registry (source of truth)

- **Lab (llm):** `~/llm/docs/models/registry/reconstruction.yaml`  
- **WeftOS pointer:** this doc + periodic copy/sync of conclusions into `docs/weftos/models/` if needed  
- **Curator:** `mlx-model-curator` / reconstruction notes (Apple fit is secondary; **CUDA runner** is first-class for many models)

### 6.2 Entry checklist (before recommending)

1. Input contract (poses? max images? resolution?)  
2. Output format (PLY attrs, SOG convert?)  
3. Domain (indoor / outdoor / object / AV)  
4. License (commercial OK?)  
5. Hardware (VRAM, CUDA-only, Metal)  
6. Metric vs generative  
7. Install path (`pip`, container, HF)  
8. One-line WeftOS role  

### 6.3 Evaluation harness (later)

| Test set | Purpose |
|----------|---------|
| Phone room walk (your Desktop video class) | Indoor classical path |
| 4-cam known-pose room | Multi-cam + Instant-NuRec |
| Quilt: 2 contributions same region | Layer quality |
| Metric: known object dimensions | Scale sanity |

Never claim quality without a fixture run.

---

## 7. Implementation phases

| Phase | Work | Exit |
|-------|------|------|
| **T0** | This doc + reconstruction registry + links | Collection started |
| **T1** | splatd `train.backend` enum: `brush` only still default | Config surface |
| **T2** | Adapter for Instant-NuRec (NCore export + PLY ingest) experimental | Optional flag |
| **T3** | gsplat/ns backend on Linux GPU runner | Alternate hero |
| **T4** | Pose helper backends (MASt3R/DUSt3R) when COLMAP fails | Robustness |
| **T5** | Bench harness across fixtures | Data-driven default |

Plane: track under train-backend / ws17 tickets (create when implementing T1+).

---

## 8. Default policy (until benches exist)

1. **Default:** `brush` after COLMAP (or locked poses).  
2. **If poses locked + multi-view + GPU + experimental flag:** try `instant-nurec` for preview layer.  
3. **Never** use generative mesh/GS as world-model geometry.  
4. **Always** run structure stage (ADR-078) separately from train backend choice.  
5. **Expand registry** as models appear; re-verify input contracts (lesson from Instant-NuRec card vs NCore reality).

---

## 9. History

- 2026-07-30: Initial backends design; Instant-NuRec evaluation; link to `~/llm` deep dive and reconstruction registry.
