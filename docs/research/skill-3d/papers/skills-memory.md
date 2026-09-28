# Skills & Memory Cluster Synthesis

Cluster: refs 10, 21, 25, 26, 29, 30, 32, 36, 46, 54, 65, 77, 78, 79, 83, 87, 90, 94 of the Skill-3D reference list (arXiv 2606.07436). All 18 are cited in a single passage, Skill-3D §2.3 "Agent Skills," which groups them into three sub-clusters plus a memory-baseline pair. Individual analyses are at `papers/analysis/NNN-*.md`.

## Reference table

| # | Short name | Year | Verdict | One-line role |
|---|---|---|---|---|
| 54 | Reflexion | 2024 | PATTERN | Verbal self-reflection in an episodic buffer; the historical floor baseline |
| 94 | ExpeL | 2024 | PATTERN | Insight extraction from trajectories, no gradient update; ancestor of "skill" |
| 10 | Mem0 | 2025 | PATTERN | Production extract→consolidate→retrieve memory pipeline, flat + graph variants |
| 79 | Memory-R1 | 2025 | ADOPT | RL-trained ADD/UPDATE/DELETE/NOOP memory-manager + separate answer agent |
| 78 | Agent Skills Survey | 2026 | ADOPT | SKILL.md/progressive-loading paradigm; 26.1% of community skills vulnerable; 4-tier trust gate |
| 21 | OpenClaw survey | 2026 | WATCH (unverified) | Real-world deployed skill/plugin ecosystem; primary source inaccessible (403) |
| 29 | AgentSkillOS | 2026 | ADOPT | Capability-tree retrieval + DAG orchestration at 200-200K skill scale |
| 83 | SkillOpt | 2026 | ADOPT | Text-space skill optimizer; +19-25 pts inside Claude Code/Codex specifically |
| 25 | XSkill | 2026 | ADOPT | Dual-stream (experience/skill) visually-grounded continual learning |
| 26 | Agentic Proposing | 2026 | WATCH | Skills compose to synthesize *training problems*, not solve tasks |
| 30 | SkillsBench | 2026 | ADOPT | Paired, deterministic with/without-skill eval methodology; focused>bundled |
| 32 | SkillNet | 2026 | PATTERN | 5-axis skill quality rubric (Safety/Completeness/Executability/Maintainability/Cost) |
| 36 | SELF-VLA | 2026 | SKIP | Robotic disassembly; confirms cross-domain generality, no new design info |
| 90 | MemSkill | 2026 | PATTERN | Controller/executor/designer split for memory-*writing* skills specifically |
| 87 | Meta Context Eng. | 2026 | WATCH | Meta-level agent evolves the skill-writing process itself ("agentic crossover") |
| 46 | SkillOS (SAGE-curator) | 2026 | WATCH | Frozen executor + RL-trained curator managing the skill repo |
| 65 | SAGE (RL skill lib) | 2026 | PATTERN | Skill-integrated reward folded into GRPO training |
| 77 | SkillRL | 2026 | PATTERN | Hierarchical SkillBank + policy/library co-evolution, closest analog to Skill-3D itself |

Verdict counts: ADOPT 5, PATTERN 7, WATCH 5, SKIP 1.

## The design space

Reading across all 18, five recurring axes define how "agent skill" work in this cluster differs from plain trajectory memory:

**1. Representation granularity.** Everything above ExpeL/Reflexion (free-text insight/reflection, no structure) moves toward *structured, typed* units: XSkill's action-level "experience" vs. task-level "skill"; SkillRL's *hierarchical* SkillBank; MemSkill's *memory-specific* skills as distinct from task skills; SkillOpt's single skill *document* treated as an optimizable artifact. The consistent finding (SkillsBench [30]) is that **smaller, focused skill packages (≤3 modules) beat comprehensive bundles** — granularity should err toward narrow.

**2. Extraction/promotion.** Two mechanisms recur: rule-based promotion from successful rollouts (Skill-3D's own approach: promote if no compatible skill exists, else merge only if new coverage is added) and RL/optimizer-driven curation (SkillOS's [46] RL curator, SAGE's [65] skill-integrated reward, SkillOpt's [83] bounded add/delete/replace edits under a held-out validation gate). No paper in this cluster shows rule-based promotion is *insufficient* — Skill-3D's own ablations aren't part of this cluster — but SkillOpt's 52/52 win rate over rule-based and one-shot baselines is the strongest evidence that **learned/optimized curation beats static rules once a validation signal exists.**

**3. Retrieval at scale.** Flat vector/embedding retrieval (implicit in most of the cluster, explicit in Mem0 [10]) degrades as a library grows; AgentSkillOS [29] is the only reference to test this directly (200 → 200,000 skills) and finds a **capability-tree taxonomy** approximates oracle selection at that scale, while flat invocation of even the *correct* skill set underperforms **DAG-based orchestration** of the same skills. This is a scale problem WeftOS does not yet have but will.

