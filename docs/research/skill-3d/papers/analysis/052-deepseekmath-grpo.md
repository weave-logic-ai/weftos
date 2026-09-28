# [52] Shao et al. (2024b) — DeepSeekMath: Pushing the Limits of Mathematical Reasoning in Open Language Models

**Citation:** Z. Shao, P. Wang, Q. Zhu, R. Xu, J. Song, X. Bi, H. Zhang, M. Zhang, Y. Li, Y. Wu, et al. arXiv:2402.03300.
**Venue/ID:** arXiv preprint, 2024-02. https://arxiv.org/abs/2402.03300

## Summary
DeepSeekMath continues pretraining DeepSeek-Coder-Base-v1.5 7B on 120B math-related tokens mined from Common Crawl plus natural-language and code data, reaching 51.7% on MATH without external tools/voting (60.9% with 64-sample self-consistency), approaching Gemini-Ultra/GPT-4-era math performance at 7B scale. Its lasting contribution to the field, and the reason Skill-3D cites it, is **introducing GRPO** (Group Relative Policy Optimization).

## The GRPO objective (as canonically formulated by this paper)
GRPO removes PPO's learned value/critic network. For each query `q`, sample a *group* of `G` outputs `{o_1,...,o_G}` from the old policy `π_θ_old`, score each with a reward model/rule to get `{r_1,...,r_G}`, then compute a **group-relative advantage** by normalizing within the group instead of bootstrapping a value function:

```
A_i = (r_i - mean({r_1,...,r_G})) / std({r_1,...,r_G})
```

The policy is then updated with a PPO-style clipped surrogate objective applied per-token, using this group-normalized advantage, plus a KL penalty toward a reference policy:

```
J_GRPO(θ) = E[ (1/G) Σ_i (1/|o_i|) Σ_t
    min( ρ_{i,t}(θ)·A_i , clip(ρ_{i,t}(θ), 1-ε, 1+ε)·A_i )
  ] − β · D_KL(π_θ || π_ref)

where ρ_{i,t}(θ) = π_θ(o_{i,t}|q,o_{i,<t}) / π_θ_old(o_{i,t}|q,o_{i,<t})
```

This is memory- and compute-cheaper than PPO (no critic network to train) and is the algorithm both DeepSeek-R1 [13] and Skill-3D itself use for RL.

## Key results
MATH: 51.7% (single-pass), 60.9% (self-consistency@64); competitive with Gemini-Ultra/GPT-4 at far smaller (7B) scale.

## Availability
Model/code details not confirmed from the fetched abstract page — check the DeepSeek-Math GitHub release for weights/license before depending on it directly.

## How Skill-3D uses it
Directly cited as the source of GRPO used in Skill-3D's own agentic-RL stage (paper-2606.07436.txt line 199, Eq. 1–2): "Group Relative Policy Optimization (GRPO) DeepSeek-AI et al. (2025); Shao et al. (2024b)." Skill-3D's reward is `R(τ) = R_ans(τ) + R_fmt(τ) + R_tool(τ)`, a rule-based scalar fed straight into this same GRPO update rule.

## WeftOS relevance — Verdict: ADOPT (the training algorithm)
GRPO is the concrete algorithm WeftOS's Python training side should implement first: it needs only a reward function (computable in Rust or Python) and grouped rollouts, no critic network. This dictates the trajectory schema the Rust agent must emit — see the "what the Rust side must emit" section of [[papers/benchmarks-and-rl]].
