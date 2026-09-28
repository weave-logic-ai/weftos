# Skill-3D research for WeftOS spatial intelligence

Research date: 2026-09-28. Dashboard ticket: `67c4b93b`.

Skill-3D (arXiv 2606.07436; code at [weave-logic-ai/Skill-3D](https://github.com/weave-logic-ai/Skill-3D),
a mirror of `skill-3d/Skill-3D`, Apache-2.0) is an agent for 3D spatial questions. A multimodal model
calls perception and geometry tools, keeps a memory of past rollouts, and builds a skill library:
successful rollouts become workflows, and failures become lessons. The paper then distills those
rollouts into small Qwen3-VL models with SFT and GRPO.

We plan to rewrite it in Rust inside WeftOS. Urth regions become its scene memory, MentraOS glasses
are a capture source, and its tools reach Claude Code, Grok and Codex through the WeftOS MCP server.

## Documents

| File | What it holds |
|---|---|
| [weftos-adaptation-plan.md](weftos-adaptation-plan.md) | The Rust rewrite plan: crate placement, typed tool layer, phased tasks, agent-host surface, MentraOS capture, risks, and five open decisions |
| [code-review.md](code-review.md) | The code, tool contracts, training, licensing, and the §10 rewrite inventory with behaviors to preserve exactly |
| [papers/paper-skill-3d.md](papers/paper-skill-3d.md) | A deep read of the paper: method, reward, results tables, where the paper and code disagree, and limitations |
| [papers/skills-memory.md](papers/skills-memory.md) | Synthesis of 18 cited works on skill libraries and agent memory |
| [papers/spatial-vlm-3d-embodied.md](papers/spatial-vlm-3d-embodied.md) | Synthesis of 27 cited works on spatial VLMs, 3D-scene LLMs, and embodied models |
| [papers/tool-agents.md](papers/tool-agents.md) | Synthesis of 29 cited works on tool-using and visual-programming agents |
| [papers/benchmarks-and-rl.md](papers/benchmarks-and-rl.md) | Synthesis of 15 cited benchmarks and RL methods |
| [papers/experts-and-frontier-models.md](papers/experts-and-frontier-models.md) | Synthesis of the 6 expert models and 5 frontier models, with the Rust inference and license matrix |
| `papers/analysis/NNN-*.md` | One file for each of the paper's 100 references, each with a verdict of ADOPT, PATTERN, WATCH or SKIP |
| `papers/paper-2606.07436.txt`, `papers/refs.tsv` | The extracted paper text and the numbered reference list (for search, not quotation) |

The paper text is CC BY-NC-SA. Cite and paraphrase it; do not copy its text or figures into
published docs. The repository code is Apache-2.0.

## Conclusions

- **Skill-3D stores no geometry.** Its "scene signature" is a question-type taxonomy. WeftOS supplies
  the missing half: a geometric key (the BVH region) and a separate semantic HNSW index over question
  and evidence text.
- **The learning loop is the valuable part.** Extract, retrieve and inject skills; keep failures as
  lessons. It maps onto a ReasoningBank-style store with Memory-R1's add, update, delete and no-op
  operations, and skill trust goes through the governance gate.
- **Honest geometry must be enforced in the types.** Metric distance, relative depth and pixel boxes
  are separate types that cannot convert into each other. Only Depth Anything 3 metric-large makes a
  metric claim, and only with known camera intrinsics. The released code labels relative depth as
  "Metric". That is a bug to fix, not to port.
- **Licensing:** Pi3 weights are CC BY-NC (research only). SAM 3.1 is under Meta's custom license.
  Orient-Anything v2's license is unresolved. Depth Anything 3's license differs by checkpoint.
  Models are served by `~/llm`: SAM 3.1 is the segmenter (a `bin/` runner over mlx-vlm's
  `Sam3Predictor`), Grounding DINO is an unbuilt catalog fallback, and metric depth is `bin/depth`
  (DA3METRIC-LARGE, metric only when a focal length or FOV is supplied). See plan §9.
- **Evaluate with ReVSI and MMSI-Bench, not raw VSI-Bench.** BLINK is non-commercial. The paper's
  splits are disjoint by question, not by scene, so our held-out set must be scene-disjoint. No
  benchmark covers glasses capture, so we build one.
- **The first Rust milestone needs no GPU.** It runs the agent loop and skill library against recorded
  tool outputs. A compat mode reproduces the prompts, tags and repair rules exactly and must score
  within ±3 points of Python. Fixtures recorded from the Python code are the specification, because
  its largest module has no tests.

## Open decisions

The adaptation plan's section 8 lists five open decisions: the inference runtime, the Pi3 and SAM 3.1
policy, the glasses transport, where the host wrappers are written, and the timing of monocular
promotion and distillation.

## Related work

- [Spatial intelligence 2026 survey](../spatial-intelligence-2026/README.md) covers the reconstruction
  layer (VGGT, DUSt3R, scene graphs) that sits beneath this work.
- `docs/research/agent-skills-design/` holds the SciVisAgentSkills paper (2606.05525) on how authored
  skills are designed and evaluated.
- `docs/research/agent-directory/` holds the dashboard agent directory, teams and review/approval design.
