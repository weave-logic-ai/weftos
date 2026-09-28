# [87] Ye et al. (2026) — Meta Context Engineering via Agentic Skill Evolution

**Citation:** H. Ye, X. He, V. Arak, H. Dong, G. Song. *Meta context engineering via agentic skill evolution.* arXiv:2601.21557.
**Source:** https://arxiv.org/abs/2601.21557

## Summary
Meta Context Engineering (MCE) treats "context engineering" (the discipline of optimizing what's fed to an LLM at inference time) itself as something to be learned and evolved, rather than a fixed set of hand-designed methods. It runs two levels simultaneously: a meta-level agent that refines *engineering skills* (the strategies for building context), and a base-level agent that executes those skills to produce actual context artifacts (files/code).

## Method
- **Two-level architecture:** meta-level agent improves the *process* of context engineering; base-level agent applies the current process to produce context representations (structured as files and code, not just prompt strings).
- **Agentic crossover:** the core evolution mechanism — a deliberative search over the accumulated history of skills, their executions, and their evaluations, mixing/recombining prior skills rather than mutating a single lineage.
- Evaluated in both offline and online settings across five domains.

## Results
- **5.6%–53.8% relative improvement** (mean 16.9%) over state-of-the-art agentic context-engineering methods.
- Reports superior context adaptability, transferability, and efficiency (both context-usage and training cost).

## Code / License
Not stated in the fetched content.

## Relation to Skill-3D
Cited in §2.3's second cluster (procedural memory for decision-time guidance). MCE operates one level of abstraction above the rest of the cluster: instead of learning task-solving skills directly, it learns **skills for building context/skills** (a meta-skill layer). This is conceptually adjacent to Skill-3D's own skill-vs-lesson distinction but MCE's "agentic crossover" (recombining prior skill+execution+evaluation history) is a more explicit evolutionary-search mechanism than Skill-3D's rule-based promote/merge.

## WeftOS Relevance — Verdict: **WATCH**
The "agentic crossover" idea (searching over accumulated skill+execution+evaluation history to recombine, not just append or overwrite) is an interesting future upgrade for the WeftOS skill library's curation step — a middle ground between Skill-3D's simple rule-based merge and SkillOS's [46] full RL-trained curator. Not an immediate ADOPT: standing up a meta-level agent that evolves the context-engineering process itself is a significant scope increase relative to WeftOS's near-term need (a working skill library for Claude Code/Grok/Codex), but worth revisiting once basic skill curation is stable and the question becomes "how do we get better at writing skills," not just "how do we store them."
