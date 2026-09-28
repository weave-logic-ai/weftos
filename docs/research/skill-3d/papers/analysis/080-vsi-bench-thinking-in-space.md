# [80] Yang et al. (2025a) — Thinking in Space: How MLLMs See, Remember, and Recall Spaces (VSI-Bench)

**Citation:** J. Yang, S. Yang, A. W. Gupta, R. Han, L. Fei-Fei, S. Xie. In CVPR 2025, pp. 10632–10643.
**Venue/ID:** CVPR 2025; arXiv:2412.14171. https://arxiv.org/abs/2412.14171, project page vision-x-nyu.github.io/thinking-in-space.github.io/

## Summary
VSI-Bench is a video-based benchmark testing whether MLLMs build usable "visual-spatial intelligence" — seeing, remembering, and recalling spatial layout — from egocentric video, across 8 indoor-scene categories (counting, distance, size, direction, route planning, appearance-order, room-size, etc.). Finds MLLMs show competitive-but-subhuman spatial intelligence, that standard textual chain-of-thought techniques (CoT, self-consistency, tree-of-thoughts) do **not** help, but explicitly generating a cognitive map during answering does improve distance estimation — evidence that these models have a latent, extractable "local world model."

## Specifics
- **Task types:** 8 categories — object counting, absolute distance, object size, room size, relative distance, relative direction, route planning, appearance order.
- **Data sources:** indoor scene video from real-world scanned environments; Skill-3D's own eval protocol (borrowed from Think3D, Zhang et al. 2026c) uniformly samples 7 frames per video as model input rather than using full video (paper-2606.07436.txt line 474-478) — implying VSI-Bench's native format is full egocentric video over scanned indoor scenes (consistent with ScanNet/ARKitScenes-family capture, not independently confirmed from the fetched abstract page).
- **Size:** >5,000 QA pairs.
- **Ground-truth provenance:** derived from the underlying 3D scans/point clouds of the source scenes (not confirmed in detail from the fetched abstract).
- **Known flaws / leakage — important:** ReVSI [92] exists specifically because VSI-Bench (and sibling benchmarks) were found to have **annotation artifacts from point-cloud data producing invalid QA pairs**, and evaluations that assume full-scene access while models actually use sparse frame sampling — i.e., some VSI-Bench questions may be unanswerable or mis-keyed under realistic sparse-sampling evaluation. Treat VSI-Bench numbers with that caveat; see [[092-revsi]] for the fix.
- **License:** CC BY 4.0.

## Key results
MLLMs show non-trivial but clearly subhuman spatial intelligence; cognitive-map generation (not CoT) is the intervention that actually helps distance estimation.

## How Skill-3D uses it
One of Skill-3D's four primary evaluation benchmarks (paper-2606.07436.txt line 474, 1783, and the headline result in the abstract: "skill-guided agentic post-training further boosts Qwen3-VL-8B by 43% on VSI-Bench Yang et al. (2025a)"). Skill-3D follows the Think3D protocol: uniformly sample 7 frames per scene video, 30%/70% category-stratified train/test split.

## WeftOS relevance — Verdict: ADOPT (with the ReVSI caveat)
VSI-Bench is Skill-3D's headline eval and the field's de facto standard for indoor egocentric spatial intelligence — WeftOS's Rust reimplementation should evaluate on it for direct comparability, under CC BY 4.0 (commercial-safe). But given ReVSI's documented annotation-artifact and sparse-sampling-mismatch findings, WeftOS should prefer **ReVSI's re-annotated version** [92] as the actual scoring benchmark and only use raw VSI-Bench for direct literature comparison. See [[papers/benchmarks-and-rl]].
