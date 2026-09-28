# [25] Jiang et al. (2026) — XSkill: Continual Learning from Experience and Skills in Multimodal Agents

**Citation:** G. Jiang, Z. Su, X. Qu, Y. R. Fung. *Xskill: continual learning from experience and skills in multimodal agents.* arXiv:2603.12056.
**Source:** https://arxiv.org/abs/2603.12056

## Summary
XSkill is a dual-stream continual-learning framework for multimodal agents that separates two knowledge types distilled from past trajectories: **experiences** (concise, action-level guidance for tool selection and low-level decisions) and **skills** (structured, task-level guidance for planning). Both streams are grounded in visual observations rather than text alone, and both feed back into future accumulation, forming a closed continual-learning loop.

## Method
- **Two-phase loop:** accumulation phase distills knowledge from multi-path rollouts via visually grounded summarization and cross-rollout critique; inference phase retrieves and adapts the relevant experience/skill pair to the current visual context.
- **Multimodal grounding:** extraction and retrieval keys are tied to visual observations, not just task text — a direct analog to scene-conditioned retrieval.
- Usage history from inference is fed back into accumulation, closing the loop (skills/experiences improve as more tasks are run).

## Results
- Evaluated across **5 benchmarks** and **4 backbone models**.
- Outperforms both tool-only and learning-based (memory/skill) baselines, with stronger zero-shot generalization to unseen tasks.
- Ablations show experience-stream and skill-stream contribute complementary reasoning improvements (neither stream alone matches the combination).

## Code / License
Not stated in the fetched abstract — mark as not found.

## Relation to Skill-3D
Cited in §2.3 in the "skills distilled from historical interactions" group (with Xu and Yan 2026, Li et al. 2026a, He et al. 2026, Yang et al. 2026). XSkill is the closest of the 18 references in this cluster to Skill-3D's own architecture: both are **multimodal, visually-grounded** dual-memory systems (XSkill's experience/skill split parallels Skill-3D's Scene Memory / Skill Library split), and both close the loop by feeding inference-time usage back into future distillation.

## WeftOS Relevance — Verdict: **ADOPT (pattern-level)**
The experience-vs-skill separation (low-level action guidance vs. high-level task guidance, both visually grounded) is directly applicable to a Rust skill library serving vision-capable agents (e.g. MentraOS egocentric capture). Recommend mirroring the two-tier split explicitly in the WeftOS skill schema — a `tactic` tier (tool/argument-level, analogous to XSkill's "experience") and a `plan` tier (task-level workflow, analogous to XSkill's "skill") — rather than one flat skill representation, and grounding both tiers' embedding keys in scene/visual context so retrieval can condition on what the agent currently sees, not just task text.
