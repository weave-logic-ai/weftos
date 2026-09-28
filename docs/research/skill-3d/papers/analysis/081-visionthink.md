# [81] Yang et al. (2025b) — VisionThink: Smart and Efficient VLM via Reinforcement Learning

**Citation:** S. Yang, J. Li, X. Lai, B. Yu, H. Zhao, J. Jia. arXiv:2507.13348.
**Venue/ID:** arXiv preprint, 2025-07. https://arxiv.org/abs/2507.13348, code: github.com/dvlab-research/VisionThink

## Summary
VisionThink addresses the token-efficiency problem in VLMs: high-resolution images cost many visual tokens, but most queries don't need full resolution. Rather than a fixed compression rate, the model starts at low resolution and — per-sample — decides via RL whether to request a resolution upsample, using an "LLM-as-Judge" signal to decide when low-res was insufficient.

## Objective / reward design
- Model emits a special token requesting image upsampling when it judges low-resolution input insufficient to answer.
- RL reward combines task-answer correctness with a calibrated penalty for over-requesting upsamples, keeping the upgrade rate low on easy queries while still upgrading for OCR/fine-detail-demanding queries.
- Uses "LLM-as-Judge" (rather than exact-match) to assess whether the low-res answer was adequate — needed because many VQA-style answers are open-ended.

## Key results
Matches strong performance on general VQA at 1/4 resolution (i.e., far fewer visual tokens) while retaining capability on detail-demanding OCR tasks, substantially cutting visual-token usage versus fixed-compression efficient-VLM baselines.

## Availability
Code and models public at github.com/dvlab-research/VisionThink. Paper license: CC-BY-SA 4.0 (share-alike — commercial use allowed but derivatives must be released under the same license).

## How Skill-3D uses it
Cited in the related-work sweep on RL-for-tool-use/efficiency in VLM agents (paper-2606.07436.txt line 151, grouped with other RL-training-for-VLM-agents citations) — not adopted directly; Skill-3D's tool-use efficiency reward (`R_tool`, Eq. 2) penalizes excess *tool calls*, a related but distinct concern from VisionThink's per-image *resolution* decision.

## WeftOS relevance — Verdict: PATTERN
Directly useful for a Rust agent doing MentraOS glasses capture: variable-resolution, RL-learned "when do I need to actually zoom in / re-capture at higher fidelity" decisions map cleanly onto compute/bandwidth-constrained edge capture (glasses have real power/bandwidth limits unlike a datacenter VLM). The CC-BY-SA 4.0 code could technically be adopted, but a share-alike obligation on anything built from it is a real constraint to weigh before pulling in code directly — safer to reimplement the reward-shaping pattern (correctness reward minus a calibrated over-request penalty) independently. See [[papers/benchmarks-and-rl]].
