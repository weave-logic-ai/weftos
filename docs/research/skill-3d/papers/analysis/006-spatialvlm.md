# [006] SpatialVLM: Endowing Vision-Language Models with Spatial Reasoning Capabilities

**Citation:** B. Chen, Z. Xu, S. Kirmani, B. Ichter, D. Sadigh, L. Guibas, and F. Xia. *SpatialVLM: endowing vision-language models with spatial reasoning capabilities*. In Proceedings of the IEEE/CVF Conference on Computer Vision and Pattern Recognition (CVPR), pp. 14455–14465, 2024. (Full author list per arXiv also includes Driess, Florence.)

**arXiv:** [2401.12168](https://arxiv.org/abs/2401.12168) · CVPR 2024

## Summary
SpatialVLM tackles VLMs' poor quantitative spatial reasoning (distances, sizes, relative positions) by building an **internet-scale, automatic 3D spatial VQA data-generation pipeline**: 2 billion VQA examples synthesized from 10 million real-world 2D images, explicitly described as "the first internet-scale 3D spatial reasoning dataset in metric space." A VLM fine-tuned on this data acquires both qualitative (left/right, near/far) and quantitative (metric distance/size estimation) spatial reasoning, and the resulting model is shown useful for chain-of-thought spatial reasoning and as a reward/grounding signal for robotics.

## Method specifics
Underlying representation: single 2D RGB images are lifted to per-pixel 3D via (unspecified in accessible text, but consistent with the paper's known pipeline) monocular depth + semantic segmentation + camera-pose estimation to back out object-level 3D boxes/point estimates, from which metric VQA (e.g., "how far apart are X and Y in cm") is templated. **This is the paradigm case of "metric outputs from monocular input"** flagged as a WeftOS doctrine violation: it claims metric spatial answers (real-world units) derived from monocular RGB, which is fundamentally scale-ambiguous without an external metric anchor (camera height prior, known-object-size prior, or similar) — the WeftOS honest-geometry rule treats this class of output as unverified/non-authoritative, not ground truth.

## Key results
Not found — accessible content confirms strong qualitative and quantitative spatial VQA improvements and downstream robotics chain-of-thought applications but no specific benchmark scores were retrievable in this pass.

## Code/weights
Not found in accessible content — a project website is referenced but no explicit code/data release statement was retrievable; historically SpatialVLM's full training pipeline/dataset has not been fully open-sourced (unverified here — check project page directly before relying on this).

## Skill-3D relation
Cited in the §2.1 related-work opening list of "stronger backbones" for MLLM spatial reasoning, alongside Wake 2024 (GPT-4V for robotics), Liu 2025a (coarse correspondences), and Lee 2025b (perspective-aware reasoning); also re-cited in the second clause grouped with Cheng 2024/SpatialRGPT etc. as incorporating "3D reconstruction, depth cues, spatial VQA data." It is foundational background for the field Skill-3D operates in, not a component Skill-3D reuses directly.

## WeftOS relevance
SpatialVLM is the canonical example of exactly the failure mode WeftOS's ADR-078/079 honesty doctrine is designed to reject: monocular RGB → metric spatial claims, with scale derived from learned priors rather than measurement. It is instructive as a **negative pattern** — any WeftOS component doing "estimate distance from a single frame" (e.g., an MentraOS glasses helper answering "how far is that chair") must be labeled as an estimate/prior, never written into BVH as measured `AabbWire` geometry. No code to adopt; the dataset-generation idea (templated metric VQA from vision pipelines) is a training-data pattern, not a WeftOS runtime component.

**Verdict: SKIP** (as a component) — but keep as the reference example of the anti-pattern when writing Urth/glasses-agent copy about what counts as "metric."
