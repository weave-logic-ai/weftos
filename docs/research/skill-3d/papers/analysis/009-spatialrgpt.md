# [009] SpatialRGPT: Grounded Spatial Reasoning in Vision-Language Models

**Citation:** A. Cheng, H. Yin, Y. Fu, Q. Guo, R. Yang, J. Kautz, X. Wang, and S. Liu. *SpatialRGPT: grounded spatial reasoning in vision-language models*. Advances in Neural Information Processing Systems (NeurIPS) 37, pp. 135062–135093, 2024.

**arXiv:** [2406.01584](https://arxiv.org/abs/2406.01584) · NeurIPS 2024

## Summary
SpatialRGPT (Spatial Region GPT) improves VLM spatial reasoning via two mechanisms: (1) a data-curation pipeline that learns **regional representations from 3D scene graphs**, and (2) a **plugin depth-integration module** that injects depth information into an existing VLM's visual encoder. Given a user-specified image region (box/mask), the model can accurately estimate that region's relative direction and distance to other regions. The paper also introduces **SpatialRGPT-Bench**, a benchmark with ground-truth 3D annotations spanning indoor, outdoor, and simulated environments, and shows the trained model doubles as a region-aware dense reward annotator for robotic tasks.

## Method specifics
3D representation: **3D scene graphs** (built during data curation, likely from depth/point-cloud-derived object relations, consistent with prior NVIDIA scene-graph-VQA work) plus a **depth plugin** fused into the VLM's vision tower at inference. This is closer to depth-grounded regional reasoning than raw monocular metric claims — depth is an explicit input channel (sourced from depth sensors/estimators feeding the curation pipeline), not purely hallucinated from RGB by the language model. Exact metric-scale provenance (sensor depth vs. monocular-estimator depth in curation) is **not found** in accessible content — flag results as depth-estimator-dependent unless the full paper confirms sensor ground truth.

## Key results
Not found — specific score tables for SpatialRGPT-Bench were not retrievable in this pass; the accessible summary only confirms the benchmark's existence and general "performance improvements."

## Code/weights
**Available.** Repo: `AnjieCheng/SpatialRGPT` (GitHub, official NeurIPS'24 implementation). Paper states code, dataset, and benchmark are released. License: CC BY 4.0 on the paper; repo license not independently confirmed here.

## Skill-3D relation
Cited in the §2.1 second clause with SpatialVLM, VLM-3R, Chat-Scene, Synthetic Vision, Chat-3D, Zhang 2025a as work incorporating "3D reconstruction, depth cues, spatial VQA data, and explicit grounding." Not directly reused by Skill-3D's tool-augmented pipeline, but the region-grounded depth-query idea (ask about a specific detected region's distance) is conceptually close to what a Skill-3D "depth tool call" would return.

## WeftOS relevance
SpatialRGPT-Bench (indoor/outdoor/simulated scenes with ground-truth 3D annotations) is a useful **evaluation reference** if WeftOS ever needs to benchmark a Rust spatial-QA tool against published numbers, and the "region proposal + depth plugin → relative direction/distance" pattern maps cleanly onto a WeftOS tool-call shape: agent selects a region (via SAM-class segmentation) → tool queries BVH for the AABBs under that region → returns measured relative distance, which is honest (BVH-backed) rather than VLM-hallucinated. That's an architecture pattern worth stealing conceptually (region → structured geometry query), even though SpatialRGPT itself answers via a fine-tuned VLM rather than a real spatial index. Overlaps with scene-graphs-open-vocab.md's "3D scene graph as regional representation" territory (ConceptGraphs/HOV-SG lineage) but at 2D-region-query granularity rather than object-instance identity.

**Verdict: PATTERN** — don't adopt the model; steal the "region query → grounded relative-geometry answer" shape as a template for a future WeftOS BVH-backed spatial-QA tool, and reuse SpatialRGPT-Bench as an eval reference if a comparable Rust tool is ever benchmarked.
