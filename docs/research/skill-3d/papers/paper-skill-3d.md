# Skill-3D: Evolving Scene-Aware Skills for Agentic 3D Spatial Reasoning — Deep Read

Source: arXiv:2606.07436v2 [cs.CV], 11 Jun 2026 (Li, Hu, Wang, Fan, Yang —
Zhejiang University / UTS / OPPO). Full text at
`docs/research/skill-3d/papers/paper-2606.07436.txt`. Cross-checked against
`~/dev/Skill-3D` (skill3d/, train/, README.md), read-only. License note: the
paper is CC BY-NC-SA 4.0 (arXiv license line); the cloned repo declares
Apache-2.0 (its own `LICENSE` + README badge) — a real discrepancy.

## 1. Problem framing and claimed failure modes

Target: agentic 3D spatial reasoning — an MLLM agent answering indoor 3D
questions via external perception/geometry tool calls mid-reasoning, not
end-to-end (§1). Claimed failure mode: existing agents "apply a uniform
tool-use strategy to all scenes" rather than conditioning on scene+task
(Abstract, §1). Two symptoms named: **biased tool preference** — GPT-5.4
"mostly calls GroundingDINO," Think3D "heavily relies on Pi3," regardless of
question (§4.3, Fig. 4); and **evidence mismatch** — the worked example
(Fig. 1a): an object-to-object *distance* question needs depth evidence, but
the uniform strategy reaches for detection + 3D reconstruction, which give
relative layout, not metric depth. Attributed to **scene heterogeneity**:
"required evidence and tool workflows vary across scenes" (§1). Quantified
later by Effective Tool Usage (§4.3, Eq. 3): w/ Tools gets only 39.2% of tool
calls on VSI-Bench to contribute valid, used evidence — the source of the
abstract's "39%→78%" headline.

## 2. Method

### 2.1 Scene Memory and skill extraction (§3.1, Fig. 2a)

A rollout = `(question, observations O, reasoning trace, selected skills,
tool calls, tool outputs, final answer)`. The library updates after every
rollout, asymmetrically:

- **Successes → workflows.** Extract trigger condition, required evidence,
  tool order, key arguments, evidence-to-answer mapping. Promoted to a *new*
  dynamic skill only if no compatible skill exists; otherwise merged only if
  it "adds useful coverage... a new scene condition, stronger evidence
  source, or lower-cost workflow," else only success statistics are bumped
  (§3.1, "Successes as Workflows") — a compactness constraint on the library.
- **Failures → lessons.** Diagnosed into named error types: wrong tool
  selection, missing evidence, invalid tool input, ignored tool output,
  redundant tool calls (§3.1, "Failures as Lessons"). Reliable corrections
  patch the skill with a fallback rule; repeated failures under a *static*
  skill spawn a new failure-aware *dynamic* skill.
- **Skill Maintenance** ("Skill Manager") gates insert/merge/patch/reject,
  accepting only evidence-supported, previously-consistent updates. Static
  skills stay fixed task-level priors; only dynamic skills evolve.

**What a "scene" key actually is.** The paper's prose ("scene context",
"scene signature") reads as keyed on the visual scene. The code
(`AdaptiveSkillManager`, `skill3d/core/skill_learning.py`) resolves every
question to one of **eight seed patterns** — `spatial_localization,
depth_distance, multiview_occlusion, detection_counting,
segmentation_boundary, fine_grained_pointing, custom_category,
comprehensive_scene_understanding` (`:150-159`, mirrored in the 8 static
skills of `skill_normalization.py:13-109`) via keyword scoring of the
question text (`_classify_seed_pattern:800-813`), plus a dataset-task layer
(`_task_class_profile:827-887`, e.g. `object_absolute_distance`). So "scene"
in the code is a **two-level question taxonomy**, not a geometric
scene-clustering signature — narrower than the prose implies (see §5, §7).

