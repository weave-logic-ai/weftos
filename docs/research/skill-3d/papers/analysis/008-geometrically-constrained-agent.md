# Ref 8: Geometrically-Constrained Agent (GCA)

**Citation:** Z. Chen, X. Lu, Z. Zheng, P. Li, L. He, Y. Zhou, J. Shao, B. Zhuang, L. Sheng. "Geometrically-Constrained Agent for Spatial Reasoning." arXiv:2511.22659, 2025.

**Source:** arXiv:2511.22659 (https://arxiv.org/abs/2511.22659); project page: gca-spatial-reasoning.github.io; code: github.com/gca-spatial-reasoning/gca

## Summary

GCA targets what the authors call the "semantic-to-geometric gap": VLMs handle qualitative/semantic spatial questions well but fail at high-fidelity geometric computation. GCA is a training-free agentic method that decouples the VLM's role into two stages — a semantic analyst that translates an ambiguous natural-language query into a formal, verifiable task constraint (defining reference frame and objective), and a task solver that then generates and executes tool calls strictly within the bounds set by that constraint, producing a deterministic, verifiable geometric answer rather than a free-form guess.

## Method specifics

- **Tool API shape:** described as "generates and executes tool calls" within deterministic bounds — consistent with a code/structured tool-call agent, but the exact call format (JSON vs generated code) is not stated in the accessible abstract/project page text.
- **Output return path:** not explicitly documented in accessible sources; framed as producing a verifiable geometric answer, implying structured/numeric tool outputs.
- **Planner/executor split:** yes, explicit two-stage split — semantic analyst (planner/formalizer) then task solver (executor) — same VLM instance in two role-prompted passes, not necessarily two separate models.
- **Error handling:** the constraint-formalization step is the verification mechanism (a query must be formalized into a checkable constraint before execution); the paper's own error analysis attributes ~30% of errors to the Task Formalization stage and ~70% to the Geometric Computation stage, implying no full automatic repair loop — errors are analyzed post hoc, not corrected online (unverifiable whether there's a runtime retry).

## Results

Reported as "~27%" improvement over existing training-based and tool-integrated methods on multiple (unnamed in accessible text) spatial reasoning benchmarks; "37% relative improvement" on average across tested backbones, up to "49% gain" on Gemini-2.5-Pro. Specific benchmark names not found in accessible abstract/project-page text — flagged as not found.

## Code/weights

Code link present (github.com/gca-spatial-reasoning/gca) per the project page, but a specific code license was not verified — not found.

## Skill-3D relation

Cited in §2.1 ("MLLMs for Spatial Reasoning"): "Other works enhance spatial reasoning through prompting, mental simulation, visual chain-of-thought, reinforcement learning, code-driven 3D reasoning, and generative imagination of 3D space Taguchi et al. (2025); ...; Chen et al. (2025c); ..." — grouped among methods that improve spatial reasoning without necessarily using Skill-3D's scene-aware skill memory.

## WeftOS relevance

**PATTERN.** The semantic-analyst/task-solver split that forces a formal, checkable constraint before geometric tool execution is directly relevant to WeftOS's "honest geometry" principle — it's a concrete pattern for preventing appearance-based reasoning from silently minting metric claims. Worth studying the constraint-formalization step as a template for a WeftOS tool-call gate, but not adoptable as-is since the exact tool schema and metric-grounding source (camera calibration? depth sensor? none — possibly pixel-only) are unverified.
