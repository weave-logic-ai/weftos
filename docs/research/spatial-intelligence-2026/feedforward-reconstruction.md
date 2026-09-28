# Feed-forward reconstruction vs COLMAP — VGGT, MASt3R, SpatialLM

**Date:** 2026-09-21  
**Status:** Session research capture (not an ADR)  
**Priority:** A (apply / steal a pattern) — survey ID **S3**  
**Companions:** [README.md](./README.md), [ADR-078](../../adr/adr-078-splat-feeds-world-model.md), [splat-to-world-model.md](../../weftos/splat-to-world-model.md), [splat-pipeline-design.md](../../weftos/splat-pipeline-design.md), [splat-train-backends.md](../../weftos/splat-train-backends.md)

**Honesty rule (same as the survey):** feed-forward cameras, depths, and layout boxes are **reconstruction proposals**. They do not replace BVH leaves + chain. Generative fill stays cosmetic (ADR-079). Live BVH insert is still deferred (`bvh_published: false`).

---

## WeftOS splat pipeline today

Landed path (splatd / `clawft-splat-pipeline`):

```
capture → frames → COLMAP (or glomap mapper) → Brush → splat.ply / splat.sog → package
```

| Stage | Tool | Output |
|---|---|---|
| 1 probe | ffprobe | duration / size |
| 2 frames | ffmpeg (or image-set copy) | `dataset/images/*.jpg` |
| 3 **sfm** | COLMAP feature + matcher + mapper (`stages.rs` `sfm`) | `dataset/sparse/0` (poses + sparse points) |
| 4 train | Brush | `artifacts/splat.ply` |
| 5 compress | splat-transform | `artifacts/splat.sog` |
| 6 package | `write_world_model` | `world_model.json` + `manifest.json` |

W0/W1 already mint structure **from that sparse cloud** (or ASCII PLY fallback):

- W0 (`WEFT-708`): `SPLAT_SCENE` AABB, `frame: scene_local`, `scale_m: null`
- W1 (`WEFT-709`): RANSAC planes → `WM_SURFACE`; Euclidean clusters → `WM_OBJECT` (`label: unknown`); coarse free-space → `WM_VOLUME`
- Every leaf: `"vector": null` (ADR-088 / `partition.rs`)
- `"bvh_published": false` — splat-pipeline stays free of `clawft-bvh`

Typical host timings in the design doc: SfM ~5 min, train ~15 min on a phone walk of 100–300 frames. COLMAP can also emit **multiple disconnected models**; splatd already keeps only the largest (`keep_largest_sparse_model`) because dual worlds explode the splat.

The 2025–2026 feed-forward line attacks **stage 3 latency** and **W1 layout quality**, not Brush/SOG.

---

## 1. Pose-free / feed-forward vs COLMAP overnight

Classical SfM (COLMAP) is a brittle chain: keypoints → matching → essential matrices → triangulation → bundle adjustment → dense MVS. Each hop adds failure modes (few views, textureless walls, insufficient motion). Overnight is the operator experience on a room capture that does not register.

Feed-forward reconstructors skip that chain. They regress **cameras + geometry in one (or a few) neural passes**, then optionally refine.

### Family map

| Model | Venue | What it predicts in one pass | Multi-view fusion | Metric scale? | COLMAP export? |
|---|---|---|---|---|---|
| **DUSt3R** | CVPR 2024 | Pairwise **pointmaps** + confidence (both maps in view-1 frame) | Global alignment of pointmaps in 3D (not 2D reprojection BA) | Up to scale (normalize by mean distance) | No (poses recovered from maps) |
| **MASt3R** | ECCV 2024 | Metric pointmaps + local descriptors | Sparse global alignment (matches in 3D, then 2D reprojection) | **Yes** (`…_catmlpdpt_metric` ckpt) | Toys: `demo_glomap.py` / kapture; not the main path |
| **MASt3R-SLAM** | CVPR 2025 Highlight | Incremental MASt3R prior | Frontend local fusion + 2nd-order global opt, ~15 FPS | Metric prior, scale still a known inconsistency | Dense cloud + poses (SpatialLM’s RGB-video path) |
| **Fast3R** | CVPR 2025 | Joint local+global pointmaps for **1000+** unposed views | **No** pairwise GA — one transformer pass | Up to scale | Not the WeftOS seam |
| **VGGT** | **CVPR 2025 Best Paper** | Cameras (ext+int), depth, pointmaps, tracks | None required; optional BA | Not guaranteed metric | **Yes** — `demo_colmap.py` → `sparse/{cameras,images,points3D}.bin` for **gsplat** |
| **FastVGGT** | ICLR 2026 | Same as VGGT, token-merged attention | Training-free | Same as VGGT | Yes (added 2025-09-10) |
| **VGGT-Ω** | CVPR 2026 Best Paper Finalist / Oral (2026-05) | Camera + depth (compact targets); registers | Register attention replaces most global attn; **static + dynamic** | Improved cameras (Sintel +77% vs prior best) | Code at facebookresearch/vggt-omega |

