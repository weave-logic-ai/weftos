# 095 — CoV: Chain-of-View Prompting for Spatial Reasoning

**Citation:** Zhao, H., Liu, A., Zhang, Z., Wang, W., Chen, F., Zhu, R., Haffari, G., Zhuang, B. (2026). *CoV: Chain-of-View Prompting for Spatial Reasoning.* arXiv preprint.

**arXiv:** [2601.05172](https://arxiv.org/abs/2601.05172) · Code: github.com/ziplab/CoV

## Summary

CoV is a **training-free** framework for embodied QA in 3D environments where the answer-relevant context is spread across multiple viewpoints and partially occluded. It runs in two stages: a View Selection agent filters redundant frames and finds question-relevant "anchor" views, then a fine-grained stage alternates reasoning with discrete camera actions to actively gather more observations from the scene — i.e. iterative, agentic viewpoint selection rather than a fixed frame sample.

## Method

**3D representation:** "the underlying 3D scene representation" is referenced but not specified in accessible excerpts (likely a pre-existing scanned/reconstructed scene, per the ScanQA/SQA3D benchmarks it's tested on) — the paper's contribution is the **view-selection and camera-action policy** layered on top, not a new geometry representation. **Not found** — verify exact scene format from the PDF.

**Metric scale:** inherited from whatever underlying scene dataset is used (ScanQA/SQA3D scenes are typically metric ScanNet-derived reconstructions); CoV itself adds no new scale claim.

## Results

OpenEQA: **+11.56%** average LLM-Match improvement, up to **+13.62%** on Qwen3-VL-Flash. Test-time scaling ablation: **+2.51%** average, **+3.73%** on Gemini-2.5-Flash. ScanQA: **116 CIDEr / 31.9 EM@1**. SQA3D: **51.1 EM@1**. (Self-reported; not independently verified.)

## Code / license

Code public at github.com/ziplab/CoV; license not specified in accessible content.

## Skill-3D relation

Grouped in the §2.1 backbone/benchmark list (line 147). Structurally CoV is close kin to Skill-3D's own tool-invocation loop and to Think3D ([[093]]): both use an active, multi-step exploration policy over a scene rather than a single forward pass — CoV's "which view do I need next" agent is analogous to Skill-3D's "which tool do I need next" skill selection.

## WeftOS relevance

The two-stage "coarse view/tool triage → fine-grained iterative action" pattern maps directly onto Graph Views F10 (subgraph pack: coarse ANN seed → k-hop expand) and onto how a WeftOS agent might decide which BVH region/View to query next when reasoning about a room. It is a reasoning-*policy* pattern (view/tool selection under partial observability), not a geometry producer — consistent with "honest geometry": CoV never claims new geometric truth, it just chooses where to look within an already-reconstructed scene.

**Verdict: PATTERN.** Steal the coarse-to-fine active-viewpoint/tool-selection policy shape for agent context-gathering (F10-style), not for geometry ingestion.
