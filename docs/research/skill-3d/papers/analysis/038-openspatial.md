# 038 — OpenSpatial: A Principled Data Engine for Empowering Spatial Intelligence

**Citation:** Liu, J., Sun, H., Li, W., Zhang, Y., Yang, R., Zhu, Z., Yang, Y., Zheng, S., Jiang, N., Jiang, J., et al. *OpenSpatial: A Principled Data Engine for Empowering Spatial Intelligence.* arXiv preprint.

**arXiv:** [2604.07296](https://arxiv.org/abs/2604.07296) · GitHub: `VINHYU/OpenSpatial` (per fetch).

## Summary
Not a model — a **data-generation engine**. Builds a scalable pipeline around 3D bounding boxes as the atomic primitive, generating training data across five task families (spatial measurement, spatial relationship, camera perception, multi-view consistency, scene-aware reasoning). Produces **OpenSpatial-3M**, a 3M-sample dataset, and trains models on it to show downstream gains.

## Method specifics
- **Representation:** 3D bounding boxes as the hierarchy root; the engine composes measurement/relationship/camera/multi-view/scene tasks on top of boxes, presumably sourced from existing 3D-annotated scene datasets (not confirmed which upstream 3D source — not found in the fetched abstract).
- **Metric scale:** boxes imply a metric or near-metric 3D annotation source, but the abstract fetched did not specify whether OpenSpatial-3M itself is metric-scaled or normalized/synthetic. **Not found** — flag for follow-up before citing scale claims.

## Key results
- Models trained on OpenSpatial-3M show **~19% average relative improvement** across spatial reasoning benchmarks (per fetched summary; exact benchmark list not confirmed — treat as approximate until PDF is read).

## Code / license
Code at github.com/VINHYU/OpenSpatial. **License not found** in the fetched content.

## Skill-3D relation
Cited twice in §2.1: first among "dedicated benchmarks" (line 147, alongside Chow et al. 2025, Cai et al. 2025, Majumdar et al. 2024), then again grouped with training-data/benchmark infrastructure for spatial reasoning. It's cited as a benchmark/data-infrastructure precedent, not a method Skill-3D borrows algorithmically.

## WeftOS relevance
A **data engine**, not a runtime component — the closest WeftOS parallel is thinking about how to synthesize training/eval data for a Rust Skill-3D reimplementation (e.g., generating labeled AABB/relationship pairs from Urth W1 leaves for regression tests or fine-tuning a small routing model). The 3D-bounding-box-as-primitive design is philosophically aligned with WeftOS's `WM_OBJECT` AABB leaves, but OpenSpatial is a dataset-generation tool for training VLMs, which is out of WeftOS's current scope (WeftOS uses hosted models, not custom fine-tunes).

**Verdict: WATCH** — relevant only if WeftOS later needs synthetic spatial-QA training/eval data generated from Urth leaves; not adoptable today.
