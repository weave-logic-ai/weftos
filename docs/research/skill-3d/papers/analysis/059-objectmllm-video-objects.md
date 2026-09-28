# Ref 59 — Tang et al. (2025b), ObjectMLLM

**Citation:** Z. Tang, S. Wang, J. Cho, J. Yoo, and C. Sun. "How Can Objects Help Video-Language Understanding?" arXiv preprint arXiv:2504.07454, 2025.

**Source:** arXiv:2504.07454, https://arxiv.org/abs/2504.07454 (v1 Apr 10 2025, v2 Aug 5 2025). Code: https://github.com/brown-palm/ObjectMLLM.

## Summary
The paper asks whether explicit, object-centric visual representation is still needed once an MLLM backbone is strong. It introduces ObjectMLLM, a framework that pipes structured object-level output from off-the-shelf computer-vision tools (detectors/trackers) into an MLLM for video-language tasks. Across six video QA benchmarks the authors find explicit object integration still helps, and — the paper's headline methodological finding — quantizing continuous object attributes (boxes, tracks, attributes) into short text tokens before handing them to the LLM is a more effective and more data-efficient integration strategy than feeding raw/continuous features.

## Method specifics
- **Tool API shape:** Arbitrary external CV algorithms (detector/tracker) are called as black-box modules outside the LLM; there is no code-generation or JSON-function-call loop — object outputs are pre-computed then serialized.
- **Return path:** Structured object data (boxes, IDs, attributes, temporal tracks) is quantized/discretized into plain text tokens and spliced into the MLLM's text context — not returned as images or raw tensors.
- **Planner/executor split:** Effectively a single-pass pipeline (CV tool extraction → text serialization → MLLM answer), not an iterative agentic loop; no explicit planner deciding which tool to call per-question.
- **Error handling:** Not found — no retry/self-correction loop described in the fetched abstract/summary; robustness comes from the quantization choice, not runtime repair.

## Results
Six video-QA benchmarks (exact benchmark names/numbers not found in the fetched abstract text — full comparison tables were not accessible via WebFetch). The reported qualitative finding is that explicit object-centric integration, especially via text quantization, outperforms continuous-feature integration and implicit (no-object) baselines.

## Code / license
Code and models released at github.com/brown-palm/ObjectMLLM. License: not found (not confirmed in this pass).

## Relation to Skill-3D
Cited at Skill-3D §2.2 in the sentence: "A complementary line of work trains VLMs to use tools through supervised fine-tuning or reinforcement learning ... Tang et al. (2025b) ..." — grouped with MLLM-Tool, DetToolChain, and VTool-R1 as training-based tool-use approaches, contrasted with Skill-3D's prompting/skill-library approach.

## WeftOS relevance
**PATTERN.** The "quantize structured perception output to text before it reaches the LLM" idea is directly relevant to how WeftOS MCP tools should serialize CV/geometry results for Claude/Grok/Codex, but object boxes/tracks here are image-relative, not metric — no scale/units are asserted, consistent with "honest geometry" as long as WeftOS doesn't relabel them as real-world measurements. Not egocentric-specific but video-general, so only loosely relevant to MentraOS capture.
