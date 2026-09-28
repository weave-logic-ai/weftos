# [32] Liang et al. (2026) — SkillNet: Create, Evaluate, and Connect AI Skills

**Citation:** Y. Liang, R. Zhong, H. Xu, C. Jiang, Y. Zhong, R. Fang, J. Gu, S. Deng, Y. Yao, M. Wang, et al. *SkillNet: create, evaluate, and connect ai skills.* arXiv:2603.04448.
**Source:** https://arxiv.org/abs/2603.04448

## Summary
SkillNet is an infrastructure/platform paper: it argues agents fail to advance long-term because they don't systematically **accumulate and transfer** skills across sessions, and proposes a unified ontology plus a large public skill repository (600K+ skills) with multi-dimensional quality evaluation and routing.

## Method
- **Unified ontology:** skills from heterogeneous sources are normalized into one schema with explicit relational connections between skills (a skill graph, not just a flat store).
- **Multi-dimensional evaluation:** every skill is scored on **Safety, Completeness, Executability, Maintainability, Cost-awareness** before being trusted.
- **SkillNet-Fabric:** task-specific skill routing implemented via lightweight per-domain "Wikis" rather than a single global router.
- **SkillNet-Gym:** a benchmark specifically for skill retrieval and composition.

## Results
- Evaluated on **ALFWorld, WebShop, ScienceWorld**: **+40% average reward**, **-30% execution steps** across multiple backbone models vs. baselines.

## Code / License
Public platform at skillnet.openkg.cn (OpenKG project); explicit code license not stated in the fetched abstract.

## Relation to Skill-3D
Cited in §2.3's second cluster (procedural memory for decision-time guidance). SkillNet is the most "ecosystem platform"-flavored reference in this group — closer to AgentSkillOS [29] in ambition (managing a huge public skill corpus) than to XSkill/Skill-3D's per-agent dual-memory design.

## WeftOS Relevance — Verdict: **PATTERN**
The five-dimension quality rubric (Safety, Completeness, Executability, Maintainability, Cost-awareness) is a good starting checklist for a **skill review/promotion gate** before a dynamically-extracted skill graduates from ephemeral scene memory into the durable skill library that ships to Claude Code/Grok/Codex — this maps naturally onto WeftOS's existing governance-gate concept (clawft-governance-specialist's gate backend). Not an ADOPT because SkillNet's own routing/ontology tech (per-domain "Wikis," a 600K-skill public corpus) is out of scope for a project building its own closed Rust skill library on AgentDB/RVF — the quality-rubric idea is the transferable part, not the platform.
