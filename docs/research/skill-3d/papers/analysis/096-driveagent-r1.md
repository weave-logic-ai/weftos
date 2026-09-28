# 096 — DriveAgent-R1: Advancing VLM-based Autonomous Driving with Active Perception and Hybrid Thinking

**Citation:** Zheng, W., Mao, X., Ye, N., Li, P., Zhan, K., Lang, X., Zhao, H. (2025). *DriveAgent-r1: advancing vlm-based autonomous driving with hybrid thinking and active perception.* arXiv e-prints, arXiv–2507.

**arXiv:** [2507.20879](https://arxiv.org/abs/2507.20879) (note: refs.tsv gives no arXiv id; confirmed via search — title/date match Skill-3D's citation exactly)

## Summary

DriveAgent-R1 is (per the authors) the first autonomous-driving VLM agent with **active perception for planning**: instead of passively reasoning over whatever sensor frame it's given, it proactively invokes visual-reasoning tools in complex scenes to ground its driving decisions in explicit visual evidence. A "hybrid thinking" framework — inspired by human driver cognition — lets it switch between cheap text-only reasoning and expensive tool-augmented visual reasoning depending on scene complexity, trained with a cascaded RL scheme.

## Method

**3D representation:** not detailed in accessible excerpts — likely camera/BEV-style driving-scene input typical of the VLM-driving literature, with tool calls (object detection, depth, etc.) invoked on demand rather than a single upfront 3D reconstruction. **Not found** — verify representation specifics from the PDF.

**Metric scale:** driving-domain systems generally require metric distance/velocity estimates for safety-critical planning, but the paper's own scale-acquisition mechanism (camera-only? LiDAR-fused?) is **not found** in accessible content — do not assume metric honesty without checking the PDF.

## Results

Authors claim performance "comparable to top models and human drivers with efficient resource usage" in complex scenarios; specific benchmark numbers **not found** in accessible excerpts.

## Code / license

Not found in accessible content.

## Skill-3D relation

Cited at line 151 in the paragraph on works that "train VLMs to use tools through supervised fine-tuning or reinforcement learning" — i.e. as an example of RL-trained tool-invocation policy in an embodied (driving) domain, parallel to how Skill-3D trains skill-guided post-training (Skill-3D-4B/8B) for indoor 3D reasoning. The "adaptively switch between cheap text-only and expensive tool-augmented reasoning" idea is the same cost-aware policy shape Skill-3D needs when choosing whether a question requires reconstruction tools at all.

## WeftOS relevance

Not a driving-stack adoption target (WeftOS is not building an AV stack), but the **hybrid-thinking / cost-aware tool-invocation policy**, trained via cascaded RL, is a reusable pattern for any WeftOS agent deciding whether a query needs an expensive BVH/reconstruction round-trip versus a cheap in-context answer — directly relevant to skill/hook cost-tiering already present in the 3-tier model-routing doctrine (Agent Booster / Haiku / Sonnet-Opus) and to MentraOS glasses agents that must budget compute against battery/latency.

**Verdict: PATTERN.** The active-perception / hybrid-thinking cost-aware tool-invocation RL pattern is worth studying for WeftOS's own tool-cost routing; no direct geometry or driving-stack relevance.
