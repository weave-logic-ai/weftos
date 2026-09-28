# 084 — MindJourney: Test-Time Scaling with World Models for Spatial Reasoning

**Citation:** Yang, Y., Liu, J., Zhang, Z., Zhou, S., Tan, R., Yang, J., Du, Y., Gan, C. (2025). *MindJourney: Test-Time Scaling with World Models for Spatial Reasoning.* arXiv preprint.

**arXiv:** [2507.12508](https://arxiv.org/abs/2507.12508) · Project: umass-embodied-agi.github.io/MindJourney · License: CC BY 4.0

## Summary

MindJourney gives a frozen VLM spatial reasoning it doesn't natively have by coupling it, at **test time only**, to a controllable video-diffusion world model. The VLM proposes camera trajectories ("look left," "move forward"); the world model synthesizes the corresponding novel views; the VLM reasons over the accumulated multi-view evidence. No fine-tuning of the VLM.

## Method

**3D representation:** none, explicitly — the "3D" is implicit in a video-diffusion model's learned prior over viewpoint change. There is no point cloud, depth map, or explicit camera geometry; the world model is a generative rollout conditioned on camera-motion instructions.

**Metric scale:** not obtained, and not claimed. This is the closest thing in the cluster to WeftOS's "generative fill is cosmetic" warning (ADR-079 §2) — MindJourney explicitly treats the synthesized views as *evidence to reason over*, not as measured geometry, which is more honest than papers that silently imply metric output. Still, an agent consuming these synthesized views has no way to know when the diffusion model hallucinated occluded content vs. correctly extrapolated it.

## Results

Reports a **7.7%** improvement on the SAT spatial-reasoning benchmark (self-reported, not independently verified here), and claims to outperform VLMs trained via RL for spatial reasoning at test time, without fine-tuning. Exact SAT baseline/absolute numbers: **not found** in accessible excerpts — verify from the PDF before citing.

## Code / license

CC BY 4.0 stated on project page; explicit code/weights repo link not confirmed in the fetched content — **not found**, check project page directly.

## Skill-3D relation

Grouped in the §2.1 "prompting, mental simulation, visual chain-of-thought... generative imagination of 3D space" cluster (line 147, alongside Taguchi/[[057]], Lee/[[028]]). MindJourney is the purest instance of "generative imagination" in that list — world-model rollout as the mechanism, versus Skill-3D's tool-invocation loop over real (or reconstructed) geometry.

## WeftOS relevance

Directly instructive as a **negative pattern to guard against**, and a positive pattern for LeWM (ADR-090). A test-time world-model rollout used purely as *reasoning scaffold* (never written to BVH, never treated as occupancy) is consistent with WeftOS's "latent WM is a sub-layer, not the twin" doctrine — this is effectively LeWM's use case done today with off-the-shelf video diffusion. The risk, flagged per the brief: nothing here prevents an integrator from mistaking a synthesized "look behind the couch" frame for observed space. WeftOS's unobserved-stays-unobserved rule is the fix MindJourney itself does not enforce.

**Verdict: WATCH** — good LeWM-adjacent pattern (test-time generative exploration as reasoning aid), explicitly not a geometry producer; revisit if/when WeftOS builds an interactive rollout prior for agent planning.
