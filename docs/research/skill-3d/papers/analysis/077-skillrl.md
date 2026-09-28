# [77] Xia et al. (2026) — SkillRL: Evolving Agents via Recursive Skill-Augmented Reinforcement Learning

**Citation:** P. Xia, J. Chen, H. Wang, J. Liu, K. Zeng, Y. Wang, S. Han, Y. Zhou, X. Zhao, H. Chen, et al. *SkillRL: evolving agents via recursive skill-augmented reinforcement learning.* arXiv:2602.08234.
**Source:** https://arxiv.org/abs/2602.08234

## Summary
SkillRL bridges "raw experience" and "policy improvement" by turning trajectories into a hierarchical, reusable skill store (SkillBank) via automatic discovery, then letting that store **co-evolve** with the policy across RL training — the skill library isn't frozen after one distillation pass, it keeps changing as the policy changes, and vice versa.

## Method
Three components:
1. **Experience-based Distillation** — builds **SkillBank**, a hierarchical skill library, by extracting reusable behavioral patterns from trajectories (hierarchy implies multiple levels of abstraction, unlike a flat skill list).
2. **Adaptive Retrieval Strategy** — combines general heuristics and task-specific heuristics to decide which skills to retrieve for a given state, rather than pure nearest-neighbor similarity.
3. **Recursive Evolution Mechanism** — the skill library and the RL policy update each other in a loop over training, reducing token consumption while improving reasoning quality.

## Results
- Evaluated on **ALFWorld, WebShop**, and **7 search-augmented tasks**.
- State-of-the-art performance, **outperforming strong baselines by >15.3%**, with robustness maintained as task complexity increases.

## Code / License
Authors state code is available on GitHub; the specific repository URL was a placeholder in the fetched abstract page (not resolvable at time of review) — treat availability as **claimed but not verified**.

## Relation to Skill-3D
Cited in §2.3's third cluster (RL priors). SkillRL's hierarchical SkillBank + recursive co-evolution is architecturally the closest reference in the entire 18-item cluster to Skill-3D's own claimed novelty ("Scene Memory and the Skill Library co-evolve," §Fig.2) — the key difference is that Skill-3D grounds the loop in **scene/visual context for 3D spatial reasoning with real tool calls** (detection, depth, reconstruction), while SkillRL operates in text/web-interaction environments (ALFWorld, WebShop) without a 3D-perception grounding requirement.

## WeftOS Relevance — Verdict: **PATTERN**
The hierarchical (multi-level) skill representation and the adaptive (general + task-specific heuristic) retrieval strategy are both good structural references for the Rust skill library — a flat single-tier skill store is likely to underperform once library size grows, consistent with AgentSkillOS's [29] tree-retrieval finding at scale. The "co-evolution" framing is useful vocabulary for WeftOS's own design (matches Skill-3D's own architecture, which WeftOS is directly rewriting), but the RL co-evolution machinery itself is a heavier lift than needed until a training loop exists — track as a pattern, not an immediate build target.
