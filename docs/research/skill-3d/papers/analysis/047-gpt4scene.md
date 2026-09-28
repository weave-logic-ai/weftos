# 047 — GPT4Scene: Understand 3D Scenes from Videos with Vision-Language Models

**Citation:** Qi, Z., Zhang, Z., Fang, Y., Wang, J., Zhao, H. *GPT4Scene: Understand 3D Scenes from Videos with Vision-Language Models.* arXiv:2501.01428.

**arXiv:** [2501.01428](https://arxiv.org/abs/2501.01428) · project site `gpt4scene.github.io`.

## Summary
Targets the gap where VLMs fail at 3D scene understanding from video because they lack **global-local correspondence** between the full scene and individual frames. Fix: build a **Bird's-Eye-View (BEV) image** from the video, annotate **consistent object IDs** across both the BEV image and individual video frames, and feed the BEV + marked frames together to the VLM as a single prompt.

## Method specifics
- **Representation:** BEV map derived from video (likely via SfM/point-cloud reconstruction, though the exact reconstruction step wasn't detailed in the fetched abstract — **not found**) + marker-annotated 2D keyframes. This is the same "BEV + consistent object IDs" pattern named in the survey taxonomy alongside scene-graph methods.
- **Metric scale:** not confirmed from the fetched content whether the underlying BEV construction is metric or relative — **flag as unverified**; BEV construction from monocular video typically requires either known camera poses (metric) or SfM-derived relative scale, so this needs primary-source confirmation before assuming metric honesty.

## Key results
- Zero-shot performance **exceeds closed-source GPT-4o** on the reported 3D understanding tasks.
- Trained on **165K** annotated video examples; reports **SOTA on all 3D understanding tasks** evaluated (benchmark names not itemized in fetched summary).
- Notable ablation: after training, the model retains improved 3D inference even **without** explicit BEV prompts at inference time (suggests the BEV supervision teaches an implicit spatial prior).

## Code / license
Project page exists; code/weights availability **not found** in fetched content.

## Skill-3D relation
Grouped in §2.1 with methods that "improve fine-grained spatial understanding by incorporating 3D reconstruction, depth cues, spatial VQA data, and explicit grounding" (Cheng et al. 2024 SpatialRGPT, Chen et al. 2024 SpatialVLM, Fan et al. 2025b VLM-3R, Huang et al. 2024 Chat-Scene, Wang et al. 2023 Chat-3D, etc.) — i.e., cited as one of several video/BEV/3D-augmented VLM precedents for the design space Skill-3D operates in, not as a direct algorithmic dependency.

## WeftOS relevance
BEV-from-video with cross-view object-ID consistency is directly analogous to what a WeftOS agent would need to correlate MentraOS egocentric frames against an Urth room's `WM_OBJECT` leaves — "does this frame's chair equal that BVH leaf's chair" is exactly GPT4Scene's global-local correspondence problem, solved by BEV + IDs rather than by a persistent geometric index. WeftOS already has the persistent index (BVH); GPT4Scene's ID-marking trick is a candidate **prompting pattern** for the agent side (mark frames with known leaf IDs before asking an MLLM to reason about a scene), not a reconstruction method to adopt.

**Verdict: PATTERN** — the BEV + consistent-ID visual-prompting trick is worth stealing for agent-side scene QA prompts once Urth leaf IDs exist; the reconstruction pipeline itself is not adopted (scale honesty unverified).
