# [70] Wang et al. (2025e) — Perception-Aware Policy Optimization for Multimodal Reasoning (PAPO)

**Citation:** Z. Wang, X. Guo, S. Stoica, H. Xu, H. Wang, H. Ha, X. Chen, Y. Chen, M. Yan, F. Huang, H. Ji. arXiv:2507.06448.
**Venue/ID:** arXiv preprint, 2025-07. https://arxiv.org/abs/2507.06448

## Summary
PAPO targets a specific failure mode in multimodal RL: models often fail not because their reasoning logic is wrong but because they misperceive the image in the first place, and existing GRPO/DAPO-style RL only rewards final-answer correctness, giving no signal that isolates perception errors from reasoning errors. PAPO adds a perception-aware term directly into the RL objective.

## Objective / reward design
- **Implicit Perception Loss:** a KL-divergence term added on top of a base RL objective (GRPO or DAPO), designed to be a drop-in addition rather than a new algorithm — no extra reward model, no additional labeled data, no stronger teacher model required.
- **Double Entropy Loss:** an added regularization/stability term to prevent the perception loss from destabilizing training.
- Net effect: the policy is pushed to get the *visual grounding* right, not just the final answer, closing a gap that outcome-only RL leaves open.

## Key results
+4.4% to +17.5% overall across multimodal benchmarks; +8.0% to +19.1% specifically on vision-dependent (perception-heavy) tasks; 30.5% reduction in perception-attributable errors.

## Availability
Stated "code and data will be made publicly available for research purposes," with a project page; exact license not confirmed from the fetched abstract.

## How Skill-3D uses it
Cited in the related-work sweep on RL-based spatial-reasoning enhancement (paper-2606.07436.txt line 147, "...reinforcement learning...Wang et al. (2025e)...") grouped with other RL-for-spatial-reasoning methods — not adopted as a component; Skill-3D's own GRPO reward (answer + format + tool-efficiency) does not include an explicit perception-loss term.

## WeftOS relevance — Verdict: PATTERN
Directly relevant to the "honest geometry" mandate: PAPO's core insight — separate perception errors from reasoning errors instead of only rewarding final-answer correctness — is exactly the discipline a Rust agent needs when metric claims must be traceable to a metric source (e.g., depth sensor vs. hallucinated scale). Implementing an equivalent perception-consistency term (e.g., KL between predicted and ground-truth-grounded visual features, or a Rust-side geometric-consistency check) is a good candidate addition to the WeftOS reward stack, layered on top of the base GRPO recipe from [52]/[13]. See the RL recipe section of [[papers/benchmarks-and-rl]].
