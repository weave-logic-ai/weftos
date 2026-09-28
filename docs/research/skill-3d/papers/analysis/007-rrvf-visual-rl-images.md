# [7] Chen et al. (2025b) — Learning Only with Images: Visual RL with Reasoning, Rendering, and Visual Feedback (RRVF)

**Citation:** Y. Chen, Y. Shen, W. Huang, S. Zhou, Q. Lin, X. Cai, Z. Yu, J. Bu, B. Shi, Y. Qiao. arXiv:2507.20766.
**Venue/ID:** arXiv preprint, 2025-07. https://arxiv.org/abs/2507.20766

## Summary
RRVF trains multimodal LLMs on image-to-structured-representation tasks (chart/UI image → code) using only raw images, no paired text supervision. It exploits an "Asymmetry of Verification": checking whether a rendered output matches the source image is easier than generating the structured representation in the first place. A closed loop — reason, render the model's own code, compare the render against the source image — produces a visual-feedback reward, optimized with GRPO.

## RL objective / reward design
- Policy optimized with GRPO (group-relative, reference to DeepSeekMath/DeepSeek-R1 lineage — same family as refs [13],[52] in this cluster).
- Reward = visual similarity between the rendered output of the model's generated code and the original source image (a rendering-based, self-supervised verifier), enabling multi-turn self-correction without a stronger teacher model or curated labels.
- No additional reward model or human-labeled ground truth required — the image itself is the supervision signal.

## Key results
- Outperforms comparable open-source MLLMs and supervised baselines on image-to-code tasks (data charts, web interfaces).
- Exceeds the performance of the (stronger) model used to provide visual feedback during training, and shows better generalization than SFT baselines.

## Availability
Code referenced as available on GitHub (exact URL not resolved from the abstract page); arXiv nonexclusive-distribution license. Not independently verified — treat as "reported, unconfirmed" until the repo is checked directly.

## How Skill-3D uses it
Cited once in the related-work sweep of RL-for-spatial/visual-reasoning approaches (§2.2, alongside ARPO [14] and others) as an example of visual RL with reward computed from rendering/visual feedback rather than text labels — not otherwise discussed or built on.

## WeftOS relevance — Verdict: PATTERN
The "verification is easier than generation" self-supervised-reward pattern is directly reusable for a Rust agent that must judge its own tool outputs (e.g., comparing a re-projected/rendered 3D estimate against source imagery) without a labeled dataset. WeftOS's Rust side would need to emit the same trajectory shape (query, sampled group of trajectories, per-trajectory scalar reward) for Python-side GRPO training — see [[papers/benchmarks-and-rl]] for the shared trajectory schema. Not an adoption target as code (Python/VLM-specific), but the reward-design pattern (cheap verifier vs. expensive generator) is worth encoding as a skill/tool-reward primitive.
