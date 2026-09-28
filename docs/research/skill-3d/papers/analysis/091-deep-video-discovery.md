# Deep Video Discovery: Agentic Search with Tool Use for Long-form Video Understanding

**Citation:** X. Zhang, Z. Jia, Z. Guo, J. Li, B. Li, H. Li, and Y. Lu. "Deep Video Discovery: Agentic Search with Tool Use for Long-form Video Understanding." arXiv preprint arXiv:2505.18079, 2025.

**arXiv:** 2505.18079 — https://arxiv.org/abs/2505.18079. Accepted NeurIPS 2025. Code: https://github.com/microsoft/DeepVideoDiscovery.

## Summary
Deep Video Discovery (DVD) is a "deep-research"-style agent for answering questions over extra-long videos. Rather than a fixed, uniform pipeline applied to every query, DVD treats segmented video clips as a searchable environment and lets an LLM autonomously plan: it observes its current state, selects from a set of search-centric tools over a multi-granular video database, formulates tool parameters, and iteratively refines its reasoning as new information arrives.

## Method specifics
- **Tool API shape:** a defined toolset (including a `global_browse_tool` that returns textual descriptions of video clips rather than raw pixels) invoked through the agent's autonomous planning loop; whether calls are structured JSON/function-calling or code execution was not confirmed in the accessible README/abstract — marked unverifiable.
- **Output return to model:** tool outputs largely surface as **text descriptions** of video clips/segments (per the `global_browse_tool` description) fed back into the LLM's reasoning context, rather than raw frames or structured numeric data.
- **Planner vs executor:** single LLM performs both planning (tool/strategy selection) and execution (parameter formulation, answer synthesis) in one adaptive loop — no separate planner/executor model pairing was found.
- **Error handling/repair:** the agent "iteratively refines its internal reasoning in light of gathered information," which functions as an implicit refinement loop, but no explicit retry/verification/self-correction mechanism is documented.

## Key quantitative results
State-of-the-art on **LVBench**: 74.2% accuracy without transcripts, improving to **76.0%** with transcripts — described as substantially surpassing prior work. (Numbers found via search summaries of the paper; not independently re-verified against the PDF table.)

## Code/weights availability and license
Code released at https://github.com/microsoft/DeepVideoDiscovery under the **MIT License** (per repository license badge).

## Skill-3D's citation
Cited at §2.2, in the sentence: "Recent tool-augmented VLM agents have been developed for long-video understanding, high-resolution image analysis, medical diagnosis, and general visual reasoning ... Zhang et al. (2025b) [DVD] ..." — grouped as a representative long-video tool-augmented agent in the related-work survey of heterogeneous, uniform-workflow tool-use systems that Skill-3D contrasts against.

## WeftOS relevance
**Verdict: PATTERN.** DVD's adaptive, state-aware tool-selection loop (vs. fixed pipelines) is architecturally close to what WeftOS wants from Claude Code/Grok/Codex agents driving MCP tools — worth studying as a reference design for query-driven tool orchestration over long video/sensor logs. Its outputs are qualitative video-clip descriptions and search hits, not metric geometry, so it poses no "honest geometry" risk; egocentric/robotics relevance is indirect (long-form video, not first-person capture specifically).
