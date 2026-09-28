# 062 — Cambrian-1: A Fully Open, Vision-Centric Exploration of Multimodal LLMs

**Citation:** Tong, S., Brown, E., Wu, P., Woo, S., Middepogu, M., Akula, S. C., Yang, J., Yang, S., Iyer, A., Pan, X., et al. *Cambrian-1: A Fully Open, Vision-Centric Exploration of Multimodal LLMs.* NeurIPS 2024, Advances in Neural Information Processing Systems 37, pp. 87310–87356.

**arXiv:** [2406.16860](https://arxiv.org/abs/2406.16860) · NeurIPS 2024 Oral · code: `github.com/cambrian-mllm/cambrian` · project: `cambrian-mllm.github.io/cambrian-1`.

## Summary
A systematic, fully-open study of vision-centric MLLM design: evaluates 20+ vision encoders and connector designs, introduces a dynamic **spatially-aware connector** that fuses features from several vision encoders into the LLM while reducing token count, and releases **CV-Bench** (a vision-centric benchmark) and **Cambrian-10M** (an instruction-tuning dataset). It is a backbone/methodology paper, not a 3D-scene-specific system.

## Method specifics
- **Representation:** 2D image features from multiple vision encoders (no point cloud/depth/BEV/scene graph — this is an MLLM-architecture paper, not a 3D-reconstruction paper).
- **Metric scale:** not applicable — Cambrian-1 does not produce 3D geometry; CV-Bench measures depth-ordering/relative-distance/spatial-layout/multi-view-consistency as **2D-image-grounded QA**, not metric output.

## Key results
- Released **CV-Bench**, a vision-centric benchmark used downstream by Skill-3D (as **CV-3D**, see below).
- Broad backbone/connector ablation results across 20+ encoders — specific numeric comparisons **not found** in this pass (see Cambrian-1 paper Table results for exact figures; not fetched in full here).

## Code / license
Code at `github.com/cambrian-mllm/cambrian`; **license not confirmed** (verify Apache/MIT before assuming permissive).

## Skill-3D relation — evaluation dependency, not just related work
Skill-3D **evaluates on CV-3D** (paper text line 474, 1791): *"We evaluate on VSI-Bench ..., BLINK ..., CV-3D Tong et al. (2024), and MMSI-Bench ... CV-3D Tong et al. (2024) focuses on geometric spatial reasoning, including depth ordering, relative distance, spatial layout, and multi-view consistency."* This is almost certainly Cambrian-1's **CV-Bench** (the paper text's "CV-3D" naming likely refers to CV-Bench's 3D-vision subset, or is the Skill-3D authors' own shorthand — **verify exact benchmark identity against the Skill-3D paper's Appendix C.1** before treating "CV-3D" and "CV-Bench" as strictly identical). Either way, Skill-3D uses this as a **held-out test benchmark**, sampling 30% per category for training (per the Think3D script) and testing on the rest.

## WeftOS relevance
Cambrian-1 itself (backbone architecture, encoder ablations) is **not** relevant to WeftOS, which consumes hosted models (Claude, Grok, Codex) rather than training/fine-tuning a custom vision backbone. The relevant artifact is **CV-Bench/CV-3D as an eval set**: if WeftOS ever validates a Rust Skill-3D reimplementation's spatial-reasoning quality, CV-3D-style depth-ordering/relative-distance/layout/multi-view-consistency tasks are a reasonable eval battery to borrow or adapt against Urth-backed scenes.

**Verdict: WATCH** (SKIP on the backbone/architecture; WATCH on CV-Bench/CV-3D as a candidate eval benchmark for a Rust reimplementation).
