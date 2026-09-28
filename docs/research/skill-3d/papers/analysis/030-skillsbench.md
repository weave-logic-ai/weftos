# [30] Li et al. (2026b) — SkillsBench: Benchmarking How Well Agent Skills Work Across Diverse Tasks

**Citation:** X. Li, W. Chen, Y. Liu, S. Zheng, X. Chen, Y. He, Y. Li, B. You, H. Shen, J. Sun, et al. *SkillsBench: benchmarking how well agent skills work across diverse tasks.* arXiv:2602.12670.
**Source:** https://arxiv.org/abs/2602.12670

## Summary
SkillsBench is a benchmark (not a method) for measuring whether curated "agent skills" — structured procedural-knowledge packages injected at inference time — actually help, and by how much, across model/harness combinations. It pairs 87 tasks across 8 domains with hand-curated skills and **deterministic verifiers**, enabling paired (with-skill vs. without-skill) comparison rather than relying on subjective LLM judging.

## Method
- **87 tasks / 8 domains**, each with a curated skill package and a deterministic pass/fail verifier.
- **Paired evaluation** methodology: same task run with and without the skill, same model/harness, isolating the skill's marginal contribution.
- Tested across **18 model-harness configurations**.

## Results
- Curated skills raise average pass rate from **33.9% → 50.5%** (+16.6 points absolute, 25.5% normalized gain).
- Per-configuration gains range from **+4.1 to +25.7 points** — skill value is highly harness/model-dependent, not uniform.
- **Focused skills (≤3 modules) beat larger bundles** — comprehensiveness hurts, not helps.
- Smaller models **with** skills can match larger models **without** skills.

## Code / License
CC BY 4.0 license stated; no explicit repository link found in the fetched content.

## Relation to Skill-3D
Cited in §2.3's second cluster — "skills as procedural memory for decision-time guidance" (with Liu et al. 2026a, Liang et al. 2026, Jiang et al. 2026, Zhang et al. 2026a, Ye et al. 2026). SkillsBench is evaluation methodology, not an agent architecture; it is the closest reference in the whole cluster to being directly re-usable as an **evaluation harness** for any skill library, including Skill-3D's own.

## WeftOS Relevance — Verdict: **ADOPT (evaluation methodology)**
This is the strongest, most directly actionable finding in the cluster and should shape how WeftOS validates its own skill library, independent of any specific skill content: (1) always evaluate skills with **paired, deterministic** with/without comparisons per task, not vibes-based judging; (2) the "focused skills ≤3 modules beat large bundles" finding is a concrete design constraint — cap skill packages delivered to Claude Code/Grok/Codex at a small module count rather than writing exhaustive skills; (3) the small-model-with-skill vs. large-model-without-skill result is a strong argument for investing in the skill library as a cost/latency lever (tier-2 Haiku-class models + good skills competing with tier-3 models), consistent with WeftOS's existing 3-tier model routing design.
