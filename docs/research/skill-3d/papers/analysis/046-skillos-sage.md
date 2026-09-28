# [46] Ouyang et al. (2026) — SkillOS: Learning Skill Curation for Self-Evolving Agents

**Citation:** S. Ouyang, J. Yan, Y. Chen, R. Han, Z. Wang, B. D. Mishra, R. Meng, C. Li, Y. Jiao, K. Zha, et al. *SkillOS: learning skill curation for self-evolving agents.* arXiv:2605.06614.
**Source:** https://arxiv.org/abs/2605.06614

## Summary
SkillOS separates the roles of *using* skills and *curating* skills: a frozen executor agent retrieves and applies skills unmodified, while a separately-trained **curator** manages the external skill repository via reinforcement learning, learning long-term curation policy rather than using fixed heuristics (e.g., "always add," "dedupe by similarity") for what to keep, merge, or discard.

## Method
- **Frozen executor + trainable curator** split — decouples task execution from skill-repository management, so curation quality can be optimized independently of the executor model.
- **RL training over grouped task streams:** earlier trajectories populate the repo (SkillRepo); later, related tasks are used to score/evaluate the curator's updates, giving the curator a learning signal tied to downstream task success.
- Skills evolve into structured **Markdown files** that can encode higher-level meta-skills over time (skills about when/how to use other skills).

## Results
Reports consistent improvements over memory-free and memory-based baselines on multi-turn and reasoning task suites, in both effectiveness and efficiency; specific numeric deltas not available from the fetched abstract.

## Code / License
Not stated in the fetched content.

## Relation to Skill-3D
Cited in §2.3's third cluster — "skills as high-level priors for reinforcement learning" (with Xia et al. 2026, Wang et al. 2025b, Jiao et al. 2026). SkillOS is architecturally close to Skill-3D's own Scene Memory → Skill Library pipeline (successes promoted to skills, failures used as correction signal) but goes further by training a dedicated curator policy via RL, whereas Skill-3D's promotion/merge logic (§3.1, "Successes as Workflows") is rule-based (promote if no compatible skill exists; merge only if new coverage added).

## WeftOS Relevance — Verdict: **WATCH**
The frozen-executor/trainable-curator split and Markdown-as-skill-representation are both directly compatible with a Rust skill library (skills-as-files is already the Claude Code/Grok/Codex delivery format WeftOS targets). Not an immediate ADOPT because it requires standing up an RL training loop around the curator, which is a heavier investment than WeftOS's current rule-based promotion/merge needs justify — but it is the clearest evolution path if rule-based curation (à la Skill-3D's own "promote if no compatible skill, else merge only if new coverage") proves insufficient as the library scales. Revisit once curation-quality problems (duplicate/stale skills, poor merge decisions) actually show up in practice.
