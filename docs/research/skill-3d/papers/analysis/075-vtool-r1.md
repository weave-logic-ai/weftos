# Ref 75 — Wu et al. (2025d), VTool-R1

**Citation:** M. Wu, J. Yang, J. Jiang, M. Li, K. Yan, H. Yu, M. Zhang, C. Zhai, and K. Nahrstedt. "VTool-R1: VLMs Learn to Think with Images via Reinforcement Learning on Multimodal Tool Use." arXiv preprint arXiv:2505.19255.

**Source:** arXiv:2505.19255, https://arxiv.org/abs/2505.19255. Code: https://github.com/VTOOL-R1/vtool-r1. Reported accepted to ICLR 2026 per arXiv page metadata.

## Summary
VTool-R1 trains vision-language models to generate genuine "multimodal chains of thought" — reasoning traces that interleave text with intermediate visual-editing steps, not just text describing images — using reinforcement learning (RFT) rather than supervised imitation of a fixed tool-use trace. The model learns, through outcome-based reward alone (final-answer correctness), when and how to invoke a small set of Python-based image-editing tools during its reasoning process, applied to structured visual QA over tables and charts.

## Method specifics
- **Tool API shape:** Code generation — the model calls Python-based image-editing operations (Highlight Column/Row, Mask Column/Row, Draw Column/Row: semi-transparent overlays, white masking, and bounding boxes over rows/columns of tables/charts), not JSON/structured tool calls.
- **Return path:** Tool outputs are the modified image itself (re-highlighted/masked/boxed table or chart), which is re-inserted into the model's multimodal context for the next reasoning step — a genuine image-in/image-out loop rather than text-only tool results.
- **Planner/executor split:** Single model does both — the same VLM decides which visual edit to apply (planning) and "executes" it by emitting the edit code, all inside one RL-trained policy; no separate planner/executor network.
- **Error handling:** Outcome-based reward only (final task-accuracy reward, no process-level/step-level supervision) — the paper explicitly frames this as learning "without explicit process-level supervision," i.e., no built-in retry/verification loop beyond what RL training implicitly shapes.

## Results
Chart/Table VQA splits, Qwen2.5-VL backbones: 3B — 64.0% (chart) / 57.9% (table); 7B — 80.7% (chart) / 71.7% (table); 32B — 86.7% (chart) / 84.5% (table). GPT-4o baseline: 82.9% (chart) / 77.0% (table). VTool-R1 (7B) is reported to beat the concurrent method DeepEyes on charts (80.7% vs. 60.0% at comparable scale). These figures came from a secondary WebFetch summarization pass over the arXiv HTML and were not cross-checked against the primary results table — treat as indicative.

## Code / license
Code open-sourced at github.com/VTOOL-R1/vtool-r1. GitHub reports the license as Apache License 2.0 (Apache-2.0).

## Relation to Skill-3D
Cited at Skill-3D §2.2: "A complementary line of work trains VLMs to use tools through supervised fine-tuning or reinforcement learning ... Wu et al. (2025d) [= this paper] ..." — grouped with MLLM-Tool, Tang et al. (2025b), and DetToolChain as an RL/SFT-trained tool-use approach, contrasted with Skill-3D's prompted skill-library mechanism (no policy weights updated for tool choice).

## WeftOS relevance
**PATTERN.** The image-in/image-out RL loop over visual editing ops (highlight/mask/box) is a clean pattern for training or evaluating agentic visual tool use, but every edit is a visual annotation, not a metric measurement — strict "honest geometry" alignment as a negative example (never claim scale from a highlight/mask/box). Applies to static table/chart images, not egocentric or robotics contexts, so limited MentraOS relevance.
