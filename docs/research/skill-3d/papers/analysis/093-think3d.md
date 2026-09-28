# 093 — Think3D: Thinking with Space for Spatial Reasoning

**Citation:** Zhang, Z., Wu, Y., Jia, L., Wang, Y., Zhang, Z., Li, Y., Ran, B., Zhang, F., Sun, Z., Yin, Z., Wang, L., Lu, H. (2026). *Think3D: thinking with space for spatial reasoning.* arXiv preprint.

**arXiv:** [2601.13029](https://arxiv.org/abs/2601.13029) · Code/models/data: github.com/zhangzaibin/spagent (Apache 2.0)

## Summary

Think3D is Skill-3D's **direct prior work and primary baseline**: it gives a VLM interactive, **3D chain-of-thought reasoning** by wiring in a suite of 3D manipulation tools (a "Pi3X" reconstruction/geometry expert tool among them) that the model can invoke mid-reasoning, turning passive 2D perception into active spatial exploration. A reinforcement-learned variant, Think3D-RL, lets smaller open models (Qwen3-VL-4B) learn effective 3D exploration strategies autonomously rather than following a fixed tool-use script.

## Method

**3D representation:** point clouds, reconstructed via the "Pi3X" expert tool and **pre-computed / cached to disk** for RL training efficiency (per the repo README) — i.e. an offline reconstruction step feeds a point-cloud cache that the agent's tool calls query during the reasoning loop, rather than reconstructing on every step. This is a **reconstruction-based reasoning loop**: geometric evidence (point clouds) grounds each reasoning step, in contrast to MindJourney's ([[084]]) purely generative rollout.

**Metric scale:** not explicitly documented in accessible sources — the point-cloud reconstruction tool's scale honesty (monocular up-to-scale vs. metric) is **unverified**; this is the single most important open question for anyone adopting Think3D's pattern, since Skill-3D inherits the same reconstruction-based loop and WeftOS doctrine (ADR-078/079) requires flagging any monocular-to-metric claim. Treat as **not found / verify before trusting** rather than assume honesty either way.

## Results

Reports gains across BLINK Multi-view, MindCube-1K, and VSI-Bench-Tiny for proprietary backbones (GPT-4.1, Gemini 2.5 Pro); Think3D-RL enables Qwen3-VL-4B to learn 3D exploration policies autonomously. Exact numeric deltas **not found** in accessible README/abstract excerpts — the full comparison table lives in the PDF and was not independently verified here.

## Code / license

Code, models, and data public at github.com/zhangzaibin/spagent under **Apache 2.0** — the most concretely reusable license found in this batch.

## Skill-3D relation

This is the closest prior work in the entire reference list, cited four times (§1, §2.2, §4.1 ×2), not just listed. Specifically: (1) Skill-3D's related-work framing at line 115 opens with "Recent 3D agentic methods further introduce reconstruction-based reasoning loops for limited-view spatial understanding Zhang et al. (2026c)" — i.e. Think3D *is* the reconstruction-based-loop paradigm Skill-3D is positioned against/alongside; (2) Skill-3D's evaluation methodology directly reuses Think3D's benchmark-splitting scripts (line 474: "we use the scripts provided by Think3D... to randomly sample 30% of the questions from each category... as the training set") for VSI-Bench, BLINK, CV-3D, and MMSI-Bench, and follows Think3D's exact 7-frame uniform sampling protocol for VSI-Bench; (3) Think3D is one of four head-to-head evaluated settings in Skill-3D's experiments table (line 477: "w/o Tools, w/ Tools, Think3D... and Skill-3D") across both closed backbones (GPT-4o, GPT-5.4, Gemini-2.5-Pro, Gemini-3-Flash) and open backbones (Qwen3-VL-4B/8B). Skill-3D's stated critique of the reconstruction-loop paradigm (line 118-120) is that such methods "exhibit preferences toward a few dominant tools, regardless of what each scene actually requires" and yield only marginal gains — Skill-3D's own skill-selection mechanism is explicitly framed as the fix for that weakness, with Think3D as the direct baseline it must beat.

## WeftOS relevance

Highest-priority reference in this cluster for the Rust rewrite: Skill-3D's whole evaluation harness (dataset splits, sampling protocol, baseline comparisons) is built on top of Think3D's, so a faithful WeftOS reimplementation of Skill-3D's eval pipeline needs Think3D's split scripts too (Apache 2.0, so redistributable/portable to Rust tooling). Architecturally, the point-cloud-cache-backed tool loop is the closest published analog to a WeftOS agent calling a BVH/Urth query mid-reasoning — but only as a *pattern* (tool-call → cached geometric evidence → continue reasoning), since scale honesty is unverified and the cache is offline/precomputed rather than a live twin. Do not adopt Think3D's point-cloud tool as a BVH geometry source without first confirming its scale-acquisition method; the reasoning-loop shape (skill/tool invocation over cached geometric evidence) is directly reusable for how WeftOS skills should call into Urth.

**Verdict: PATTERN.** Adopt the reasoning-loop shape and reuse the (Apache 2.0) eval-split scripts for a faithful Rust reimplementation's benchmark harness; do not adopt the point-cloud tool as a metric geometry source without further verification.
