# Ref 27: ECP (training-free, task-agnostic high-resolution MLLM framework)

**Citation:** J. Lee, Y. Choi, H. Choi, H. Kim, S. Kim. "A Training-Free, Task-Agnostic Framework for Enhancing MLLM Performance on High-Resolution Images." arXiv:2507.10202, 2025 (CVPR 2025 Workshop on Emergent Visual Abilities and Limits of Foundation Models).

**Source:** arXiv:2507.10202 (https://arxiv.org/abs/2507.10202); code: github.com/yenncye/ECP

## Summary

ECP ("Extract Candidate then Predict") addresses the train-test resolution mismatch that hurts MLLMs on high-resolution images (4K/8K), where fine detail is lost by downsampling to the model's native input resolution. It is a two-stage, training-free, task-agnostic inference pipeline: stage 1 runs the MLLM on a downsampled version of the image to coarsely identify an instruction-relevant candidate region (point or box); stage 2 re-predicts using the cropped high-resolution patch, either alone or together with the downsampled full image for context.

## Method specifics

- **Tool API shape:** none — this is not an agent/tool-calling framework. Both stages are ordinary MLLM inference passes; there is no external tool, code execution, or structured tool-call protocol.
- **Output return path:** n/a — it's a pixel-crop-and-reinfer pipeline, not a tool-output-to-model loop.
- **Planner/executor split:** none — same model runs both stages; "planning" is implicit in stage 1's candidate localization.
- **Error handling:** minimal — the paper's only verification-style analysis is an ablation comparing instruction-guided region selection against random-region sampling, showing the value of meaningful candidate extraction; no runtime retry, fallback, or self-correction mechanism is described.

## Results

ScreenSpot-Pro 4K GUI grounding (OS-Atlas-7B, EC+P): 40.4% (+21.3% absolute over baseline). HR-Bench 4K Overall (Qwen2-VL-7B, EC+P): 68.3% (+5.8%); HR-Bench 4K FSP: 81.0% (+9.5%). HR-Bench 8K Overall: 60.3% (+5.2%); HR-Bench 8K FSP: 71.8% (+10.3%). Numbers found in the paper's reported results tables.

## Code/weights

Code released at github.com/yenncye/ECP under CC BY-NC-SA 4.0 (non-commercial, share-alike). No separately released model weights — it's a training-free wrapper around existing backbones (OS-Atlas-7B, Qwen2-VL-7B).

## Skill-3D relation

Cited in §2.2 ("MLLM Agents"): "Recent tool-augmented VLM agents have been developed for long-video understanding, high-resolution image analysis, medical diagnosis, and general visual reasoning Chen et al. (2025a); Zhang et al. (2025b); Taguchi et al. (2025); Yang et al. (2025e); Zhu et al. (2025); Lee et al. (2025a); ..." — grouped under high-resolution image analysis, though ECP itself is not actually a tool-augmented agent (see above); Skill-3D's citation groups it loosely with the agent line despite ECP being training-free inference-only.

## WeftOS relevance

**WATCH.** No tool calls, no metric geometry, no agent loop — but the underlying "coarse localize, then re-infer on a high-res crop" idea is a cheap technique worth keeping in mind for any WeftOS vision pipeline that has to deal with high-resolution egocentric captures (e.g. MentraOS glasses frames) before invoking a real detection/depth tool, purely as a pre-processing efficiency trick, not as an evidence-generation mechanism.
