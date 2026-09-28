# [54] Shinn et al. (2024) — Reflexion: Language Agents with Verbal Reinforcement Learning

**Citation:** N. Shinn, F. Cassano, E. Berman, A. Gopinath, K. Narasimhan, S. Yao. *Reflexion: language agents with verbal reinforcement learning*, 2023 (NeurIPS 2023 / arXiv 2023). arXiv:2303.11366.
**Source:** https://arxiv.org/abs/2303.11366

## Summary
Reflexion is the foundational "learn without weight updates" agent paper in this cluster: an agent that receives feedback (scalar or free-form language, from the environment or a self-simulated critic), generates a **verbal self-reflection** on that feedback, and stores the reflection text in an **episodic memory buffer** that conditions future trials on the same or similar task. No gradient update occurs — the "reinforcement" is purely linguistic/in-context.

## Method
- Loop: act → receive feedback → verbally reflect on what went wrong/right → append reflection to episodic memory buffer → retry with memory in context.
- Feedback can be scalar (binary success/fail) or free-form language; the framework is agnostic to feedback source (external environment or an LLM-simulated evaluator).
- No fine-tuning — flexible and cheap, but reflections are per-episode text, not a curated or deduplicated skill representation.

## Results
- **91% pass@1 on HumanEval**, exceeding GPT-4's 80% baseline at the time.
- Also evaluated on sequential decision-making tasks (e.g. ALFWorld-style) and language reasoning; consistent improvement over non-reflective baselines. Ablations vary feedback type/source.

## Code / License
CC BY 4.0 license stated; no explicit repository link found in the fetched content (the method is widely known to have an open reference implementation, but this was not confirmed from the primary source fetched here).

## Relation to Skill-3D
Cited in §2.3's opening sentence alongside Zhao et al. (2024, ExpeL) as the canonical example of "memory-based agents [that] store trajectories for reflection or experience replay" — the baseline paradigm that the rest of §2.3 (skill-based approaches, including Skill-3D) is positioned as improving on, because raw reflections/trajectories are "long, redundant, and noisy" (next sentence, citing Chhikara et al. 2025 and Yan et al. 2025).

## WeftOS Relevance — Verdict: **PATTERN**
Reflexion is the historical baseline every later skill/memory paper in this cluster differentiates against, and it is worth keeping as the "floor" comparison when evaluating WeftOS's own skill library: does structured skill distillation actually beat naive verbal-reflection-in-episodic-buffer? The verbal self-reflection step itself (agent narrates *why* an attempt failed, in natural language, before retrying) is a cheap, always-available fallback worth keeping as a lightweight failure-signal mechanism in the Rust agent loop even after a proper skill library exists — not a replacement for it.