**Skill/lesson schema.** Paper's abstract fields: trigger, applicable scene
context, required evidence type, historical metadata (success rate, failure
lessons, tool cost) (§3.2). Code's static `Skill` (`skill.py:8-21`) is
`{name, when, strategy:[...], tool_usage}`, e.g. Skill 2 ("Depth & Distance
Reasoning"): `when: "closer/farther / distance / front-back / which is
nearer"`, `strategy: ["...treat depth_estimation_tool as the anchor
first-pass tool rather than a weak auxiliary cue...", ...]`, `tool_usage:
"Common tools: depth_estimation_tool, segment_image_tool, moondream_tool,
swinir_tool, detect_objects_tool, pi3_tool"`
(`skill_normalization.py:25-37`). Dynamic skills (`SkillMemoryUnit`,
`skill_retrieval.py:89-123`) add `tool_candidates,
fallbacks:[{failed_tool, failure_type, next_best_tool}], failure_memory,
view_policy, success_rate, correct_rate, reward_avg, maturity`, plus
`preconditions, skip_conditions, stop_conditions, required_evidence,
answer_pattern, coverage{...}, observed_support` at prompt time.

### 2.2 Retrieval at inference (§3.2, "Scene-Task Skill Retrieval")

Paper: top-k retrieval scored by "semantic alignment with query category,
target entities, scene signature, evidence requirement," plus historical
success rate, failure lessons, tool cost. Code (`skill_retrieval.py`) runs
three stages: (1) **dense retrieval** — embed each skill's templated
`retrieval_text` (trigger, tool chain, pre/skip-conditions, fallbacks,
historical stats, failure memory, view policy) with a sentence-transformer
(default `Qwen/Qwen3-Embedding-0.6B`, hashing fallback), cosine-score
against the query (`retrieve:437-472`); (2) **rerank** — hand-weighted
linear combiner, `0.45·semantic + 0.20·class_match + 0.15·tool_compat +
0.10·success + 0.05·reward + 0.05·freshness + 0.05·view_policy_match −
0.15·failure_penalty` (`rerank_skill_candidates:691-748`), `tool_compat`
matching keyword→tool signals (e.g. "closer/farther"→
`depth_estimation_tool`); (3) **diversity filter** — MMR pass (λ=0.72)
deferring near-duplicate skills (similarity ≥0.88 *and* same seed
class/tool chain/blocker type, `filter_similar_skill_candidates:577-688`),
its own comment noting it stands in for "the paper['s] learned router."
Default `RETRIEVAL_TOP_K=6`; Appendix B.1: 0.5s/query retrieval overhead.

### 2.3 Injection into the planner prompt (§3.2, "Skill Selection")

Paper: "the policy select[s] a compact subset of skills... [and] generates
short fallback rules." The shipped system prompt
(`train/system_prompt/system_prompt_skill3d_agentic.txt`) makes this a tag
protocol: a non-final turn must contain exactly one
`<skill_choice>{id,source,name,reason}</skill_choice>` then one
`<tool_call>{...}</tool_call>`; "Before the first tool call, choose the most
relevant retrieved skill... Do not invent skill ids." Six tools are
whitelisted with per-argument contracts (`detect_objects_tool,
segment_image_tool, depth_estimation_tool, orient_anything_tool,
swinir_tool, pi3_tool`). This is what makes the "purple marks retrieved
skills" trace in Fig. E.1/E.2 auditable — the model must cite a skill id
before acting.

### 2.4 Algorithm, step by step (§3.1–3.2 + code)

**Memory update (per rollout, training-time):** (1) classify question →
seed pattern + dataset task class; (2) diagnose rollout —
`is_successful = success ∧ reward≥0.5 ∧ is_correct≠False`, else enumerate
error types from tool logs (`_diagnose_rollout_errors:1877-1925`, the
code's literal version of the paper's failure taxonomy); (3) look for a
compatible existing skill with the same tool sequence + class
(`_find_compatible_workflow_skill:1927-1947`); (4) Skill Manager decision
(`_build_skill_management_decision:1992-2066`): success+compatible→merge
coverage (cap 64 items); success+no tools→insert direct-answer skill;
success+no match→insert new dynamic skill; failure+diagnosable→attach
lesson or patch; failure+no tools→attach missing-tool lesson; else→ignore;
(5) record decision — workflows/lessons dedup by `(class, tool_sequence)`,
with a failed-tool→fallback-tool map.

**Inference (per query):** build scene-task context → retrieve top-k
candidates (dense→rerank→MMR) → inject cards → policy emits
`<skill_choice>` then `<tool_call>` → execute, append evidence, repeat until
`<answer>` (max 3 turns in the training config, `SPAGENT_MAX_TURNS=3`,
`train/train_grpo.sh:52`).

## 3. Training

**Data.** GPT-5.4 is the teacher, used only on training splits for skill
distillation and SFT data (§4.1) — never on test data. Category-wise random
30/70 train/test split per benchmark, question-level disjoint (Table C.3):
VSI-Bench 708/1654, MMSI-Bench 157/345, CV-3D 360/840, BLINK 40/93. SFT: 500
samples; GRPO: 1k samples; 1 epoch each. Scene Memory/Skill Library is built
once by **pooling training splits of all four benchmarks**, then **frozen**
for eval and post-training.

**Agentic SFT** (§3.3): trains the full structured interaction — not just
tool-call imitation but *when/how to select skills* — as a stable GRPO
initialization (causally confirmed by the "Offline w/o Cold Start" ablation,
Fig. 5, which shows early degradation without it).

**GRPO** (§3.3 Eq. 1-2; Appendix C.3 Eq. C.1-C.5). Per query, sample `G`
trajectories `{τ⁽¹⁾..τ⁽ᴳ⁾}` conditioned on `(q, O, S_cand)`; reward
`R(τ) = R_ans(τ) + R_fmt(τ) + R_tool(τ)` (Eq. 1), with `R_tool(τ) =
R_exec(τ) − |A|/B` (Eq. 2), `A` = tool calls, `B` = max tool budget.
`R_exec` is binary, 1 only when the trajectory obtains evidence required by
"the benchmark task type and the frozen scene-task parser" — explicitly
*not* a function of which skill was picked, "to prevent the policy from
selecting easier skills to obtain higher tool-use reward" (§3.3). Appendix
C.3 (Eq. C.2) restates `R_exec` slightly differently, as successful
execution with non-empty output — a minor internal inconsistency. Advantage:
group-normalized `A_i = (R(τ⁽ⁱ⁾)−mean_j R)/std_j R` (Eq. C.3); clipped
PPO-style surrogate with KL to `π_ref` = the agentic-SFT policy (Eq. C.4-C.5).

**Reward weights**: 0.6/0.2/0.2 (answer/tool-efficiency/format, §4.1) —
confirmed verbatim in `train/train_grpo.sh:60-62`
(`ACC_WEIGHT/FORMAT_WEIGHT/TOOL_USE_WEIGHT`), wired to `--reward_funcs
external_r1v_acc external_agentic_skill3d_format external_skill3d_tool_use`.
**Gap**: those reward functions and `--multi_turn_scheduler
spagent_tool_call_scheduler` live in `plugin/plugin_all_angles.py`, which is
**not present** in the clone (only a test file references it) — the actual
reward implementation is unreleased.

**Models/hyperparameters/compute** (Table C.4): base Qwen3-VL-4B/8B; teacher
GPT-5.4. SFT lr 1e-5, batch 16, warmup 0.03; GRPO lr 1e-6, batch 16, group
`G=8`, clip ε=0.2, grad-accum 4, KL β=0.05; both bf16/Flash Attn/grad
checkpointing/AdamW, max seq len 4096, on 4× RTX PRO 6000 Blackwell. SFT
≈3h, GRPO ≈28h. GRPO-side script defaults match exactly
(`NUM_GENERATIONS=8, GENERATION_BATCH_SIZE=16, GRAD_ACC=4, RL_BETA=0.05`);
**SFT script defaults diverge**: `train_sft.sh` ships `LR=5e-6` (paper:
1e-5) and effective batch `1×1×4 GPUs=4` (paper: 16); both scripts default
`MAX_LENGTH=32768` vs. the table's 4096 (plausibly the table describes a
non-agentic stage, but it's unreconciled).

## 4. Results

Four settings compared: `w/o Tools`, `w/ Tools`, `Think3D` (prior SOTA
baseline), `Skill-3D`, across VSI-Bench (Obj. Cnt./Abs. Dist./Obj.
Size/Room Size/Rel. Dist./Rel. Dir./Route Plan/Appr. Order), BLINK
(multi-view), CV-3D (Depth Order/Rel. Dist.), MMSI-Bench (positional
relationship).

**Closed-source (Table 1)**, `w/o Tools → Think3D → Skill-3D` (Obj.Cnt /
Abs.Dist / RoomSize / BLINK-MV / CV-Depth / MMSI-PR): GPT-4o
`38.1→50.4→56.8 / 7.9→32.7→42.6 / 37.4→61.4→69.5 / 47.9→62.7→72.4 /
72.6→88.3→92.0 / 28.7→38.4→43.2`; GPT-5.4 `55.8→61.1→66.2 / 43.6→55.0→61.5 /
55.7→69.9→77.6 / 73.4→78.3→82.0 / 91.6→93.0→96.9 / 42.7→53.4→60.4`;
Gemini-3-Flash (vs. w/o Tools only) `45.3→60.9 / 9.2→56.1 / 39.8→71.8 /
59.1→77.6 / 84.6→93.2 / 32.7→54.8`.

Averaged over all four closed-source agents, Skill-3D lifts VSI-Bench
average from 42.9 (w/o Tools) to 64.5 — **50.3% relative gain** (§4.2); the
Gemini-3-Flash MMSI-Bench delta (32.7→54.8) is the abstract's "+67% on
MMSI-Bench." **Open-source (Table 2):** Qwen3-VL-4B/8B w/o Tools →
Skill-3D-4B/8B on VSI-Bench: **+59.7%/+60.3%** relative gain — the latter is
the abstract's "boosts Qwen3-VL-8B by 60%" (sample row, 8B Obj.Cnt.:
32.5→44.7 w/Tools→48.3 Think3D→**56.5** Skill-3D).

