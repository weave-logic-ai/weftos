# Ref 67 — Wang et al. (2025d), VisuoThink

**Citation:** Y. Wang, S. Wang, Q. Cheng, Z. Fei, L. Ding, Q. Guo, D. Tao, and X. Qiu. "VisuoThink: Empowering LVLM Reasoning with Multimodal Tree Search." arXiv preprint arXiv:2504.09130.

**Source:** arXiv:2504.09130, https://arxiv.org/abs/2504.09130 (submitted Apr 12 2025, 12 pages); accepted ACL 2025 Main. Code: https://github.com/ekonwang/VisuoThink.

## Summary
VisuoThink is a training-free, inference-time framework that lets a large vision-language model (LVLM) "think slowly" over geometry and spatial-navigation problems by interleaving visual and textual reasoning steps and searching over them with look-ahead tree search (MCTS-style) instead of committing to a single chain-of-thought. Each search node carries an evolving visual-textual state; the model proposes candidate actions (e.g., draw a construction line, annotate a shape), executes them to update the visual state, and evaluates the resulting branch, letting it backtrack from bad intermediate steps rather than being locked into one linear trajectory.

## Method specifics
- **Tool API shape:** Code generation — the model emits Python (matplotlib-based) drawing/editing snippets that are executed to progressively construct or annotate diagrams, rather than JSON/structured function calls.
- **Return path:** Tool outputs return as updated images (the modified sketch/diagram), which re-enter the LVLM's visual context for the next reasoning step; textual facts deduced from the image are also appended to the running trace.
- **Planner/executor split:** A single LVLM plays both roles across a Thought → Action → Observation cycle inside a look-ahead tree search controller; the search procedure (MCTS) acts as an external planning wrapper around one model rather than a separate learned planner network.
- **Error handling:** Implicit via tree search — bad branches are pruned/backtracked through the search's value estimates rather than an explicit retry or verification sub-routine; no dedicated self-correction module beyond re-expanding alternative nodes.

## Results
- Geometry: Geomverse-109 — 28.9% (GPT-4o), 25.6% (Qwen2-VL-72B), 27.8% (Claude-3.5-Sonnet). Geometry3K — 33.3% (GPT-4o), 25.0% (Qwen2-VL-72B), 43.8% (Claude-3.5-Sonnet). Reported gains of +17.1% and +16.7% over plain Chain-of-Thought and Visual Sketchpad baselines respectively.
- Spatial/navigation: Visual Navigation (level-3) — 93.8% (GPT-4o), 81.3% (Qwen2-VL-72B), 93.8% (Claude-3.5-Sonnet); Visual Tiling up to 84.0% (Claude-3.5-Sonnet). These numbers came from a secondary WebFetch summarization pass and were not independently cross-checked against the primary tables — treat as indicative, not verified to the decimal.

## Code / license
Code open-sourced at github.com/ekonwang/VisuoThink. Repository license: not found (GitHub API reports no license file). ACL papers carry ACL Anthology's standard terms; the arXiv preprint follows arXiv's non-exclusive license.

## Relation to Skill-3D
Cited at Skill-3D §2.1 (not §2.2): "Other works enhance spatial reasoning through prompting, mental simulation, visual chain-of-thought, reinforcement learning, code-driven 3D reasoning, and generative imagination of 3D space Taguchi et al. (2025); Marsili et al. (2025); Tang et al. (2025a); Lee et al. (2025b); Fan et al. (2025a); Wang et al. (2025d) [= VisuoThink]; ..." — grouped as a prompting/search-based spatial-reasoning enhancement rather than a tool-training method.

## WeftOS relevance
**PATTERN.** The code-generation-to-image-edit loop plus tree-search backtracking is a strong pattern for WeftOS agentic tool orchestration (Claude Code style multi-step visual reasoning), but VisuoThink's drawing tools are explicitly relative/qualitative sketch aids (matplotlib annotations), never metric measurement — good alignment with "honest geometry" as a *negative* example to imitate carefully (never let sketch overlays imply real-world scale). Spatial-navigation grid tasks touch embodied/robotics framing but are simulated 2D, not egocentric video, so only loosely relevant to MentraOS.
