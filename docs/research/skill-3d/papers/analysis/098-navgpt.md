# 098 — NavGPT: Explicit Reasoning in Vision-and-Language Navigation with Large Language Models

**Citation:** Zhou, G., Hong, Y., Wu, Q. (2024). *NavGPT: explicit reasoning in vision-and-language navigation with large language models.* Proceedings of the AAAI Conference on Artificial Intelligence, Vol. 38, pp. 7641–7649.

**arXiv:** [2305.16986](https://arxiv.org/abs/2305.16986) (2023 preprint; AAAI 2024 publication — refs.tsv gives no arXiv id, confirmed via search/GitHub) · Code: github.com/GengzeZhou/NavGPT

## Summary

NavGPT is a purely LLM-driven (GPT-class), **zero-shot** instruction-following navigation agent for Vision-and-Language Navigation (VLN). At each step it converts visual observations, navigation history, and the set of explorable future directions into **text**, and has the LLM reason explicitly over that text to decide the next action — no learned navigation policy network, no explicit 3D map. The paper's point is that GPT-class LLMs already contain enough embodied commonsense and planning ability to do zero-shot sequential VLN when perception is translated into language.

## Method

**3D representation:** none in the geometric sense — navigable directions/panorama nodes from the VLN simulator (Matterport3D-style discrete graph) are converted directly to **textual descriptions** of the scene and available headings; there is no point cloud, depth map, or occupancy grid consumed by the LLM itself. The "3D" is entirely in the simulator's pre-built navigation graph, which NavGPT treats as a black box it queries via text.

**Metric scale:** not applicable at the reasoning layer — the underlying VLN simulator graph (Matterport3D scans) is metric, but NavGPT's LLM policy operates purely on textualized topology/instructions, with no metric quantities exposed to or reasoned about by the model.

## Results

Demonstrated capabilities (decomposing instructions into sub-goals, using commonsense knowledge, landmark identification, progress tracking, exception/plan adjustment) are described qualitatively in the paper; specific VLN success-rate numbers (e.g., on R2R) **not found** in accessible excerpts — verify from the PDF/proceedings before citing.

## Code / license

Code public at github.com/GengzeZhou/NavGPT (official AAAI 2024 implementation). License not confirmed in accessible content.

## Skill-3D relation

Listed in the §2.1 "extended to embodied and robotic settings" group (line 147) alongside [[097]] RoboRefer, [[095]] CoV, and the Gemini/RoboBrain robotics papers. NavGPT predates the agentic-tool-use framing Skill-3D itself uses (2023 vs. Skill-3D's 2026 tool-invocation loop) — it's cited as an early example of "explicit reasoning" over textualized perception in an embodied task, the conceptual ancestor of turning perception into LLM-legible text that Skill-3D, GR3D ([[088]]), and CoV ([[095]]) all still do in different ways.

## WeftOS relevance

The core move — convert scene structure into text an LLM reasons over explicitly, step by step, rather than baking navigation into a learned policy network — is the same move WeftOS already makes at Graph Views F10 (query → subgraph → text pack for the agent) and is a clean historical precedent for "text-mediated spatial reasoning over a graph." NavGPT's navigation graph (discrete panorama nodes + edges) is structurally close to a simplified Graph View over BVH region leaves. No geometry, no metric claims to audit — it's honest by construction (it never touches raw geometry).

**Verdict: PATTERN.** Early, clean precedent for "textualize the graph, let the LLM reason explicitly" — reinforces the F10 subgraph-pack design already in WeftOS docs; no code/geometry to adopt directly.