**Effective Tool Usage** (§4.3, Fig. 3, Eq. 3 — fraction of tool calls both
valid and consumed downstream): Skill-3D vs. w/ Tools — VSI-Bench
**39.2%→78.7%**, BLINK 36.4%→79.2%, CV-3D 31.8%→87.5%, MMSI-Bench
30.5%→80.3%; normalized per call, so this isolates quality from quantity.

**Tool distribution / efficiency** (Fig. 4; Table B.1, GPT-5.4, VSI-Bench):
GPT-5.4 defaults to GroundingDINO, Think3D to Pi3, regardless of task;
Skill-3D shifts depth/distance/size tasks toward Depth Anything 3 and
direction/relation tasks toward Orient Anything v2. Table B.1
(avg-score/calls/ETU/latency): w/o Tools 52.1/0; w/ Tools
58.2/1.2/39.2%/13.2s; Think3D 64.7/1.8/58.5%/35.1s; **Skill-3D
70.0/2.6/78.7%/0.5s-retrieval+20.8s-total** — more accurate, more
tool-efficient, *and* faster than Think3D, by avoiding Pi3's ~21.35s cost
(vs. ~0.77-1.51s for other tools) when cheaper tools suffice.

**Module ablation** (Table 3, GPT-5.4, VSI-Bench; full pipeline avg 69.9),
removed-component → avg (Δ): Failure Lessons 68.1 (−1.8); Dynamic Skills
67.8 (−2.1); Static Skills 65.6 (−4.3); MLLM Skill Selection 65.5 (−4.4);
Skill Retrieval 64.1 (**−5.8**, largest). Retrieval matters most, then
selection — *finding* the right skills dominates over dynamic adaptation or
failure lessons; static task-level priors (−4.3) carry nearly as much
weight as retrieval/selection.

