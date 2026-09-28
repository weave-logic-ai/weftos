# Ref 5: LVAgent

**Citation:** B. Chen, Z. Yue, S. Chen, Z. Wang, Y. Liu, P. Li, Y. Wang. "LVAgent: Long Video Understanding by Multi-Round Dynamical Collaboration of MLLM Agents." arXiv:2503.10200, 2025 (accepted ICCV 2025).

**Source:** arXiv:2503.10200 (https://arxiv.org/abs/2503.10200); code: https://github.com/64327069/LVAgent

## Summary

LVAgent tackles long-video understanding by having a team of MLLM agents (Qwen2-VL, InternVL-2.5, LongVU, LLaVA-Video) collaborate over multiple rounds instead of relying on one model plus external tools. A four-stage loop runs: (1) Selection — a pseudo-label voting pass over 150 sample videos picks the top-3 agents for the task; (2) Perception — an ASP-CLIP retrieval step splits the video into 6 chunks and scores each by CLIP similarity to focus attention on relevant temporal windows; (3) Action — agents independently generate answers with reasoning; (4) Reflection — agents cross-score each other's reasoning (1-10) and the low performers are filtered and regenerate, iterating up to 3 rounds.

## Method specifics

- **Tool API shape:** none in the code/JSON-tool sense — no external tool invocation or program execution. Coordination is peer MLLM discussion.
- **Output return path:** plain text (answers + rationale + scores) exchanged between agents.
- **Planner/executor split:** none — each agent both reasons and answers; team selection is more of a meta-controller step than a distinct planner role.
- **Error handling:** score-based dynamic agent filtering plus iterative multi-round refinement (up to 3 rounds) substitutes for explicit retry/verification.

## Results

LongVideoBench 80.0% (+13.3% over GPT-4o), EgoSchema 82.9% (+5.0%), MLVU 83.9% (+8.2% vs InternVL-2.5), VideoMME 81.7%/86.6% (no-subs/with-subs, +6.0-6.2%). Numbers found in the paper abstract/summary.

## Code/weights

Code released at github.com/64327069/LVAgent under CC BY-NC-SA 4.0 (non-commercial). No separate model weights beyond the off-the-shelf backbones used.

## Skill-3D relation

Cited in §2.2 ("MLLM Agents") in the sentence: "Recent tool-augmented VLM agents have been developed for long-video understanding, high-resolution image analysis, medical diagnosis, and general visual reasoning Chen et al. (2025a); ..." — grouped as an example of a tool-augmented VLM agent applied to long-video understanding, contrasted with Skill-3D's scene-aware, skill-guided 3D spatial agent.

## WeftOS relevance

**SKIP.** No tool-execution, no metric geometry, and no code/structured-call API — it's multi-agent text discussion for video QA, orthogonal to WeftOS's tool-exposed MCP agent architecture and its "honest geometry" constraint. Worth knowing only as a contrast case for multi-agent voting/reflection patterns, not for adoption.
