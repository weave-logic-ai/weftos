# [19] A New Era of Intelligence with Gemini 3

**Citation:** Google (2025). "A new era of intelligence with gemini 3." External link (Google blog post, not arXiv — no technical report available at time of writing).
**URL:** Google's official Gemini 3 announcement (blog.google), released November 18, 2025.

## Summary
Google's announcement post for Gemini 3, positioned as a "reasoning-first" frontier multimodal model with a "Deep Think" extended-reasoning mode and integration into Google Antigravity (an agentic infrastructure layer aimed at long-context, low-latency multi-step workflows). Third-party coverage reports strong multi-image/video understanding at launch (MMMU-Pro ~81.0%, Video-MMMU ~87.6%, ARC-AGI-2 ~31.1% per secondary sources) — these figures are from post-launch analysis, not independently re-verified against Google's primary announcement in this pass, and should be treated as directional.

## Model Tiers
- Gemini 3 Pro — the flagship reasoning tier referenced in most third-party benchmark coverage.
- **Gemini-3-Flash** — the smaller/cheaper/faster tier, and the specific variant Skill-3D evaluates as an agent backbone, consistent with agent-loop use cases where many tool-calling turns are needed and per-call latency/cost matters more than peak single-shot capability.

## Role in Skill-3D
**Agent backbone, and the paper's own headline result.** Gemini-3-Flash is one of four closed-source MLLM agents evaluated (§4.1); the abstract's single most-quoted number is that Skill-3D "improves Gemini-3-Flash by 67% on MMSI-Bench" — making Gemini-3-Flash the model on which Skill-3D demonstrates its largest single-model relative gain of any backbone in the paper. Skill-3D also drives the shared closed-source VSI-Bench average from 42.9 to 64.5 across all four backbones including this one.

## API / Weights
Closed weights, API-only (Gemini API / Vertex AI / Google AI Studio). No open release of any Gemini 3 tier, including Flash. Standard commercial API terms.

## Reported Spatial Benchmark Numbers
No isolated primary-source Gemini-3-Flash VSI-Bench/MMSI-Bench number was independently verified beyond what Skill-3D itself reports. The paper's "67% on MMSI-Bench" figure is Skill-3D's *improvement over* Gemini-3-Flash's own w/o-tools baseline on that benchmark, not an absolute score in isolation — the absolute baseline number sits in Table 1, which was not fully machine-extracted in this pass. Treat any absolute spatial-benchmark figure quoted for Gemini-3-Flash as needing verification against the raw table before citing elsewhere.

## WeftOS Relevance
**Verdict: WATCH.** As with Gemini 2.5, this is a viable *cloud-tier* reasoning backbone candidate for a WeftOS spatial agent, not an on-device component — closed weights rule out edge/offline use entirely. Notably, Skill-3D's largest reported single-model gain is on this exact model, which is a useful signal that scene-aware skill routing compounds well even with an already-strong frontier backbone — worth citing in any WeftOS design doc arguing that tool-routing skill matters even when the base model itself is already frontier-grade, not just as a crutch for weaker models.
