# [65] Wang et al. (2025b) — Reinforcement Learning for Self-Improving Agent with Skill Library (SAGE)

**Citation:** J. Wang, Q. Yan, Y. Wang, Y. Tian, S. S. Mishra, Z. Xu, M. Gandhi, P. Xu, L. L. Cheong. *Reinforcement learning for self-improving agent with skill library.* arXiv:2512.17102.
**Source:** https://arxiv.org/abs/2512.17102

## Summary
This paper proposes SAGE (Skill Augmented GRPO for self-Evolution), an RL framework that integrates a growing skill library directly into policy optimization rather than treating skills as static prompt-time context. It argues prior skill-augmented agents mostly use skills only at inference (prompting), leaving the underlying policy untouched; SAGE instead lets skill use and skill generation both shape the RL objective.

## Method
- Built on **GRPO** (Group Relative Policy Optimization), extended with a **Skill-integrated Reward** term added to the outcome-based reward, so the policy is rewarded not just for task success but for good skill generation/use.
- **Sequential Rollout:** the agent is deployed across a chain of tasks; skills generated on earlier tasks in the chain accumulate into a library available to later tasks in the same chain — a progressive, within-episode-chain skill-building mechanism, distinct from a persistent cross-episode store.
- Base policy is supervised-finetuned on expert trajectories before RL.

## Results
- Benchmark: **AppWorld**.
- **+8.9%** Scenario Goal Completion vs. existing approaches.
- **-26%** interaction steps, **-59%** tokens generated — efficiency gains alongside accuracy gains.

## Code / License
Not stated in the fetched content.

## Relation to Skill-3D
Cited in §2.3's third cluster — "skills as high-level priors for reinforcement learning." SAGE's Skill-integrated Reward is a more direct RL-in-the-loop analog to Skill-3D's own third stage (§3, "skill-guided trajectories are used for agentic SFT and GRPO... encouraging compact agents to internalize skill selection, tool use, and evidence-grounded spatial reasoning") — both use GRPO and both fold skill quality into the training signal rather than treating the skill library as a frozen, inference-only artifact.

## WeftOS Relevance — Verdict: **PATTERN**
The Skill-integrated Reward idea (reward shaped by skill generation/use quality, not just task outcome) is directly relevant if/when the WeftOS Rust rewrite trains or fine-tunes any compact model against its own skill library, mirroring Skill-3D's own SFT+GRPO stage. Not an immediate ADOPT for the skill-library *storage/retrieval* design (that's orthogonal to RL training), but a strong reference to hand to whoever designs a future post-training loop for a WeftOS-hosted small model, alongside SkillRL [77] and SkillOS [46].
