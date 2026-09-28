# Agent-skills research nexus: map onto WeftOS research

Source: [JayLZhou/Awesome-Agent-Skills](https://github.com/JayLZhou/Awesome-Agent-Skills), the
companion repo to *A Comprehensive Survey on Agent Skills: Taxonomy, Techniques, and Applications*
(Zhou et al., arXiv 2605.07358). Fetched 2026-09-28 via `gh api repos/JayLZhou/Awesome-Agent-Skills/readme`;
157 taxonomy-cell entries covering 126 distinct arXiv papers, plus 6 related surveys, 19
benchmarks, 21 ecosystem platforms, and an open research-problems list. Per-paper detail is in
[`nexus-index.tsv`](nexus-index.tsv) (taxonomy section, subsection, title, arXiv id, year, and
which of our own docs already covers it, if any).

This map does not duplicate our existing research. It is written against, and should be read
alongside: [`refs.tsv`](refs.tsv) and [`paper-scivis-agent-skills.md`](paper-scivis-agent-skills.md)
(SciVisAgentSkills), [`higgsfield-format-standard.md`](higgsfield-format-standard.md),
[`image-analysis-skills-survey.md`](image-analysis-skills-survey.md),
[`voltagent-catalog.tsv`](voltagent-catalog.tsv), the Skill-3D corpus at
[`../skill-3d/`](../skill-3d/) (100 refs, `papers/analysis/NNN-*.md`, `skills-memory.md`,
`tool-agents.md`), [`../episteme/`](../episteme/), and [`../agent-directory/`](../agent-directory/).

## (a) The nexus taxonomy, in our words

The survey frames agent skills as closing a "procedural gap": tool access tells an agent *what*
it can call, not *when* to call it, how to sequence and recover, or how to judge success. It
defines a skill as a triple `S = (M, R, C)` — a main instruction document, auxiliary resources,
and applicability conditions — and organizes the literature around a five-stage lifecycle:

```
experience / expertise / corpus / task
        -> acquisition -> representation -> retrieval + selection -> execution
        -> feedback, validation, evolution, governance (loops back)
```

The paper list sections follow that lifecycle, plus a foundations layer underneath it:

| Section | What it covers |
|---|---|
| 0. Foundations | Tool use (Toolformer, ReAct, ToolLLM), protocols (MCP, function calling), retrieval (RAG, DPR, GraphRAG variants), memory (MemGPT, Think-in-Memory) — infrastructure every later section assumes |
| 1. Representation | How `M`/`R`/`C` get packaged: text-based (Reflexion-style reflections, SKILL.md), code-backed (Voyager, programmatic skills), hybrid (JARVIS-1, visual skills) |
| 2. Acquisition | Where skills come from: human-derived, experience-derived (rollout distillation), task-derived (tool synthesis), corpus-derived (mined from logs/docs) |
| 3. Retrieval and selection | Surfacing the right skill from a growing library (dense/sparse/generative retrieval, graph retrieval) and deciding whether to invoke, compose, or adapt it |
| 4. Evolution and governance | Revision/RL-optimization, memory-centric re-entry, and — its own subsection — trust, security, and ecosystem risk |

Full citation for each cell is in `nexus-index.tsv`; do not copy the survey's prose or figures
(the paper is CC BY-NC-SA per the license note already on file in `skill-3d/README.md`).

## (b) Where our research already lands on this taxonomy

We match papers by arXiv id against `skill-3d/papers/refs.tsv` (Skill-3D's 100 references, each
with a `papers/analysis/NNN-*.md` verdict) and `agent-skills-design/refs.tsv` (SciVisAgentSkills'
44 references). **8 of the nexus's 126 arXiv papers are papers we already have deep or shallow
coverage of** — about 6%. They land in every lifecycle stage but one:

| Nexus paper | Taxonomy cell(s) | Our coverage |
|---|---|---|
| SciVisAgentSkills (2606.05525) | 2-Acquisition / Human-Derived | This is literally the subject of `paper-scivis-agent-skills.md` — not a citation, our own deep read |
| SoK: Agentic Skills (2602.20867) | Related Surveys; 2-Acquisition / Human-Derived | `agent-skills-design/refs.tsv` #22, discussed in `paper-scivis-agent-skills.md` §2 |
| AgentSkillOS (2603.02176) | 1-Representation / Hybrid; 2-Acquisition / Human-Derived; 3-Retrieval / Retrieval | `skill-3d/papers/analysis/029-agentskillos.md` — ADOPT verdict, capability-tree + DAG orchestration at 200-200K skill scale |
| SkillNet (2603.04448) | 2-Acquisition / Human-Derived; 3-Retrieval / Retrieval; 4-Evolution / Governance-Trust | `skill-3d/papers/analysis/032-skillnet.md` — PATTERN verdict, 5-axis skill quality rubric |
| MemSkill (2602.02474) | 3-Retrieval / Selection-Routing; 4-Evolution / Memory-Centric | `skill-3d/papers/analysis/090-memskill.md` — PATTERN verdict, controller/executor/designer split |
| SkillRL (2602.08234) | 4-Evolution / Formation-Refinement-RL | `skill-3d/papers/analysis/077-skillrl.md` — PATTERN verdict, closest analog to Skill-3D itself |
| Agent Skills (Claude Skills data-driven analysis, 2602.08004) | 4-Evolution / Governance-Trust-Risk | `agent-skills-design/refs.tsv` #24; its "~26% of community skills vulnerable" finding is already load-bearing in `skill-3d/papers/skills-memory.md` |
| SkillsBench (2602.12670) | Benchmarks | `skill-3d/papers/analysis/030-skillsbench.md` (ADOPT) and `agent-skills-design/refs.tsv` #23; its paired with/without-skill methodology already underlies our SciVis eval read |

**Empty cells worth naming honestly:**

- **0-Foundations is entirely uncovered by direct citation match**, though we depend on several of
  its ideas operationally (MCP is WeftOS's actual tool transport; ReAct-style loops underlie the
  agent harness). We have not separately reviewed Toolformer/ReAct/RAG/MemGPT as papers — treat
  them as assumed background, not reviewed literature.
- **1-Representation is nearly empty**: only the Hybrid subsection has a hit (AgentSkillOS). The
  Text-Based subsection (SKILL.md conventions, Ctx2Skill, the "skill smells" empirical study) and
  Code-Backed subsection (Voyager, SkillCraft, PolySkill, Skill-as-Pseudocode) are untouched —
  directly relevant to the Agent Directory's package-format decision and not yet reviewed by us.
- **3-Retrieval's "Selection and Routing" subsection** has one hit (MemSkill) out of ten papers;
  SkillRouter, GraSP, SkillDAG, Maestro, GraphSkill are all new to us and all bear directly on
  Episteme's router (see §c).
- **4-Evolution's "Governance, Trust, and Ecosystem Risk" subsection** is our best-covered corner
  (3 of 17 papers) but still mostly open — the newest 2606-2607 security papers (SkillGuard, When
  Safe Skills Collide, Skills Are Not Islands, POISE, SkillHarm) postdate our existing reviews.
- **Benchmarks beyond SkillsBench are entirely unreviewed** — see §d.

## (c) Top 20 uncovered papers, ranked by relevance to our four efforts

Efforts: **STANDARD** = the WeftOS skill package standard and registry (Agent Directory);
**GOVERNANCE** = skill trust/governance and evaluation gates; **SKILL3D** = the Skill-3D
evolved-skill Rust library; **ROUTER** = Episteme's router. Summaries below are written from each
paper's own abstract (fetched via the arXiv API 2026-09-28), not from the nexus repo's one-line
titles.

1. **From Anatomy to Smells: An Empirical Study of SKILL.md in Agent Skills** (2607.01456) —
   STANDARD — **DEEP-READ**. Qualitatively analyzes 238 real skills into a 13/44-component
   taxonomy, reviews 29 sources for authoring best practice, and ships an automated "skill smell"
   detector: over 99% of real SKILL.md files contain at least one smell, and smells persist as
   skills evolve rather than getting fixed. Directly informs the Agent Directory's package schema
   and argues for a lint gate, not just a security gate.

2. **From Registry to Repository: How AI Agent Skills Are Written, Adapted, and Maintained**
   (2607.00911) — STANDARD — **DEEP-READ**. First empirical study of skills as maintained
   software artifacts, mining 18,463 skills.sh registry skills and 23,199 personal-repo skills
   (3,709 reuse links). Finds reuse is mostly one-time copying (53% never modified after adoption)
   and that the *behavioral contract* — how a skill talks to users, monitors state, recovers from
   failure — almost never changes even when skills are customized. Shapes what "version" and
   "maintenance" should mean for `AgentPackageVersion`.

3. **Skills Are Not Islands: Measuring Dependency and Risk in Agent Skill Supply Chains**
   (2607.01136) — GOVERNANCE — **DEEP-READ**. Defines Agent Skill Supply Chains (SBOM-style
   dependency graphs) over 1.43M skills; finds metadata that is activation-ready but
   governance-poor, and hidden package inventory via recursive skill reuse invisible to per-skill
   scanning. Directly informs the Agent Directory's trust tiers and argues for typed dependency
   manifests and lockfile-like records — a concrete addition to `design.md`'s `AgentPackageVersion`.

4. **Towards Secure Agent Skills: Architecture, Threat Taxonomy, and Security Analysis**
   (2604.02837) — GOVERNANCE — **DEEP-READ**. First systematic security analysis across the
   Creation/Distribution/Deployment/Execution lifecycle; a 7-category, 17-scenario threat taxonomy
   validated against 5 confirmed incidents. Argues the worst threats — no data/instruction
   boundary, single-approval persistent trust, no mandatory marketplace review — need structural
   fixes, not incremental mitigation. Core reading before finalizing the eval-gate design.

5. **Audited Skill-Graph Self-Improvement (ASG-SI)** (2512.23760) — GOVERNANCE — **DEEP-READ**.
   Treats self-improvement as compiling an agent into a growing, auditable skill graph: each
   candidate skill is extracted, normalized, and promoted only after verifier-backed replay and
   contract checks, with rewards decomposed into independently auditable, replayable evidence.
   The closest existing blueprint for a promotion/rollback gate on evolved skills.

6. **SkillsVote: Lifecycle Governance of Agent Skills** (2605.18401) — GOVERNANCE — **SKIM**.
   Full collection/recommendation/attribution/evolution lifecycle over a million-scale corpus,
   with evidence-gated updates and both online (test-time) and offline (frozen-library) evolution.
   Skim for the attribution mechanism — crediting outcomes to skill vs. exploration vs. environment
   is a real gap in our own governance thinking.

7. **Library Drift: Diagnosing and Fixing a Silent Failure Mode in Self-Evolving LLM Skill
   Libraries** (2605.19576) — SKILL3D / GOVERNANCE — **DEEP-READ**. Names and isolates "library
   drift": unbounded skill accumulation without outcome-driven lifecycle management silently
   degrades retrieval and causes false-positive injections. A minimal governance recipe
   (outcome-driven retirement + bounded active-cap + meta-skill-authoring prior) lifts held-out
   pass@1 from 0.258 to 0.584. Directly actionable for Skill-3D's Rust library, which has the same
   unbounded-accumulation shape.

8. **When Safe Skills Collide: Measuring Compositional Risk in Agent Skill Ecosystems**
   (2606.00448) — GOVERNANCE — **DEEP-READ**. Individually-safe ClawHub skills compose into
   unsafe installed sets; of skills that pass individual inspection, a population-weighted ~18%
   of flagged pairs are real compositional risks, and whether risk is *realized* depends on host
   model disposition, not just the skill content. Motivates install-time compositional checks in
   the Agent Directory's approval gate, not just per-skill scanning.

9. **SkillGuard: A Permission-Centric Framework for Agent Skill Security** (2606.03024) —
   GOVERNANCE — **DEEP-READ**. A dual-plane governance model (context influence + action side
   effects) via skill manifests, runtime permission control, and policy enforcement; evaluated on
   1,260 real skills with 99.93% object coverage and measurable attack-success reduction. A
   concrete manifest schema to compare against the capability fields already drafted in
   `agent-directory/design.md` (`executes_code`, `network_egress[]`, `reads_secrets[]`, ...).

10. **POISE: Position-Aware One-Instruction Skill Injection** (2606.07943) — GOVERNANCE —
    **SKIM**. A position-aware poisoning attack that hides a single command-bearing instruction
    in the SKILL.md body and matches YAML-frontmatter-level reliability while evading LLM-judge
    audits. Useful as a documented evasion pattern the eval gate must specifically test against.

11. **SkillHarm: Lifecycle-Aware Skill-Based Attacks via Automated Construction** (2606.02540) —
    GOVERNANCE — **SKIM**. A benchmark of 879 attack samples across 71 skills, contrasting
    fixed-payload poisoning with self-mutating poisoning that defers harm to a later reuse; up to
    86% attack success. Useful as a red-team test suite reference for the gate, not core design
    reading.

12. **From Context to Skills (Ctx2Skill)** (2604.27660) — STANDARD / SKILL3D — **SKIM**. A
    self-evolving multi-agent loop (Challenger/Reasoner/Judge, plus a Cross-time Replay mechanism
    to stop adversarial collapse) that discovers and refines skills for long-context reasoning
    tasks. The anti-collapse replay idea generalizes; the target problem (context learning) is
    different enough from task/tool skills that only that mechanism is worth lifting.

13. **GraphSkill: Documentation-Guided Hierarchical Retrieval-Augmented Coding** (2603.06620) —
    ROUTER — **SKIM**. Treats technical documentation as hierarchical rather than flat for
    retrieval, plus a self-debugging coding agent using small generated test cases. The
    hierarchy-aware retrieval idea generalizes; the benchmark is graph-algorithm-specific, not a
    general skill router.

14. **SkillDAG: Self-Evolving Typed Skill Graphs for LLM Skill Selection at Scale** (2606.03056)
    — ROUTER — **DEEP-READ**. Models inter-skill relationships as a typed, agent-callable,
    self-evolving directed graph — each query returns vector matches, typed-edge neighbors, and
    conflict signals, with a propose-then-commit protocol that lets the agent register new edges
    from execution. Beats the strongest Graph-of-Skills baseline by +12.8/+8.6 points on
    SkillsBench. A strong candidate architecture for Episteme's router once the skill count grows
    past what flat retrieval handles well.

15. **SkillRouter: Skill Routing for LLM Agents at Scale** (2603.22455) — ROUTER —
    **DEEP-READ**. Shows progressive disclosure (hiding skill bodies, routing on name+description
    only) costs 37-44 points of routing accuracy at ~80K-skill scale — the missing signal is
    body-resident, not a length artifact. Ships a compact 1.2B body-aware retrieve-and-rerank
    router at 74% Hit@1, 13x fewer parameters and 5.8x faster than the strongest baseline this
    paper tests. Directly answers whether Episteme's router should see full skill bodies (yes),
    with a cheap distillation fallback if that gets too expensive.

16. **GraSP: Graph-Structured Skill Compositions for LLM Agents** (2604.17870) — ROUTER —
    **DEEP-READ**. Compiles flat skill sets into typed DAGs with precondition-effect edges,
    node-level verification, and locality-bounded repair (replanning drops from O(N) to O(d^h));
    beats ReAct/Reflexion/ExpeL/flat-skill baselines across four benchmarks and confirms the
    "more skills ≠ better performance" finding that also drives Library Drift (#7). A second
    strong router/orchestration architecture to weigh against SkillDAG for Episteme.

17. **Maestro: RL to Orchestrate Hierarchical Model-Skill Ensembles** (2605.22177) — ROUTER —
    **SKIM**. An RL-trained policy that composes ensembles of frozen expert models and a two-tier
    skill library step by step; a 4B orchestrator beats GPT-5 and Gemini-2.5-Pro on ten
    multimodal benchmarks and generalizes to unseen registry entries without retraining. The
    "which model to call" framing differs from Episteme's "which skill to load" — skim for the
    zero-retrain generalization result only.

18. **EvoSkill: Automated Skill Discovery for Multi-Agent Systems** (2603.02766) — SKILL3D —
    **DEEP-READ**. A self-evolving framework that analyzes execution failures, proposes new
    skills or edits, and materializes them into structured folders under Pareto-frontier
    selection (held-out validation gate, base model frozen); shows cross-task zero-shot transfer
    of evolved skills. Structurally the closest published analog to what Skill-3D's Rust library
    needs for its own promote/merge loop.

19. **AutoSkill: Experience-Driven Lifelong Learning via Skill Self-Evolution** (2603.01145) —
    SKILL3D — **SKIM**. A model-agnostic plugin layer abstracting, evolving, and injecting skills
    from dialogue traces without retraining, with a standardized cross-agent skill representation
    for sharing. Framed as personalization rather than task mastery — relevant mainly for the
    representation-sharing format, not the evolution mechanism.

20. **SkillCoach: Self-Evolving Rubrics for Evaluating and Enhancing Agentic Skill-Use**
    (2607.01874) — GOVERNANCE — **DEEP-READ**. Scores trajectories on four process axes (skill
    selection, following, composition, grounded reflection), kept separate from final-verifier
    outcome success, so process quality doesn't get masked by accidental task completion. The
    best available blueprint for a per-skill "did it actually use the skill well" eval gate,
    distinct from pass/fail — complements SciVis's own evaluation-centric paradigm citation
    (`refs.tsv` #2).

## (d) Benchmarks: what to run alongside SkillsBench and the SciVis methodology

We already use SkillsBench's paired with/without-skill methodology (`skill-3d/papers/analysis/030-skillsbench.md`)
and SciVis's evaluation-centric paradigm (`refs.tsv` #2-3, SciVisAgentBench). From the nexus's 19
benchmarks, four more are worth adding to a per-host, per-skill eval suite, and two are worth
declining:

- **SkillEvolBench** (2605.24117) and **SkillGenBench** (2605.18693) — measure evolution quality
  and generation quality specifically, not just end-task success. Skill-3D's Rust library needs
  exactly this: a way to score whether a *newly evolved* skill is good, before it reaches a task.
- **MalSkillBench** (2606.07131) and the SkillHarm benchmark (#11 above) — red-team suites for the
  governance gate; run these against any skill before it's promoted to `trusted`, the same way
  SciVis's methodology treats correctness checks as a precondition for adoption.
- **SkillCoach** (#20 above) — process-rubric scoring, complementary to outcome benchmarks; use it
  where a pass/fail signal alone would hide *how* a skill was (mis)used.
- **A Framework for Evaluating Agentic Skills at Scale** (2606.17819) — general scale-eval
  methodology; worth a skim for whether it generalizes better than SkillsBench's paired design as
  our own library grows past SkillsBench's tested range.
- **Decline for now**: AgentBench, WebArena, TaskBench, STULIFE, TRACE, Evo-Memory, SRA-Bench,
  R3-Skill are general agent/tool benchmarks, not skill-specific, and R3-Skill shares an arXiv id
  (2606.03565, "Skill Is Not Document") with a retrieval paper already in `nexus-index.tsv` under
  3-Retrieval — read that paper, not the benchmark, if router work needs it.

## (e) Ecosystem platforms: new registries filed

Checked all 21 platforms in the nexus's Ecosystem Platforms table against
`~/.claude/skills/skill-builder/references/skill-sources.md`. Sixteen are single-repo skill
packages (BulkPublish, BrowserAct, Hermes Tweet, Markstream, ax, Tree Ring Memory, UnifAPI,
RunAPI, Orkas VideoStudio, Duvo, Before You Build, shidi-skill, Agent Coordinator, UIZZE, and two
more) — not registries, so they don't belong in the Registries table; none is a domain fit we're
actively reviewing, so none was filed under "Domain collections already reviewed" either. Five are
genuine registries/marketplaces not already listed and were appended (HTTP 200 checked 2026-09-28)
to the Registries table in both `~/.claude/skills/skill-builder/references/skill-sources.md` and
its project mirror `weftos/.claude/skills/skill-builder/references/skill-sources.md`, nothing else
in that file was changed:

- **SkillNet** (skillnet.openkg.cn) — large-scale skill repository and organization
- **ClawHub** (clawhub.ai) — skill sharing/discovery marketplace; also the measurement substrate
  for two of the top-20 papers above (SkillGuard, When Safe Skills Collide), so it doubles as a
  live dataset source for governance work
- **SkillHub** (skillhub.club) — community skill resources
- **SkillsMP** (skillsmp.com) — marketplace-style skill ecosystem
- **skillZs** (skillzs.dev) — discovery, authoring guides, and security resources

Note: the "Start here: the research nexus" pointer to this same repo, and the `officialskills.sh`
row, were already present in both files as of this session's start (added by earlier work today,
2026-09-28) — not added by this pass.

## (f) Research opportunities WeftOS is positioned to contribute to

The nexus's own open-problems list names five directions. Three map onto work already staged in
our research corpus, not yet published outward:

- **"Unified skill schema"** (common fields for scope, triggers, dependencies, versioning,
  resources, safety constraints, provenance) and **"library evolution under non-stationarity"**
  (API drift, compatibility checks, rollback, regression recovery) are close to exactly what
  `agent-directory/design.md`'s `AgentPackageVersion` + `ProjectAgentLock` + gate/eval-receipt
  model is building. None of the nexus's governance papers (top-20 #1-11) ship a schema this
  concrete or cover rollback; that's a real gap we could fill and publish back.
- **"Multimodal and domain-specific benchmarks"** (embodied, GUI, robotics, UAV, healthcare) names
  a gap Skill-3D's honest-geometry constraint already addresses in one domain: typed geometric
  claims (metric distance, relative depth, pixel boxes) that cannot silently convert into each
  other, enforced at the type level rather than by convention. No paper in the nexus's Skill
  Representation section enforces this; it's a concrete, exportable pattern once the Rust library
  ships.
- **"Causality-driven skill diagnosis"** (attributing failures to retrieval mismatch, policy
  mis-selection, unsafe composition, stale dependencies, or tool malfunction) is close to
  unclaimed territory even inside the top-20: SkillsVote (#6) attributes *outcomes*, and Library
  Drift (#7) diagnoses *one* failure mode (accumulation) with reproducible ablations, but nothing
  in the nexus builds a general cross-cause diagnostic. A cross-host eval gate that classifies
  *why* a skill failed — not just whether it did — is a genuine opening for Episteme's router work
  and would be the first of its kind in this literature.

Governed skill registries and cross-host skill evaluation (the request's other two named
opportunities) are covered by the same three points above: the Agent Directory instantiates
governed registries, and a causality-aware eval gate run once per host (Claude Code, Grok, Codex)
is the natural extension of the per-host coverage matrix already in `agent-directory/design.md`.