**Cold start / skill updating** (Fig. 5, GRPO, Qwen3-VL-8B): **Offline**
(frozen library during GRPO) most stable/highest reward; **Online**
(updating during GRPO) is non-stationary since policy and retrieved skills
shift together; **Offline w/o Cold Start** degrades early, converges
slower — motivates the frozen-library + SFT-warm-up design.

**Cross-benchmark transfer** (Table B.2, static skills+GPT-5.4 fixed): w/o
Dynamic Skills VSI/BLINK/CV-3D/MMSI-PR = 63.4/77.2/92.2/52.6; VSI-sourced
dynamic skills alone → 68.7/78.5/93.7/57.8 (lifting even non-VSI targets);
**pooling all four benchmarks** → **69.9/82.0/95.3/60.4**, beating every
single source on every target — the empirical basis for the shared,
cross-benchmark Skill Library.

## 5. Where paper text and released code diverge

**"Scene" is a question-classification taxonomy, not a scene-geometry
signature** — 8 seed patterns × dataset task labels via keyword scoring of
question text (§2.1), not a visual embedding of the observations. **GRPO
reward is unreleased** — `plugin/plugin_all_angles.py` (reward funcs +
multi-turn scheduler) is absent from the clone, only referenced by one test
file, so Eq. 1-2/C.1-C.5 are a spec, not independently verifiable. **SFT
hyperparameter defaults diverge from Table C.4** (lr, effective batch size);
GRPO-side defaults match exactly (§3). **License mismatch**: paper CC
BY-NC-SA 4.0 vs. repo Apache-2.0. **Codebase lineage**: README states
Skill-3D "builds on the Think3D/SPAgent codebase" — the core class is
literally `SPAgent` (`agent.py:41`), and Think3D is the paper's strongest
*baseline* (Tables 1-2), so part of the accuracy delta may reflect shared
infrastructure (retry logic, tool-call repair, argument validation in the
~3000-line `agent.py`), not purely skill-library gains — neither paper
attributes this. **Retrieval/selection is more engineered than described**:
the paper frames selection as "the policy selects a compact subset"
(model-driven), but the code's MMR diversity filter is a deterministic
pre-filter run *before* the model sees candidates, its own comment noting it
substitutes for "the paper['s] learned router."

