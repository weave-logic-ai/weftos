# [44] Visual Agentic AI for Spatial Reasoning with a Dynamic API (VADAR)

**Citation:** Marsili et al. (2025). D. Marsili, R. Agrawal, Y. Yue, and G. Gkioxari. "Visual agentic ai for spatial reasoning with a dynamic api." In Proceedings of the Computer Vision and Pattern Recognition Conference (CVPR 2025), pp. 19446–19455.
**arXiv:** https://arxiv.org/abs/2502.06787 (v1 Feb 10 2025, revised Mar 28 2025) · Project page: https://glab-caltech.github.io/vadar/ · CVPR: https://openaccess.thecvf.com/content/CVPR2025/html/Marsili_Visual_Agentic_AI_for_Spatial_Reasoning_with_a_Dynamic_API_CVPR_2025_paper.html

## Summary
VADAR (Caltech: Marsili, Agrawal, Yue, Gkioxari) is a training-free agentic program-synthesis approach for 3D spatial visual reasoning. Rather than relying on a fixed, human-authored API of vision functions, LLM agents collaboratively generate a *dynamic* Pythonic API — new functions are synthesized on the fly to solve subproblems a query requires, then composed to answer 3D spatial queries. The authors also introduce a new benchmark of multi-step grounding-and-inference queries (built in part on Omni3D-Bench) and show VADAR outperforms prior zero-shot 3D visual-reasoning baselines.

## Method Specifics
- **Tool API shape:** code/program generation — LLM agents write and execute a dynamically-extended Pythonic API rather than issuing structured/JSON tool calls to a fixed toolset. This is explicitly the flexible alternative to static human-defined APIs.
- **Output return:** the generated program breaks a query into subproblems addressed by vision-specialist modules (e.g. object detection), composing their outputs (detections, attributes, positions) programmatically; results are pythonic values (numbers, object lists) passed between generated functions rather than free text.
- **Planner vs executor:** effectively planner+executor combined via a dependency-first strategy — VADAR performs a depth-first-search-style dependency resolution to build a "tree of dependencies," implementing/generating each needed method before it is called at runtime, then executes the composed program. This is closer to a single-model program-synthesis planner that also authors the executable code, rather than a separate planner LLM + separate executor LLM.
- **Error handling/repair:** the dependency-first construction (ensuring all called methods exist before execution) is itself a structural error-avoidance mechanism; the authors' failure analysis notes common errors stem from underlying vision-module failures and that performance degrades on queries requiring 5+ inference steps — no explicit retry/self-correction loop beyond this was confirmed in fetched content.

## Quantitative Results
Evaluated on Omni3D-Bench (500 non-templated queries) and CLEVR; reported to outperform prior zero-shot 3D visual-reasoning baselines. Specific numeric accuracy scores were not found in the fetched content (the project page/abstract summary did not surface exact percentages) — treat precise numbers as not found pending a direct PDF read.

## Metric vs Relative Geometry (important per task brief)
VADAR explicitly "grounds objects in 3D and combin[es] predicted attributes to reason about distances and dimensions in three dimensions" — i.e. it does attempt metric-style distance/dimension reasoning, built from per-object grounding/attribute-prediction modules composed via the generated program. Whether the underlying grounding modules produce calibrated real-world-scale metric estimates or relative/estimated dimensions was not confirmed in fetched content; the framing ("predicted attributes") suggests model-estimated (not sensor-calibrated) values, so treat outputs as estimated/approximate rather than verified metric ground truth.

## Code/Weights/License
Code available via GitHub (linked from project page); Omni3D-Bench dataset hosted on Hugging Face. License: CC BY-NC-SA 4.0 (non-commercial, share-alike) per arXiv abstract page.

## Relation to Skill-3D
Cited at §2.1 (MLLMs for Spatial Reasoning): "Other works enhance spatial reasoning through prompting, mental simulation, visual chain-of-thought, reinforcement learning, code-driven 3D reasoning, and generative imagination of 3D space Taguchi et al. (2025); Marsili et al. (2025); Tang et al. (2025a) ..." — VADAR is grouped alongside other reasoning-strategy enhancements to MLLM spatial reasoning (not in the §2.2 tool-agent cluster, despite being architecturally a tool/code-generation agent), suggesting Skill-3D reads it primarily as a spatial-reasoning-strategy paper rather than a general tool-augmentation paper.

## WeftOS Relevance
**Verdict: PATTERN.** The dynamic-API / program-synthesis pattern (agents author new callable functions on demand rather than being limited to a fixed toolset) is a genuinely useful architectural idea for a Rust agent OS exposing tools via MCP — it suggests a path for "compose new composite tools at runtime" rather than a fixed tool roster. However, its estimated (not sensor-grounded) distance/dimension outputs mean it should be treated as an architecture reference, not a source of trustworthy metric geometry; non-commercial license also blocks direct code reuse. Not directly egocentric/robotics-relevant beyond general applicability.
