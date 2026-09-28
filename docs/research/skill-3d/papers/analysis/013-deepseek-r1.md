# [13] DeepSeek-AI et al. (2025) — DeepSeek-R1: Incentivizing Reasoning Capability in LLMs via RL

**Citation:** DeepSeek-AI, D. Guo, D. Yang, H. Zhang, J. Song, R. Zhang, et al. (200+ authors). ArXiv abs/2501.12948.
**Venue/ID:** arXiv preprint, 2025-01; published in Nature (2025). https://arxiv.org/abs/2501.12948

## Summary
DeepSeek-R1 shows that LLM reasoning (self-reflection, verification, strategy adaptation) can emerge from pure large-scale reinforcement learning on verifiable tasks, without SFT on human-annotated chain-of-thought. DeepSeek-R1-Zero is trained via RL directly on the base model; DeepSeek-R1 adds a small cold-start SFT stage plus multi-stage RL for readability/alignment. The RL algorithm used is GRPO (Group Relative Policy Optimization), introduced by the same lab in DeepSeekMath [52].

## Objective / reward design
- **Algorithm:** GRPO — see [[052-deepseekmath-grpo]] for the objective written out; DeepSeek-R1 reuses it as-is rather than introducing a new optimizer.
- **Reward:** rule-based, verifiable rewards — correctness of final answer (math/code, checked automatically, e.g. against unit tests or exact-match) plus a format reward enforcing the `<think>...</think>` reasoning-then-answer structure. Deliberately avoids a learned neural reward model to sidestep reward hacking.
- Emergent behaviors (longer chain-of-thought, self-verification, "aha moments") arise purely from this RL signal, not from imitation.
- Distillation: reasoning patterns from the large RL-trained model are distilled into smaller dense models (1.5B–70B) via SFT on R1-generated trajectories, which outperforms applying RL directly to the small models.

## Key results
Matches/approaches OpenAI o1-level performance on math, code-competition, and STEM reasoning benchmarks; distilled smaller models transfer much of this gain.

## Compute / availability
Models and weights released publicly (MIT-licensed model weights per DeepSeek's public release); training compute not detailed in the fetched abstract — large-scale (multi-thousand-GPU class), consistent with public reporting.

## How Skill-3D uses it
Directly cited as the source of the GRPO algorithm used in Skill-3D's own agentic RL stage (paper-2606.07436.txt line 199: "Group Relative Policy Optimization (GRPO) DeepSeek-AI et al. (2025); Shao et al. (2024b)"), and in §1 as the general precedent that RL-with-verifiable-rewards elicits reasoning.

## WeftOS relevance — Verdict: ADOPT (recipe, not code)
DeepSeek-R1's rule-based/verifiable-reward + GRPO recipe is the direct template for Skill-3D's own post-training and should be WeftOS's default RL recipe too: cheap, hard-to-game rewards (format + exact-match correctness) computed by Rust-side verifiers, with GRPO training done in Python from Rust-emitted trajectories. See the RL recipe section of [[papers/benchmarks-and-rl]].