## 6. Limitations

**Admitted** (paper's entire Limitations section is one sentence): scope is
indoor 3D reasoning; outdoor/embodied/robotic transfer "may require new tool
interfaces, scene signatures, and safety constraints." Notably thin for the
claims made.

**Not admitted, visible from paper+code together:** pooled-library gains
(Table B.2) conflate skill *transfer* with simply having more relevant
training data, and no ablation isolates the two; train/test splits (Table
C.3) are question-disjoint, not *scene*-disjoint, so a skill keyed on
question type + a soft scene signature could be capturing scene-specific
regularities the "generalizes across scene-internal variations" claim (§1)
implies it shouldn't need; metric-distance/room-size gains (Table 1) ride on
unaudited monocular/few-view depth tools (Depth Anything 3's indoor-metric
variant, Pi3) whose own metric accuracy is never characterized — directly
the "appearance/monocular output should never mint metric geometry" concern
(§7); `R_tool` penalizes tool-call *count* uniformly (Eq. 2), not the ~2
orders-of-magnitude latency gap between Pi3 (~21.35s) and other tools
(~0.77-1.51s, Table B.1), so the observed shift to cheaper tools (Fig. 4) is
an accuracy-driven side effect, not something the reward prices in; and
SFT/GRPO data plus initial dynamic skills are all distilled from GPT-5.4, so
Skill-3D-4B/8B gains partly reflect imitating GPT-5.4's tool-use policy, not
purely learning to use the library — no ablation trains on skills distilled
from a weaker model.

## 7. WeftOS reading

**Transfers directly.** The three-part loop — extract skills from
successes, extract lessons from failures, retrieve-then-inject at inference
— is architecture-agnostic and maps onto a ReasoningBank-style procedural
memory store; the insert/merge/patch/reject Skill Manager (§2.4) is a clean
state machine worth porting near-verbatim, and it's also the part the paper
actually validates (Table 3 ablation). The `SkillMemoryUnit` schema
(trigger, preconditions/skip-conditions, tool_candidates, fallbacks,
failure_memory, success/correct/reward stats, maturity) is already close to
a ReasoningBank memory-item shape and should port with light renaming;
treat the rerank weight formula (§2.2) as a starting hyperparameterization
to tune, not a validated result — the paper only ablates "retrieval" and
"selection" as monolithic on/off switches (Table 3), never the weights.

**Benchmark scaffolding to replace, not keep.** The 8 static seed skills and
6-tool prompt whitelist are benchmark-specific priors tied to a
per-question, HTTP-microservice tool architecture and short discrete-frame
inputs — a starting seed set for the rewrite, not a taxonomy to preserve.
Nothing here handles skill invalidation when the world changes under you (a
room gets rearranged) — a genuinely new problem for WeftOS.

**Scene Memory → BVH regions + HNSW features.** Given §5's finding that
Skill-3D's "scene" key is a question-type classification, not a geometric
signature, the honest WeftOS mapping is two indices the paper conflates
into one: a **BVH-region key** (geometric — which region of Urth's BVH
world-model the observations fall in, honoring "unobserved space stays
unobserved" so a skill never claims coverage of unscanned geometry), and an
**HNSW-indexed question/evidence-type embedding** (semantic — what
`SkillRetrievalIndex` already does today, as brute-force cosine over a
NumPy matrix, `skill_retrieval.py:437-472`; a real ANN index is a drop-in
upgrade). A WeftOS skill should key on `(BVH region, HNSW question/evidence
cluster)` rather than the paper's single question-derived class — closer to
what the prose describes than what the code implements, and it structurally
closes the "kitchen depth-estimation vs. living room depth-estimation"
overfitting risk the paper names as a goal (§1) but only guards with a soft
text field, not a hard geometric partition.

**Egocentric glasses capture changes the framing, not just the sensor.**
Skill-3D's tools assume static, composed frames with recoverable per-shot
intrinsics; MentraOS capture is continuous, motion-blurred, head-relative,
uncalibrated — "scene-aware retrieval" needs a *temporal* continuity signal
(is this still the same space as 30s ago?) with no analogue in Skill-3D's
per-question, scene-resets-every-time framing. Privacy is a structural gap
too: nothing in the precondition/skip-condition vocabulary
(`_default_preconditions`, `_default_skip_conditions`,
`skill_learning.py:1650-1690`) models what's *permissible to capture* — a
skill triggering face/plate/screen detection needs a consent or redaction
precondition, not just an evidence-sufficiency one.
