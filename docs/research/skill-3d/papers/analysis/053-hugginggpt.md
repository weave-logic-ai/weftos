# Ref 53 — Shen et al. (2023), "HuggingGPT"

**Full citation:** Y. Shen, K. Song, X. Tan, D. Li, W. Lu, and Y. Zhuang. "HuggingGPT:
Solving AI Tasks with ChatGPT and its Friends in Hugging Face." Advances in Neural
Information Processing Systems (NeurIPS) 36, pp. 38154–38180, 2023.

**arXiv / venue:** arXiv:2303.17580 (https://arxiv.org/abs/2303.17580); NeurIPS 2023.
Official code released as "JARVIS" (microsoft/JARVIS on GitHub).

**Summary:** HuggingGPT uses an LLM (ChatGPT) as a controller that orchestrates a large pool
of existing, specialized AI models hosted on Hugging Face to solve complex, multi-modal user
requests end to end. The philosophy: LLMs are strong at language understanding/planning but
weak at cross-modal execution, so let the LLM plan and delegate to existing vision/speech/etc
expert models via natural-language-mediated task descriptions, then stitch results back
together. It is the foundational tool-orchestration (rather than tool-generation) paper in
this citation cluster.

**Method specifics:**
- Tool API shape: **structured JSON task objects, not generated/executed code.** The
  four-stage pipeline is: (1) Task Planning — ChatGPT parses the user request into a list of
  JSON task records of the form `{"task": <type>, "id": <int>, "dep": [<ids>],
  "args": {"text":..., "image":<URL>, "audio":<URL>, "video":<URL>}}`; (2) Model Selection —
  for each task, Hugging Face models are pre-filtered by task type and the LLM picks the
  best one from a top-K in-context candidate list (using descriptions/tags/downloads);
  (3) Task Execution — the selected model is actually run (a real inference call, not LLM
  simulation) on a local or Hugging Face Inference Endpoint; (4) Response Generation — the
  LLM summarizes all execution results into a final natural-language answer.
- Output channel: execution results are heterogeneous (images, audio, video, bounding boxes,
  text) and are threaded back into subsequent tasks' `args` via a resource-reference symbol
  `<resource>-task_id`, letting a downstream task consume an upstream task's raw output
  (not just its textual description) before the final LLM-authored language summary.
- Planner/executor split: **explicit separation** — the LLM plans and selects models
  (planner) but delegates actual computation to the external expert models (executors); the
  LLM never performs the vision/speech/etc computation itself.
- Error handling: the dependency-gated (`dep`) scheduling means a task only launches once
  its prerequisite tasks finish, but no explicit retry/self-correction/verification loop for
  failed model calls was found in the retrieved abstract/summary — not found beyond the
  dependency-gating mechanism itself.

**Key quantitative results:** Not found — no specific accuracy/benchmark numbers were
retrieved from the abstract; the paper reports qualitative case studies and (per secondary
sources) human/GPT-4-based evaluation of task success, not independently verified here.

**Code/license:** Official repo microsoft/JARVIS is MIT licensed (verified via GitHub API,
LICENSE file present).

**Skill-3D relation:** Cited in §2.2's opening sentence on tool augmentation: "Tool
augmentation extends MLLM by allowing them to invoke external modules through prompting,
structured APIs, or code generation. Representative systems demonstrate that external tools
can compensate for limitations of end-to-end multimodal models Shen et al. (2023); Wu et al.
(2023); Surís et al. (2023)." — HuggingGPT is the "structured APIs"/orchestration exemplar
in that triad, alongside code-generation (ViperGPT) and prompting-based tool use.

**WeftOS relevance: ADOPT (as architectural pattern).** The planner/executor split with
typed, dependency-gated JSON task records and a resource-reference mechanism for passing
non-text outputs between tool calls maps directly onto how WeftOS should structure MCP tool
orchestration for Claude Code/Grok/Codex — worth mirroring the dep-graph + resource-handle
idea explicitly rather than re-deriving it; all outputs here are model artifacts (images,
audio) with no metric-geometry claims, so it's orthogonal to "honest geometry" but a strong
fit for MCP tool-chaining design.
