# 063 — GPT-4V(ision) for Robotics: Multimodal Task Planning from Human Demonstration

**Citation:** Wake, N., Kanehira, A., Sasabuchi, K., Takamatsu, J., Ikeuchi, K. *GPT-4V(ision) for Robotics: Multimodal Task Planning from Human Demonstration.* IEEE Robotics and Automation Letters, 2024. DOI 10.1109/LRA.2024.3477090.

**arXiv:** [2311.12015](https://arxiv.org/abs/2311.12015) · IEEE RAL (matches refs.tsv venue).

## Summary
A pipeline that uses GPT-4V to enable one-shot visual teaching of robot manipulation from human-demonstration video. GPT-4V analyzes the demo video to produce textual explanations of environment and actions; a GPT-4-based task planner turns those into a symbolic task plan; vision systems then spatially/temporally ground the plan back onto the video — an open-vocabulary object detector identifies objects, and hand-object interaction analysis locates grasp/release moments — yielding affordance info for robot execution.

## Method specifics
- **Representation:** 2D video frames + open-vocabulary object detection + hand-object interaction analysis. No 3D reconstruction, point cloud, or depth pipeline — spatial grounding is 2D-detection-based, with 3D/metric information (if any) supplied by the downstream robot's own sensors at execution time, not by this pipeline.
- **Metric scale:** none claimed by this paper itself — it produces symbolic task plans + affordance annotations from RGB video, deferring metric execution to the robot's native perception/control stack. Honest by scope (doesn't claim geometry it doesn't have).

## Key results
- Qualitative/pipeline demonstration paper (2023/2024 vintage, predates most rigorous spatial-VLM benchmarks in this cluster); **no standardized quantitative benchmark numbers found** in the fetched summary — treat any success-rate claims as unverified until the PDF is read.

## Code / license
Not found in fetched content.

## Skill-3D relation
Cited in the same embodied/robotic closing clause of §2.1 as RoboBrain 2.0, Gemini Robotics, RoboRefer, NavGPT, CoV — establishes that GPT-4V-class MLLMs were already being applied to embodied task planning well before Skill-3D's agentic 3D reasoning work; purely related-work framing.

## WeftOS relevance
An early (2023) demonstration that a general VLM plus classical open-vocab detection can bootstrap task plans from egocentric-style human demonstration video — structurally similar to what a MentraOS-capture → WeftOS-agent pipeline could do for "watch me do X once, then help me do it again," though this paper targets robot execution, not glasses-based human assistance. The one-shot-from-demonstration framing and the "detect objects + infer hand-object interaction" step are a plausible pattern for MentraOS egocentric task-teaching skills, but the paper itself contributes no 3D/geometric artifact for Urth.

**Verdict: WATCH** — conceptual precedent for demonstration-driven task planning from egocentric video; no geometry or code to adopt.
