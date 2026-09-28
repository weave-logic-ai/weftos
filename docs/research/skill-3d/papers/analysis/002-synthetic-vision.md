# [002] Synthetic Vision: Training Vision-Language Models to Understand Physics

**Citation:** V. Balazadeh, M. Ataei, H. Cheong, A. Hosein Khasahmadi, and R. G. Krishnan. *Synthetic vision: training vision-language models to understand physics*. arXiv e-prints, arXiv–2412, 2024.

**arXiv:** [2412.08619](https://arxiv.org/abs/2412.08619) (paper title on arXiv is "Physics Context Builders: A Modular Framework for Physical Reasoning in Vision-Language Models" — this appears to be a retitled/renamed version of the "Synthetic Vision" preprint cited by Skill-3D; content matches: physical reasoning via simulated scene descriptions) · venue not found (arXiv e-print only per refs.tsv)

## Summary
The paper targets physical reasoning in VLMs — predicting object behavior in dynamic scenes (e.g., stability, collisions) — which base VLMs handle poorly. Instead of fine-tuning one large VLM end-to-end, it introduces Physics Context Builders (PCBs): smaller VLMs fine-tuned on synthetic/simulated scene data to produce detailed physical-scene text descriptions, which are then fed as context to a larger, frozen VLM to improve its physical reasoning. This modular split (small specialist perception model → text context → large reasoning model) avoids repeated expensive fine-tuning of the large model.

## Method specifics
Purely 2D image input (simulated and real-world scene photos/renders) — no point cloud, depth map, BEV, or 3D scene graph is used. Physical scene understanding is mediated through natural-language descriptions produced by the PCB, not geometric reconstruction. **Not metric** — there is no depth/3D output at all; the representation is 2D pixels in, physics-relevant text out.

## Key results
Reported: up to **13.8%** accuracy improvement on complex physical-reasoning tasks (exact benchmark condition not fully specified in the retrievable abstract) and demonstrated sim-to-real transfer. Benchmarks used: **CLEVRER** and a custom **Falling Tower** stability dataset (simulated + real scenes). Exact per-benchmark score table: not found in accessible content.

## Code/weights
Not found in accessible content.

## Skill-3D relation
Cited in the §2.1 related-work paragraph's second clause ("Recent methods improve fine-grained spatial understanding by incorporating 3D reconstruction, depth cues, spatial VQA data, and explicit grounding") alongside Cheng 2024 (SpatialRGPT), Chen 2024 (SpatialVLM), Fan 2025b (VLM-3R), Huang 2024 (Chat-Scene), Wang 2023 (Chat-3D), and Zhang 2025a — grouped as prior work that adds structured cues (here: physics/simulation-derived context) to improve spatial/physical VQA. Not used as a direct baseline or component in Skill-3D's method.

## WeftOS relevance
No 3D or geometric representation at all — this is a physics-VQA/context-distillation technique, not a spatial-reconstruction method. It has no direct BVH/HNSW seam. The "small specialist model produces structured context for a larger reasoning model" pattern is a mildly interesting agent-architecture idea (comparable to how WeftOS composes narrow tool outputs into agent context) but is generic, not spatial-specific, and not something to graft onto Urth.

**Verdict: SKIP** — not a 3D/geometric method; the physics-QA angle and PCB modular-context pattern don't map onto Urth's spatial index or MentraOS capture pipeline in any concrete way.
