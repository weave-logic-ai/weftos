# [42] WSI-Agents: A Collaborative Multi-Agent System for Multi-Modal Whole Slide Image Analysis

**Citation:** Lyu et al. (2025). X. Lyu, Y. Liang, W. Chen, M. Ding, J. Yang, G. Huang, D. Zhang, X. He, and L. Shen. "Wsi-agents: a collaborative multi-agent system for multi-modal whole slide image analysis." arXiv preprint arXiv:2507.14680.
**arXiv:** https://arxiv.org/abs/2507.14680 (submitted Jul 19 2025) · Venue: MICCAI 2025 (per papers.miccai.org listing) · Code: https://github.com/CVI-SZU/WSI-Agents (pointer to https://github.com/XinhengLyu/WSI-Agents for implementation)

## Summary
WSI-Agents addresses gigapixel whole-slide-image (WSI) analysis in digital pathology, where general-purpose multimodal LLMs underperform task-specific models across the many distinct pathology tasks a WSI analysis pipeline must support. It proposes a collaborative multi-agent system with three components: (1) a task-allocation module that routes incoming queries to expert agents drawn from a "model zoo" of patch-level and WSI-level MLLMs, (2) a verification mechanism that checks outputs via internal consistency checks plus external validation against pathology knowledge bases and domain-specific models, and (3) a summary module that synthesizes a final report with visual interpretation maps. The authors report the system outperforms both existing WSI-specific MLLMs and general medical agent frameworks across diverse pathology tasks.

## Method Specifics
- **Tool API shape:** structured agent-to-agent task allocation and model-zoo invocation rather than code generation — a coordinator routes tasks to specialist MLLM agents.
- **Output return:** not fully documented in fetched content; the summary module's output includes text plus "visual interpretation maps," implying at least text+image structured returns from specialist agents into the synthesis step.
- **Planner vs executor:** explicit split — task-allocation module acts as planner/router, expert agents (patch-level and WSI-level MLLMs) are executors, and a separate summary module performs final synthesis — a three-stage planner/executor/synthesizer architecture, not a single model doing everything.
- **Error handling/repair:** the verification mechanism is an explicit built-in check — internal consistency checks plus external validation against pathology knowledge bases and domain-specific models — functioning as a verification/repair loop before the summary module commits to a final answer.

## Quantitative Results
Not found — no specific benchmark numbers were surfaced from the abstract or GitHub README in the fetched content; only qualitative claims of "superiority to current WSI MLLMs and medical agent frameworks."

## Code/Weights/License
Code referenced as available (GitHub: CVI-SZU/WSI-Agents pointing to XinhengLyu/WSI-Agents), but no license was found in the fetched README content — license unverifiable, mark as not found.

## Relation to Skill-3D
Cited at §2.2 (MLLM Agents): "Recent tool-augmented VLM agents have been developed for long-video understanding, high-resolution image analysis, medical diagnosis, and general visual reasoning ... Lyu et al. (2025); Liu et al. (2025b); Su et al. (2025)" — grouped as a medical-diagnosis example of the broader tool-augmented VLM agent category, not discussed individually beyond that clustering.

## WeftOS Relevance
**Verdict: SKIP.** Domain (digital pathology / gigapixel WSI) is unrelated to 3D spatial or egocentric reasoning, and no metric-geometry or robotics angle exists. The one transferable idea — an explicit planner/task-allocator + specialist-executor + verification/synthesis three-stage architecture — is a generically useful multi-agent pattern, but it is better represented elsewhere (e.g. RieMind's or Skill-3D's own perception/reasoning decoupling) with more direct applicability to WeftOS's spatial-tool use case.
