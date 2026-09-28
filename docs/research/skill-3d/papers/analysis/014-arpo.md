# [14] Dong et al. (2025) — Agentic Reinforced Policy Optimization (ARPO)

**Citation:** G. Dong, H. Mao, K. Ma, L. Bao, Y. Chen, Z. Wang, Z. Chen, J. Du, H. Wang, F. Zhang, et al. arXiv:2507.19849.
**Venue/ID:** arXiv preprint, 2025-07. https://arxiv.org/abs/2507.19849

## Summary
ARPO is a GRPO-family RL algorithm specialized for multi-turn, tool-using LLM agents (as opposed to single-turn reasoning). It targets the observation that LLMs show a spike in token-entropy immediately after receiving a tool's output — i.e., genuine uncertainty about what to do next — and uses that signal to decide where to spend rollout budget.

## Objective / reward design
- **Entropy-based adaptive rollout:** dynamically switches between whole-trajectory sampling and step-level (branching) sampling, concentrating exploration at high-entropy steps right after tool calls, rather than sampling uniformly across the trajectory.
- **Advantage attribution estimation:** assigns credit at the step level for tool-use interactions, so the policy can learn which specific tool call helped or hurt, not just the trajectory-level outcome (this is the standard GRPO group-relative-advantage machinery adapted to branched trajectories).
- Trained on verifiable rewards (task-outcome correctness), consistent with the DeepSeek-R1/DeepSeekMath GRPO lineage [13],[52].

## Key results
Outperforms trajectory-level RL baselines across 13 benchmarks spanning computational reasoning, knowledge reasoning, and deep-search/tool-use domains, while using roughly half the tool-call budget of prior methods to reach comparable or better performance.

## Availability
Code and datasets released on GitHub (exact license not confirmed from the abstract page).

## How Skill-3D uses it
Cited in the related-work sweep on RL for tool-augmented VLM agents (paper-2606.07436.txt line 151, "train VLMs to use tools through supervised fine-tuning or reinforcement learning ... Chen et al. (2025b); Dong et al. (2025) ...") — grouped with other tool-use RL methods, not adopted directly; Skill-3D's own agentic-RL reward (Eq. 1–2) is simpler (answer + format + tool-efficiency reward) rather than ARPO's entropy-adaptive rollout.

## WeftOS relevance — Verdict: PATTERN
ARPO's core idea — spend exploration budget where the policy is uncertain right after a tool call, and attribute credit per-step rather than per-trajectory — is directly applicable to a Rust agent orchestrating multiple tools (geometry solvers, capture pipelines, memory lookups). The entropy-triggered branching sampling strategy is a training-time (Python) concern, but the Rust trajectory emitter must expose step-level tool-call boundaries and per-step model uncertainty/logprobs for this to be usable — a schema requirement to fold into the trajectory format in [[papers/benchmarks-and-rl]].
