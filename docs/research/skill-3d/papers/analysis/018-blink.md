# [18] Fu et al. (2024) — BLINK: Multimodal LLMs Can See but Not Perceive

**Citation:** X. Fu, Y. Hu, B. Li, Y. Feng, H. Wang, X. Lin, D. Roth, N. A. Smith, W. Ma, R. Krishna. In ECCV 2024, pp. 148–166.
**Venue/ID:** ECCV 2024; arXiv:2404.12390. https://arxiv.org/abs/2404.12390, code: github.com/zeyofu/BLINK_Benchmark

## Summary
BLINK reformats 14 classic computer-vision tasks (not typical VQA) into a multiple-choice benchmark testing core visual perception that humans solve "within a blink" — relative depth, visual correspondence, forensic detection, multi-view reasoning, jigsaw, spatial relation, etc. The point is that these tasks resist mediation through natural language, exposing a gap between VLM "seeing" and actual perceiving.

## Specifics
- **Task types:** 14 tasks reformatted as multiple choice, incl. relative depth estimation, visual correspondence, forensic detection, multi-view/spatial reasoning, jigsaw, IQ-test-style visual puzzles.
- **Data sources:** curated/re-purposed from existing CV datasets and newly collected images (exact per-task source list not resolved from the fetched abstract; check the paper for per-task provenance).
- **Size:** 3,807 multiple-choice questions, single- or multi-image, with visual prompting overlays (boxes/arrows/masks) where relevant.
- **Metrics:** accuracy (multiple choice).
- **Ground-truth provenance:** derived from the original CV task's ground truth (e.g., depth sensors, camera calibration for multi-view) re-packaged as multiple-choice distractor sets — not confirmed in detail from the fetched page.
- **Known flaws:** none flagged in the fetched material; the benchmark's own thesis is that current VLMs are near-random on several sub-tasks, which is a model-capability finding, not a benchmark defect.
- **License:** CC BY-NC-SA 4.0 — **non-commercial**.

## Key results
Human accuracy 95.70% average; best 2024-era models (GPT-4V 51.26%, Gemini 45.72%) barely above the ~38% random baseline on several perception-heavy sub-tasks.

## How Skill-3D uses it
One of Skill-3D's four evaluation benchmarks (paper-2606.07436.txt line 474, 1787): "BLINK Fu et al. (2024) evaluates challenging multimodal reasoning. We use its multi-view spatial reasoning subset, which tests spatial inference from multiple visual observations." Skill-3D subsamples 30% per category for training (per the Think3D protocol) and reports on the remaining disjoint test split.

## WeftOS relevance — Verdict: ADOPT (with license caveat)
Skill-3D itself uses BLINK's multi-view subset as an eval, so WeftOS's Rust reimplementation should too, for apples-to-apples comparison. **CC BY-NC-SA 4.0 blocks commercial use** — fine for internal research/eval, but WeftOS cannot ship BLINK data or a BLINK-trained checkpoint in a commercial product without a separate license. See the "benchmarks to avoid commercially" note in [[papers/benchmarks-and-rl]].
