# Ref 20: TIGeR

**Citation:** Y. Han, C. Chi, E. Zhou, S. Rong, J. An, P. Wang, Z. Wang, L. Sheng, S. Zhang. "TIGeR: Tool-Integrated Geometric Reasoning in Vision-Language Models for Robotics." arXiv:2510.07181, 2025.

**Source:** arXiv:2510.07181 (https://arxiv.org/abs/2510.07181); project page: hany01rye.github.io/TIGeR/; code: github.com/hany01rye/tiger; weights: huggingface.co/hany01rye/TIGeR

## Summary

TIGeR gives VLMs precise geometric reasoning for robotics by having them generate and execute code (via Qwen3-Coder) against a defined tool API rather than relying on pattern-matched, probabilistic spatial guesses. The model is trained in two stages on TIGeR-300K (a mix of 274K template-based samples from CA-1M and 35K LLM-rewritten samples from SSR-CoT): SFT to instill tool usage, then RL (GRPO) with a hierarchical reward to sharpen accuracy and task completion. Target application is real-world robotic manipulation requiring centimeter-level precision.

## Method specifics

- **Tool API shape:** code generation and execution (Qwen3-Coder writes Python that runs in a sandbox), calling structured tools with named parameters: `camera_intrinsics`/`camera_extrinsics`, `depth_sensor`, `object_segmentation` (SAM2-based), `box_2d_to_box_3d`, `point_3d_to_point_2d`, and a general `code_executor` for arbitrary geometric computation.
- **Output return path:** structured and metric — real-world coordinates in meters/centimeters, 3D poses, rotation matrices, trajectories, grounded in calibrated camera parameters and depth-sensor data. This is genuinely metric geometry, not relative/qualitative.
- **Planner/executor split:** no separate planner model — a single VLM (fine-tuned) decides tool sequencing and generates the executing code itself.
- **Error handling:** a five-part hierarchical reward (format, tool-invocation validity, parameter-content accuracy, code-generation/execution correctness, final-answer accuracy) enforces correctness during RL training; the paper describes "quality checks" for valid tool formats and accurate final answers, but this is a training-time reward shaping mechanism rather than a documented runtime retry/repair loop.

## Results

Spatial reasoning benchmarks: 79.30% average, beating Gemini 2.5-Pro by 5.83%. Simulation (Open6DOR V2): 83.7% average success rate. Real-world manipulation: 55-70% success on metric-precision placement and spatial-relation tasks. Numbers found via paper summary; not independently re-verified against the PDF table, and the project page's own results image could not be read to cross-check.

## Code/weights

Code and weights both listed as released (GitHub repo + Hugging Face weights linked from the project page), but a specific license was not confirmed in accessible content — not found.

## Skill-3D relation

Cited in §2.2 ("MLLM Agents"): "A complementary line of work trains VLMs to use tools through supervised fine-tuning or reinforcement learning Liu et al. (2024a); Wang et al. (2025a); Han et al. (2025); ..." — grouped with SFT/RL tool-use-training approaches, as opposed to Skill-3D's inference-time skill retrieval plus agentic SFT/GRPO combination.

## WeftOS relevance

**ADOPT (pattern-level).** TIGeR is the closest match in this batch to WeftOS's own goals: a defined tool API (camera intrinsics/extrinsics, depth, segmentation, 2D↔3D conversion) producing genuinely metric outputs traceable to calibrated sensors — exactly the "honest geometry" contract WeftOS wants. Its hierarchical reward structure (format / tool-call validity / parameter accuracy / execution correctness / answer correctness) is a strong template for a WeftOS tool-use verification and training signal design, and the tool set itself maps closely to what an MCP geometry toolset for Claude Code/Grok/Codex would need.
