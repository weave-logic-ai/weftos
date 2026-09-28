# ReVPT: Reinforced Visual Perception with Tools

**Citation:** Z. Zhou, D. Chen, Z. Ma, Z. Hu, M. Fu, S. Wang, Y. Wan, Z. Zhao, and R. Krishna. "Reinforced Visual Perception with Tools." arXiv preprint arXiv:2509.01656, 2025.

**arXiv:** 2509.01656 — https://arxiv.org/abs/2509.01656 (submitted Sept 1, 2025). Code: https://github.com/ls-kelvin/REVPT.

## Summary
ReVPT trains multimodal LLMs to reason about and invoke visual tools using **reinforcement learning** rather than supervised fine-tuning on curated tool-use traces. The authors note prior SFT-based tool-augmentation approaches suffer from expensive data generation, reliance on careful filtering, and poor generalization. ReVPT instead introduces a GRPO-based RL algorithm that trains the model end-to-end to reason with a suite of four visual tools — reported as object detection, zoom-in, edge detection, and depth estimation — achieving state-of-the-art results on several perception-heavy benchmarks.

## Method specifics
- **Tool API shape:** four fixed visual tools (object detection, zoom-in, edge detection, depth estimation); whether invocation is structured JSON/function-calling or code execution was **not confirmed** from the accessible abstract/PDF extract — unverifiable from what was retrieved.
- **Output return to model:** not confirmed — likely returned as re-inserted cropped/processed images (typical for zoom-in/edge/depth tools) but this was not explicitly stated in accessible material; marking as **not found**.
- **Planner vs executor:** the RL training is applied to a single MLLM that both decides which tool to call and produces the final answer — no separate planner/executor model architecture was found.
- **Error handling/repair:** not found in accessible material; GRPO-based RL provides outcome-level reward shaping during training rather than an explicit runtime self-correction/retry loop.

## Key quantitative results
CV-Bench: ReVPT-3B shows a **9.03%** improvement over the instruct-tuned baseline; ReVPT-7B shows a **9.44%** improvement over its baseline. Full results across SAT, CV-Bench, BLINK, and MMStar were referenced in the paper's tables but exact per-benchmark numbers were not extracted from the fetched PDF (tables present but not machine-readable in the fetch). Depth Anything V2 is cited as the depth-estimation tool component.

## Code/weights availability and license
Code released at https://github.com/ls-kelvin/REVPT. License: **CC BY 4.0** (per repository page). Model/weight release status not independently confirmed.

## Skill-3D's citation
Cited at §2.2, in the sentence: "A complementary line of work trains VLMs to use tools through supervised fine-tuning or reinforcement learning ... Zhou et al. (2025b) [ReVPT]." — positioned as an RL-trained tool-use approach, contrasted with Skill-3D's own skill-distillation-based method that does not require RL training of the base model.

## WeftOS relevance
**Verdict: WATCH.** ReVPT is a training-time approach (RL over a fixed tool suite) rather than an inference-time agent architecture, so it doesn't map directly onto WeftOS's already-deployed Claude Code/Grok/Codex-over-MCP setup, which uses frozen foundation models. Its depth-estimation tool output is worth flagging under "honest geometry": Depth Anything V2 depth maps are **relative, not metric** (no calibrated scale/units) unless separately calibrated — a pattern WeftOS should replicate carefully (never presenting such relative depth as measured geometry).
