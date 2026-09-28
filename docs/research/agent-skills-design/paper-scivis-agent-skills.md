# SciVisAgentSkills: Design and Evaluation of Agent Skills for Scientific Data Analysis and Visualization — Deep Read

Source: arXiv:2606.05525v1 [cs.HC/cs.AI] (Ai, Miao, Tang, Liu, Wang — Univ. Notre Dame /
Lawrence Livermore National Laboratory). Full text at
`docs/research/agent-skills-design/paper-2606.05525.txt`; reference list at
`docs/research/agent-skills-design/refs.tsv` (42 rows). Code/skills repo:
`https://github.com/KuangshiAi/SciVisAgentSkills` — 4 skill packages (ParaView, napari,
VMD/MDAnalysis, TTK). **License note: the repo ships no `LICENSE` file and GitHub's API
reports `license: null`** — publicly viewable and forkable under GitHub ToS, but no
explicit reuse grant. This is a real gap worth flagging against Episteme's own source
repo (`k-dense-ai/scientific-agent-skills`, MIT — see `docs/research/episteme/inventory.md`).

## 1. What an "agent skill" is in this paper

The paper adopts the Anthropic Agent Skills format verbatim and cites it as prior art
rather than redefining it — its own contribution is a *design recipe* and an *evaluation*,
not a new skill schema.

**Definition quoted (§1):** "An agent skill is a structured package of instructions,
code templates, references, and verification logic that augments agent behavior at
inference time without changing model parameters [8]" — citing Anthropic's "Equipping
agents for the real world with agent skills" post (ref [8], `refs.tsv` row 8).

**Format (§3, "Skill format and construction"):** "All four skills follow the agent
skill format [8]: YAML frontmatter for discovery, followed by Markdown guidance that
includes usage rules, script templates, API summaries, and troubleshooting notes. The
ParaView skill additionally includes separate reference files because its API surface is
larger. In the context of coding agents, this structure supports **progressive
disclosure**, where the skill metadata is checked first, and the body and references are
loaded only when relevant."

Confirmed against the actual repo (`paraview-viz/SKILL.md`, fetched via `gh api`):

```yaml
---
name: paraview
description: >
  ParaView scientific visualization for volume data and meshes. Use this skill when Claude needs to:
  (1) Visualize 3D volume data (CT, MRI, scientific simulations), (2) Create isosurfaces, slices, volume renderings,
  (3) Visualize vector fields with streamlines/glyphs, (4) Generate publication-quality screenshots,
  (5) Work with VTK, EXODUS, RAW, or other scientific data formats
---
```

followed by a `> **API Documentation Version: 5.12.1**` callout, a numbered `## Rules`
block (e.g. "Never open a GUI — always use `pvpython` for headless batch execution"),
a "Workflow Decision Tree," and canonical script templates. `paraview-viz/` also ships
`references/` and `assets/` directories; the other three skills are single-file.

**Operational definition (§3):** "We therefore define an agent skill operationally as a
**self-contained, version-pinned procedural module** for one SciVis tool, constructed
through a unified process that combines **environment specification, documentation
alignment, executable exemplars, and failure-aware refinements**." Restated in §5 as
"fix the tool version, distill official documentation, reuse SciVis agent exemplars, and
encode empirical fixes" — generalized as "a form of **procedural knowledge distillation
for scientific software**."

**Explicit design guideline — benchmark-leakage avoidance (§3):** "To avoid benchmark
leakage, the skills contain only tool-general procedural knowledge, including
environment assumptions, documented API patterns, and reusable examples. They do not
include benchmark-specific solutions, expected outputs, hidden labels, or evaluation
rubrics." A hard authoring constraint, not a suggestion — worth lifting directly into
any WeftOS skill-authoring or curation gate (§6).

**Authoring process:** manually authored by visualization researchers, "refined through
several rounds of observing agents on representative SciVis workflows," targeting
"recurring tool-use failures, such as incorrect headless rendering and API misuse."
Exemplars were adapted from four existing SciVis agent codebases — ParaView-MCP [25],
BioImage-Agent [31], GMX-VMD-MCP [16], TopoPilot [19] — and reviewed/tested "for
correctness and safe headless execution in sandboxes" before evaluation.

