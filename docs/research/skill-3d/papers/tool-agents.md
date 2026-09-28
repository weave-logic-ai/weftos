# Cluster synthesis: tool-agents

Covers refs 5, 8, 16, 20, 27, 34, 37, 39, 41, 42, 44, 49, 50, 51, 53, 55, 56, 58, 59, 64,
67, 71, 75, 76, 85, 86, 91, 99, 100 cited by Skill-3D (arXiv 2606.07436). Full analyses are
at `papers/analysis/NNN-*.md`. This synthesis is written for the WeftOS Rust rewrite of
Skill-3D's approach, where tools are exposed through the WeftOS MCP server to Claude Code,
Grok, and Codex, and where "honest geometry" — appearance never mints metric geometry — is a
hard design constraint.

## Reference table

| # | Paper | Tool-call shape | Verdict |
|---|-------|------------------|---------|
| 5 | LVAgent | none (multi-agent text voting) | SKIP |
| 8 | Geometrically-Constrained Agent (GCA) | tool calls within a formalized constraint | PATTERN |
| 16 | GRIT | none (internal bbox tokens) | SKIP |
| 20 | TIGeR | generated code vs typed geometry API | ADOPT |
| 27 | ECP (high-res crop-reinfer) | none (inference pipeline) | WATCH |
| 34 | Olympus | structured routing tags | PATTERN |
| 37 | InsightX Agent | structured tool + reflection module | SKIP |
| 39 | LLaVA-Plus | inline text-tag tool calls | PATTERN |
| 41 | PySpatial | generated code (Python visual programs) | WATCH |
| 42 | WSI-Agents | agent-to-agent task allocation | SKIP |
| 44 | VADAR | dynamically generated Pythonic API | PATTERN |
| 49 | RieMind | structured queries on a persistent 3DSG | ADOPT |
| 50 | ByDeWay | none (fixed prompt-augmentation pipeline) | SKIP |
| 51 | Visual CoT | none (internal bbox re-attend) | PATTERN |
| 53 | HuggingGPT | structured JSON task graph | ADOPT |
| 55 | OpenThinkIMG | standardized RL-trained tool interface | WATCH |
| 56 | ViperGPT | generated code (typed Python API) | PATTERN |
| 58 | Object-Centric 3D Rollout | none (training-time augmentation) | WATCH |
| 59 | ObjectMLLM | pre-computed CV tool output, quantized to text | PATTERN |
| 64 | MLLM-Tool | structured tool selection (classification) | PATTERN |
| 67 | VisuoThink | generated code (matplotlib) + tree search | PATTERN |
| 71 | Visual ChatGPT | NL "template" tool invocation | PATTERN |
| 75 | VTool-R1 | generated code (image-edit ops), RL-trained | PATTERN |
| 76 | DetToolChain | structured visual-prompt toolkit | PATTERN |
| 85 | VCA | none (tree-search over video segments) | WATCH |
| 86 | MM-REACT | NL/ReAct-style prompted tool calls | PATTERN |
| 91 | Deep Video Discovery | tool calls in an adaptive search loop | PATTERN |
| 99 | ReVPT | fixed tool suite, RL-trained | WATCH |
| 100 | SegAgent | text click-point actions -> external executor | PATTERN |

## Taxonomy of tool-use designs

**1. JSON / structured-call orchestration.** A planner LLM emits typed task records; a
router dispatches to fixed executor models and threads results back by reference. HuggingGPT
(53) is canonical: `{"task", "id", "dep", "args"}` records with a `<resource>-task_id` handle
so a downstream task can consume an upstream task's raw output, not just its text
description. Olympus (34) is the same idea with a closed vocabulary of XML-style routing
tags. Most portable to a typed, compiled language — the schema is the contract.

**2. Generated-program / visual-programming.** An LLM writes and executes code against a
fixed or dynamically-extended API of vision primitives (ViperGPT 56, TIGeR 20, PySpatial 41,
VADAR 44, VisuoThink 67, VTool-R1 75). ViperGPT's `ImagePatch`/`VideoSegment` class is purest:
methods return typed Python values (crops, booleans, floats) the interpreter carries as state
between steps, no re-encoding through the LLM's text context. VADAR extends this to
*synthesizing new functions on demand* via dependency-first program construction. TIGeR is
the standout: it pairs code generation with a **metric** tool API (calibrated camera
intrinsics/extrinsics, depth sensor, SAM2 segmentation, 2D<->3D conversion) and a five-part
hierarchical RL reward (format / tool-call validity / parameter accuracy / execution
correctness / answer correctness) verifying each stage, not just the final answer.

**3. RL-trained adaptive tool use.** OpenThinkIMG (55), VTool-R1 (75), ReVPT (99), GRIT (16)
train the policy itself, via outcome-based reward, to decide when and how to invoke tools,
rather than prompting a frozen model — adaptive, but requires gradient access to the base
model, which is inapplicable to WeftOS's frozen-foundation-model setup (Claude Code, Grok,
Codex called over API). Useful only as a training-signal reference for a future in-house
router/verifier model.

**4. Iterative visual state loop.** DetToolChain (76), SegAgent (100), VTool-R1, and
VisuoThink (67) return an *updated image* (annotated, masked, cropped, click-refined) and let
the model re-observe and issue a correcting action. SegAgent's click -> SimpleClick ->
updated-mask Markov loop and DetToolChain's diagnose-then-refine chain are the cleanest: the
return value is itself the next input, and repair is implicit in issuing another action.

