# 057 — SpatialPrompting: Keyframe-Driven Zero-Shot Spatial Reasoning with Off-the-Shelf Multimodal Large Language Models

**Citation:** Taguchi, S., Deguchi, H., Hamazaki, T., Sakai, H. *SpatialPrompting: Keyframe-Driven Zero-Shot Spatial Reasoning with Off-the-Shelf Multimodal Large Language Models.* arXiv:2505.04911.

**arXiv:** [2505.04911](https://arxiv.org/abs/2505.04911) (submitted 2025-05-08, 18 pages, 11 figures).

## Summary
A zero-shot spatial-reasoning framework that avoids 3D-specific fine-tuning entirely by **selecting a small set of informative keyframes** from an image sequence (using vision-language similarity, Mahalanobis distance, field-of-view, and image-sharpness metrics) and pairing them with camera-pose metadata in the prompt sent to an off-the-shelf MLLM.

## Method specifics
- **Representation:** keyframes + camera poses, explicitly **not** point clouds or voxels ("Rather than using point clouds or voxels, it employs a keyframe-driven prompt generation strategy"). This is a prompt-engineering method over posed 2D frames, not a 3D reconstruction pipeline.
- **Metric scale:** camera poses are used, implying the input sequence already carries pose info (e.g., from a SLAM/capture pipeline upstream); SpatialPrompting itself does not claim to produce metric geometry — it consumes poses, it doesn't estimate them. **Not a monocular-metric claim** — honest by omission (defers metric responsibility upstream).

## Key results
- Reports "state-of-the-art zero-shot performance on ScanQA and SQA3D across several metrics" (per fetched abstract; exact numbers not found — verify before quoting).

## Code / license
Not found in fetched content.

## Skill-3D relation
Double-cited: (1) §2.1 line 147, grouped with "prompting, mental simulation, visual chain-of-thought, reinforcement learning, code-driven 3D reasoning, and generative imagination of 3D space" methods; (2) §2.2 line 151, grouped with tool-augmented VLM agents for "long-video understanding, high-resolution image analysis, medical diagnosis, and general visual reasoning." The double citation signals SpatialPrompting sits at the intersection of "spatial reasoning via smart prompting" and "tool-augmented agent" framing — closer to Skill-3D's own zero-shot, tool-orchestrating design than most refs in this cluster.

## WeftOS relevance
Keyframe selection by VL-similarity + pose + sharpness is a directly reusable **capture-triage pattern** for MentraOS egocentric video: before running any expensive reconstruction or promoting frames to Urth Events, select the small keyframe subset worth reasoning over. It composes cleanly with WeftOS's Event/Object split (keyframes as `IdentityKind::Event` candidates) and requires no metric geometry itself, so there's no honesty violation to flag. Camera poses are assumed as input, matching WeftOS's own stance that pose/gravity must come from IMU/capture protocol, not be invented.

**Verdict: PATTERN** — adopt the keyframe-selection heuristic (VL-similarity + pose spread + sharpness) as a capture-triage step ahead of Urth Event ingestion / agent context packing.
