# 060 — RoboBrain 2.0 Technical Report

**Citation:** BAAI RoboBrain Team (Cao, M., Tan, H., Ji, Y., Chen, X., Lin, M., Li, Z., Cao, Z., Wang, P., Zhou, E., et al., incl. Tiejun Huang, Shanghang Zhang). *RoboBrain 2.0 Technical Report.* arXiv:2507.02029.

**arXiv:** [2507.02029](https://arxiv.org/abs/2507.02029) · project `superrobobrain.github.io` · 52 authors, BAAI.

## Summary
An embodied vision-language foundation model family (7B lightweight + 32B full-scale, heterogeneous vision-encoder + LLM architecture) that unifies perception, reasoning, and planning for embodied tasks: affordance prediction, spatial referring, trajectory forecasting, closed-loop interaction, multi-agent long-horizon planning, and scene-graph updating.

## Method specifics
- **Representation:** unspecified in the fetched abstract beyond "spatial understanding (affordance, spatial referring, trajectory forecasting)" and "scene graph updating" — **exact 3D representation (point cloud/depth/BEV) not found**; likely image/video + learned spatial heads rather than an explicit metric 3D backbone, consistent with the VLA family this belongs to.
- **Metric scale:** not stated. Given it targets robot manipulation, some downstream metric grounding (via robot proprioception/depth sensors at deployment) is plausible but **not confirmed by the fetched abstract** — flag as unverified, do not assume monocular-metric claims either way without reading the full paper.

## Key results
- 32B variant reported to surpass prior open-source and proprietary models on spatial/temporal benchmarks; **specific numbers not found** in fetched content.

## Code / license
"Code, checkpoint and benchmark are available" at the project site. **License not confirmed.**

## Skill-3D relation
Cited in §2.1's final clause: "These capabilities have also been extended to embodied and robotic settings Ji et al. (2025); Team et al. (2025a) [RoboBrain 2.0]; Team et al. (2025b) [Gemini Robotics]; Abdolmaleki et al. (2025) [Gemini Robotics 1.5]; Zhou et al. (2025a) [RoboRefer]; Zhou et al. (2024) [NavGPT]; Zhao et al. (2026) [CoV]." Positioned purely as related-work context establishing that spatial VLM capability transfers to embodied/robotic domains — Skill-3D itself stays in the indoor-3D-QA/agentic-tool-use regime, not robot control.

## WeftOS relevance
RoboBrain 2.0 is a full VLA (vision-language-action) foundation model for physical robot control — outside WeftOS's current scope, which is a spatial-index/agent-OS (BVH + HNSW + Graph Views) delivering skills/hooks to Claude Code, Grok, Codex, not a robot-control stack. Its scene-graph-updating capability is conceptually adjacent to Graph Views F1–F10 but the paper doesn't expose enough architectural detail (from this fetch) to extract a concrete pattern. Worth a second look only if/when WeftOS scope extends to actuated embodiment.

**Verdict: WATCH** — no current WeftOS seam; revisit if embodied/robotic actuation enters scope.