**5. Scene-graph / persistent-structure query.** RieMind (49) is the outlier: perception
(build a typed 3D scene graph once) is fully decoupled from reasoning (query
distances/poses/dimensions/areas/volumes via structured calls) — no code generation, no
per-question re-perception, every returned field carries real units. GCA (8) reaches a
similar place differently: a semantic-analyst stage formalizes an ambiguous NL query into a
verifiable geometric constraint *before* any tool call, so the executor cannot silently
redefine what "distance" means mid-computation.

## Best design for a Rust tool-call layer with typed contracts and repair

None of these 29 papers is Rust, so this is a synthesis, not a port:

- **Contract shape:** typed structs over JSON, not generated code, as the default —
  HuggingGPT's dependency-gated task record (`task`, `args`, `dep: Vec<TaskId>`, typed
  `Resource` handles for non-text payloads) maps directly onto Rust enums/structs with serde,
  unlike ViperGPT's free-generated-Python approach, which has no static contract. Reserve a
  `code_executor` escape hatch (as TIGeR does) for VADAR-style cases where no fixed
  composition suffices — sandboxed, narrowly scoped, downstream of a typed pre-check.
- **Metric discipline at the type level:** distinguish `MetricDistance(f64, Unit)` /
  `RelativeDepth(f32)` / `PixelBox(u32,u32,u32,u32)` as distinct types that cannot silently
  coerce. TIGeR and RieMind earn ADOPT because their return types trace to calibration
  (camera intrinsics, depth sensor, ground-truth 3DSG); ViperGPT's `compute_depth`, ReVPT's
  Depth-Anything-V2 tool, and ByDeWay's LDP layers are the negative examples — relative,
  monocular depth that must never type-check as metric.
- **Repair:** adopt TIGeR's staged verification (format -> tool-call validity -> parameter
  accuracy -> execution -> answer) as a `Result` chain with a distinct error variant per
  stage, paired with DetToolChain/SegAgent's "return updated state, allow another action"
  loop for inherently iterative tools — repair as "call again with more context," not hidden
  retries.
- **Planner/executor split:** keep it explicit in the type system (`Plan` vs
  `ExecutionTrace`), following HuggingGPT/WSI-Agents/RieMind rather than single-model designs
  (LLaVA-Plus, MM-REACT, ViperGPT's base loop) — the split is what makes a TIGeR-style
  verification stage attachable later without a rewrite.

## How an MCP-exposed tool surface serves multiple host agents

HuggingGPT's resource-reference symbol (`<resource>-task_id`) and Olympus's closed
routing-tag vocabulary both solve the same problem WeftOS's MCP server has: several distinct
callers (Claude Code, Grok, Codex) need to compose tool outputs without re-serializing large
payloads through each model's own text context. The transferable idea is a **host-agnostic
resource handle** — an opaque ID the MCP server resolves server-side — so an image, point
cloud, or scene-graph fragment produced by one tool call can be passed as an argument to the
next call by reference, regardless of which host model issued the request. MLLM-Tool (64)
adds a second lesson: tool *selection* should condition on the actual multimodal input
(image/scene), not just the text instruction, which argues for the MCP server surfacing tool
descriptions plus lightweight applicability hints (available modalities, coordinate frame,
metric vs relative) rather than names alone, so any host model — not just one tuned for tool
selection — can disambiguate correctly.

## Where geometry-grounded agents beat Skill-3D's approach

Skill-3D's own related-work framing (§1) critiques prior per-question tool-invocation
agents — naming PySpatial (41) and RieMind (49) alongside Zhang et al. (2026c) and Yuan et
al. (2026) — for "exhibit[ing] preferences toward a few dominant tools, regardless of what
each scene actually requires." RieMind answers a different critique that applies to Skill-3D
itself: by building a persistent, typed 3D scene graph *once* per scene and answering
arbitrarily many queries by structured lookup against it, RieMind never re-derives geometry
per question and never risks an appearance-based guess masquerading as a measurement — its
reported 16% gain over prior work and 33-50% gain over base VLMs on VSI-Bench's static split
(measured with a ground-truth-built 3DSG, an explicitly acknowledged upper bound) comes from
*not* reasoning about geometry implicitly at all. TIGeR (20) beats Skill-3D's approach on
precision-critical robotics: its calibrated camera/depth/segmentation tool chain and
hierarchical reward reach centimeter-level manipulation accuracy (79.30% average on its
spatial-reasoning benchmarks, beating Gemini 2.5-Pro by 5.83%; 55-70% real-world manipulation
success), a regime a skill-retrieval strategy without sensor grounding cannot match. GCA (8)
beats Skill-3D on reliability under ambiguity: forcing a semantic-analyst stage to convert a
vague query into a checkable constraint before computation runs, GCA reports large relative
gains (~27-49% depending on backbone) on queries where the bottleneck is deciding *what* to
compute — a formalization step Skill-3D's skill-memory retrieval has no equivalent for. The
common thread: all three win where geometry is decoupled from appearance and made persistent
(RieMind), calibrated (TIGeR), or formally constrained (GCA) — exactly the axis WeftOS's
"honest geometry" principle is built around, and where skill-library retrieval is weakest.
