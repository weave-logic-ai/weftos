# 072 — Spatial-MLLM: Boosting MLLM Capabilities in Visual-based Spatial Intelligence

**Citation:** Wu, D., Liu, F., Hung, Y.-H., Duan, Y. (2025). *Spatial-MLLM: Boosting MLLM Capabilities in Visual-based Spatial Intelligence.* arXiv preprint.

**arXiv:** [2505.23747](https://arxiv.org/abs/2505.23747) · Project: diankun-wu.github.io/Spatial-MLLM/ · Code: github.com/THU-SI/Spatial-MLLM

## Summary

Spatial-MLLM targets spatial reasoning from **2D-only video input** (no depth sensors, no known poses). It adds a dual-encoder design — a standard 2D semantic encoder plus a 3D structure encoder initialized from a feed-forward visual-geometry foundation model — fused through a connector, with a space-aware frame-sampling strategy at inference. The claim is that most of the spatial reasoning gap in VLMs is not about lacking a 3D encoder per se but about how spatial structure is surfaced and sampled from ordinary video.

## Method

**3D representation:** feed-forward geometry features (depth/point-map-style latents) extracted per-frame from a visual-geometry backbone (VGGT-class), not an explicit point cloud or mesh artifact — the geometry stays inside the encoder as features, fused with 2D semantics via a connector module.

**Metric scale:** not obtained. The pipeline is monocular/feed-forward video → geometry features; no calibration, IMU, or known baseline is used. Outputs are **relative/up-to-scale spatial reasoning** (distance ordering, size comparison), not measured geometry.

## Results

VSI-Bench: reported as the best among open-source methods at 16-frame input (exact number not verified from available excerpts — flagged, do not cite a figure). Also evaluated on ScanQA and SQA3D with competitive results (numbers not confirmed in accessible text — **not found**, verify against the PDF before citing precisely).

## Code / license

Code public at github.com/THU-SI/Spatial-MLLM. Project page under CC BY-SA 4.0; weight license not separately specified in accessible pages — verify before reuse.

## Skill-3D relation

Cited at line 115 as one of the two papers (with Think3D, [[093]]) that open Skill-3D's "agentic 3D spatial reasoning" framing: tool use lets an MLLM "acquire spatial and geometric evidence that is difficult to infer from the MLLM alone." Spatial-MLLM is the non-agentic, architecture-level answer (bake a geometry encoder in); Skill-3D and Think3D are the agentic, tool-use-at-inference answer. Also listed in the §2.1 backbone/grounding group.

## WeftOS relevance

This is exactly the honesty gap WeftOS doctrine (ADR-078/079) exists to prevent: a monocular feed-forward geometry encoder producing spatial *reasoning* with no metric grounding, dressed as "3D structure." Fine as a **feature/prior** for an LLM's chain-of-thought, never as BVH input. Overlaps the feedforward-reconstruction.md survey's VGGT family but Spatial-MLLM stays entirely in-network (features, not COLMAP-shaped output) — no reconstruction artifact to even audit for scale.

**Verdict: WATCH.** Relevant as an architecture pattern for a "spatial-aware" chat/vision encoder bolted onto an agent, but not adoptable until scale honesty is added; not a BVH/Urth producer.
