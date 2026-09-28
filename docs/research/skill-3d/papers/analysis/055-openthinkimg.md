# Ref 55 — Su et al. (2025), "OpenThinkIMG"

**Full citation:** Z. Su, L. Li, M. Song, Y. Hao, Z. Yang, J. Zhang, G. Chen, J. Gu, J. Li,
X. Qu, et al. "OpenThinkIMG: Learning to Think with Images via Visual Tool Reinforcement
Learning." arXiv preprint arXiv:2505.08617, 2025.

**arXiv / venue:** arXiv:2505.08617 (https://arxiv.org/abs/2505.08617). Code:
github.com/zhaochen0110/OpenThinkIMG.

**Summary:** OpenThinkIMG is presented as the first open-source, end-to-end framework and
infrastructure for training tool-augmented large vision-language models (LVLMs) to interact
with external vision tools adaptively, rather than following fixed tool-use scripts. It
provides standardized vision-tool interfaces, scalable trajectory generation for SFT-style
policy initialization, and a training environment for its RL algorithm, V-ToolRL, which
trains an LVLM policy to invoke vision tools by optimizing directly for downstream task
success from tool-interaction feedback.

**Method specifics:**
- Tool API shape: a **standardized vision-tool interface** invoked by the LVLM policy (tool
  calls, not free-form generated code) — the framework's contribution is precisely
  standardizing this interface across heterogeneous tools so RL training environments are
  interchangeable.
- Output channel: not fully specified in retrieved material, but tool results are fed back
  into the policy's context to continue multi-step "thinking with images" (the framework
  supports iterative image-tool interaction rather than a single tool call).
- Planner/executor split: a single trained LVLM policy both decides which tool to call and
  interprets results — no separate planner/executor model; the split instead is
  SFT-initialized policy vs. RL-refined policy (V-ToolRL) as training stages.
- Error handling: not found in retrieved material — the RL framework optimizes for task
  success via reward signal rather than an explicit code-repair/retry mechanism.

**Key quantitative results:** On chart-reasoning tasks with a Qwen2-VL-2B backbone, the
V-ToolRL-trained agent outperforms its SFT-initialized counterpart by +28.83 points, beats
supervised tool-learning baselines Taco and CogCom by an average of +12.7 points, and
surpasses GPT-4.1 by +8.68 accuracy points (benchmark suite: chart-reasoning tasks, per
paper's own reporting — exact benchmark name(s)/table not independently verified beyond
this summary).

**Code/license:** Code is publicly available at github.com/zhaochen0110/OpenThinkIMG; no
LICENSE file was found via the GitHub API (license field returned null) — license not found.

**Skill-3D relation:** Cited in §2.2 among "Recent tool-augmented VLM agents ... developed
for long-video understanding, high-resolution image analysis, medical diagnosis, and general
visual reasoning ... Su et al. (2025)" — grouped as a tool-augmented VLM agent exemplar
alongside a complementary line of work (same paragraph) that "trains VLMs to use tools
through supervised fine-tuning or reinforcement learning."

**WeftOS relevance: WATCH.** The RL-trained adaptive-tool-invocation approach and its
standardized tool-interface abstraction are relevant design references for training
tool-calling policies, but the framework targets chart/2D-image reasoning, not 3D/metric
geometry, and no license was confirmed — watch for a licensed release and for any extension
to spatial/3D tool use before considering adoption.
