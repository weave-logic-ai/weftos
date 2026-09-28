# [82] Yang et al. (2025c) — MMSI-Bench: A Benchmark for Multi-Image Spatial Intelligence

**Citation:** S. Yang, R. Xu, Y. Xie, S. Yang, M. Li, J. Lin, C. Zhu, X. Chen, H. Duan, X. Yue, D. Lin, T. Wang, J. Pang. In ICLR 2026.
**Venue/ID:** ICLR 2026; arXiv:2505.23764. https://arxiv.org/abs/2505.23764, code/data: github.com/InternRobotics/MMSI-Bench (formerly OpenRobotLab/MMSI-Bench), project page runsenxu.com/projects/MMSI_Bench

## Summary
MMSI-Bench targets spatial reasoning across *multiple* images — most prior benchmarks only probe single-image spatial relations. It structures questions around three spatial elements (camera, object, region) and their pairwise relations (camera-camera, camera-object, camera-region, object-object, object-region, region-region), plus attribute reasoning (measurement, appearance), motion reasoning (camera motion, object motion), and multi-step reasoning.

## Specifics
- **Task types:** 10 fundamental two-image tasks (six relation types + two attribute + two motion categories) plus a multi-image multi-step reasoning category.
- **Data sources:** drawn from >120,000 source images (exact underlying scene-dataset provenance — e.g. ScanNet/real-world capture — not resolved from the fetched abstract; check the paper for the source dataset list).
- **Size:** 1,000 multiple-choice questions, hand-crafted by 6 3D-vision researchers over 300+ hours, each with designed distractors and a stepwise reasoning rationale.
- **Metrics:** accuracy across 37 evaluated open-source and proprietary MLLMs; an automated error-analysis pipeline classifies failures into 4 modes (grounding errors, overlap-matching/scene-reconstruction errors, situation-transformation reasoning errors, spatial-logic errors) — useful diagnostic beyond raw accuracy.
- **Ground-truth provenance:** expert-authored (6 3D-vision researchers), not crowd-sourced or automatically mined — comparatively high-trust ground truth given the labor investment (300+ hours for 1,000 questions).
- **Known flaws/leakage:** none reported; the small, hand-curated size (1,000 Qs) makes it harder to overfit to via leakage but also a smaller statistical sample than VSI-Bench.
- **License:** CC BY 4.0.

## Key results
Best open-source model ~30% accuracy; GPT-5-class reasoning model ~40%; humans 97% — one of the largest human/model gaps in this cluster, indicating genuine multi-image spatial reasoning is still largely unsolved.

## How Skill-3D uses it
One of Skill-3D's four primary evaluation benchmarks (paper-2606.07436.txt line 474, 1795, and headline result: "lifts Gemini-3-Flash by 67% on MMSI-Bench"). Evaluates the "PR" (positional relationship) subset specifically, per its Table C.3 protocol (30%/70% train/test split per Think3D).

## WeftOS relevance — Verdict: ADOPT
CC BY 4.0 (commercial-safe), expert-authored ground truth, and it is one of Skill-3D's own two headline benchmarks (the 67% Gemini-3-Flash lift is the paper's marquee result) — WeftOS's Rust reimplementation should evaluate on the PR subset at minimum for direct comparability, and ideally the full 10-task suite since multi-image reasoning (camera-camera, object-region relations) is exactly what a multi-view/glasses capture agent needs. The 4-mode error taxonomy is also a good template for WeftOS's own failure-diagnosis logging. See [[papers/benchmarks-and-rl]].
