# [024] RoboBrain: A Unified Brain Model for Robotic Manipulation from Abstract to Concrete

**Citation:** Y. Ji, H. Tan, J. Shi, X. Hao, Y. Zhang, H. Zhang, P. Wang, M. Zhao, Y. Mu, P. An, et al. *Robobrain: a unified brain model for robotic manipulation from abstract to concrete*. In Proceedings of the Computer Vision and Pattern Recognition Conference (CVPR), pp. 1724–1734, 2025.

**arXiv:** [2502.21257](https://arxiv.org/abs/2502.21257) · CVPR 2025 (selected for CVPR 2025 Embodied AI Trends Commentary)

## Summary
RoboBrain argues MLLMs applied to long-horizon robot manipulation lack three specific capabilities: **planning** (decomposing instructions into sub-tasks), **affordance perception** (recognizing how objects can be interacted with), and **trajectory prediction** (anticipating the end-effector's full manipulation path). The authors build **ShareRobot**, a heterogeneous dataset with multi-dimensional annotations (task plans, object affordances, end-effector trajectories) refined by human annotators, and train RoboBrain — an MLLM combining robotic and general multimodal data via multi-stage training with long-video and high-resolution image support — to unify all three capabilities in one model.

## Method specifics
No explicit 3D scene representation (point cloud/depth/BEV/scene graph) is described in accessible content; the model operates on video/image + language and outputs planning text, affordance regions, and trajectory predictions, likely as 2D image-space annotations (affordance masks/points) plus predicted action/trajectory sequences rather than a reconstructed 3D scene. Metric scale: **not found** — trajectory prediction for real manipulation would need to be grounded to a robot's actual workspace/kinematics at deployment time, but the paper-level representation itself appears to be perception + language, not a metric 3D model.

## Key results
Not found — abstract confirms "state-of-the-art performance across various robotic tasks" but no benchmark names or scores were retrievable in this pass.

## Code/weights
Available: repo `FlagOpen/RoboBrain` (GitHub), "[CVPR 2025] Official Repository." License not independently confirmed here.

## Skill-3D relation
Cited in the §2.1 closing clause ("extended to embodied and robotic settings") alongside Team 2025a (RoboBrain 2.0, ref 60 — RoboBrain's own successor), Team 2025b (Gemini Robotics, ref 61), Abdolmaleki 2025 (Gemini Robotics 1.5, ref 1), Zhou 2025a/2024 (RoboRefer, NavGPT, refs 97/98), Zhao 2026 (CoV, ref 95). Background/scope-setting for embodied extensions of spatial-reasoning MLLMs, not a direct method dependency.

## WeftOS relevance
RoboBrain is a robot-manipulation planning/affordance/trajectory model — it has no scene-geometry output relevant to BVH/HNSW, and WeftOS has no robot-actuator scope today. The "planning + affordance + trajectory as three separable, jointly-trained capabilities" framing is a reasonable reference architecture *if* WeftOS agents ever drive physical actuators, but that is out of current scope (skills/hooks/agents target software environments — Claude Code, Grok, Codex — not robot arms). Open code/data (ShareRobot) is a plus for future reference but not actionable now.

**Verdict: WATCH** — no current WeftOS actuation scope to apply this to; revisit only if/when WeftOS gains a robot-control surface. RoboBrain 2.0 (ref 60) supersedes it and should be the primary watch target going forward.
