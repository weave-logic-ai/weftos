# Ref 51 — Shao et al. (2024a), "Visual CoT"

**Full citation:** H. Shao, S. Qian, H. Xiao, G. Song, Z. Zong, L. Wang, Y. Liu, and H. Li.
"Visual CoT: Advancing Multi-Modal Language Models with a Comprehensive Dataset and
Benchmark for Chain-of-Thought Reasoning." Advances in Neural Information Processing
Systems (NeurIPS) 37, pp. 8612–8642, 2024. (Datasets and Benchmarks Track.)

**arXiv / venue:** arXiv:2403.16999 (https://arxiv.org/abs/2403.16999); NeurIPS 2024
Datasets and Benchmarks Track.

**Summary:** Visual CoT targets the interpretability gap in MLLMs on complex visual inputs.
It contributes a 438K-pair question-answering dataset annotated with intermediate bounding
boxes marking the image region needed to answer each question, of which ~98K pairs include
full chain-of-thought reasoning steps. It also proposes a benchmark and a multi-turn
processing pipeline in which the model dynamically attends to a localized image region
(the bounding box) before answering, producing an interpretable "look here, then reason"
trace.

**Method specifics:**
- Tool API shape: not a code/program-execution or JSON tool-calling framework — it is a
  dataset + a multi-turn model architecture/training recipe that outputs a bounding box as
  an intermediate reasoning step, then re-attends to the cropped region.
- Output channel: the bounding-box coordinates and the re-attended image crop are consumed
  internally by the same model (not routed through an external tool or separate module).
- Planner/executor split: single model performs both the "where to look" localization step
  and the final answer generation — no separate planner/executor or external tool caller.
- Error handling: not found — no retry/self-correction loop is described; this is a
  supervised-training pipeline over annotated CoT+bbox data, not an inference-time agent.

**Key quantitative results:** Not found — specific accuracy numbers were not visible in the
abstract/summary content retrieved; the paper reports results across multiple benchmark
splits in the full text (not independently verified here).

**Code/license:** Dataset, benchmark, and pretrained models are stated to be released on the
project page; exact license terms not found.

**Skill-3D relation:** Cited in §2.1 among methods that "improve fine-grained spatial
understanding by incorporating 3D reconstruction, depth cues, spatial VQA data, and explicit
grounding," and also listed among "stronger backbones" driving growing spatial-reasoning
capability in MLLMs (§2.1 opening sentence, "Shao et al. (2024a)").

**WeftOS relevance: PATTERN.** The "localize a region, then re-attend for a grounded answer"
two-step pattern is a useful reasoning-trace idea, but its "geometry" (a 2D bounding box) is
purely pixel-space, not metric — worth noting as a design pattern for grounding-before-answer
prompting, not as an adoptable tool or a source of real-world measurement.
