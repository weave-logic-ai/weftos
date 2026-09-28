# Cluster Synthesis: Expert Tool Models & Frontier MLLMs (Skill-3D refs 4, 12, 19, 23, 31, 33, 40, 45, 48, 66, 69)

Sources: see per-reference files in `analysis/004-*.md` through `analysis/069-*.md`. All tool-cost timings below are Skill-3D's own measurements (Appendix B), on 4× NVIDIA RTX PRO 6000 Blackwell, shared across a full agent rollout — not isolated single-tool benchmarks. Treat them as directional, not as ground truth for a different GPU or Apple Silicon.

## 1. Per-expert Rust inference feasibility

| Expert | Ref | Output type | Rust path today | Evidence |
|---|---|---|---|---|
| GroundingDINO | 40 | 2D open-vocab boxes | **`ort` (ONNX) now; native `candle` assembly feasible** — DETR-family + BERT already exist separately in `candle-transformers` | mature community/official ONNX exports; Apache-2.0 |
| Depth Anything 3 (metric variant) | 33 | metric-ish depth-ray, from assumed intrinsics | **`ort` (ONNX) now; `candle-transformers::models::depth_anything_v2` is a near scaffold** for a DA3 port (same DINO-ViT backbone family) | community ONNX exports (`TillBeemelmanns/Depth-Anything-V3-ONNX`), TensorRT/ROS2 deployment exists |
| SAM 3 | 4 | open-vocab instance masks + video tracking | **Python sidecar only** — `candle-transformers` has SAM v1, not SAM 3's concept-prompt/tracker head; no ONNX export found | no port evidence found |
| Pi3 / Pi3X | 66 | multi-view points+poses, scale-ambiguous | **Python sidecar only** — no ONNX/candle/burn port found; also NC-licensed weights (see below), so porting effort is lower priority anyway | no port evidence found |
| Orient Anything v2 | 69 | orientation angle distribution | **Python sidecar only** — VGGT-derived, no ONNX/candle/burn port, and VGGT itself has no known Rust port | no port evidence found |
| SwinIR | 31 | 2D image restoration | **`ort` plausible (community ONNX exports exist, unverified parity); native `candle` buildable** — Swin-Transformer blocks are a known, implementable pattern, just not pre-packaged for SwinIR specifically | Apache-2.0, small param count |

**Bottom line:** GroundingDINO and DA3-metric are the two experts with a realistic near-term Rust/edge path (`ort` today, native `candle` as a stretch goal reusing existing modules). SAM 3, Pi3, and Orient-Anything-v2 are Python-sidecar-only until someone ships ONNX weights; none of the three currently have any Rust port evidence. SwinIR is low-priority (2D preprocessing, not core geometry) but cheap to port if needed.

## 2. License matrix (verify before shipping — this is a snapshot, not legal advice)

| Model | Code license | Weights license | Commercial-safe? |
|---|---|---|---|
| GroundingDINO (40) | Apache-2.0 | Apache-2.0 | **Yes** |
| Depth Anything 3 — Small/Base/**Metric-Large**/Mono-Large (33) | Apache-2.0 | Apache-2.0 | **Yes**, for those specific checkpoints |
| Depth Anything 3 — Large/Giant/Nested-Giant-Large (33) | Apache-2.0 | **CC BY-NC 4.0** | No (non-commercial only) — confirm the exact checkpoint tag before use; don't assume "Large" ≠ metric-large |
| SwinIR (31) | Apache-2.0 | Apache-2.0 (same repo) | **Yes** |
| SAM 3 (4) | custom "SAM License" (Meta) | custom "SAM License" — non-OSI, field-of-use restrictions, mandatory attribution/redistribution terms | **Conditional** — generally commercial-permitting but read the license file directly; not a clean Apache/MIT |
| Pi3 / Pi3X (66) | BSD 3-Clause | **CC BY-NC 4.0** | **No** — weights are explicitly non-commercial research/education only |
| Orient Anything v2 (69) | **unverified** | **unverified** — no LICENSE file found; VGGT/FLUX/Hunyuan3D-2.0 lineage in training pipeline raises real risk of inherited NC terms | **Unknown — do not assume commercial-safe** |
| Qwen3-VL 4B/8B/30B-A3B/235B-A22B (48) | Apache-2.0 | Apache-2.0 | **Yes** |
| Gemini 2.5 / Gemini 3 / GPT-4o / GPT-5.4 (12,19,23,45) | closed, API-only | closed, API-only | N/A — commercial API terms, no local weights at all |

**Two hard blockers for a commercial WeftOS deployment:** Pi3's NC weight license, and Orient-Anything-v2's unverified (likely-restrictive-by-lineage) license. Everything else in the expert cluster is either clean Apache-2.0 or a workable custom license (SAM 3) that just needs a real legal read before shipping.

## 3. Honest-geometry status of each expert output

WeftOS's honest-geometry doctrine requires every "metric" claim to state its assumptions. Status per expert:

- **DA3METRIC-LARGE (33):** *Conditionally metric.* Requires an assumed/estimated camera intrinsics model; a single unknown MentraOS camera needs calibration or intrinsics supplied, not blind trust. The non-metric DA3 variants (Small/Base/Large/Giant) are relative depth-ray only — never metric.
- **Pi3 / Pi3X (66):** *Not metric by default.* Base Pi3 outputs affine-invariant poses / scale-invariant point maps — explicitly "up to a similarity transform." Pi3X can be metric *only* when externally conditioned on known poses/intrinsics/depth; without that anchor it is still scale-ambiguous. Never treat raw Pi3 output as metric.
- **GroundingDINO (40):** *Not geometric at all* — 2D bounding boxes only, no depth/scale claim of any kind.
- **SAM 3 (4):** *Not geometric at all* — 2D/video masks + tracking, no depth/scale claim.
- **Orient Anything v2 (69):** *Not metric, and not translation/position either* — angular/rotational output only (facing direction, relative rotation), inherently scale-free by construction.
- **SwinIR (31):** *Not geometric* — image-to-image restoration, purely 2D pixel-space.

