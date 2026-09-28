# [028] Perspective-Aware Reasoning in Vision-Language Models via Mental Imagery Simulation

**Citation:** P. Y. Lee, J. Je, C. Park, M. A. Uy, L. Guibas, and M. Sung. *Perspective-aware reasoning in vision-language models via mental imagery simulation*. arXiv preprint, 2025.

**arXiv:** [2504.17207](https://arxiv.org/abs/2504.17207) · venue not found (arXiv preprint; project page apc-vlm.github.io) · submitted April 24, 2025 · CC-BY-4.0

## Summary
The paper targets **perspective-taking** — reasoning about a scene as it would appear from a viewpoint other than the camera's own — which VLMs handle poorly due to egocentric bias. Its method, **Abstract Perspective Change (APC)**, does not synthesize a novel-view image; instead it builds an abstracted scene representation via a pipeline of vision foundation models (object detection, segmentation, orientation estimation) and performs the viewpoint transform **on that abstraction** — i.e., mental imagery simulation over structured scene abstractions rather than pixel-level view synthesis. It reports outperforming both fine-tuned spatial-reasoning models and novel-view-synthesis-based approaches on synthetic and real-image benchmarks.

## Method specifics
3D representation: an **abstracted scene layout** built from detected object boxes/masks + estimated object orientations — effectively a lightweight object-level scene abstraction (positions + orientations), not a dense point cloud, depth map, or mesh. The perspective transform is applied to this abstraction (a geometric operation: re-project/re-describe object positions and orientations relative to a hypothetical alternate viewpoint) rather than regenerating pixels. This is **explicitly metric-relative** in the sense that it manipulates estimated object poses geometrically (a defined transform), but the underlying object positions/orientations themselves originate from monocular vision-foundation-model estimates, so absolute metric scale of the scene is not established — treat as **relative/proportional geometry**, not measured/calibrated 3D.

## Key results
Not found — accessible content confirms outperformance of fine-tuned spatial-reasoning baselines and NVS-based approaches on "synthetic and real-image benchmarks" but no benchmark names or numeric scores were retrievable in this pass.

## Code/weights
Not found in accessible content beyond project page reference (apc-vlm.github.io); no explicit release statement retrieved.

## Skill-3D relation
Cited twice in §2.1: once in the "backbones" opening list (with Yang 2023, Wake 2024, Shao 2024a, Liu 2025a) and once in the "prompting, mental simulation, visual chain-of-thought..." clause (with Taguchi 2025, Marsili 2025, Tang 2025a, Fan 2025a, Wang 2025d/e, Chen 2025c, Luo 2026, Yang 2025d) — i.e., it is Skill-3D's explicit example of "mental simulation" as a spatial-reasoning enhancement strategy, distinct from tool-based/agentic approaches. Not a direct component Skill-3D calls, but a named exemplar of a competing reasoning paradigm (simulate mentally vs. call an external tool).

## WeftOS relevance
The "abstract the scene into structured object-pose data, then apply a defined geometric transform (not pixel regeneration)" pattern is a reasonable analog for how a WeftOS agent might answer "what would this room look like from the doorway" using BVH object AABBs/poses rather than image generation — i.e., compute the transform against real Urth geometry instead of simulating it from a VLM's internal abstraction. Because WeftOS would have actual measured geometry (BVH leaves) rather than monocular-estimated poses, a WeftOS-native version of this idea would be strictly more honest than APC's own abstraction (which still originates from monocular estimation). Not directly adoptable as code; useful as a conceptual template for a future "viewpoint query" agent tool over Urth.

**Verdict: PATTERN** — the perspective-transform-over-structured-abstraction idea is worth keeping as a template for a future BVH-backed "view from elsewhere" query tool; the model/paper itself is not adoptable (monocular-estimated poses, no code found).
