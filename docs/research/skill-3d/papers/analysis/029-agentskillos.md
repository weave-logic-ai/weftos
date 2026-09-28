# [29] Li et al. (2026a) — Organizing, Orchestrating, and Benchmarking Agent Skills at Ecosystem Scale (AgentSkillOS)

**Citation:** H. Li, C. Mu, J. Chen, S. Ren, Z. Cui, Y. Zhang, L. Bai, S. Hu. *Organizing, orchestrating, and benchmarking agent skills at ecosystem scale.* arXiv:2603.02176.
**Source:** https://arxiv.org/abs/2603.02176

## Summary
AgentSkillOS is presented as the first principled framework for skill **selection, orchestration, and ecosystem-level management** at scale (tested from 200 to 200,000 skills). It addresses the operational problem of a skill *library becoming a skill ecosystem* — too large to search flatly, and too interdependent to invoke as isolated single calls.

## Method
- **Organization:** skills are indexed in a **capability tree** via node-level recursive categorization (a taxonomy, not a flat vector index), used for scalable discovery/retrieval as the library grows.
- **Orchestration:** multiple skills are composed and executed through **DAG-based pipelines** rather than one-shot flat invocation, allowing skill outputs to feed other skills' inputs.
- **Benchmark:** 30 artifact-rich tasks across 5 categories (data computation, document creation, motion/video, visual design, web interaction); quality judged via LLM pairwise comparison aggregated with a **Bradley-Terry model**.

## Results
- Tree-based retrieval closely approximates oracle (ground-truth) skill selection even at the largest ecosystem scale tested (200K skills).
- DAG-based orchestration **substantially outperforms** flat/single-call invocation given the *identical* skill set — i.e., the orchestration structure, not just the skill content, drives quality.

## Code / License
GitHub repository referenced in the paper; license stated as **CC BY 4.0**.

## Relation to Skill-3D
Cited in §2.3's first cluster — "skills distilled from historical interactions" (with He et al. 2026, Xu and Yan 2026, Yang et al. 2026). It addresses a scale problem Skill-3D does not yet face (Skill-3D pools a single global Scene Memory/Skill Library across training benchmarks, order of magnitude far smaller than 200K skills), but it's directly relevant to what happens as a skill library grows past a few hundred entries.

## WeftOS Relevance — Verdict: **ADOPT (design pattern)**
This is the most load-bearing reference in the cluster for WeftOS's long-term skill-library scaling story. Two concrete recommendations: (1) don't rely on flat HNSW/embedding retrieval alone once the skill count grows past low hundreds — pair it with a **capability-tree taxonomy** for coarse narrowing before vector search, matching AgentSkillOS's finding that tree retrieval approximates oracle selection at 200K scale; (2) when an agent needs more than one skill for a task, orchestrate them as a **DAG rather than sequential flat calls** — this is a bigger lever than skill quality alone per their ablation. Both map cleanly onto a Rust implementation: a capability-tree index alongside AgentDB/RVF's HNSW, and a DAG-execution layer already implied by WeftOS's agent effect algebra / governance gates.