**Only one of the six experts (DA3, specifically the metric checkpoint) makes any metric claim at all, and even that claim is conditional on intrinsics.** A WeftOS implementation must treat DA3-metric's output as "metric under assumed/calibrated intrinsics," and treat every other expert's output as relative, 2D, or angular — this is the single most important honest-geometry finding from this cluster.

## 4. Which experts are needed for egocentric glasses capture (single head-mounted RGB camera)

MentraOS context: one RGB camera, no depth sensor, no known extrinsics between frames beyond what the wearer's head motion provides. Given that constraint:

- **DA3METRIC-LARGE** is the only source of any per-pixel distance estimate, and only under an intrinsics assumption calibratable from the specific MentraOS camera model (fixed lens = fixed, cacheable intrinsics — a real practical win for egocentric capture vs. a general "any camera" system).
- **GroundingDINO** is directly useful for voice-directed "find X" queries against a single frame — cheap, fast, commercially clean, and the most Rust-ready expert in the set.
- **Pi3** is the multi-view option for when the wearer's head motion produces a usable image set, but its ~21s/7-frame cost (vs. ~0.77–1.51s for the single-frame experts) makes it a poor fit for anything real-time, and its NC license blocks commercial shipping regardless. If multi-view reconstruction is needed commercially, DA3's multi-view mode (part of the same unified model, Apache-2.0 on the right checkpoints) is the license-clean substitute to evaluate first.
- **SAM 3** adds instance-level segmentation/tracking on top of GroundingDINO's boxes — useful for "keep tracking that object as I move," but Python-sidecar-only today and license-conditional.
- **Orient Anything v2** answers "which way is that object facing," relevant for glasses-based assistance (doors, vehicles, people) but blocked on license verification.
- **SwinIR** is optional preprocessing for a noisy/low-light egocentric feed; lowest priority, easily deferred or substituted.

**Practical minimum viable egocentric stack, license-clean today: GroundingDINO + DA3METRIC-LARGE.** Both Apache-2.0, both have an ONNX path, and together they cover "what is it / where roughly is it (2D) / how far is it (metric, conditional)" — the core spatial primitives for a single-camera assistant. Everything else in the cluster is an enhancement layered on top, gated by license confirmation or a Python sidecar boundary.

## 5. Model choice: agent and SFT/GRPO base, given licenses

- **SFT/GRPO base model: Qwen3-VL-4B or Qwen3-VL-8B (48).** Apache-2.0, open weights, and this is literally the recipe Skill-3D itself validates: 500 SFT + 1k GRPO samples, one epoch, 4 GPUs, ~3h SFT + ~28h GRPO, yielding a 59.7–60.3% relative VSI-Bench gain. 4B is the more edge-plausible size for eventual WeftOS on-device or modest-remote-GPU deployment; 8B is the stronger ceiling if remote GPU budget allows (Skill-3D's own results show 8B outperforming 4B at every VSI-Bench category post-training). No larger Qwen3-VL tier (30B-A3B, 235B-A22B) was used by Skill-3D and neither is a near-term edge target.
- **Teacher / distillation source: GPT-5.4 (45), as Skill-3D itself uses.** Closed, API-only, cloud-side-only role — used exclusively at training time for skill distillation and SFT data generation, never shipped. This is a clean split: open Apache-2.0 student model for the shipped artifact, closed API teacher only in the training pipeline, matching Skill-3D's own design and avoiding any weight-license entanglement with the closed frontier models.
- **Cloud agent backbone (optional, if a hosted-reasoning tier is wanted alongside the local Qwen3-VL agent):** Gemini 3 (19) is the current frontier choice over Gemini 2.5 (12) or GPT-4o (23) — it's the newest, and it's also the model on which Skill-3D reports its single largest gain (67% on MMSI-Bench), suggesting skill-routing compounds well even on top of an already-strong backbone. GPT-4o is the oldest/weakest baseline in the paper's own comparison and not a serious candidate for a new design.

## Verdicts summary

| Ref | Item | Verdict |
|---|---|---|
| 4 | SAM 3 | WATCH |
| 12 | Gemini 2.5 | WATCH |
| 19 | Gemini 3 | WATCH |
| 23 | GPT-4o | SKIP |
| 31 | SwinIR | PATTERN |
| 33 | Depth Anything 3 | ADOPT (metric ckpt) / PATTERN (Rust port) |
| 40 | Grounding DINO | ADOPT |
| 45 | GPT-5.4 | WATCH (as cloud teacher) |
| 48 | Qwen3-VL | ADOPT |
| 66 | Pi3 | SKIP (shipped) / PATTERN (architecture) |
| 69 | Orient Anything v2 | WATCH (license unverified) |

## Unverified / flagged for follow-up
- Orient Anything v2's actual license file (not found; assume restrictive until checked).
- Exact license tag on Skill-3D's specific "indoor metric-depth variant" of DA3 (confirm it's the Apache-2.0 metric-large checkpoint, not a Large/Giant NC variant with a similar name).
- SAM 3's full license text (custom terms summarized from secondary sources, not read verbatim here).
- No VRAM figures were found published by any of the six expert-model authors; all latency/cost figures in this synthesis are Skill-3D's own shared-GPU measurements, not isolated benchmarks, and no Apple Silicon (MPS/CoreML) benchmark exists for any of the six experts — feasibility claims above are architectural plausibility, not measured results.
