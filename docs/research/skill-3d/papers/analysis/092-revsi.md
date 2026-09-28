# [92] Zhang et al. (2026b) — ReVSI: Rebuilding Visual Spatial Intelligence Evaluation for Accurate Assessment of VLM 3D Reasoning

**Citation:** Y. Zhang, J. Chen, J. Tan, Y. Mao, W. Chen, A. X. Chang. arXiv:2604.24300.
**Venue/ID:** arXiv preprint, 2026-04; ICML 2026. https://arxiv.org/abs/2604.24300, project page: 3dlg-hcvc.github.io/revsi/

## Summary
ReVSI exists to fix systematic evaluation flaws found across existing visual-spatial-intelligence benchmarks (VSI-Bench [80] and four sibling datasets). Two core problems: (1) annotation artifacts inherited from point-cloud-derived ground truth produce QA pairs that are invalid or unanswerable from the images actually shown; (2) evaluation protocols assume the model has full-scene access, while real deployments (and most published evals) use sparse frame sampling — so a question can be "correct" against the full scene but unanswerable from the sampled frames actually given to the model. ReVSI re-annotates to guarantee every QA pair is answerable and correct *under the model's actual input*.

## Specifics
- **Task types:** inherits/re-annotates the task taxonomy of the 5 source benchmarks it rebuilds (VSI-Bench-style: counting, distance, size, direction, route planning, etc.), rather than defining new task categories.
- **Data sources:** 381 scenes re-annotated across five existing spatial-intelligence datasets (VSI-Bench and 4 others — exact sibling list not resolved from the fetched material; check the paper for the full source list).
- **Size:** 381 scenes; multiple frame-budget variants provided (16/32/64/all frames) so evaluation can match a model's actual sampling regime, plus fine-grained per-object visibility metadata (so a question can be filtered to only ask about objects actually visible under a given frame budget).
- **Metrics:** same task-level accuracy metrics as source benchmarks, but scored only against answerable-under-input QA pairs; supports diagnostic evaluation across frame budgets.
- **Ground-truth provenance:** re-annotated with human verification and explicit bias-mitigation strategies, directly addressing the point-cloud-artifact problem in the source data.
- **Known flaws (of the *originals*, which is ReVSI's raison d'être):** point-cloud annotation artifacts producing invalid QA; full-scene-access assumption mismatched to sparse-sampling deployment — see [[080-vsi-bench-thinking-in-space]] for the specific benchmark this most directly patches.
- **License:** CC BY 4.0.

## Key results
Not resolved in detail from the fetched material beyond the methodology; the paper's contribution is primarily the corrected benchmark and diagnostic frame-budget variants rather than a single headline accuracy number — check the paper directly for re-scored model rankings under ReVSI vs. original VSI-Bench.

## How Skill-3D uses it
Cited only once, in the related-work list of "dedicated benchmarks" (paper-2606.07436.txt line 147, "...Majumdar et al. (2024); Liu et al. (2026b); Zhang et al. (2026b)") — **not used as an evaluation benchmark in Skill-3D's own experiments**, which still report on the (potentially flawed) original VSI-Bench. This is a gap in Skill-3D's own eval rigor worth noting.

## WeftOS relevance — Verdict: ADOPT
Given WeftOS's "honest geometry" mandate, ReVSI's exact failure mode — questions that are only correct under an assumption of full-scene access the deployed model doesn't actually have — is precisely the trap a metric-honest Rust agent must avoid. WeftOS should score against ReVSI rather than raw VSI-Bench wherever possible, and adopt its frame-budget-variant methodology (evaluate under the *actual* sampling regime the deployed agent will use, not an idealized full-scene one) as house practice for all WeftOS spatial evals, not just this one benchmark. See [[papers/benchmarks-and-rl]].
