# [001] Gemini Robotics 1.5: Pushing the Frontier of Generalist Robots with Advanced Embodied Reasoning, Thinking, and Motion Transfer

**Citation:** A. Abdolmaleki, S. Abeyruwan, J. Ainslie, J. Alayrac, M. G. Arenas, A. Balakrishna, N. Batchelor, A. Bewley, J. Bingham, M. Bloesch, et al. *Gemini Robotics 1.5: pushing the frontier of generalist robots with advanced embodied reasoning, thinking, and motion transfer*. arXiv preprint, 2025.

**arXiv:** [2510.03342](https://arxiv.org/abs/2510.03342) · Google DeepMind technical report (not peer-reviewed venue)

## Summary
Gemini Robotics 1.5 is a two-model system: a multi-embodiment Vision-Language-Action (VLA) model that turns visual input and instructions into low-level robot actions, and a companion Embodied Reasoning model (Gemini Robotics-ER 1.5) that handles visual/spatial understanding, planning, and progress/success estimation. The headline mechanism is "Motion Transfer" (MT), which lets the VLA learn from heterogeneous, multi-embodiment robot demonstration data (different robot bodies, different action spaces) and generalize skills across embodiments. The VLA additionally interleaves action generation with an explicit multi-step natural-language reasoning process before acting, aimed at better long-horizon task decomposition.

## Method specifics
No point cloud, depth, BEV, or explicit 3D scene representation is described in the accessible material — the system consumes RGB video/image observations plus language, and ER 1.5 reasons over 2D visual input to produce spatial/embodied judgments (e.g., grounding, affordances, task planning) in natural language, not metric geometry. Metric scale is not established by this system; it is an action-policy and reasoning stack, not a 3D reconstruction method. Treat as **not metric** / representation details **not found** beyond "vision + language + action."

## Key results
Not found — the accessible abstract/report text states ER 1.5 achieves a new state of the art on embodied-reasoning benchmarks and MT improves cross-embodiment generalization, but no specific benchmark names or numeric scores were retrievable from this pass. Do not cite numbers without primary-source verification of the full PDF.

## Code/weights
Not found in accessible material — no code or weights release statement located; this is a Google DeepMind closed/limited-access model family historically.

## Skill-3D relation
Cited in the §2.1 related-work paragraph's closing clause, grouped with Ji et al. (RoboBrain, ref 24), Team et al. 2025a/b (RoboBrain 2.0, Gemini Robotics, refs 60/61), Zhou et al. 2025a/2024 (RoboRefer, NavGPT, refs 97/98), and Zhao et al. 2026 (CoV, ref 95) as work that "extended [MLLM spatial-reasoning] capabilities to embodied and robotic settings." It is background/scope-setting, not a method Skill-3D builds on or compares against directly.

## WeftOS relevance
Gemini Robotics 1.5 is an action-policy + reasoning stack for physical robot control, not a scene-reconstruction or spatial-index method — it has no BVH/HNSW-relevant geometry output to adopt. It is one data point on where embodied-VLM agent design is heading (multi-step reasoning before acting, cross-embodiment transfer), which is thematically adjacent to WeftOS agent/skill delivery (Claude Code, Grok, Codex) but orthogonal to Urth's spatial-index doctrine — no robot-actuation scope exists in WeftOS today. No overlap with the feedforward-reconstruction or scene-graph surveys already done.

**Verdict: WATCH** — relevant as a signal of embodied-agent-reasoning direction (multi-step "thinking" before action, cross-embodiment skill transfer) if WeftOS ever extends into robot/actuator control, but not adoptable today; closed weights and no geometric output to graft onto Urth.
