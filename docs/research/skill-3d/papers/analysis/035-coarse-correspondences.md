# 035 — Coarse Correspondences Boost Spatial-Temporal Reasoning in Multimodal Language Model

**Citation:** Liu, B., Dong, Y., Wang, Y., Ma, Z., Tang, Y., Tang, L., Rao, Y., Ma, W.-C., Krishna, R. *Coarse Correspondences Boost Spatial-Temporal Reasoning in Multimodal Language Model.* CVPR 2025, pp. 3783–3792.

**arXiv:** [2408.00754](https://arxiv.org/abs/2408.00754) · CVPR 2025 (openaccess.thecvf.com) · authors from UW, Tsinghua, Tencent, DeepMind, AI2, Cornell.

## Summary
A training-free visual-prompting technique: a lightweight tracker finds coarse object correspondences across video frames or multi-view images, and those correspondences are drawn directly onto the 2D images (markers/IDs) before feeding them to an off-the-shelf MLLM (GPT-4V/GPT-4o or open models). No architecture change, no fine-tuning — the "3D-ish" signal is injected purely through prompting.

## Method specifics
- **Representation:** 2D images + a lightweight correspondence/tracking model (not full 3D reconstruction). No point cloud, no depth map, no BEV — object identity across frames is the only structure added.
- **Metric scale:** none. This is explicitly a 2D-only method; there is no camera pose, no depth, no metric geometry. Purely appearance/track-based correspondence.

## Key results
- GPT-4V/4o gains: **+20.5%** ScanQA, **+9.7%** OpenEQA episodic-memory subset, **+6.0%** EgoSchema (long video), **+11%** R2R navigation (numbers as reported in search summaries of the CVPR paper — verify against PDF before citing precisely).
- Open-source MLLMs: **+6.9%** ScanQA; generalizes to SQA3D (+3.1%).

## Code / license
Project likely has code (CVPR papers of this type usually do) — **not verified**; not fetched directly. Treat as "not found" until confirmed.

## Skill-3D relation
Cited in the §2.1 related-work sentence "driven by stronger backbones ... Liu et al. (2025a) ... Lee et al. (2025b)" — grouped with methods that improve MLLM spatial-temporal reasoning via better prompting/visual signal rather than 3D reconstruction or fine-tuning. It is the training-free / visual-prompting precedent for Skill-3D's own tool-use loop (different mechanism, same "give the MLLM better visual evidence" goal).

## WeftOS relevance
No 3D geometry to adopt — this is a 2D correspondence trick riding on top of a frozen VLM, closest analog to prompting-time context assembly (MetaHarness contextBuilder), not to Urth/BVH. Track markers are like a cheap coarse tracking prior that could inform egocentric MentraOS video summarization before any 3D fusion, but it produces no metric or structural artifact worth storing.

**Verdict: WATCH** — interesting as a cheap pre-fusion visual-prompting trick for agent context, not adoptable at the geometry layer.
