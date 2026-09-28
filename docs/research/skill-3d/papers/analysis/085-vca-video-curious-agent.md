# VCA: Video Curious Agent for Long Video Understanding

**Citation:** Z. Yang, D. Chen, X. Yu, M. Shen, and C. Gan. "VCA: Video Curious Agent for Long Video Understanding." In *Proceedings of the IEEE/CVF International Conference on Computer Vision (ICCV)*, 2025, pp. 20168-20179.

**arXiv:** 2412.10471 — https://arxiv.org/abs/2412.10471 (v1: Dec 2024). Venue: ICCV 2025, official page at openaccess.thecvf.com/content/ICCV2025/html/Yang_VCA_Video_Curious_Agent_for_Long_Video_Understanding_ICCV_2025_paper.html

## Summary
VCA is a curiosity-driven video agent built on a VLM that autonomously explores long videos instead of uniformly sampling frames or bolting on external tool calls. It frames exploration as a **tree-search over video segments**: at each node the agent decides which segment to expand next, using a self-generated **intrinsic reward** from the VLM itself (rather than external supervision or task reward) to prioritize which frames are worth collecting. The goal is to keep long-video reasoning both effective and computationally cheap by avoiding brute-force dense frame sampling.

## Method specifics
- **Tool API shape:** not a general tool-calling agent in the MM-REACT/DVD sense — no external vision-expert APIs or code execution. The "action space" is video-segment selection/navigation within a tree-search structure.
- **Output return to model:** collected frames from expanded segments are fed back into the VLM's context as images for subsequent reasoning; no structured JSON tool-return channel is used.
- **Planner vs executor:** single VLM performs both the exploration decisions (tree expansion) and the final answer reasoning — no separate planner/executor split reported.
- **Error handling/repair:** none described; the intrinsic-reward-guided search substitutes for external verification — it is a self-generated heuristic for where to look next, not a correction mechanism over wrong answers.

## Key quantitative results
Abstract claims "superior effectiveness and efficiency" on multiple long-video benchmarks; specific benchmark names and numbers were not found in the accessible abstract/summary.

## Code/weights availability and license
Not found (no code/weight release statement located in the fetched abstract page; paper itself is CC BY 4.0 per arXiv metadata, which covers the paper text, not necessarily code).

## Skill-3D's citation
Cited at §2.2, in the sentence: "Recent tool-augmented VLM agents have been developed for long-video understanding, high-resolution image analysis, medical diagnosis, and general visual reasoning ... Yang et al. (2025e) [VCA] ..." — grouped as one instance of the broader tool-augmented-VLM-agent literature Skill-3D positions itself against.

## WeftOS relevance
**Verdict: WATCH.** VCA's intrinsic-reward tree-search over video segments is a self-exploration pattern (not a tool-calling architecture), and its outputs are qualitative frame selections, not metric geometry — no relevance to "honest geometry." Its curiosity-driven exploration idea could inform egocentric video triage (e.g. MentraOS glasses footage) for *which* segments deserve deeper agentic tool use, but it is not directly adoptable into WeftOS's tool/MCP architecture.
