# Ref 56 — Surís et al. (2023), "ViperGPT"

**Full citation:** D. Surís, S. Menon, and C. Vondrick. "ViperGPT: Visual Inference via
Python Execution for Reasoning." In Proceedings of the IEEE/CVF International Conference on
Computer Vision (ICCV), pp. 11888–11898, 2023.

**arXiv / venue:** arXiv:2303.08128 (https://arxiv.org/abs/2303.08128); ICCV 2023. Official
code: github.com/cvlab-columbia/viper. Project page: viper.cs.columbia.edu.

**Summary:** ViperGPT is the foundational "visual programming" paper: it uses a
code-generation LLM (Codex) to compose a set of pretrained vision-and-language models into
executable Python subroutines that answer a visual query. Rather than an end-to-end model
that entangles perception and reasoning, ViperGPT explicitly separates them — the LLM writes
a program against a fixed API of vision primitives, a Python interpreter executes that
program, and the final return value is the answer — requiring no additional training on top
of the frozen component models.

**Method specifics:**
- Tool API shape: **code/programs, generated and executed** (not JSON tool calls). The LLM
  is shown the API signature as in-context Python class definitions (no execution examples)
  and free-generates a Python function body. The core API is an `ImagePatch` class
  (extended by `VideoSegment` for video) exposing methods: `crop`, `overlaps_with`, `find`,
  `exists`, `best_text_match`, `verify_property`, `simple_query`, `llm_query`, and
  `compute_depth` — each method internally dispatches to a specialized vision/language model
  (detector, VQA model, CLIP-style matcher, MiDaS depth estimator, etc.).
- Output channel: **mixed and typed by return type** — methods return Python objects
  (nested `ImagePatch` crops from `find`, booleans from `exists`, strings from
  `simple_query`/`llm_query`, floats from `compute_depth`) flowing directly between program
  steps as ordinary Python values; the final return value (often a string) is the answer.
  No re-encoding through the LLM's text context between steps — the interpreter carries state.
- Planner/executor split: a **single LLM call** generates the entire program up front
  (one-shot code generation, not an interactive step-by-step agent loop); the Python
  interpreter is the "executor" but is not itself an LLM — no planner-LLM/executor-LLM pair,
  no iterative re-prompting during execution in the base method.
- Error handling: not found — the paper does not describe an explicit retry,
  self-correction, or verification loop for buggy generated programs in the base method
  (later work, e.g. "PropTest," extends ViperGPT with property-based test verification,
  implying the base method lacks this).

**Key quantitative results:** 48.1% accuracy on GQA (zero-shot compositional visual
reasoning), reported as a state-of-the-art zero-shot result at publication; the paper also
reports state-of-the-art zero-shot results on visual grounding and video reasoning tasks
(specific numbers for RefCOCO/NExT-QA not confirmed in retrieved material — not found).

**Code/license:** Code available at github.com/cvlab-columbia/viper under **CC
BY-NC 4.0** (Creative Commons Attribution-NonCommercial 4.0 International — verified via
repo LICENSE file), i.e. non-commercial use only.

**Skill-3D relation:** Cited in §2.2's opening sentence alongside HuggingGPT and Wu et al.
(2023): "Tool augmentation extends MLLM by allowing them to invoke external modules through
prompting, structured APIs, or code generation. Representative systems demonstrate that
external tools can compensate for limitations of end-to-end multimodal models Shen et al.
(2023); Wu et al. (2023); Surís et al. (2023)." ViperGPT represents the "code generation"
branch of that taxonomy, as distinct from HuggingGPT's structured-API orchestration.

**WeftOS relevance: PATTERN.** The program-as-tool-composition idea (typed API of visual
primitives, single LLM-authored program, interpreter carries intermediate state) is a clean
reference pattern for a code-execution tool in the WeftOS MCP surface, but the CC BY-NC 4.0
license blocks direct code reuse, the base method has no error-repair loop, and
`compute_depth` returns only *relative* per-patch depth — never adopt as metric geometry.
