# Ref 50 — Roy et al. (2025), "ByDeWay"

**Full citation:** R. Roy, D. Das, A. Banerjee, A. Bhattacharjee, K. Dasgupta, and S. Tripathi.
"ByDeWay: Boost Your multimodal LLM with DEpth prompting in a training-free Way." In
Proceedings of the IEEE/CVF International Conference on Computer Vision (ICCV Workshops),
pp. 6058–6064, 2025.

**arXiv / venue:** arXiv:2507.08679 (https://arxiv.org/abs/2507.08679); presented at the
ICCV 2025 Workshops (CVAM workshop, openaccess.thecvf.com/ICCV2025W/CVAM).

**Summary:** ByDeWay is a training-free prompting technique for boosting spatial reasoning
and reducing hallucination in existing multimodal LLMs, with no parameter updates. It
introduces Layered-Depth-Based Prompting (LDP): a monocular depth estimator segments the
scene into closest / mid-range / farthest depth layers, a grounded vision-language model
generates region-specific captions for each layer, and these depth-structured captions are
appended as extra text context to the original image-question prompt fed to the (frozen)
MLLM.

**Method specifics:**
- Tool API shape: not a code-execution or JSON tool-call scheme — it's a fixed preprocessing
  pipeline (depth estimator → region captioner) whose output is injected as natural-language
  text into the prompt. No agentic tool selection by the MLLM itself.
- Output channel: text only (structured depth-layer captions), not raw depth maps or images.
- Planner/executor split: none — a single frozen MLLM consumes the augmented prompt; the
  depth/caption pipeline is a fixed, non-agentic front end, not something the model invokes.
- Error handling: not found — no described retry/self-correction; it is a one-shot
  prompt-augmentation method.

**Key quantitative results:** Evaluated on POPE (hallucination) and GQA (compositional
reasoning); abstract states "consistent improvements across multiple MLLMs" but specific
accuracy deltas were not found in the accessible abstract/summary content.

**Code/license:** No official code repository was found for the paper's authors (a
same-named third-party repo exists on GitHub but is unaffiliated and unverified). Not found.

**Skill-3D relation:** Cited in §2.1's related-work sentence: "Recent methods improve
fine-grained spatial understanding by incorporating 3D reconstruction, depth cues, spatial
VQA data, and explicit grounding ... Roy et al. (2025) ..." — grouped as a depth-cue-based
spatial-reasoning method, contrasted with Skill-3D's own scene-aware skill-retrieval + tool
invocation approach.

**WeftOS relevance: SKIP.** LDP's depth layers are relative/qualitative (monocular depth,
per-patch median ordering, no metric units) and are baked into a fixed text-prompt
preprocessing step rather than an agent-invokable tool — it offers no reusable tool-call
pattern and its "depth" outputs are exactly the kind of appearance-based pseudo-geometry the
"honest geometry" principle guards against if ever mistaken for metric scale.
