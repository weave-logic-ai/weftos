# [94] Zhao et al. (2024) — ExpeL: LLM Agents Are Experiential Learners

**Citation:** A. Zhao, D. Huang, Q. Xu, M. Lin, Y. Liu, G. Huang. *ExpeL: LLM agents are experiential learners.* Proceedings of the AAAI Conference on Artificial Intelligence, Vol. 38, pp. 19632–19642, 2024.
**Source:** https://arxiv.org/abs/2308.10144 (confirmed as the ExpeL paper, AAAI-24)

## Summary
ExpeL is, with Reflexion [54], one of the two foundational "parameter-free experiential learning" agent papers this cluster builds on. It autonomously gathers experiences from a set of training tasks, extracts natural-language **insights** from them (without any gradient update), and at inference time recalls both the extracted insights and relevant past experiences to inform decisions — explicitly motivated by the fact that frontier models (GPT-4, Claude, at time of writing) are API-only, so fine-tuning isn't an option for most users.

## Method
- **Experience gathering:** the agent autonomously runs a collection of training tasks and records its trajectories.
- **Insight extraction:** natural-language insights are distilled from the recorded experiences — a step conceptually upstream of, and simpler than, the later cluster's "skill" abstraction (insights are general lessons, not structured, retrievable, tool-workflow-level skills).
- **Recall at inference:** the agent conditions on both extracted insights and specific past experiences when making new decisions.
- No fine-tuning at any stage — purely in-context / retrieval-based learning.

## Results
Reports "robust learning efficacy" with performance improving consistently as more experience accumulates, plus qualitative evidence of emerging capabilities and transfer-learning potential. Specific benchmark numbers were not available from the fetched abstract page.

## Code / License
CC BY 4.0 license stated on the arXiv page; explicit repository link not confirmed from the fetched content (ExpeL is known to have a public reference implementation, but this was not verified from the primary source here).

## Relation to Skill-3D
Cited in §2.3's opening sentence, paired with Shinn et al. 2024 (Reflexion), as the baseline "memory-based agents store trajectories for reflection or experience replay" paradigm that motivates the rest of §2.3's move toward structured, reusable skills. ExpeL's insight-extraction step is the direct conceptual ancestor of every "distill reusable knowledge from trajectories" method in this cluster (Xu and Yan [78], XSkill [25], SkillRL [77], etc.) — the difference is granularity and structure: ExpeL's insights are free-text lessons, while the skill-cluster papers formalize insights into retrievable, composable, often tool-workflow-level units.

## WeftOS Relevance — Verdict: **PATTERN**
Like Reflexion, ExpeL is the historical floor this whole cluster (and Skill-3D itself) is measured against. Its core distinction — insight (general lesson) vs. specific experience (concrete trajectory) — maps directly onto Skill-3D's own lesson/skill split ("failed rollouts are attached as lessons") and is worth keeping as explicit vocabulary in the WeftOS skill schema: a **lessons** store (free-text, cheap to write, low structure) distinct from the **skill library proper** (structured, retrievable, tool-workflow-level), rather than collapsing both into one representation.
