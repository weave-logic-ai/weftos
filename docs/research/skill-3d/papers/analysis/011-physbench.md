# [11] Chow et al. (2025) — PhysBench: Benchmarking and Enhancing VLMs for Physical World Understanding

**Citation:** W. Chow, J. Mao, B. Li, D. Seita, V. Guizilini, Y. Wang. arXiv:2501.16411.
**Venue/ID:** arXiv preprint, 2025-01. https://arxiv.org/abs/2501.16411, project page physbench.github.io, dataset on HuggingFace (USC-GVL/PhysBench).

## Summary
PhysBench evaluates whether vision-language models understand physical-world phenomena (object properties, relationships, scene dynamics, physics-based reasoning) rather than just common-sense visual QA. It finds VLMs are strong at common-sense reasoning but weak at physical understanding, and proposes PhysAgent, a framework combining VLM generalization with specialized vision-model expertise, which lifts GPT-4o by 18.4%.

## Specifics
- **Task types:** 4 domains × 19 subclasses × 8 capability dimensions — physical object properties, physical object relationships, physical scene understanding, physics-based dynamics (e.g., stability, causality, prediction).
- **Data sources:** 10,002 entries of interleaved video-image-text data, sourced from real-world and simulated content (exact scene-source breakdown not resolved from the abstract page — check the paper PDF for per-domain provenance).
- **Size:** 10,002 items; evaluated across 75 representative VLMs.
- **Metrics:** accuracy per capability dimension; relative-gain percentages reported for PhysAgent vs. base VLM.
- **Ground-truth provenance:** not confirmed from the fetched page — likely human-annotated/curated; verify against the paper before citing provenance claims.
- **Known flaws/leakage:** none identified in the fetched material — not verified either way.
- **License:** CC BY 4.0 (dataset on HuggingFace).

## Key results
VLMs underperform badly on physics-grounded tasks relative to common-sense tasks; PhysAgent (VLM + specialized vision tools) improves GPT-4o by +18.4% on PhysBench.

## Availability
Dataset public on HuggingFace under CC BY 4.0; project page physbench.github.io. Code availability for PhysAgent not confirmed from the fetched pages.

## How Skill-3D uses it
Cited in the related-work list of "dedicated benchmarks" for spatial/physical reasoning (§2.1, paper-2606.07436.txt line 147) — not used as an evaluation benchmark in Skill-3D's own experiments (Skill-3D evaluates on VSI-Bench, BLINK, CV-3D, MMSI-Bench only).

## WeftOS relevance — Verdict: WATCH
CC BY 4.0 permits commercial re-evaluation, so PhysBench is a viable secondary benchmark once WeftOS's Rust agent handles physical-dynamics claims (stability, causal prediction) rather than pure geometry — relevant to the "honest geometry" mandate since it directly tests physics claims a metric-grounded agent should get right. Not immediately needed for the initial geometry/spatial port; revisit when physical-dynamics reasoning is in scope. See cluster verdict table in [[papers/benchmarks-and-rl]].
