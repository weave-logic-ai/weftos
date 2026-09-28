# 089 — Spatial Understanding from Videos: Structured Prompts Meet Simulation Data

**Citation:** Zhang, H., Liu, M., Li, Z., Wen, H., Guan, W., Wang, Y., Nie, L. (2025). *Spatial Understanding from Videos: Structured Prompts Meet Simulation Data.* arXiv preprint. NeurIPS 2025 Spotlight.

**arXiv:** [2506.03642](https://arxiv.org/abs/2506.03642)

## Summary

A two-part framework for boosting 3D spatial reasoning in **pre-trained, architecture-unmodified** VLMs: (1) **SpatialMind**, a structured-prompting technique that decomposes a scene/question into interpretable sub-steps, and (2) **ScanForgeQA**, a QA dataset auto-generated from diverse 3D simulation scenes, used to fine-tune models on spatial questions. The pitch is that better *prompting* and better *training data*, not a new encoder, close most of the spatial gap.

## Method

**3D representation:** simulation-generated 3D scenes (a synthetic-data generator, not a real-world reconstruction pipeline) are the source of ScanForgeQA; SpatialMind itself operates over ordinary video frames via structured textual decomposition, with no explicit point cloud or depth map passed to the model at inference.

**Metric scale:** the simulation scenes used for **training data** are presumably metric within their own synthetic coordinate frame (typical for simulators), but at **inference** on real video, SpatialMind carries no explicit metric grounding — reasoning is over structured prompts, not measured geometry. **Not found** — exact simulator/scale details unverified from accessible excerpts.

## Results

Abstract claims "extensive experiments across multiple benchmarks" show individual and combined gains from SpatialMind + ScanForgeQA; specific numbers/benchmark names **not found** in accessible content — verify against the PDF.

## Code / license

Not found in accessible content.

## Skill-3D relation

Listed in the §2.1 "3D reconstruction, depth cues, spatial VQA data, and explicit grounding" group (line 147) alongside [[006]] SpatialVLM, [[009]] SpatialRGPT, [[047]] GPT4Scene. This is the synthetic-data-generation branch of that family — training-data scale via simulation rather than real capture.

## WeftOS relevance

The ScanForgeQA pattern (auto-generate spatial QA from simulated 3D scenes) is a plausible way to produce **training/eval data for Rust-side skill fine-tuning** (Skill-3D-style skill-guided post-training, if WeftOS ever pursues that) without needing real capture sessions. It is squarely a training-data technique, not a runtime geometry source, so it has no direct BVH/Urth seam. Simulation-derived "ground truth" scale must never be conflated with real captured scale if such data is ever mixed with production Urth data — keep provenance-tagged.

**Verdict: WATCH.** Interesting only if WeftOS builds synthetic spatial-QA training data for skill fine-tuning; no runtime relevance to Urth/BVH.
