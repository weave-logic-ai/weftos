# 061 — Gemini Robotics: Bringing AI into the Physical World

**Citation:** Gemini Robotics Team (Abeyruwan, S., Ainslie, J., Alayrac, J., Arenas, M. G., Armstrong, T., Balakrishna, A., Baruch, R., Bauza, M., Blokzijl, M., et al.). *Gemini Robotics: Bringing AI into the Physical World.* arXiv:2503.20020.

**arXiv:** [2503.20020](https://arxiv.org/abs/2503.20020) · Google/DeepMind, 118+ authors.

## Summary
Two foundation models built on Gemini 2.0 for robot control: **Gemini Robotics-ER** (Embodied Reasoning — extends Gemini's multimodal reasoning to physical domains: object detection, trajectory prediction, grasp estimation, multi-view correspondence, 3D bounding-box prediction) and **Gemini Robotics (VLA)** — a Vision-Language-Action generalist built on top of ER for direct robot control, supporting long-horizon dexterous tasks and few-shot learning (as few as 100 demonstrations for new short-horizon tasks).

## Method specifics
- **Representation:** 2D multi-view images → predicted 3D bounding boxes, grasp points, and trajectories via the ER model's spatial/temporal reasoning heads. No explicit point-cloud/depth/BEV pipeline described in the fetched abstract.
- **Metric scale:** **not confirmed whether ER's 3D bounding-box predictions claim metric scale from monocular/multi-view RGB alone** — the abstract fetch explicitly could not confirm this. **Flag for follow-up**: if Gemini Robotics-ER outputs metric 3D boxes from RGB without depth sensors or known camera calibration, that is exactly the honest-geometry violation WeftOS doctrine (ADR-078/079) warns against; if it relies on calibrated multi-view/depth input at deployment, it's consistent with WeftOS's stance. Needs a primary-source read before further use.

## Key results
- Learns new short-horizon tasks from **as few as 100 demonstrations** (per fetched abstract). No other benchmark numbers found.

## Code / license
**Closed model / API only** — no public code or weight release indicated. Closed-weight Google product, unlike most other refs in this cluster.

## Skill-3D relation
Cited in the same embodied/robotic clause as ref 60 (RoboBrain 2.0), ref 1 (Gemini Robotics 1.5), ref 97 (RoboRefer), ref 98 (NavGPT), ref 95 (CoV) — pure related-work context, no algorithmic dependency.

## WeftOS relevance
Closed-weight commercial VLA model; not adoptable as code/weights. Relevant only as a signal that the industry's leading labs (Google) are converging on "MLLM predicts 3D boxes/grasp points directly from RGB" for physical control — a design point WeftOS should watch critically given the honest-geometry doctrine, since this exact pattern (metric-looking 3D output from vision-only input) is the failure mode ADR-079 was written to reject. No MentraOS egocentric fit either (this is robot-arm-centric, not glasses-centric). Out of scope for a spatial-index/agent-OS.

**Verdict: SKIP** — closed weights, robot-actuation focus, and a metric-scale claim from vision that needs scrutiny rather than adoption; revisit only as a cautionary reference for the honest-geometry doctrine writeup.