**4. Skills for memory vs. skills for tasks.** A meaningful split exists between skills that guide *task execution* (most of the cluster: XSkill, SkillRL, SkillNet, SELF-VLA) and skills that guide *memory management itself* (Memory-R1 [79]'s ADD/UPDATE/DELETE/NOOP, MemSkill [90]'s controller/executor/designer loop). Skill-3D conflates these into one Scene Memory + Skill Library pipeline; the cluster suggests they are usefully separable.

**5. Evaluation discipline.** Only SkillsBench [30] treats "do skills actually help" as an empirical question with a deterministic, paired-comparison answer (+16.6 points average, but ranging +4.1 to +25.7 depending on harness, and negative-leaning for over-large bundles). Every other paper in the cluster reports its own benchmark wins without that same isolate-the-skill-variable discipline — a methodological gap worth flagging rather than repeating.

## Best-supported ideas (strongest evidence, cross-checked across ≥2 references)

1. **Focused skills beat exhaustive ones** (SkillsBench [30], reinforced by SkillNet's [32] cost-awareness axis and SkillOpt's [83] bounded-edit discipline).
2. **Skill quality and skill orchestration are separate levers, both large** (AgentSkillOS [29]: DAG orchestration beats flat invocation of the *identical* skill set; SkillsBench [30]: skill content alone swings pass rate 16+ points).
3. **Learned/optimized curation outperforms static heuristics once a validation signal exists** (SkillOpt [83] 52/52 wins; SAGE [65] and SkillRL [77] both fold skill quality into the RL objective rather than treating it as frozen).
4. **Security is not free** — 26.1% of real community skills carry vulnerabilities (Xu & Yan [78]); no other paper in the cluster addresses this, making it a blind spot the rest of the field (and Skill-3D itself) does not cover.
5. **Skills validated inside Claude Code / Codex specifically show large, consistent gains** (SkillOpt [83]: +19.1 to +24.8 points across those exact harnesses) — the strongest available signal that this whole line of research transfers to WeftOS's actual delivery targets.

## Skill-3D's own skill library, in this context

Skill-3D (§3.1) runs a rule-based promote/merge pipeline — successful rollouts become dynamic skills (promoted if no compatible skill exists, merged if they add coverage, else only success-stats updated) and failed rollouts are attached as "lessons" rather than skills — then post-trains a compact model against skill-guided trajectories via SFT+GRPO. Relative to the cluster:
- Its lesson/skill split mirrors ExpeL's [94] insight/experience distinction and XSkill's [25] experience/skill split, but Skill-3D is unique in the cluster for grounding both in **3D scene + tool-evidence** context (object detection, depth, reconstruction), not just text or 2D vision.
- Its promotion/merge logic is the *simplest* curation mechanism in the cluster — no learned curator (unlike SkillOS [46]), no text-space optimizer (unlike SkillOpt [83]), no capability-tree (unlike AgentSkillOS [29]). This is appropriate at Skill-3D's scale (one pooled global library across training benchmarks) but would need to change at ecosystem scale.
- Its SFT+GRPO training stage is the closest published analog to SAGE's [65] skill-integrated-reward RL loop, though Skill-3D's reward is a fixed 0.6/0.2/0.2 weighted composite (answer correctness, tool-use efficiency, skill-tool format) rather than a learned skill-quality term.

## Recommendations for the WeftOS Rust skill library

