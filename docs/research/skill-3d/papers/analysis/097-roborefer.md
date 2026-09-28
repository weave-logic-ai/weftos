# 097 — RoboRefer: Towards Spatial Referring with Reasoning in Vision-Language Models for Robotics

**Citation:** Zhou, E., An, J., Chi, C., Han, Y., Rong, S., Zhang, C., Wang, P., Wang, Z., Huang, T., Sheng, L., Zhang, S. (2025). *RoboRefer: towards spatial referring with reasoning in vision-language models for robotics.* arXiv preprint.

**arXiv:** [2506.04308](https://arxiv.org/abs/2506.04308)

## Summary

RoboRefer is a 3D-aware VLM for **spatial referring** in robotics ("pick up the cup to the left of the bottle behind the plate") — precise, multi-step spatial grounding rather than single-object detection. It combines a dedicated depth encoder (added via supervised fine-tuning) with reinforcement fine-tuning for generalized multi-step spatial reasoning, and is paired with **RefSpatial**, a 20M-QA-pair dataset spanning 31 spatial relations with up to 5-step reasoning chains. Deployed on real robot arms (UR5) and a G1 humanoid.

## Method

**3D representation:** RGB + a **dedicated depth encoder** fused into the VLM backbone via SFT — depth maps, not point clouds/meshes/BEV. Reinforcement fine-tuning (RFT) is applied on top for multi-step spatial reasoning chains.

**Metric scale:** depth-encoder-based, so scale is tied to whatever depth sensor/estimator supplies the depth maps (monocular depth estimation is typically **not metric** without additional calibration; stereo/RGB-D sensors would be). The paper's exact depth source (learned monocular vs. sensor RGB-D) is **not found** in accessible excerpts — this matters for the metric-honesty flag and should be verified before treating any RoboRefer output as measured.

## Results

SFT-trained model: **89.6%** average success rate (task not fully specified in accessible excerpt — likely real-robot pick tasks). RFT-trained model: **+17.4%** improvement over Gemini-2.5-Pro on **RefSpatial-Bench**. RefSpatial dataset: **20M** QA pairs, **31** spatial relations, up to **5-step** reasoning.

## Code / license

Not found in accessible content; standard arXiv non-exclusive license applies to the paper.

## Skill-3D relation

Listed in the §2.1 "extended to embodied and robotic settings" group (line 147), alongside [[001]] Gemini Robotics 1.5, [[024]] RoboBrain, [[060]] RoboBrain 2.0, [[061]] Gemini Robotics, [[098]] NavGPT, [[095]] CoV. RoboRefer is the spatial-*referring* specialization within that embodied cluster — closer to grounding/manipulation than to navigation ([[098]]) or driving ([[096]]).

## WeftOS relevance

RefSpatial's structured multi-step spatial-relation dataset (31 relation types, chained references) is a good reference taxonomy for what a WeftOS agent needs to express over Graph View edges (`located_in`, `adjacent_to`, and any future relation set) when resolving "the chair to the left of the desk near the window" style references — but only as a **relation vocabulary**, not as a geometry source, since scale honesty is unverified. If WeftOS ever exposes a manipulation/robotics arm actor, RoboRefer-style depth-grounded referring is a directly relevant capability to evaluate for that actor's perception stack — but that is out of scope for the Urth twin itself.

**Verdict: WATCH.** Useful relation-vocabulary reference for Graph View edges and a candidate perception stack if/when WeftOS gets a manipulation actor; not adoptable for Urth geometry pending scale verification.
