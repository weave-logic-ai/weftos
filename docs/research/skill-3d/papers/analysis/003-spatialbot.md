# [003] SpatialBot: Precise Spatial Understanding with Vision Language Models

**Citation:** W. Cai, I. Ponomarenko, J. Yuan, X. Li, W. Yang, H. Dong, and B. Zhao. *SpatialBot: precise spatial understanding with vision language models*. In 2025 IEEE International Conference on Robotics and Automation (ICRA), pp. 9490–9498, 2025.

**arXiv:** [2406.13642](https://arxiv.org/abs/2406.13642) · ICRA 2025

## Summary
SpatialBot addresses VLMs' weak spatial understanding — foundational for Embodied AI — by feeding models **both RGB and depth images** rather than RGB alone. The authors build SpatialQA, a training set of multi-level depth-related questions (from raw pixel-depth reading up to reasoning tasks like relative distance/size), and SpatialBench, a dedicated evaluation benchmark for spatial understanding at multiple levels. SpatialBot is a VLM trained on SpatialQA that is evaluated on spatial-understanding, general VLM, and embodied-AI benchmarks.

## Method specifics
**Explicit depth maps** are the 3D representation — the model takes paired RGB + depth image input (not point clouds, not BEV, not scene graphs). Depth is presumably sourced from sensor depth or monocular depth estimators feeding into the training pipeline (exact depth-source pipeline not found in accessible text). Because depth values are read directly rather than inferred purely from RGB by the VLM itself, this is closer to **sensor/estimator-grounded** depth than a monocular hallucination — but whether the depth maps used at train/test time are metric-calibrated sensor depth vs. relative monocular-estimator depth is **not found** in accessible content; treat scale claims as unverified.

## Key results
Not found — the abstract claims "remarkable improvements" on SpatialBench, general VLM benchmarks, and embodied-AI tasks, but no specific numeric scores were retrievable in this pass.

## Code/weights
**Available.** Paper states: "The model, code and data are available at https://github.com/BAAI-DCAI/SpatialBot" (repo: BAAI-DCAI/SpatialBot). License: not confirmed in accessible content — check repo LICENSE file before reuse.

## Skill-3D relation
Cited in the §2.1 related-work paragraph, second clause ("incorporating 3D reconstruction, depth cues, spatial VQA data, and explicit grounding") alongside SpatialVLM, SpatialRGPT, VLM-3R, Chat-Scene, Synthetic Vision, Chat-3D, and Zhang 2025a. Grouped as depth-cue-based prior work; not a direct component of Skill-3D's own pipeline.

## WeftOS relevance
The RGB+depth dual-stream training pattern is a reasonable reference point for how an egocentric-capture VLM could be grounded with real depth (e.g., MentraOS glasses with an active depth sensor, or ToF supplement), but SpatialBot itself is a 2D-image+depth-map VLM, not a 3D-scene reconstructor — it has no scene-graph or point-cloud output to feed BVH. Its value to WeftOS is as a **training-data pattern** (multi-level depth QA) for any future spatial-grounding fine-tune, not as an architecture to adopt directly. Overlaps thematically with feedforward-reconstruction.md's "what stays metric vs generative" framing — depth-conditioned QA doesn't mint metric geometry by itself.

**Verdict: WATCH** — open code/data is a plus, and the RGB-D training-signal pattern is worth remembering if WeftOS ever trains a depth-aware VLM for MentraOS capture, but it's not a component to adopt into the current Rust/BVH pipeline.
