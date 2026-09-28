# 088 — Boosting MLLM Spatial Reasoning with Geometrically Referenced 3D Scene Representations (GR3D)

**Citation:** Yuan, J., Kumar, G., Wang, B. (2026). *Boosting MLLM Spatial Reasoning with Geometrically Referenced 3D Scene Representations.* arXiv preprint.

**arXiv:** [2603.08592](https://arxiv.org/abs/2603.08592)

## Summary

GR3D is a **zero-shot, training-free** prompting method: it annotates detected objects in an image with unique IDs, computes their 3D geometric attributes (position, extent, relative distances) from an upstream reconstruction, and encodes those attributes as **text** appended to the prompt. The MLLM then reasons over positions/sizes symbolically (as numbers/words) rather than purely visually.

## Method

**3D representation:** object-level geometric attributes (per-object 3D position/extent), not a scene-wide point cloud or mesh delivered to the model — geometry is computed upstream (by an unspecified/external reconstruction step, not detailed in accessible excerpts) and then **converted to text**, i.e. a textual scene graph of coordinates.

**Metric scale:** not explicitly described — unclear whether the "3D geometric attributes" are metric (from a calibrated reconstruction) or relative/up-to-scale. **Not found** — flag as unverified; do not assume metric.

## Results

Self-reported: **+9%** on VSI-Bench with GPT-5 backbone, **+12%** on MindCube, in a **zero-shot** (no additional training) setting. Numbers are from the abstract only — treat as author-claimed, not independently verified.

## Code / license

Not found in accessible content. Standard arXiv non-exclusive distribution license applies to the paper text.

## Skill-3D relation

Cited at line 115 alongside [[093]] Think3D and [[096]] Zheng (DriveAgent-r1) in the paragraph on methods that "iteratively invoke tools within a per-question reasoning loop, e.g., object detection and segmentation for 2D perception, depth estimation and 3D reconstruction for geometric grounding." GR3D's ID+text-encoding trick is a lightweight variant of the same idea: convert perceived geometry into a symbolic form an LLM can reason over, without a full agentic tool loop.

## WeftOS relevance

The "unique object ID + geometric attributes as text reference" pattern is structurally close to what Graph Views F10 already does when packing context for an LLM agent (subgraph extraction → text pack, per `scene-graphs-open-vocab.md` §3.1) — GR3D is evidence this pattern generalizes and helps MLLM spatial reasoning even without fine-tuning. Where GR3D is silent on metric scale, WeftOS's `WmObjectPayload` (`label`, `bound: AabbWire`, `vector: Option<VectorRef>`) already keeps the equivalent split honest: geometry stays authoritative (BVH `LeafId` + AABB), and what gets serialized to an agent's prompt is a **derived textual summary**, not a re-definition of identity.

**Verdict: PATTERN.** Steal the "stable object ID + geometry-as-text for LLM consumption" idea for F10 context packing; do not import as a geometry source (scale unverified).
