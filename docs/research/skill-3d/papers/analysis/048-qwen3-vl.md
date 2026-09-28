# [48] Qwen3-VL: Sharper Vision, Deeper Thought, Broader Action

**Citation:** QwenTeam (2025). "Qwen3-vl: sharper vision, deeper thought, broader action." Note: https://qwen.ai/blog?id=99f0335c4ad9ff6153e517418d48535ab6d8afef&from=research.latest-advancements-list
**URL:** https://qwen.ai/blog (announcement); code: https://github.com/QwenLM/Qwen3-VL; weights: https://huggingface.co/Qwen (collection)

## Summary
Qwen3-VL is Alibaba Qwen team's open-weight multimodal LLM family, spanning dense (4B, 8B) and Mixture-of-Experts (30B-A3B, 235B-A22B) sizes, each with separate Instruct and Thinking (extended-reasoning) variants, released across September–October 2025. It is positioned by the Qwen team as competitive with closed frontier models on vision-language benchmarks while remaining fully open-weight and self-hostable.

## Sizes / Release Timeline
- **Qwen3-VL-4B** and **Qwen3-VL-8B** (dense, Instruct/Thinking) — released 2025-10-15. These are the two sizes Skill-3D actually trains on.
- **Qwen3-VL-30B-A3B** (MoE, Instruct/Thinking) — released 2025-10-04.
- **Qwen3-VL-235B-A22B** (MoE, Instruct/Thinking) — released 2025-09-23, the flagship/largest tier.
- FP8-quantized checkpoints published alongside the full-precision releases for several sizes.

## Role in Skill-3D
**The base model for skill-guided agentic post-training — the paper's central open-source result.** Skill-3D fine-tunes Qwen3-VL-4B and Qwen3-VL-8B with 500 SFT samples + 1k GRPO samples (composite reward: 0.6 answer-correctness + 0.2 tool-use-efficiency + 0.2 skill-tool-format), one epoch, on 4× RTX PRO 6000 Blackwell (SFT ~3h, GRPO ~28h), producing Skill-3D-4B and Skill-3D-8B. Headline result: **Skill-3D boosts Qwen3-VL-8B by 60% on VSI-Bench** (abstract). Table 2 confirms per-category numbers for the 8B model: VSI-Bench Object Counting 32.5→56.5, Abs. Dist. 24.3→48.6, Obj. Size 30.8→59.8, Room Size 41.6→67.9, Rel. Dist. 40.2→52.0, Rel. Dir. 41.6→60.1, Route Plan 35.5→58.4, Appr. Order 47.3→66.8; BLINK MV 43.8→68.5; CV-3D Depth Order 68.8→89.6, Rel. Dist. 66.5→87.4; MMSI-Bench PR 31.0→42.8. Qwen3-VL-4B shows the identical pattern at somewhat lower absolute values (e.g. Obj. Cnt. 26.6→48.6). Overall relative VSI-Bench gains: 59.7% (4B), 60.3% (8B), over each model's own w/o-Tools baseline.

## API / Weights
**Fully open weights, Apache-2.0 licensed** (confirmed via the `QwenLM/Qwen3-VL/LICENSE` file on GitHub and the HuggingFace model cards). No field-of-use restrictions, no non-commercial clause — clean permissive license across every published size.

## Reported Spatial Benchmark Numbers
See the Table 2 breakdown above — these are the most complete, directly machine-extracted numbers of any reference in this cluster, since Skill-3D's own open-source evaluation table is reproduced verbatim in the paper text.

## WeftOS Relevance
**Verdict: ADOPT.** This is the single most directly reusable artifact in the cluster for a WeftOS Rust rewrite: Apache-2.0 license (no commercial blocker at all), open weights at exactly the 4B/8B sizes proven practical for agentic post-training with modest compute (4 GPUs, low-tens-of-hours training), and a fully documented SFT+GRPO recipe to imitate or adapt. For MentraOS egocentric glasses + honest-geometry tool use, Qwen3-VL-4B/8B is the clear base-model choice — small enough to plausibly run at the edge or on a modest remote-GPU budget, large enough to show a 60% VSI-Bench gain under skill-guided training in the source paper. Inference-side Rust support (candle/burn/ort for the Qwen3-VL architecture specifically) was not directly verified in this pass and should be checked separately before committing to an on-device inference path — this analysis only confirms weights, license, and training recipe, not a Rust runtime.
