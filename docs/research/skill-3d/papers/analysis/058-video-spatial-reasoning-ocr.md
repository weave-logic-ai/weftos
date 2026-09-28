# Ref 58 — Tang et al. (2025a), "Video Spatial Reasoning with Object-Centric 3D Rollout"

**Full citation:** H. Tang, M. Cao, R. Liu, X. Liang, L. Li, G. Li, and X. Liang. "Video
Spatial Reasoning with Object-Centric 3D Rollout." arXiv preprint arXiv:2511.13190, 2025.

**arXiv / venue:** arXiv:2511.13190 (https://arxiv.org/abs/2511.13190). Per search results,
subsequently appeared in AAAI proceedings (ojs.aaai.org/index.php/AAAI/article/view/37899),
2026 — venue/date not independently confirmed beyond the search snippet.

**Summary:** The paper targets "query-locked reasoning" in video spatial-reasoning models —
the tendency of trained models to fixate only on objects explicitly named in a question
while ignoring the rest of the scene, hurting holistic spatial understanding. It proposes
Object-Centric 3D Rollout (OCR): a training-time augmentation that introduces structured
perturbations to the 3D geometry of selected objects, degrading their object-specific visual
cues and re-projecting the perturbed 3D geometry back into 2D, then trains on a mixture of
vanilla and this "region-noisy" rolled-out video data so the model is forced to use
whole-scene context rather than shortcut on the queried object alone.

**Method specifics:**
- Tool API shape: not a tool-calling or code-execution framework at inference time — OCR is
  a **training-data augmentation / regularization strategy** (a rollout-based training
  pipeline), not an agentic tool the model invokes.
- Output channel: not applicable in the tool-call sense; the "3D geometry → perturb → project
  to 2D" step is an offline/training-time data transform, and the model's own outputs are
  presumably standard VSI-Bench-style spatial QA answers (text) — not found beyond this.
- Planner/executor split: not applicable — a single video-LLM is trained (via the
  rollout-augmented data) to answer spatial questions directly; no separate planner/executor
  or external tool module described.
- Error handling: not found / not applicable — this is a training-time regularization method,
  not an inference-time agent loop, so no retry/self-correction/verification mechanism is
  expected or described.

**Key quantitative results:** 47.5% accuracy on VSI-Bench with a 3B-parameter model,
reported as outperforming several larger (7B) baseline models; ablations show OCR
outperforming prior perturbation/regularization approaches T-GRPO and NoisyRollout (exact
comparison numbers not found in retrieved material).

**Code/license:** Not found — no code repository or license information was located in the
retrieved material.

**Skill-3D relation:** Cited in §2.1 among methods that "enhance spatial reasoning through
prompting, mental simulation, visual chain-of-thought, reinforcement learning, code-driven 3D
reasoning, and generative imagination of 3D space Taguchi et al. (2025); Marsili et al.
(2025); Tang et al. (2025a); ..." — grouped as a representative "code-driven 3D reasoning" /
training-time 3D-augmentation approach to spatial reasoning, contrasted with Skill-3D's
inference-time skill-retrieval-and-tool-invocation strategy.

**WeftOS relevance: WATCH.** The underlying geometry here is **not metric** — the "3D
rollout" perturbs and re-projects object geometry purely to regularize training and force
holistic attention, with no claim to real-world scale; it's an interesting training-signal
idea for making a model attend to full-scene context (relevant background for egocentric
video spatial QA), but as a training recipe rather than a runtime tool it has no direct MCP
tool-call analog for WeftOS, and no code was found to build on.