1. **Adopt Memory-R1's ADD/UPDATE/DELETE/NOOP** as the literal operation vocabulary for whatever component writes to durable skill/memory storage on AgentDB/RVF — it's minimal, implementable, and already validated at small model scale (3B-14B) with low training-data requirements (152 QA pairs), lowering the bar if WeftOS ever wants to learn this policy rather than hand-write it.
2. **Cap skill packages at ≤3 modules** when authoring or auto-generating skills for Claude Code/Grok/Codex, per SkillsBench's [30] finding that focused skills outperform comprehensive bundles — this is a concrete authoring constraint, not just a design preference.
3. **Wire skill trust into the existing governance gate** using Xu & Yan's [78] four-tier, provenance-linked permission model, and budget for the fact that roughly a quarter of any externally-sourced skill corpus will carry vulnerabilities — this is the one axis (security) the rest of the cluster, and Skill-3D itself, doesn't address at all.
4. **Plan for a capability-tree index alongside AgentDB/RVF's flat HNSW** before the skill library exceeds a few hundred entries, and prefer **DAG-based orchestration** over sequential flat skill calls when a task needs more than one skill — both per AgentSkillOS [29], whose 200-200K-skill scale test is the only one in this cluster that stress-tests retrieval/orchestration at anything resembling ecosystem scale.
5. **Separate the memory-writing skill loop from the task-execution skill loop** architecturally (per MemSkill's [90] controller/executor/designer split and Memory-R1 [79]), rather than treating "what to remember" and "what skill to apply" as the same decision — Skill-3D itself doesn't make this distinction, but the broader cluster suggests it's a useful seam.
6. **Use SkillsBench's [30] paired, deterministic with/without-skill methodology** to validate the WeftOS skill library empirically, rather than relying on end-to-end benchmark deltas that conflate skill quality with everything else changing at the same time.
7. **Treat SkillOpt [83] as the first thing to benchmark against Skill-3D's own rule-based promote/merge logic** once the Rust skill library exists — it's the only reference in the cluster with public code, a held-out-validation acceptance discipline directly implementable as a CI step, and evaluation numbers specifically inside Claude Code and Codex.
8. **Re-verify [21] (OpenClaw survey) before citing it** — its primary source was inaccessible (HTTP 403) during this research pass; everything in `analysis/021-openclaw-language-infra.md` is search-snippet-derived and unconfirmed.

## Grounding for rUv-tool comparisons

Claims above about WeftOS's likely substrate (AgentDB/RVF with HNSW) are grounded via `mcp__plugin_ruvnet-brain_ruvnet-brain__search_ruvnet`, not assumed:
- **ReasoningBank's 4-factor retrieval scoring** (similarity 65%, recency 15%, reliability 20%, diversity 10%) and its RETRIEVE→JUDGE→DISTILL→CONSOLIDATE 4-phase cycle — path: `agentic-flow/.claude/agents/reasoning/adaptive-learner.md`. This is architecturally closest, within the rUv stack, to Memory-R1's [79] manager/answer-agent split and MemSkill's [90] controller/executor/designer loop — none of the three are identical, but all three separate *retrieval-selection* from *write-decision* as distinct, individually-tunable steps.
- **AgentDB v2.0's HNSW performance** (61μs p50 vector search latency, 16,400 QPS, 150-12,500x speedup claims vs. v1, tensor compression 2-32x via f32→f16→PQ8→PQ4→Binary tiers) — path: `agentdb/docs/RUVECTOR-INTEGRATION-V2.md` and `agentdb/docs/archive/reviews/DEEP-REVIEW-V2-LATENT-SPACE.md`. This is the flat-vector-retrieval baseline that AgentSkillOS's [29] capability-tree finding argues needs a taxonomy layered on top once skill count grows past low hundreds — AgentDB's HNSW alone does not solve the retrieval-at-ecosystem-scale problem AgentSkillOS identifies, it solves a different problem (raw vector search latency).
- **ReasoningBank's MoE routing** (`ENABLE_MOE_ROUTING`, 8 domain experts, top-K=2 selection) — path: `agentdb/docs/archive/ATTENTION_INTEGRATION.md`. This is a plausible implementation vehicle for AgentSkillOS's capability-tree idea (domain-routed retrieval) but is not the same mechanism — MoE routing selects among a small fixed expert set, while AgentSkillOS's tree is a recursive taxonomy scaling to 200K leaf skills. Do not conflate the two when scoping the capability-tree recommendation above.

None of the surveyed 18 papers name ReasoningBank or AgentDB directly (they are a disjoint literature); the comparison above is WeftOS-side synthesis, not a claim found in any cited paper.

## Open questions for the honest-geometry / MentraOS context

Two parts of the WeftOS brief — honest geometry (appearance never mints metric geometry) and MentraOS egocentric capture — are not addressed by any of the 18 references in this cluster, which is worth flagging rather than papering over:

- **None of these 18 papers discuss geometric grounding or metric honesty at all.** XSkill [25] and SELF-VLA [36] are the only two with any visual/physical grounding, and neither distinguishes appearance-based inference from metric measurement. If WeftOS's skill library needs to encode a hard rule like "a depth/scale claim must cite a metric tool output, not a VLM guess," that rule has to come from outside this cluster — most plausibly from Skill-3D's own method section (its tool-evidence-to-answer mapping in §3.1) rather than from any of its cited skill-memory literature.
- **Egocentric, continuous-capture memory (MentraOS-style) is closest to OpenClaw's [21] "agents running continuously across heterogeneous platforms" framing**, but that reference could not be verified in this pass. Re-fetching it should be a near-term follow-up specifically because it's the one reference in this cluster that plausibly speaks to continuous, always-on operation rather than episodic task-by-task rollouts.

## Files written

18 per-reference analyses in `papers/analysis/` (010, 021, 025, 026, 029, 030, 032, 036, 046, 054, 065, 077, 078, 079, 083, 087, 090, 094) plus this synthesis at `papers/skills-memory.md`.