### DUSt3R / MASt3R — pointmaps and the global-alignment cost

DUSt3R ([naver/dust3r](https://github.com/naver/dust3r), [arXiv:2312.14132](https://arxiv.org/abs/2312.14132)) casts pairwise reconstruction as **pointmap regression**: a dense \(W \times H \times 3\) field \(X\) with a 1:1 pixel↔3D map, both views expressed in image-1’s frame. No known poses or intrinsics. CroCo-pretrained ViT encoder + cross-attending decoders.

For \(N>2\) images it builds a connectivity graph and **aligns in 3D**, not via classical bundle adjustment. The paper’s cost (Eq. 5) is a confidence-weighted 3D residual:

\[
\chi^{*} = \arg\min_{\chi,P,\sigma}
\sum_{e \in \mathcal{E}} \sum_{v \in e} \sum_{i}
C^{v,e}_{i}\,\bigl\| \chi^{v}_{i} - \sigma_{e} P_{e} X^{v,e}_{i} \bigr\|
\]

with \(\prod_e \sigma_e = 1\) to kill the trivial \(\sigma=0\) collapse. That is the “global alignment cost”: **rigid+scale alignment of pairwise pointmaps into a shared world**, typically a few hundred GD steps (seconds on a GPU). Pairwise inference is ~40 ms/pair on H100.

**MASt3R** ([naver/mast3r](https://github.com/naver/mast3r), [arXiv:2406.09756](https://arxiv.org/abs/2406.09756)) adds a local-feature head and a **metric** checkpoint. MASt3R-SfM (3DV 2025) replaces dense GA with retrieval + **sparse** 3D matching alignment, then a 2D reprojection stage. License is **CC BY-NC-SA 4.0** (non-commercial) — a product blocker for WeftOS shipping weights, not for reading the algebra.

**MASt3R-SLAM** ([rmurai0610/MASt3R-SLAM](https://github.com/rmurai0610/MASt3R-SLAM), [arXiv:2412.12392](https://arxiv.org/abs/2412.12392)) turns the two-view prior into real-time dense SLAM: pointmap matching, local fusion, loop closure, second-order global opt. ~15 FPS, tested on RTX 4090. SpatialLM’s published RGB-video testset is reconstructed with this system. Authors note **scale is often inconsistent across MASt3R predictions** even when some train data is metric — IMU / yardstick still matter.

### VGGT — feed-forward cameras, depth, pointmaps, tracks

VGGT ([facebookresearch/vggt](https://github.com/facebookresearch/vggt), [arXiv:2503.11651](https://arxiv.org/abs/2503.11651), project [vgg-t.github.io](https://vgg-t.github.io/)) is a **1B-parameter** transformer that infers, from 1 / few / hundreds of views, **in under a second**:

- extrinsic + intrinsic cameras (OpenCV, camera-from-world)
- depth maps + confidence
- point maps + confidence (authors recommend **unprojecting depth with cameras** over the point-map branch)
- 3D point tracks (query points → track_head)

No pose input. Single-view reconstruction works (never trained for it; competitive with DepthAnything v2 / MoGe on informal tests). Downstream: feature backbone for non-rigid tracking and feed-forward NVS.

**COLMAP export is the WeftOS-shaped gift.** `demo_colmap.py --scene_dir=…` (optional `--use_ba`) writes the exact Brush/gsplat layout:

```
SCENE_DIR/
├── images/
└── sparse/
    ├── cameras.bin
    ├── images.bin
    └── points3D.bin
```

That is `dataset/sparse/0` after a rename. Official integration target is [gsplat](https://github.com/nerfstudio-project/gsplat) (`simple_trainer.py`); Brush consumes the same COLMAP dataset. Optional BA trades robustness for extra GPU time (`max_query_pts`, `query_frame_num`).

License split (2025-07-29): **code is commercial-friendly**; only checkpoint **[VGGT-1B-Commercial](https://huggingface.co/facebook/VGGT-1B-Commercial)** is licensed for commercial use (LLaMA-style access form; military excluded). Original `facebook/VGGT-1B` remains non-commercial. Co3D AUC@30 ~90.37 vs 89.98 — treat them as interchangeable quality.

2026-05-15 memory fix: redundant intermediate tensors dropped → **~2–3× more frames** in the same VRAM.

### VGGT-Ω (2026-05) — registers, static+dynamic, VLA proxy

[VGGT-Ω](https://vggt-omega.github.io/) ([arXiv:2605.15195](https://arxiv.org/abs/2605.15195), code [facebookresearch/vggt-omega](https://github.com/facebookresearch/vggt-omega), HF [facebook/VGGT-Omega](https://huggingface.co/facebook/VGGT-Omega)):

- Single dense prediction head, multi-task supervision; **drop expensive high-res convs**.
- **Registers** hold a compact scene summary; **register attention** restricts inter-frame exchange to those tokens (partial replacement of global attention).
- Training VRAM **~30% of VGGT** → 15× more supervised data + unlabeled video.
- **Static and dynamic** scenes. Camera accuracy on Sintel **+77%** vs previous best.
- Authors’ takeaway: **cameras + depth are sufficient compact reconstruction targets**; reconstruction is a **scalable proxy for spatial understanding**.
- Registers improve **VLA** models and can be aligned with language — LeWM-adjacent, **not** a BVH replacement (ADR-090).
- 2026-09-18: training code + a second 1B checkpoint (`vggt_omega_1b_416_reproduce.pt`) as the comparison reference.

### Fast3R / FastVGGT

**Fast3R** ([facebookresearch/fast3r](https://github.com/facebookresearch/fast3r), [arXiv:2501.13928](https://arxiv.org/abs/2501.13928), CVPR 2025): DUSt3R without the pairwise+GA bottleneck. One pass over 1000+ unposed images; train-short/test-long via randomized positional indices (train 20 views → infer 1000+). ~251 FPS at 108×224; up to ~1500 views on one A100 (DUSt3R OOMs past ~32). **Repo archived 2026-07-01 (read-only).** Do not productize; steal the “one pass, no GA” idea.

**FastVGGT** ([mystorm16/FastVGGT](https://github.com/mystorm16/FastVGGT), [arXiv:2509.02560](https://arxiv.org/abs/2509.02560), **ICLR 2026**): **training-free** token merging on VGGT attention (ToMe-style, 3D-aware partition). **~4× faster at 1000 images**, less long-sequence error accumulation. COLMAP writer added 2025-09-10 (`eval/eval_custom_colmap.py`). Experiments on A800 80 GiB. Sibling to watch: InfiniteVGGT (endless streams).

### What this is **not**

- Not a Brush replacement. Appearance train still wants poses + a COLMAP-shaped dataset.
- Not metric truth. VGGT/Fast3R/DUSt3R are **up-to-scale** unless a yardstick, IMU gravity, stereo/ToF, or MASt3R-metric + known calib is applied.
- Not a reason to delete COLMAP. Hard indoor loops, mixed cameras, and dual-world reject still want a classical mapper or BA as a **fallback / verifier**.

---

## 2. SpatialLM as W1 layout proposer (`WM_SURFACE` / `WM_OBJECT`, `vector: None`)

[SpatialLM](https://github.com/manycore-research/SpatialLM) (NeurIPS 2025, [arXiv:2506.07491](https://arxiv.org/abs/2506.07491), site [manycore-research.github.io/SpatialLM](https://manycore-research.github.io/SpatialLM)) is a small multimodal LLM that **reads a point cloud and emits structured indoor layout**: walls, doors, windows, and **oriented object boxes** with semantic class.

It is the closest published analog to ADR-078 W1 “geometric partition → room shell + blobs,” except it also labels architecture and furniture.

### Inputs / outputs

- **In:** axis-aligned point cloud, **z-up**. Authors reconstruct RGB video with **MASt3R-SLAM** (testset of 107 noisy indoor clouds). Also accepts RGB-D and LiDAR.
- **Out:** layout text → walls/doors/windows + OBBs. SpatialLM1.1 tasks:
  - Structured reconstruction (walls+doors+windows+boxes)
  - Layout estimation (architecture only)
  - 3D object detection (boxes; user-specified subset of **59** furniture classes)

Models (HF): `SpatialLM1.1-Llama-1B`, `SpatialLM1.1-Qwen-0.5B` (1.1 doubles resolution, Sonata encoder, category-conditioned detect). 1.0 used SceneScript encoder.

### Numbers that matter for W1

| Task | Result |
|---|---|
| Layout on Structured3D (finetuned 1.1-Qwen-0.5B) | F1 @.25 IoU **94.3**, @.5 **93.5** (vs RoomFormer 83.4 / 81.4) |
| ScanNet 18-class det (finetuned) | F1 @.25 **65.6** (vs V-DETR 65.1) |
| Zero-shot on MASt3R-SLAM video clouds | Bed ~96 F1 @.25; chair ~21–32; cabinet ~11–15 — **large objects work, clutter does not** |

Train set: 12,328 synthetic indoor scenes / 54,778 rooms (professional 3D designers).

### Mapping onto WeftOS leaves

Do **not** write SpatialLM boxes into BVH as truth. Mint them as **W1 proposals** in `world_model.json`, same schema WEFT-709 already emits:

| SpatialLM | WeftOS leaf | Notes |
|---|---|---|
| Wall / floor / ceiling plane | `WM_SURFACE` `tag_u32=0x5350_0011` | `label` = wall/door/window; keep `normal` + thin AABB |
| Door / window | `WM_SURFACE` or `WM_OBJECT` | Door as object if it needs instance_id later (W5) |
| Furniture OBB | `WM_OBJECT` `tag_u32=0x5350_0010` | `label` = class; `confidence` from model; convert OBB → AABB for v1 broad-phase |
| (none) | `WM_VOLUME` | Still from residual free-space (W1/W2), not SpatialLM |
| (none) | `vector` | **Always `null` / `None`** (ADR-088). Class names are strings, not HNSW ids. Open-vocab embeddings are a later `VectorRef` fill (scene-graphs deep dive), not this ticket. |

Keep the current RANSAC/cluster path as **baseline**. SpatialLM is an **optional proposer** that can:

1. Replace unknown clusters with labeled boxes when IoU vs RANSAC shell is sane.
2. Propose walls the RANSAC miss (thin, occluded).
3. Fail closed: if the LLM emits garbage, fall back to WEFT-709 unlabeled blobs.

Human confirm in Agent Workspace remains the high-trust path (splat-to-world-model §4.2). W3 (2D SAM/YOLO → project) stays later; SpatialLM skips 2D masks and goes cloud→layout.

**License caution:** Llama 3.2 for the Llama variant; Qwen 2.5 is Apache-2.0; **Sonata / SceneScript weights are CC-BY-NC-4.0**. For a product adapter prefer **1.1-Qwen-0.5B** and treat NC encoder weights as research-only until counsel says otherwise.

---

## 3. Exact pipeline insertion points

Two seams. Do not invent a third daemon.

```
capture session (RGB ± IMU/ToF)
        │
        ▼
   [A] OPTIONAL feed-forward SfM     ← BEFORE COLMAP
        VGGT / FastVGGT / VGGT-Ω
        (or MASt3R-SLAM on video)
        │  writes dataset/sparse/0  (COLMAP bins)
        │  fallback: existing colmap/glomap mapper
        ▼
   Brush train → splat.ply → SOG     ← unchanged
        │
        ▼
   load_scene_points()               ← AFTER SPARSE CLOUD
        COLMAP points3D  else  splat.ply
        │
        ├─ WEFT-709 RANSAC/cluster   (always, baseline)
        └─ [B] OPTIONAL SpatialLM    (proposer)
        │
        ▼
   world_model.json
        bvh_published: false
        vector: null
        frame: scene_local  (or scene_gravity_aligned after 5a)
```

### A — before COLMAP (replace or seed stage 3)

**File:** `crates/clawft-splat-pipeline/src/stages.rs` `sfm()`.  
**Disk contract:** Brush already requires `dataset/images/` + `dataset/sparse/0`. VGGT `demo_colmap.py` already writes that.

Config sketch (do not implement here):

```toml
[sfm]
backend = "colmap"          # default
# backend = "vggt"           # feed-forward → COLMAP bins
# backend = "fastvggt"       # long sequences
# backend = "vggt-omega"     # when CUDA runner exists
# fallback = "colmap"        # if n_registered < threshold
use_ba = false               # VGGT optional bundle adjustment
```

Rules:

1. Run VGGT (or FastVGGT) on `dataset/images/`.
2. Write `sparse/0/{cameras,images,points3D}.bin`.
3. Reuse `keep_largest_sparse_model` + `model_analyzer` counts (`n_registered`).
4. If registration fraction is poor, **fall back to COLMAP/glomap** — do not ship dual worlds.
5. Known-pose sessions (multi-cam rig, ADR-077 IMU poses in `poses.jsonl`) should **skip** feed-forward cameras and only use the network for dense cloud / depth if at all.

**MASt3R-SLAM** is the other before-COLMAP option, aimed at **video / live capture**, not still-set SfM: it can emit a dense metric-ish cloud + poses while the operator walks. That cloud is SpatialLM’s native input; poses can still be converted to COLMAP for Brush.

GLOMAP stays the cheap classical speedup (already wired). Feed-forward is the GPU alternative, not a GLOMAP killer.

### B — after sparse cloud (W1 proposer)

**Files:** `world_model.rs` `load_scene_points` / `write_world_model`; `partition.rs` `partition_to_json_leaves`.

Today W1 always runs RANSAC+cluster on COLMAP points (sparse) or PLY (dense). Insertion:

1. **Gravity / z-up.** SpatialLM **requires** z-up axis-aligned clouds. Stage 5a in splat-to-world-model (`ground / gravity align` via IMU or yardstick) must run **before** SpatialLM. Until then, skip the proposer rather than emit rotated walls.
2. Export a downsampled PLY of `load_scene_points()` (or MASt3R-SLAM dense cloud).
3. Run `inference.py --point_cloud … --model_path SpatialLM1.1-Qwen-0.5B`.
4. Parse layout → `WM_SURFACE` / `WM_OBJECT` JSON with `"vector": null`.
5. **Compose** with RANSAC: union of surfaces; objects only if they sit on the floor plane and do not contradict the scene AABB.
6. Package still writes `bvh_published: false`. Publish remains a later daemon path (research queue item 1).

Do **not** put SpatialLM inside Brush. Structure is async after SOG (ADR-078 §2).

### What not to insert

| Temptation | Why not |
|---|---|
| Feed-forward Gaussians as train backend | Different family (splat-train-backends). VGGT is **SfM**, not a splat network. |
| Registers → LeWM / ECC | ADR-090: latent codes compose later; ECC stays authority. |
| SpatialLM class strings → HNSW | ADR-088: `VectorRef` is a handle, default None. Scene-graph embeddings are S4, not S3. |
| Replace W1 RANSAC | SpatialLM fails on chairs/cabinets in zero-shot video noise. Keep unlabeled blobs. |

---

## 4. What stays metric vs generative

ADR-078 dual output + ADR-079 sparse-first: **measurement is the world model; generation is makeup.**

| Signal | Status | Source of truth |
|---|---|---|
| **Camera stats** (intrinsics, `T_world_cam`, timestamps) | **Metric** (or known-unknown) | Capture protocol / IMU VIO / VGGT+BA / COLMAP. Free-form quilt already requires this ([splat-freeform-quilt.md](../../weftos/splat-freeform-quilt.md)). |
| **Gravity / z-up** | **Metric** | IMU; else door-height / yardstick (`SPLAT_YARDSTICK`). SpatialLM will not run until aligned. |
| **Scene AABB** | **Metric in `scene_local`** | `load_scene_points` min/max. Scale may be unknown (`scale_m: null` today). |
| **W1 RANSAC planes / clusters** | Geometric, **not** semantic | WEFT-709. class = `unknown`. |
| **VGGT / DUSt3R pointmaps** | Reconstruction, **up to scale** | Use as SfM seed. Do not treat depths as survey. |
| **MASt3R metric ckpt / MASt3R-SLAM** | **More metric**, still drift | Prefer when we need SpatialLM input from video. Still fuse IMU. |
| **SpatialLM walls / boxes** | **Proposal** (learned layout) | Mint leaves with `confidence`; require overlap with metric AABB. Not chain-authoritative until human/policy confirm (W3/W5). |
| **Brush / SOG appearance** | Photometric, not occupancy | Viewer only. |
| **Generative fill / Marble-style completion** | **Cosmetic, labeled non-metric** | ADR-079 §2. Never write as `WM_VOLUME` occupied. |
| **VGGT-Ω registers / VLA features** | Latent | Optional HNSW `index_id` later. Never SoT for “what is where.” |

`scale_m` in `world_model.json` should stay `null` until a real scale anchor exists. Feed-forward SfM does not flip that bit by itself.

---

## 5. Apply / compose / watch

| Item | Stance | WeftOS seam |
|---|---|---|
| **VGGT COLMAP export** | **Apply** | Optional `sfm.backend = "vggt"` writing `dataset/sparse/0`; Brush unchanged. Use **VGGT-1B-Commercial** if we ever ship weights. |
| **FastVGGT** | **Apply** (if sequences ≫ 200 frames) | Same export; token merge is a flag, not a new stage. |
| **W1 RANSAC/cluster** | **Keep** | Baseline; never delete. |
| **SpatialLM 1.1-Qwen** | **Compose** | Optional W1 proposer → `WM_SURFACE` / `WM_OBJECT`, `vector: None`. After gravity align. |
| **MASt3R-SLAM** | **Compose** | RGB-video → dense cloud for SpatialLM; poses→COLMAP if Brush still wants them. |
| **VGGT-Ω** | **Watch → apply when runner exists** | Better cameras, dynamic scenes, 30% train VRAM. Registers = VLA/LeWM research, not BVH. |
| **DUSt3R GA cost** | **Watch / borrow algebra** | 3D alignment of pairwise maps is the mental model for “why feed-forward still sometimes optimizes.” Do not ship CC-BY-NC weights. |
| **Fast3R** | **Watch only** | Archived 2026-07-01. Idea absorbed by VGGT-Ω / FastVGGT. |
| **InfiniteVGGT / StreamVGGT** | **Watch** | Endless capture streams; quilt, not v1 splatd. |
| **VGGT-Ω as VLA proxy** | **Watch** | Reconstruction-as-pretrain for agents. ADR-090 compose-later. |

---

## 6. GPU / memory caveats

splatd today: **COLMAP on CPU**, Brush on **Metal (M5 Pro)** or wgpu Vulkan. Feed-forward SfM is a **CUDA (or future MPS) sidecar**, not an in-process crate. Kernel still must not link these models (ADR-096 removable; splatd is already a separate process).

| System | Practical box | Notes |
|---|---|---|
| VGGT-1B | NVIDIA Ampere+ (`bfloat16` CC≥8 else `fp16`) | Reconstruct **<1 s**; viser/gradio viz is the slow part. 2026-05 fix: 2–3× more frames / same VRAM. Hundreds of views, not thousands, without FastVGGT/Ω. |
| FastVGGT @ 1000 imgs | A800 **80 GiB** in the paper | 4× vs VGGT; π³ / StreamVGGT OOM at that length. |
| VGGT-Ω | Lower **train** VRAM (~30% of VGGT); inference still a 1B dense model | Dynamic video is the point; still not phone-side. |
| Fast3R | A100, up to ~1500 views | Archived; ignore for procurement. |
| DUSt3R / MASt3R | CUDA 12.x, pairwise then GA | GA memory grows with pairs; MASt3R sparse GA is the scalable one. **NC license.** |
| MASt3R-SLAM | **RTX 4090** class; 15 FPS | Repro note: multiprocessing / shared memory issues on WSL. |
| SpatialLM1.1 | CUDA 12.4, PyTorch 2.4, Python 3.11 | Sonata path builds **flash-attn** (slow). Qwen-0.5B is the small inference target. Cloud is noisy RGB-video; do not expect ScanNet-clean F1. |
| Brush (unchanged) | Metal / Vulkan | Still the appearance train. Do not steal its GPU mid-job; serial splatd queue already exists. |

Operational:

- One **GPU runner** behind splatd (same pattern as Shasta `SPLAT_RUNNER_URL`) — do not glob Forge PC or start a second process-compose (ADR-098).
- Peak VRAM belongs in job metrics (`peak_vram_mb`) next to `n_registered`.
- Phone / Android edge (ADR-077) stays **capture**; reconstruction stays Mac/cloud.
- If CUDA is missing, `sfm.backend` stays `colmap`. Graceful degrade, no hard fail of the appearance path.

---

## 7. Sources + recommended tickets (do not file)

### Sources (fetched this session)

- VGGT code: <https://github.com/facebookresearch/vggt> · paper [arXiv:2503.11651](https://arxiv.org/abs/2503.11651) · COLMAP/gsplat export in README (2025-06-02)
- VGGT-Ω: <https://vggt-omega.github.io/> · [arXiv:2605.15195](https://arxiv.org/abs/2605.15195) (2026-05-14) · [facebookresearch/vggt-omega](https://github.com/facebookresearch/vggt-omega) · training+repro ckpt 2026-09-18
- DUSt3R: <https://github.com/naver/dust3r> · [arXiv:2312.14132](https://arxiv.org/abs/2312.14132) · GA cost Eq. 5
- MASt3R: <https://github.com/naver/mast3r> · [arXiv:2406.09756](https://arxiv.org/abs/2406.09756) · metric ckpt `MASt3R_ViTLarge_BaseDecoder_512_catmlpdpt_metric`
- MASt3R-SLAM: <https://github.com/rmurai0610/MASt3R-SLAM> · [arXiv:2412.12392](https://arxiv.org/abs/2412.12392) · CVPR 2025 Highlight
- SpatialLM: <https://github.com/manycore-research/SpatialLM> · [arXiv:2506.07491](https://arxiv.org/abs/2506.07491) · NeurIPS 2025 · HF `SpatialLM1.1-Qwen-0.5B`
- Fast3R: <https://github.com/facebookresearch/fast3r> · [arXiv:2501.13928](https://arxiv.org/abs/2501.13928) · archived 2026-07-01
- FastVGGT: <https://github.com/mystorm16/FastVGGT> · [arXiv:2509.02560](https://arxiv.org/abs/2509.02560) · ICLR 2026 · COLMAP writer 2025-09-10
- WeftOS: ADR-078, ADR-079, ADR-088, `docs/weftos/splat-to-world-model.md`, `crates/clawft-splat-pipeline/{stages,world_model,partition}.rs`

### Recommended Plane tickets (research only — **do not file** unless asked)

Plane currently has **no open spatial tickets**. These would be 0.8.x+ research/impl, not 0.7 must-ship.

| Suggested title | Cycle | Acceptance (sketch) | Depends |
|---|---|---|---|
| **Optional VGGT SfM backend** writing COLMAP `sparse/0` | 0.8.x | `sfm.backend=vggt` produces Brush-loadable bins; `n_registered` metric; fallback to COLMAP; commercial ckpt documented | splatd GPU runner; WEFT-708 layout |
| **FastVGGT flag for long image-sets** | 0.8.x | 500–1000 frames without OOM; same COLMAP contract | VGGT backend |
| **SpatialLM W1 proposer** → `WM_SURFACE` / `WM_OBJECT`, `vector: null` | 0.8.x | Layout leaves in `world_model.json`; RANSAC baseline still runs; `bvh_published` stays false | Gravity align; point-cloud PLY export |
| **Gravity / z-up stage 5a** before layout LLM | 0.8.x | `frame: scene_gravity_aligned` when IMU/yardstick present; SpatialLM skipped otherwise | Capture IMU (ADR-077) |
| **MASt3R-SLAM video path** as dense-cloud producer | later | RGB video → PLY for SpatialLM; optional pose dump | NC-license review |
| **VGGT-Ω eval spike** (static+dynamic, registers) | later | Receipt: camera error vs VGGT on one WeftOS room capture; no promote | GPU runner |
| **License/counsel note** on VGGT-Commercial vs MASt3R NC vs SpatialLM Sonata NC | 0.8.x | Written constraint in splat-train-backends / this folder | — |

Non-goals for those tickets: live BVH publish, VectorRef fill, LeWM from registers, deleting COLMAP, phone-side 1B inference.

---

## WeftOS seam (one paragraph)

Stage 3 becomes **pose-free feed-forward with a COLMAP-shaped socket** so Brush and W0 AABB keep working; stage 6 W1 gains an **optional layout LLM** that proposes the same `WM_*` leaves we already serialize with `vector: null` and `bvh_published: false`. Cameras, gravity, and AABBs stay metric (or honestly unscaled). SpatialLM boxes and VGGT-Ω registers do not. COLMAP overnight remains the CPU fallback, not the product identity.
