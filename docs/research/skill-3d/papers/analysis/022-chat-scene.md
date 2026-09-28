# [022] Chat-Scene: Bridging 3D Scene and Large Language Models with Object Identifiers

**Citation:** H. Huang, Y. Chen, Z. Wang, R. Huang, R. Xu, T. Wang, L. Liu, X. Cheng, Y. Zhao, J. Pang, et al. *Chat-scene: bridging 3d scene and large language models with object identifiers*. In The Thirty-eighth Annual Conference on Neural Information Processing Systems (NeurIPS), 2024.

**arXiv:** [2312.08168](https://arxiv.org/abs/2312.08168) (latest v4, Sep 2024) · NeurIPS 2024 (also TPAMI 2026 extension per repo)

## Summary
Chat-Scene lets an LLM converse about and ground references within a 3D scene by decomposing the scene into a set of **object proposals**, each tagged with a unique **object-identifier token**. Scene embeddings are represented as a sequence of explicit **object-level** embeddings (derived from semantic-rich 2D and/or 3D features per object) rather than a dense per-point or per-voxel field. By routing all scene-language tasks (referring expression, captioning, QA) through this identifier-token interface, the model unifies ScanRefer-, Scan2Cap-, ScanQA-, and SQA3D-style tasks into one QA format without task-specific heads, and reports strong results with minimal fine-tuning across all of them.

## Method specifics
3D representation: **object-instance-level** — a discrete set of object proposals (from an upstream 3D instance segmentation stage over a point cloud, consistent with the ScanNet-derived benchmarks it targets), each represented by pooled 2D/3D features and addressed by an identifier token. This is an **object-graph-adjacent** representation, not a scene graph with explicit relations, but structurally similar to the "flat object soup" pattern already documented in WeftOS's scene-graphs-open-vocab.md (ConceptGraphs-class). Metric scale: implicit — object proposals come from real-scanned point clouds (ScanNet-family data), so underlying geometry is metric, but Chat-Scene itself does not expose or reason about metric coordinates in its output; it exposes identifiers and relations via language only.

## Key results
Not found — accessible content confirms the model "significantly outperforms existing methods" on ScanRefer, Multi3DRefer, Scan2Cap, ScanQA, and SQA3D but no numeric scores were retrievable in this pass.

## Code/weights
Available: repo `ZzZZCHS/Chat-Scene` (GitHub), tagged "[NeurIPS 2024 & TPAMI 2026]." License: paper is CC BY 4.0; repo license not independently confirmed.

## Skill-3D relation
Cited in the §2.1 second clause with SpatialRGPT, SpatialVLM, VLM-3R, Synthetic Vision, Chat-3D (Wang 2023, ref 68 — a direct predecessor by an overlapping author line), Zhang 2025a. Grouped as depth/3D-reconstruction-informed prior art; not a direct Skill-3D pipeline component.

## WeftOS relevance
The "object proposals + stable identifier token, addressed by language rather than by re-describing geometry every turn" pattern is a close conceptual match to WeftOS's `LeafId` addressing scheme — Chat-Scene's identifier tokens are the VLM-world analog of a stable `IdentityKind::Object` leaf reference used in agent dialogue. That's a genuinely useful pattern: an agent conversing about an Urth scene should refer to objects by a stable id (mappable to `LeafId`) rather than re-grounding a fresh bounding box every turn. The model/training itself (ScanNet-scale instance segmentation + LLM fine-tune) is not something to adopt; the addressing pattern is.

**Verdict: PATTERN** — steal the "durable per-object identifier token used in dialogue" addressing idea for how a WeftOS agent should refer to `LeafId`-backed Objects across a conversation turn, not the model or training pipeline. No overlap conflict with existing WEFT scene-graph doctrine (identity via `LeafId`, not via strings) — this reinforces rather than contradicts it.
