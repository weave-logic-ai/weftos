# [73] Wu et al. (2025b) — SpatialScore: Towards Unified Evaluation for Multimodal Spatial Understanding

**Citation:** H. Wu, X. Huang, Y. Chen, Y. Zhang, Y. Wang, W. Xie. arXiv:2505.17012.
**Venue/ID:** arXiv preprint, 2025-05; CVPR 2026 (Highlight). https://arxiv.org/abs/2505.17012

## Summary
SpatialScore is an attempt to unify and consolidate the fragmented spatial-reasoning-benchmark landscape (VSI-Bench, MMSI-Bench, BLINK-style tasks, etc. each cover different slices) into one comprehensive suite, plus a large training corpus and a tool-augmented agent baseline that improves spatial reasoning without retraining the base model.

## Specifics
- **Task types:** 30 distinct spatial-reasoning tasks spanning multiple visual data types/input modalities (image, multi-image, video — exact modality breakdown not resolved from the fetched abstract).
- **Data sources:** aggregated/unified from prior spatial benchmarks plus new manually verified samples (specific source datasets not confirmed from the fetched page — check the paper for the exact aggregation list).
- **Size:** ~5K manually verified evaluation samples (SpatialScore); a separate 331K-sample training corpus (SpatialCorpus) for fine-tuning.
- **Metrics:** per-task accuracy across 49 evaluated MLLMs (exact metric formulas not resolved from the fetched page).
- **Ground-truth provenance:** manually verified (human-in-the-loop) for the eval set; provenance of the underlying 3D/scene data not confirmed from the fetched material.
- **Known flaws:** none flagged in the fetched material; as an aggregator benchmark, it inherits whatever flaws its source benchmarks carry (e.g., VSI-Bench's annotation issues that motivated ReVSI [92]) unless those were specifically corrected during aggregation — not verified from the abstract.
- **License:** stated as "all data, code, and models will be released"; exact license terms not confirmed from the fetched page.

## Key results
Evaluated 49 MLLMs; finds a substantial, persistent gap to human-level spatial intelligence across the unified 30-task suite. Fine-tuning on SpatialCorpus improves target models (e.g., Qwen3-VL); the SpatialAgent tool-augmented framework (12 specialized spatial-perception tools, Plan-Execute/ReAct) improves reasoning at inference time without retraining.

## Availability
GitHub/project release promised per the abstract; license and exact repo URL not confirmed from the fetched page — verify before depending on it.

## How Skill-3D uses it
Cited in the related-work list of "dedicated benchmarks" for spatial reasoning (paper-2606.07436.txt line 147) — not used in Skill-3D's own four-benchmark eval suite (VSI-Bench/BLINK/CV-3D/MMSI-Bench).

## WeftOS relevance — Verdict: WATCH
SpatialAgent's tool-augmented, no-retraining-needed architecture (12 spatial-perception tools + Plan-Execute/ReAct) is structurally close to what a WeftOS Rust agent orchestrating geometry tools would look like, making this a useful architectural reference even before license terms are confirmed. As a 30-task unified benchmark it could reduce eval fragmentation, but license uncertainty and un-verified provenance mean it should be revisited once the actual release lands, not adopted sight-unseen. See [[papers/benchmarks-and-rl]].
