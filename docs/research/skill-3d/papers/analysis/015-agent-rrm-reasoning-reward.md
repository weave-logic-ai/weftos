# [15] Fan et al. (2026) — Exploring Reasoning Reward Model for Agents (Agent-RRM / ReAgent)

**Citation:** K. Fan, K. Feng, M. Zhang, T. Peng, Z. Li, Y. Jiang, S. Chen, P. Pei, X. Cai, X. Yue. arXiv:2601.22154.
**Venue/ID:** arXiv preprint, 2026-01. https://arxiv.org/abs/2601.22154 — note: date is later than Skill-3D's own listed epoch conventions in this reference set (2025/2026-dated preprints appear throughout refs.tsv); treat as a genuine but very recent/fast-moving preprint, not independently corroborated beyond the fetched abstract.

## Summary
Addresses the weakness of sparse, outcome-only rewards in agentic RL (a single pass/fail signal at the end of a long tool-use trajectory gives no credit assignment for intermediate steps). Introduces Agent-RRM, a reasoning reward model that produces structured feedback for a trajectory: an explicit reasoning trace, a critique pinpointing flaws, and a scalar performance score — richer than a bare reward number.

## Objective / reward design
- Reward model (Agent-RRM) is trained to output three things per trajectory: (1) reasoning trace, (2) targeted critique of failure points, (3) scalar score.
- Three integration strategies tested: **Reagent-C** (critique used as text-augmented refinement signal fed back to the policy), **Reagent-R** (critique/score used as reward-augmented guidance in RL), **Reagent-U** (unified — both channels combined).
- Reagent-U performs best, suggesting text-level critique and scalar reward are complementary supervision channels for agent RL.

## Key results
Evaluated on 12 agentic benchmarks; Reagent-U reaches 43.7% on GAIA and 46.2% on WebWalkerQA (both hard, real-world tool-use benchmarks), the strongest of the three variants tested.

## Availability
Code, models, and datasets stated as "all released" (GitHub: kxfan2002/Reagent per search metadata); license not confirmed from the fetched page.

## How Skill-3D uses it
Cited in the skills-for-RL related-work sweep (paper-2606.07436.txt line 155) as an example of skills/reward models providing "high-level priors for reinforcement learning" — grouped with other skill-RL papers, not built on directly; Skill-3D's own reward (Eq. 1–2, answer+format+tool-efficiency) is a simpler rule-based scalar, not a learned reasoning-reward model.

## WeftOS relevance — Verdict: WATCH
A learned critique-generating reward model is a heavier dependency (needs its own training data and model) than WeftOS's likely first cut (rule-based verifiable rewards, per DeepSeek-R1/GRPO). Worth revisiting once WeftOS has enough labeled trajectory failure cases to train a critique model — the "reasoning trace + critique + score" output shape is a reasonable target schema for a future Rust-side or Python-side reward-model component, but not a day-one adoption.
