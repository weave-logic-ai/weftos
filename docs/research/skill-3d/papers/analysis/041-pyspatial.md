# [41] pySpatial: Generating 3D Visual Programs for Zero-Shot Spatial Reasoning

**Citation:** Luo et al. (2026). Z. Luo, C. Zhang, S. Yong, C. Dai, Q. Wang, H. Ran, G. Shi, K. Sycara, and Y. Xie. "PySpatial: generating 3d visual programs for zero-shot spatial reasoning." In The Fourteenth International Conference on Learning Representations (ICLR 2026).
**arXiv:** https://arxiv.org/abs/2603.00905 · Project page: https://pyspatial.github.io/ · Code: https://github.com/Zhanpeng1202/pySpatial (license not specified in fetched README)

## Summary
pySpatial gives an MLLM the ability to write Python code that calls spatial tools — 3D reconstruction, camera-pose recovery, novel-view rendering — turning a raw image sequence into an explorable 3D scene the model can query explicitly, rather than relying on the MLLM's implicit ("mental model") 2D cognitive-map imagination that prior spatial-reasoning methods use. It requires no gradient-based fine-tuning (fully zero-shot, training-free) and is evaluated on MindCube and Omni3D-Bench, where it beats strong MLLM baselines, including reportedly beating GPT-4.1-mini by 12.94% on MindCube. The authors also demonstrate a real-world indoor-navigation use case with a robot traversing an environment guided by the generated 3D scene.

## Method Specifics
- **Tool API shape:** code/program generation — the MLLM composes Python function calls (visual programs) against a spatial-tool library, not structured JSON tool calls. This is the code-driven variant of tool-augmented spatial reasoning (Skill-3D's related-work sentence calls this "code-driven 3D reasoning").
- **Output return:** tool calls (3D reconstruction, pose estimation, rendering) return structured/geometric scene representations plus rendered novel views, which the generated program composes and which flow back into the MLLM's context as an explorable 3D artifact.
- **Planner vs executor:** not explicitly documented in fetched content; the framework's own program-synthesis step effectively plans (compose calls) while the spatial-tool library executes — no separate planner model is confirmed beyond the single MLLM writing the code.
- **Error handling/repair:** not found in fetched content — no confirmed retry/verification loop distinct from code execution succeeding or failing.

## Quantitative Results
MindCube benchmark: outperforms GPT-4.1-mini by 12.94% (source: search-engine summary, not independently confirmed against the primary PDF — treat with light caution). Also evaluated on Omni3D-Bench, reported to "consistently surpass strong MLLM baselines," but no specific Omni3D-Bench numeric score was found.

## Metric vs Relative Geometry (important per task brief)
pySpatial's pipeline explicitly performs 3D reconstruction and camera-pose recovery from image sequences — this produces geometry grounded in reconstructed 3D structure (poses, depths, novel views), which is closer to metric/scale-consistent geometry than pure appearance-based VQA, but the exact scale-calibration story (whether reconstructed scale is metric/real-world or only relative/up-to-scale, as is typical for uncalibrated monocular SfM/MVS pipelines) was **not found** in the fetched content and should be treated as unverified — do not assume absolute metric units without checking the paper's reconstruction module (likely a Pi3/DUSt3R-family or COLMAP-style pipeline).

## Code/Weights/License
Code repository exists (github.com/Zhanpeng1202/pySpatial) with install/dataset/eval instructions; no license file content was surfaced in the fetch — license unverifiable, mark as not found.

## Relation to Skill-3D
Cited at §1 and §2.1. §1: "Recent methods explore this paradigm by iteratively invoking tools within a per-question reasoning loop, e.g., object detection and segmentation for 2D perception, depth estimation and 3D reconstruction for geometric grounding Zhang et al. (2026c); Luo et al. (2026); Yuan et al. (2026); Ropero et al. (2026). However, these methods often fail to realize the potential of tool use in 3D reasoning and exhibit preferences toward a few dominant tools, regardless of what each scene actually requires." §2.1: grouped among works that "enhance spatial reasoning through prompting, mental simulation, visual chain-of-thought, reinforcement learning, code-driven 3D reasoning, and generative imagination of 3D space" — pySpatial is Skill-3D's explicit example of the "code-driven 3D reasoning" category, and is one of the small set of directly comparable prior agentic 3D-tool systems Skill-3D positions itself against (critiquing "uniform tool-use workflows" and "preferences toward a few dominant tools").

## WeftOS Relevance
**Verdict: WATCH.** Directly comparable architecture class (code-generation-driven spatial tool agent) to what WeftOS's MCP-exposed tool agents already resemble structurally, and the training-free zero-shot framing is attractive, but the metric-vs-relative scale question is unresolved from available sources — under WeftOS's "honest geometry" principle this must be verified (likely relative/up-to-scale reconstruction, not metric) before treating any of its outputs as ground-truth distances. Worth a deeper read of the primary PDF before deciding adopt/skip; not relevant to egocentric MentraOS capture beyond being a general multi-view spatial-tool pattern.