## 2. Design and evaluation methodology

**Motivating failure modes (§3):** even in well-configured environments, general-purpose
coding agents (a) "spend multiple turns probing libraries and execution settings"
(exploration overhead), (b) "misuse APIs or follow incorrect usage patterns" from
incomplete grounding in tool-specific docs, and (c) make output errors, e.g. "capturing
the entire napari GUI rather than the visualization viewport." All three motivate the
skill content (fixed versions, distilled docs, empirical output-capture fixes).

**Benchmark:** SciVisAgentBench [3] (arXiv:2603.29139, same first author's prior work) —
**108 expert-crafted, multi-step tasks** across **five suites**: ParaView Visualization,
Molecular Visualization (VMD), Bioimage Visualization (napari), Topology Visualization
(TTK), and Object Identification (build a visualization from anonymized volumetric data,
infer the object category — hidden labels + case-specific rubrics). Pipeline combines
multimodal LLM judges, image metrics, code validators, rule-based checks, case-specific
evaluators.

**Host agents** — a 2×2 agent×skill grid, not a three-way "no skill/skill/other": Claude
Code+Sonnet-4.5 and Codex+GPT-5.2, each **with vs. without** the matching skill, **3
trials** per cell, judged primarily by **Claude-Opus-4.6** ("shown to align well with
human SciVis expert assessments [3]").

**Metrics:** Overall Score and Completion Rate (Table 1); pass@{1,2,3}/pass^{1,2,3}
(success in ≥1 vs. *all* of k trials, Figure 2); scaled PSNR/SSIM/LPIPS for ParaView
image outputs only (Table 2); input/output token counts, cached tokens folded into input
(Table 3).

### Table 1 — Overall Score, w/o → w/ skills (Claude-Opus-4.6 judge, mean±std, n=3)

| Suite | Claude-Code+Sonnet-4.5 | Codex+GPT-5.2 |
|---|---|---|
| ParaView Visualization | 62.57±0.51 → **73.93±2.56** | 60.17±1.43 → **66.67±1.10** |
| Molecular Viz (VMD) | 61.47±6.78 → 64.33±4.88 | 62.30±6.32 → **73.13±3.25** |
| Bioimage Viz (napari) | 52.83±9.80 → **58.97±4.56** | 41.90±4.69 → **55.00±2.59** |
| Topology Viz (TTK) | 45.23±8.81 → **73.63±3.85** (≈+60% rel., largest gain) | 76.43±10.06 → **83.73±4.75** |
| Object Identification | 41.50±3.55 → **69.13±1.56** | 43.33±5.28 → 47.77±0.25 |

Score rises in all 10 cells. Completion Rate (same table, not reproduced) mostly tracks
score but **diverges once**: Codex object-identification completion rate *drops*
92.59±9.80 → 80.25±5.66 even as its score rises — the paper's example of skills
"improv[ing] output quality while still introducing additional execution paths that
sometimes fail" (§4).

### Table 2 — Image-quality metrics, ParaView only (scaled aggregates, n=3)

| Setting | PSNR↑ | SSIM↑ | LPIPS↓ |
|---|---|---|---|
| Claude-Code w/o skills | 20.99±0.68 | 0.92±0.02 | 0.10±0.02 |
| Claude-Code w/ skills | 22.08±0.73 | 0.91±0.04 | 0.10±0.03 |
| Codex w/o skills | 21.27±1.02 | 0.92±0.02 | 0.10±0.03 |
| Codex w/ skills | 21.76±1.17 | 0.93±0.02 | 0.09±0.02 |

Small, mixed movement — image fidelity is not where skills' main effect shows up.

### Table 3 — Token usage, w/o → w/ skills (mean±std, n=3; K=thousand, M=million)

| Suite | Setting | Input Tokens ↓ | Output Tokens ↓ |
|---|---|---|---|
| ParaView Viz | Claude-Code | 39.49M±6.62M → 40.26M±4.37M | 425.32K±55.52K → **101.04K±1.01K** |
| | Codex | 45.57M±9.47M → 46.86M±3.78M | 396.60K±23.27K → 329.25K±60.90K |
| Topology Viz | Claude-Code | 17.26M±1.87M → **6.18M±1.59M** | 172.37K±18.51K → 118.77K±30.97K |
| | Codex | 46.04M±4.48M → 19.42M±3.56M | 193.58K±27.62K → 112.51K±12.08K |
| Molecular Viz | Claude-Code | 5.07M±0.12M → 7.64M±0.87M (↑) | 81.73K±3.45K → 33.96K±1.90K |
| | Codex | 8.63M±1.81M → 11.95M±0.68M (↑) | 112.28K±17.22K → 134.76K±8.83K (↑) |

**Finding (§4):** for Claude Code, output tokens decrease consistently across every
suite; input tokens decrease for bioimage/topology/object-identification but *increase*
for molecular visualization. Codex is mixed, with increases on bioimage and molecular.
"Because the same skill content can reduce tokens in Claude Code but increase them in
Codex, we interpret token cost as a property of the interaction among skill content,
model behavior, and harness-level context management rather than as a property of skill
verbosity alone… we do not observe a clear correlation between token usage and
performance." The paper's most transferable methodological point: **never report token
cost as an attribute of the skill alone — it is a skill×host interaction term.**

## 3. What makes a skill effective or harmful

**Granularity/length:** not directly ablated here (only 4 skills, one per tool, no
smaller/larger variants tested), but related work is cited for it: SkillsBench [23]
found "curated skills improve performance, while self-generated skills are often
unreliable" (§2), and a data-driven analysis [24] found "existing skill ecosystems
remain concentrated in software engineering" (§2). The paper's own implicit choice —
one skill per *tool*, not per *task* or per *API call* — ties skill boundaries to the
external system being wrapped (its own version, docs, failure modes), not to the
benchmark's task taxonomy.

**Examples/exemplars:** explicitly load-bearing. Every skill reuses "representative code
snippets and function usage patterns from existing SciVis agents" (§3) rather than
authoring examples from scratch, to "reduce trial-and-error during task execution."

**Tool scripts vs. instructions:** these are instruction+template packages (YAML +
Markdown + code templates the agent edits and runs itself via `pvpython`/Python), not
black-box executable tools — a real format difference from Episteme's `scripts/`-
shipping skills (106/166 execute code directly, `docs/research/episteme/inventory.md`).
This paper only validates the "template to adapt" style, not the "script to invoke"
style.

**Composition of multiple skills:** not evaluated — each of the five task suites maps to
exactly one skill (§3), so multi-skill composition/conflict is never tested.
AgentSkillOS's DAG-orchestration finding (`skills-memory.md`, ref 29) is the nearer
source for that question.

**Failure modes named:** exploration overhead, API misuse from incomplete grounding, and
wrong-content output capture (§3) — the *targets* the skills are designed against. The
paper's own residual failure, observed post-hoc, is the **score/completion-rate
divergence** for Codex object identification: **a skill can make an agent take a better
but less-robust path.**

**When skills don't help:** "gains may be limited when the base model already handles
well-documented tools effectively" — Claude Code's modest VMD gain is attributed to VMD
being "mature and widely documented" (§5). General rule: **"agent skills are most
valuable when procedural knowledge is specialized, fragmented across sources, or poorly
represented in the model's training data."**

## 4. The evaluation methodology as a reusable recipe

1. **Fix the comparison unit.** One skill = one external tool/system, matched 1:1 to a
   task suite exercising exactly that tool; skill content held to "tool-general
   procedural knowledge" only, excluding anything that would leak into the rubric (§3).
2. **Run a full with/without × host grid**, not with/without on one host alone — this
   paper's 2×2 (Claude Code/Codex × w/o/w/) is what surfaces the host-dependent
   token-cost divergence; a single-host study would have missed it.
3. **Repeat trials (n=3), report mean±std**, plus pass@k/pass^k — separates "skill
   raises the ceiling" from "skill raises the floor."
4. **Judge with a model validated against human experts** (Claude-Opus-4.6, validated in
   [3]), paired with objective image metrics (Table 2) and completion rate as a
   deterministic floor, so judge noise doesn't dominate.
5. **Report token cost as its own axis, per suite and per host** — never aggregate to
   one number, never assume it correlates with score.
6. **Separate "did it finish" from "was it good"** — Completion Rate vs. Overall Score;
   the Codex object-identification divergence is only visible because both are tracked.

Directly reusable for evaluating any WeftOS skill or agent: fix a task suite per skill,
run with/without on every host it ships to, repeat 3+ trials, judge against a validated
human baseline plus deterministic checks, and report token/latency cost per host.

## 5. Relationship to Skill-3D (arXiv 2606.07436)

Skill-3D deep-read at `docs/research/skill-3d/papers/paper-skill-3d.md`; cluster
synthesis at `docs/research/skill-3d/papers/skills-memory.md`.

**Agreement:**
- Both treat "agent skill" as *structured procedural knowledge injected at inference
  time, separate from model weights* — SciVisAgentSkills quoting Anthropic's definition
  directly [8]; Skill-3D's schema (trigger, scene context, evidence type, historical
  metadata — Skill-3D §3.2) is a superset of the same idea.
- Both diagnose the *same class* of root failure — one strategy applied uniformly
  regardless of context. Skill-3D calls it "biased tool preference"/"evidence mismatch"
  from scene heterogeneity (§1, §4.3); SciVisAgentSkills calls it exploration overhead/
  API misuse/wrong-viewport capture from tool heterogeneity (§3). Both fix it by
  encoding an explicit trigger condition plus a tool-usage/ordering strategy.
- Both find **not all tasks benefit equally** — Skill-3D's Effective Tool Usage metric
  (Eq. 3, 39.2%→78% headline) is task/scene-conditional; SciVisAgentSkills finds the
  same by tool maturity (VMD gains less than topology/object-ID).

**Difference — authored vs. evolved:**
- SciVisAgentSkills' four skills are **static, human-authored, hand-reviewed** — no
  promotion/merge/update mechanism once shipped. Skill-3D's library is
  **self-evolving**: every rollout updates it, successes promoted/merged into *dynamic*
  skills only when they add coverage, failures diagnosed into typed errors and attached
  as "lessons," gated by a "Skill Manager" (§3.1), then folded into SFT+GRPO training of
  a compact model (§3.3) — SciVisAgentSkills does none of that (both conditions use the
  same frozen Sonnet-4.5/GPT-5.2).
- SciVisAgentSkills' benchmark-leakage discipline (§3) has **no analog** in Skill-3D —
  its rollout-driven promotion has no stated guard against a dynamic skill accidentally
  encoding an eval-set-specific shortcut, a blind spot `skills-memory.md`'s cluster
  synthesis (Best-supported idea #4) already flags across the whole "evolved skill"
  literature.
- Skill-3D's "scene" key is, per the prior deep read, actually a **flat 8-pattern
  question taxonomy** (`skill3d/core/skill_learning.py`), narrower than
  SciVisAgentSkills' per-*tool* granularity (a real external system boundary).
- Neither tests multi-skill composition, for different reasons: Skill-3D retrieves
  top-k=6 skills per query and lets the planner combine them (§3.2); SciVisAgentSkills'
  1:1 skill↔suite design structurally avoids the question.

**Net read:** SciVisAgentSkills is the stronger source for *authoring and validating a
static skill package*; Skill-3D is the stronger source for *evolving a skill library
under continued use*. Complementary, not competing — see §6(c) for using both.

## 6. WeftOS implications

### (a) Episteme — STEM skill pack from `k-dense-ai/scientific-agent-skills`

Episteme's 166 MIT-licensed skills (`docs/research/episteme/inventory.md`) are mostly
Python-library wrappers (106/166 ship `scripts/`) across bioinformatics, ML, and
scientific communication — broader and shallower than SciVisAgentSkills' four
hand-tuned tool skills. Rules to carry over:

1. **Expect ROI to concentrate on niche, poorly-documented libraries, not mainstream
   ones.** Skills help least where "the base model already handles well-documented
   tools effectively" (§5) — predicts larger with/without deltas for `pkpd-modeling` or
   `relsa-severity-assessment` than for `matplotlib` or `scikit-learn`. Prioritize
   evaluation budget accordingly.
2. **Adopt the benchmark-leakage rule verbatim** (§3) as a hard constraint on any
   Episteme skill used inside an evaluation harness: no skill may encode a specific
   graded task's expected output.
3. **Version-pin every wrapped tool/library in the skill body itself** (the
   `paraview-viz/SKILL.md` "API Documentation Version: 5.12.1" callout pattern), not
   only in `tests/skill-requirements.toml` — the pin should be visible without opening a
   second file.
4. **Track token cost per skill×host before shipping, not just accuracy.** A skill that
   reduces Claude Code's output tokens can still inflate Codex's — budget per-host
   telemetry, not one number.

### (b) Agent Directory — dashboard maintaining agents/teams as skill+hook+helper packages

1. **Package manifest minimum fields**, grounded in the format this paper confirms in
   production use: `{name, trigger description (YAML frontmatter), wrapped-tool version
   pin, host compatibility list, references/ file manifest, license}`. License is not
   optional — SciVisAgentSkills itself ships without one despite backing a peer-reviewed
   evaluation; treat that as a cautionary example, not a template.
2. **Evaluation gate before a skill enters a reviewed change set**: run the §4 recipe —
   paired with/without on a small task suite, on *every host* the package targets, n≥3
   trials, tracking Overall Score, Completion Rate, and token cost separately. A skill
   that clears Claude Code but regresses Codex's token cost (Table 3) ships with a
   host-scoped warning, not a silent pass/fail.
3. **Per-host format adapters are a first-class concern.** Per the repo's own README:
   Claude Code auto-invokes a skill from its YAML `description` (symlinked into
   `~/.claude/skills/`); **Codex has no native skill loader** — the workaround is
   copying `SKILL.md` into a project `skills/` dir and adding an explicit pointer line
   per skill into `AGENTS.md`. Any Codex-hosted change set needs that same
   flatten-into-AGENTS.md step generated automatically, not improvised by the applying
   agent.
4. **Security/trust tiering on import**: apply the same 4-tier gate from the Xu & Yan
   agent-skills-security survey (ref [41] here; also cited in the Skill-3D cluster,
   "26.1% of community skills vulnerable") to every imported pack, Episteme included.

### (c) Skill-3D Rust rewrite's skill library

1. **Split the library into two authoring regimes, explicitly.** Well-scoped external
   tools with stable docs (a Rust crate, a CLI, a fixed WeftOS subsystem interface) get
   SciVisAgentSkills-style **static, version-pinned, human-reviewed** skills via its
   four-step recipe (fix version → distill docs → reuse exemplars → encode empirical
   fixes), gated by the same benchmark-leakage discipline. Behaviors hard to specify
   statically (tool selection under ambiguous/heterogeneous context — Skill-3D's own
   problem domain) stay in Skill-3D's **evolved, promote/merge/patch** regime, not
   hand-authored.
2. **Import the benchmark-leakage guard into the evolved regime**, where Skill-3D has no
   equivalent — an auto-promoted dynamic skill must not encode a held-out eval case's
   specific answer; this is the one rule SciVisAgentSkills has that Skill-3D's pipeline
   structurally lacks.
3. **Track token/latency cost per skill×host as a standing metric**, alongside
   Skill-3D's own 0.5s/query retrieval overhead (Appendix B.1) — since the Rust library
   will serve multiple downstream hosts and Table 3 shows that cost is not portable
   across hosts for the *same* skill content.
4. **Use the full with/without × host grid (§4) as the CI acceptance test**, not an ad
   hoc benchmark run — more rigorous than Skill-3D's own pooled-benchmark evaluation for
   a library that must keep passing regression checks as both the library and the
   underlying models change.

## Files written

- `docs/research/agent-skills-design/paper-2606.05525.txt` — full extracted text (title
  through references, 850 lines)
- `docs/research/agent-skills-design/refs.tsv` — 42 references (number, citation, link,
  cited-by section)
- `docs/research/agent-skills-design/paper-scivis-agent-skills.md` — this analysis
