# [33] Depth Anything 3: Recovering the Visual Space from Any Views

**Citation:** Lin et al. (2025a). H. Lin, S. Chen, J. Liew, D. Y. Chen, Z. Li, G. Shi, J. Feng, B. Kang. "Depth anything 3: recovering the visual space from any views." arXiv preprint arXiv:2511.10647.
**URL:** https://arxiv.org/abs/2511.10647 (code: https://github.com/ByteDance-Seed/Depth-Anything-3, project: https://depth-anything-3.github.io/)

## Summary
DA3 (ByteDance, Nov 2025) predicts spatially consistent geometry from one or more images, with or without known camera poses, using a single plain transformer backbone (a vanilla DINO encoder — no specialized multi-branch architecture) and one unified "depth-ray" prediction target, trained teacher-student on public data. It beats the VGGT baseline on camera pose (+44.3%) and geometric accuracy (+25.1%) and matches DA2's monocular detail/generalization while adding multi-view consistency.

## Architecture / I/O
Input: one or more RGB images (uncalibrated, no pose required). Output: per-pixel depth-ray field, from which depth maps, camera poses, and point clouds are derived. **Metric status — read carefully:** the base DA3 models output *relative/affine* depth-ray fields, not metric depth by default. Metric output requires the dedicated **DA3METRIC-LARGE** variant, which is trained/calibrated to produce metric depth under an assumed camera intrinsics model; accuracy still depends on how well the true focal length matches what the model assumes for an unseen camera (a single head-mounted MentraOS RGB camera would need intrinsics supplied or calibrated, not just "trust the model"). Multi-view point maps are scale-consistent across the input set but the *absolute* scale is only as metric as the depth head used.

## Sizes / Variants
DA3-Small, DA3-Base, DA3-Large, DA3-Giant (main series, unified depth-ray target); plus specialized DA3METRIC-LARGE (metric depth), DA3MONO-LARGE (mono depth), DA3NESTED-GIANT-LARGE. Skill-3D specifically uses "the indoor metric-depth variant" — i.e. DA3METRIC-LARGE or an indoor-finetuned sibling.

## Licenses (per-checkpoint — verify before use)
- **Code:** Apache-2.0 (repo-wide).
- **Weights:** **split by checkpoint** — DA3-SMALL, DA3-BASE, DA3METRIC-LARGE, and DA3MONO-LARGE are Apache-2.0; DA3-GIANT, DA3-LARGE, and DA3NESTED-GIANT-LARGE are **CC BY-NC 4.0 (non-commercial)**. Flag: the metric variant Skill-3D actually uses (metric-large) is Apache-2.0 per this search, but confirm the exact indoor-finetuned checkpoint's license tag on its HF model card before shipping — "Large" in the name does not by itself mean NC; check the specific repo, since DA3-LARGE (non-metric) is NC while DA3METRIC-LARGE is Apache.

## ONNX / Rust
Multiple community ONNX exports exist (`Depth-Anything-3-Onnx` forks, `TillBeemelmanns/Depth-Anything-V3-ONNX` on HF) plus a TensorRT/ROS2 deployment (`ika-rwth-aachen/ros2-depth-anything-v3-trt`) confirming edge deployability. `candle-transformers` already ships **DepthAnythingV2** (not V3) as a first-party model — a DA3 port would extend that existing module rather than start from scratch, since the backbone (DINO-style ViT) is architecturally close.

## Performance / Hardware
Skill-3D measures depth estimation at **~1.51s per call** on their setup (4× RTX PRO 6000 Blackwell, shared load, not an isolated single-GPU number). No official VRAM figures found for DA3 itself. Given ONNX/TensorRT exports exist and DA2 already runs via CoreML/MPS in community projects, Apple Silicon inference is plausible for the Small/Base tiers; Giant-tier metric variants likely need more VRAM/unified memory than a laptop-class GPU comfortably offers for real-time use.

## Use in Skill-3D
Explicitly named in §4.1 as the metric-depth expert (`Lin et al. (2025a)`); Fig. 4 shows Skill-3D "substantially increases the use of Depth Anything 3" specifically for depth/distance/size questions, versus baselines that default to GroundingDINO or Pi3 — DA3 is the tool that "directly provides metric and depth cues."

## WeftOS Relevance
**Verdict: ADOPT (pattern) / PATTERN (Rust port).** This is the closest thing in the cluster to an honest-geometry primitive for a single egocentric RGB camera, provided the metric assumption (known/estimated intrinsics) is made explicit rather than trusted blindly — exactly the "under what assumptions" caveat WeftOS's honest-geometry doctrine requires. Licensing is workable if the metric checkpoint stays on Apache-2.0. No first-party Rust port yet, but `candle-transformers::models::depth_anything_v2` is the nearest scaffold to extend for a native path; short term, ONNX-via-`ort` on the metric-large checkpoint is the lowest-risk route to Apple Silicon inference without a Python sidecar.
