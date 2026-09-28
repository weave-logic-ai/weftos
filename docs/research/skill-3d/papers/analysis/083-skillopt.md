# [83] Yang et al. (2026) — SkillOpt: Executive Strategy for Self-Evolving Agent Skills

**Citation:** Y. Yang, Z. Gong, W. Huang, Q. Yang, Z. Zhou, Z. Huang, Y. Li, X. Gao, Q. Dai, B. Liu, K. Qiu, Y. Yang, D. Chen, X. Yang, C. Luo. *SkillOpt: executive strategy for self-evolving agent skills.* arXiv:2605.23904.
**Source:** https://arxiv.org/abs/2605.23904

## Summary
SkillOpt claims to be the first **systematic, controllable text-space optimizer** for agent skills — it treats a skill document as an optimizable "external state" and trains it with a discipline modeled on weight-space optimization (learning rate, rejected-update buffer, epoch-wise updates), rather than one-shot LLM generation or uncontrolled self-revision.

## Method
- **Optimizer model** transforms scored rollouts into bounded **add/delete/replace edits** applied to a single skill document (not full regeneration).
- Edits are accepted only if they produce a **strict improvement on held-out validation scores** — a train/held-out split discipline borrowed from ML optimization, applied to prose skill documents.
- **Textual learning-rate budget** (bounds edit magnitude per step), a **rejected-edit buffer** (tracks and avoids retrying failed edits), and **epoch-wise slow/meta updates** (periodic larger revisions vs. per-step small edits).
- Zero inference-time overhead — all optimization happens offline against the skill document; deployment just uses the resulting text.

## Results
- On **GPT-5.5**, across three harnesses: **direct chat +23.5 pts**, **Codex agentic loop +24.8 pts**, **Claude Code +19.1 pts** average accuracy improvement.
- Best-or-tied on **all 52** evaluated (model, benchmark, harness) combinations, beating human-crafted skills, one-shot LLM generation, Trace2Skill, TextGrad, GEPA, and EvoSkill baselines.
- Optimized skills transfer value across model scales and execution environments.

## Code / License
Code stated available at **https://aka.ms/skillopt**.

## Relation to Skill-3D
Cited in §2.3's first cluster ("skills distilled from historical interactions"). SkillOpt is notable within this whole reference set for being explicitly evaluated **inside Claude Code and Codex**, the exact delivery targets named in the WeftOS brief — making it the most externally-validated reference for "does optimizing a skill document actually help inside a real coding-agent harness."

## WeftOS Relevance — Verdict: **ADOPT**
This is the strongest single evidence point in the whole cluster for investing in skill-document quality specifically within Claude Code/Codex-style harnesses (+19–25 points accuracy from optimization alone, on 52/52 combinations). Recommend two things: (1) treat WeftOS's own skill files as optimizable artifacts with a held-out-validation acceptance gate before any automated edit is kept — the bounded add/delete/replace + rejected-edit-buffer discipline is directly implementable as a Rust CLI/CI step over `SKILL.md`-style files; (2) benchmark this specific paper's approach (code is public at aka.ms/skillopt) against Skill-3D's own rule-based promote/merge logic as a candidate upgrade path for the WeftOS skill library's curation step, alongside SkillOS [46] and SkillRL [77].
