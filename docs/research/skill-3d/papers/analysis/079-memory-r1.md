# [79] Yan et al. (2025) — Memory-R1: Enhancing LLM Agents to Manage and Utilize Memories via Reinforcement Learning

**Citation:** S. Yan, X. Yang, Z. Huang, E. Nie, Z. Ding, Z. Li, X. Ma, J. Bi, K. Kersting, J. Z. Pan, et al. *Memory-r1: enhancing large language model agents to manage and utilize memories via reinforcement learning.* arXiv:2508.19828.
**Source:** https://arxiv.org/abs/2508.19828

## Summary
Memory-R1 replaces static/heuristic memory-bank management (the kind used in systems like Mem0 [10]) with two RL-trained agents: a **Memory Manager** that learns discrete memory operations, and an **Answer Agent** that learns to select and reason over relevant entries. Both are trained with outcome-driven RL rather than hand-written extraction/consolidation rules.

## Method
- **Memory Manager agent:** learns structured operations — **ADD, UPDATE, DELETE, NOOP** — for managing memory-bank entries, replacing fixed heuristics with a learned policy over what to write/overwrite/forget.
- **Answer Agent:** pre-selects relevant memory entries, then reasons over the selected subset to produce an answer (separating retrieval-selection from answer generation, each optimizable).
- **Training:** outcome-driven RL (PPO and GRPO variants), notably data-efficient — effective with only **152 training QA pairs**.

## Results
- Benchmarks: **LoCoMo, MSC, LongMemEval**.
- Tested at **3B–14B** model scale.
- Outperforms strong baselines and generalizes across diverse question types (single-hop, multi-hop, temporal per the shared LOCOMO-family task taxonomy).

## Code / License
Not stated beyond standard arXiv terms in the fetched content.

## Relation to Skill-3D
Cited in §2.3's opening sentence alongside Chhikara et al. 2025 (Mem0) as the "noisy raw memory" motivation for skill-based approaches: "raw trajectories are often long, redundant, and noisy [Mem0]; [Memory-R1]." Memory-R1 is itself already a step beyond raw-trajectory storage (it learns *what to keep*), making it a transitional reference between pure trajectory replay (Reflexion [54], ExpeL [94]) and full skill distillation (the rest of the cluster).

## WeftOS Relevance — Verdict: **ADOPT (design pattern for memory-tier ops)**
The **ADD/UPDATE/DELETE/NOOP** operation taxonomy is a clean, minimal, and directly implementable interface for a Rust memory-management layer sitting in front of AgentDB/RVF — worth adopting as the literal operation set for whatever component decides what gets written to durable memory, whether that decision is rule-based (as in Skill-3D's current promote/merge logic) or eventually learned (as here). The retrieval/answer split (select-then-reason as two separable steps) is also a good decomposition to keep when the memory tier's retrieval quality needs to be evaluated independently of the agent's reasoning quality. Data efficiency (152 QA pairs) is notable if WeftOS ever wants to train, rather than hand-write, its own curation policy — a lower bar than expected.
