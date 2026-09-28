# [74] Wu et al. (2025c) — Reinforcing Spatial Reasoning in VLMs with Interwoven Thinking and Visual Drawing (VILASR)

**Citation:** J. Wu, J. Guan, K. Feng, Q. Liu, S. Wu, L. Wang, W. Wu, T. Tan. arXiv:2506.09965.
**Venue/ID:** arXiv preprint, 2025-06. https://arxiv.org/abs/2506.09965

## Summary
VILASR lets a vision-language model reason about spatial relationships by *drawing* on the image (bounding boxes, auxiliary lines) interleaved with text, rather than describing space purely in words — the visual-chain-of-thought is literally visual, not just textual. It is trained in three stages: synthetic-data initialization (teach the drawing primitives), reflective rejection sampling (improve self-assessment of when a drawing helped), and RL (optimize toward task reward).

## Objective / reward design
- Three-stage curriculum: (1) SFT on synthetic drawing-annotated data to establish the "draw a box/line" action vocabulary; (2) reflective rejection sampling — generate multiple candidate reasoning traces, keep/reflect on the ones whose drawings actually helped, to build a self-correction signal; (3) RL fine-tuning against task reward (exact reward formula not resolved from the fetched abstract — likely GRPO-family given the era/lineage, not confirmed).
- Action space is elementary (bounding boxes, auxiliary lines) rather than free-form image generation, keeping the RL action space tractable.

## Key results
Average +18.4% over prior methods across maze navigation, static spatial reasoning, video-based spatial reasoning, and multi-view tasks.

## Availability
No code repository link found on the fetched arXiv page. **License: CC BY-NC-ND 4.0** — non-commercial, no-derivatives, the most restrictive license in this cluster.

## How Skill-3D uses it
Cited in the related-work list of methods that "improve fine-grained spatial understanding by incorporating 3D reconstruction, depth cues, spatial VQA data, and explicit grounding" (paper-2606.07436.txt line 147, "...Wu et al. (2025c)") — grouped with grounding-based approaches, not adopted directly; Skill-3D's own tool-use approach retrieves and invokes external tools/skills rather than emitting inline visual drawing actions.

## WeftOS relevance — Verdict: SKIP (as a direct dependency), PATTERN (as an idea)
CC BY-NC-ND 4.0 forbids both commercial use and derivative works, ruling it out as a code/weights/data dependency for a product WeftOS intends to ship commercially. The underlying idea — grounding spatial reasoning in literal visual marks (boxes/lines) drawn on the image as an intermediate representation, rather than only text — is a reusable pattern worth reimplementing independently in the Rust agent's tool vocabulary (e.g., a "draw/annotate" tool with its own reward), but no code, weights, or data from this paper should be used. See license notes in [[papers/benchmarks-and-rl]].
