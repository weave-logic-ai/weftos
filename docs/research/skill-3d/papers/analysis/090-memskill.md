# [90] Zhang et al. (2026a) — MemSkill: Learning and Evolving Memory Skills for Self-Evolving Agents

**Citation:** H. Zhang, Q. Long, J. Bao, T. Feng, W. Zhang, H. Yue, W. Wang. *MemSkill: learning and evolving memory skills for self-evolving agents.* arXiv:2602.02474.
**Source:** https://arxiv.org/abs/2602.02474

## Summary
MemSkill applies the "skill" abstraction to **memory management operations themselves** (extraction, consolidation, pruning) rather than to task-solving actions. Instead of hand-coding how memory gets extracted/consolidated/pruned from interaction traces (as Mem0 [10] does), it treats these operations as **learnable and evolvable skills** with a controller-executor-designer loop that reviews hard cases and proposes new memory-management skills over time.

## Method
- **Controller:** learns to select a small relevant subset of memory-management skills for the current context, rather than applying every operation uniformly.
- **Executor:** an LLM applies the selected skills to produce skill-guided memory writes.
- **Designer:** periodically reviews **hard cases** (where selected skills produced incorrect/incomplete memories), and proposes refinements to existing skills or entirely new skills — a closed curation loop distinct from, but structurally similar to, SkillOS's [46] curator.

## Results
- Tested on **LoCoMo, LongMemEval, HotpotQA, ALFWorld** — four benchmarks spanning long-conversation memory, long-context QA, multi-hop QA, and embodied text-game tasks.
- Reports improved performance over strong baselines with good generalization across this heterogeneous benchmark set.

## Code / License
Code available at GitHub; license stated as **CC BY 4.0**.

## Relation to Skill-3D
Cited in §2.3's second cluster (procedural memory for decision-time guidance). MemSkill is the reference in this cluster most directly analogous to applying Skill-3D's own "skill" abstraction one layer down — not skills for solving 3D spatial tasks, but skills for **how to write memory**. Its controller/executor/designer split is architecturally close to Memory-R1's [79] manager/answer-agent split but adds an explicit meta-level "designer" that evolves the skill set itself, closer to SkillOS [46] and MCE [87].

## WeftOS Relevance — Verdict: **PATTERN**
The controller (select relevant memory-skill) / executor (apply it) / designer (review hard cases, propose new memory-skills) three-role split is a clean architectural template for the memory-write side of a Rust skill library, complementary to AgentSkillOS's [29] retrieval-side capability tree. Recommend treating this as a named pattern to reference when designing the WeftOS memory-consolidation component specifically (as distinct from the task-skill library) — the "designer reviews hard cases" step is a lightweight, concrete way to bootstrap self-improving memory-write rules before committing to a full RL curator like SkillOS.
