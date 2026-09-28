# [26] Jiao et al. (2026) — Agentic Proposing: Enhancing LLM Reasoning via Compositional Skill Synthesis

**Citation:** Z. Jiao, S. Wang, Z. Zhang, X. Ren, W. Wang, B. Zhao, H. Wei, L. Zhang. *Agentic proposing: enhancing large language model reasoning via compositional skill synthesis.* arXiv:2602.03279.
**Source:** https://arxiv.org/abs/2602.03279

## Summary
Agentic Proposing reframes **problem/dataset synthesis** (not agent task-solving) as a goal-driven sequential decision process: a specialized "proposer" agent dynamically selects and composes modular reasoning skills to construct new, verifiable training problems, rather than generating them from fixed templates. It trains an Agentic-Proposer-4B model with Multi-Granularity Policy Optimization (MGPO) to produce hard, structurally-valid problems across math, code, and science.

## Method
- **Compositional skill synthesis:** the proposer iteratively reflects and invokes tools, composing modular reasoning skills (rather than fixed templates) to balance problem validity against difficulty.
- **MGPO (Multi-Granularity Policy Optimization):** the RL algorithm used to train the 4B proposer so its synthesized problems are both solvable/verifiable and appropriately hard.
- Downstream: a 30B solver is trained on the proposer's synthesized trajectories.

## Results
- Benchmark: **AIME25** — a 30B solver trained on only 11,000 agent-synthesized trajectories reaches **91.6% accuracy**, rivaling frontier proprietary models (cited as comparable to GPT-5-class performance).
- Reports robust cross-domain generalization (math/code/science) and downstream solvers outperforming baselines trained on non-agentic synthesis.

## Code / License
Page indicates **CC-BY 4.0** license; no explicit code repository link found in the fetched content.

## Relation to Skill-3D
Cited in §2.3's third cluster — skills as "high-level priors for reinforcement learning" (with Xia et al. 2026, Wang et al. 2025b, Ouyang et al. 2026). Unlike the rest of this cluster, Agentic Proposing uses skills to guide *data/problem generation* for training rather than to guide an executor agent's own task-solving — it is the odd one out, evidence that "skill" as a unit of composable reasoning generalizes beyond execution-time guidance into synthetic-data curricula.

## WeftOS Relevance — Verdict: **WATCH**
Not directly about an execution-time skill library, so it doesn't inform the Rust skill library's runtime design. It is relevant as a future direction: once WeftOS has a mature skill library, the same "compose skills to synthesize new training problems" idea could be reused to auto-generate SFT/RL curricula for Claude Code / Grok / Codex-delivered agent skills (e.g., synthesizing new 3D-spatial-reasoning or geometry-honesty test cases from composed skill primitives). Worth revisiting once the skill library has enough coverage to compose from, not before.
