# [69] Orient Anything v2: Unifying Orientation and Rotation Understanding

**Citation:** Wang et al. (2026). Z. Wang, Z. Zhang, J. Xu, J. Wang, T. Pang, C. Du, H. Zhao, Z. Zhao. "Orient anything v2: unifying orientation and rotation understanding." arXiv preprint arXiv:2601.05573. (NeurIPS 2025 Spotlight)
**URL:** https://arxiv.org/abs/2601.05573 (code: https://github.com/SpatialVision/Orient-Anything-V2, project: https://orient-anythingv2.github.io/)

## Summary
Orient Anything v2 extends the original Orient-Anything model to estimate absolute object orientation, handle rotational symmetries, and predict *relative* rotation between two objects/views. It introduces scalable 3D-asset-synthesized training data and a symmetry-aware, periodic distribution-fitting training objective that captures all plausible front-facing orientations for symmetric objects (rather than forcing one arbitrary canonical front). Reports strong zero-shot results across 11 benchmarks spanning orientation estimation, 6DoF pose, and symmetry detection.

## Architecture / I/O
Built on a **VGGT**-derived foundation (per repo acknowledgements) with a multi-frame architecture that directly predicts relative object rotations across views. Input: RGB image(s) of an object (single or multi-frame). Output: object orientation (azimuth/front-facing direction) as a *distribution* over plausible angles (handling symmetry), plus relative rotation between object instances across views/frames. This is an **angular/rotational** output, not a translation or scale estimate — it does not itself produce metric distances; "orientation" here means facing direction, not position. No claim of metric output at all — orientation angles are inherently scale-free.

## Sizes / Variants
Single released checkpoint (`Viglong/OriAnyV2_ckpt`), ~5.05 GB. No documented small/base/large tiers.

## License — unverified, flag
No explicit LICENSE file or license statement found in the repo README as fetched; the original Orient-Anything (v1) is commonly distributed under Apache-2.0-style terms, but v2's own license was **not confirmed** in this pass — treat as **unverified/assume restrictive until confirmed** given it derives from VGGT (Meta FAIR research code, historically non-commercial-leaning) and depends on FLUX/Hunyuan3D-2.0 assets in its training pipeline (both of which carry their own non-commercial-flavored licenses). Do not adopt commercially without checking the actual LICENSE file at the repo root.

## ONNX / Rust
No official ONNX export found. No `candle`/`burn`/`ort` port found. Being VGGT-derived, any future port would likely need to track a VGGT Rust port first (also not found as of this check).

## Performance / Hardware
Skill-3D measures orientation estimation at **~0.88s per call** in their pipeline (shared 4×RTX PRO 6000 Blackwell setup, not isolated). Supports bfloat16 (compute capability ≥8.0) or float16 fallback per the README, suggesting modest VRAM needs relative to Pi3, but no explicit VRAM figure is published. No CoreML/MPS path verified.

## Use in Skill-3D
Cited at §4.1 (`Wang et al. (2026)`) as the orientation expert. Fig. 4 is explicit that Skill-3D "shifts toward Orient Anything v2" specifically for spatial-relation and direction-reasoning questions, where baseline agents (Think3D, GPT-5.4) instead over-rely on Pi3 or GroundingDINO — this is one of the paper's clearest examples of scene-aware skill routing picking the *right* specialized tool over a generic one.

## WeftOS Relevance
**Verdict: WATCH.** Directly relevant to egocentric glasses capture — knowing which way an object faces (a door, a person, a vehicle) is a natural query for a head-mounted assistant, and the paper's symmetry-aware objective is a real methodological advance over naive single-angle regression. But the license is unverified and the VGGT lineage raises a real risk of hidden non-commercial terms; no Rust path exists yet. Confirm the actual license file before any adoption decision, and treat this as a research-only dependency until that's resolved.
