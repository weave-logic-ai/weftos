# Skill-3D Code Review

Repo reviewed: `~/dev/Skill-3D` (weave-logic-ai fork of `skill-3d/Skill-3D`, arXiv 2606.07436,
"Evolving Scene-Aware Skills for Agentic 3D Spatial Reasoning"). Read-only review; no files
modified, no checkpoints downloaded, no GPU services started. All line citations are
`file:line` relative to `~/dev/Skill-3D` at commit `e379d64` (2026-06-08), which is identical
across `origin/main` and `upstream/main` — **this fork has zero divergence from upstream**.

## Summary

| Area | Finding |
| --- | --- |
| Architecture | Single-threaded, iteration-bounded ReAct loop (`SPAgent.solve_problem`, `skill3d/core/agent.py:1292`), max 3 turns by default, answer-first stop condition, rule-based tool-call selector/gate (not an LLM) |
| Tool-call format | Hermes/Qwen-Agent-style `<tool_call>{json}</tool_call>` wrapped around OpenAI function-calling JSON; extensive multi-layer normalization/repair for malformed JSON, hallucinated names, and wrong arg names |
| Scene Memory / Skill Library | JSON-file-backed (`learned_skills.json` + hierarchical `memory/` dir); successes become `success_workflows`, failures become `failure_lessons` with per-tool fallback suggestions |
| Skill retrieval | Default mode is `"symbolic"` (keyword/rule classification, no embedding search); a real dense-embedding path exists (`Qwen3-Embedding-0.6B` via SentenceTransformers) but is opt-in via `memory_mode=dense/hybrid` |
| Tool contracts | 7 Flask (not FastAPI) HTTP servers, all bind `0.0.0.0`, **zero auth, zero CORS lockdown** on any of them |
| Metric honesty | One confirmed bug: `depth_tool.py` hardcodes the word "Metric" in its natural-language description even when the server reports non-metric/relative depth |
| Training | ms-swift `swift sft` / `swift rlhf --rlhf_type grpo` wrapper scripts; GRPO reward (0.6 acc / 0.2 format / 0.2 tool-use) is implemented in an **unshipped** `plugin/plugin_all_angles.py` |
| Licensing | Top-level Apache-2.0 is clean, but the README-directed **Pi3 checkpoint is CC BY-NC 4.0 (non-commercial)** and **SAM3.1 checkpoint is under Meta's custom "SAM License"** (not OSI) — both are redistribution blockers if used as documented |
| Code quality | ~87% of Python files are vendored ML model code; original logic (~10k lines) concentrates in `skill3d/core/`; test coverage is real but thin (2 unittest suites cover selector + tool-call repair only) |
| Fork status | Fork is a pure mirror of upstream at this commit — nothing here is weave-logic-ai-specific work |
| Rust rewrite | ~14,700 lines of original code/assets inventoried in §10; hardest ports are `agent.py` (3,068 lines), `skill_learning.py` (4,186 lines, untested), `skill_retrieval.py` (embedding dependency), `prompts_with_skills.py` and `tool_normalization.py` (both must match training-time behavior byte-for-byte) |

---

## 1. Architecture

`SPAgent.solve_problem()` (`skill3d/core/agent.py:1292`) is a synchronous loop, `max_iterations`
defaulting to 3 (`agent.py:1296`). Each iteration: build a prompt (system + user on turn 1,
`_create_continuation_prompt()` folding in prior tool results afterward, `agent.py:1531-1542`) →
call the model (`_run_model_inference`, `agent.py:1397-1423`) → validate the `<think>/<tool_call>/
<answer>` tag protocol (`_response_format_errors`, `agent.py:2579-2602`, with repair-and-retry for
recoverable violations) → parse `<skill_choice>` and `<tool_call>` blocks → execute approved tool
calls via `ThreadPoolExecutor` (`_execute_tools`, `agent.py:2257-2419`; `pi3_tool` calls are forced
sequential to avoid server contention, `agent.py:2327-2380`).

Stop conditions: (a) a complete `<answer>` tag pair, honored immediately even if a `<tool_call>`
appeared in the same turn (`stop_on_answer` default `True`, `agent.py:95`, discarding the tool call
at `agent.py:1665-1684`); (b) `iteration == max_iterations` with no (or no approved) tool calls
(`agent.py:1737-1779`). There is no token-budget or wall-clock stop inside the loop itself — only
per-tool retry backoff (`_call_tool_with_recovery`, `agent.py:1073-1149`). If the loop exits without
a usable answer but has successful tool results, one forced synthesis call is made
(`create_follow_up_prompt`, `agent.py:1853-1897`).

A **rule-based selector**, not an LLM call, gates which parsed tool calls actually execute:
`_validate_tool_call()` (`agent.py:536-627`) rejects schema-invalid calls, calls that skip a
required prerequisite tool (e.g. `detect_objects_tool` before `depth_estimation_tool` on a
depth question), repeated `pi3_tool` calls at the original (0,0) viewpoint, and tools missing a
required `crop_box`; `_score_tool_call()` (`agent.py:629-686`) then ranks survivors by
priority/must-try/skill-required bonuses and heuristic question-signal matches
(`_question_signals`, `agent.py:360-439`).

## 2. Tool-Call Format, Normalization, Repair

Wire format: OpenAI function-calling JSON (`Tool.to_function_schema()`, `skill3d/core/tool.py:
56-70`) serialized into `<tools>...</tools>` in the system prompt; the model must emit
`<tool_call>{"name": ..., "arguments": {...}}</tool_call>` (`skill3d/core/prompts.py:12-79`) — a
Hermes/Qwen-Agent-style ReAct-XML wrapper, not native provider tool-calling.

Parsing runs a fallback chain in `parse_tool_call_block()` (`skill3d/core/tool_normalization.py:
15-47`): `json.loads` → `ast.literal_eval` → regex-substituted `True/False/None`→`true/false/null`
→ a hand-rolled non-JSON parser (`_parse_nonstandard_tool_call_block`, `:50-168`) that handles bare
tool names, `tool_name(arg=val, ...)` call syntax, and freeform positional text mapped per-tool.
`normalize_tool_call_payload()` (`:173-245`) fixes malformed call *envelopes* (nested `{"tool":
{"name","params"}}`, string-tool-with-inline-args, alt key names). `normalize_tool_name()`
(`:252-324`) fixes hallucinated/synonymous names via a static alias map (17+ variants of
`pi3_tool`), case-insensitive/token-normalized matching, then substring keyword heuristics.
`normalize_tool_arguments()` (`:340-481`) fixes wrong argument names per tool (10+ aliases for
`detect_objects_tool`'s `text_prompt` alone) and strips any key not in the declared schema.

`test/run_tool_normalization_checks.py` exercises this concretely: tool-name normalization
(`"Pi3_3D_Tool"`→`pi3_tool`), pi3 argument coercion (`"angle":"(45, 30)"` split into
`azimuth_angle`/`elevation_angle`, string `"camera":"2"`→int), and unknown-key stripping.
`test/test_plugin_all_angles_toolcall_repair.py` targets a **module not present in this
checkout** (`plugin.plugin_all_angles.SPAgentToolCallingScheduler` — no `plugin/` directory
exists anywhere in the repo; see §Training) but documents its intended repair surface, including
converting a hallucinated `image_index` into a real `image_path`.

## 3. Scene Memory and Skill Library

Two skill representations exist: a static seed `Skill(name, when, strategy[], tool_usage)`
dataclass (`skill3d/core/skill.py:8-21`, 8 hardcoded seed skills built by `build_default_skills()`
in `skill3d/core/skill_normalization.py:10-111`), and a richer dynamic **`SkillMemoryUnit`**
(`skill3d/core/skill_retrieval.py:89-123`: `skill_id, source_type, task_pattern, trigger,
strategy[], tool_candidates[], fallbacks[], failure_memory{}, view_policy{}, success_rate,
reward_avg, maturity, retrieval_text, ...`).

Storage is env-configured JSON (README.md:279-310): `SKILL3D_SKILL_STORAGE_PATH` (a monolithic
`learned_skills.json`) and `SKILL3D_HIERARCHICAL_MEMORY_DIR` (a sharded `memory/` tree), managed
by `AdaptiveSkillManager` (`skill3d/core/skill_learning.py:104-219`) across four declared layers
(`rule_memory`, `working_memory`, `episode_memory`, `evidence_memory`, `skill_learning.py:224-246`)
plus a `skill_memory` node holding `trajectories{}` and `skill_management{decisions[],
failure_lessons{}, success_workflows{}}`. The on-disk skeleton at `statics/skill3d_shared/`
(verified via `ls`) tracks only `.gitkeep` placeholders — all generated content
(`learned_skills.json`, `SKILL.md` files, episode/evidence JSONL) is git-ignored and produced at
runtime.

Extraction from rollouts runs through `AdaptiveSkillManager.record_episode()`
(`skill_learning.py:2732-2886`), which computes a normalized reward and routes to one of five
outcomes via `_build_skill_management_decision()` (`skill_learning.py:1992-2066`): merge into an
existing dynamic skill, insert a new dynamic skill, or — on failure — `"attach_failure_lesson_or_
patch_skill"` / `"attach_missing_tool_lesson"`. Successes are persisted keyed by
`sequence_key(question_class, tool_sequence)` into **`success_workflows`**
(`skill_learning.py:2091-2110`: `{question_class, tool_sequence, total, success, coverage,
answer_pattern}`). Failures are persisted into **`failure_lessons`**
(`skill_learning.py:2112-2151`: `{tool_sequence, count, error_types{}, fallback_tools{failed_tool:
next_best_tool}, example_questions[]}`), with `fallback_tools` populated via
`_next_best_tool_for_failure()`. `_refresh_learned_skills()` (`skill_learning.py:3366`)
materializes these into prompt-facing dynamic skill cards (`_build_dynamic_skill_card`,
`skill_learning.py:1112-1176`).

## 4. Skill Retrieval

Retrieval is **not an LLM selector call**. Default `memory_mode="symbolic"`
(`agent.py:61-63`) does pure keyword/rule classification — `_classify_question()` /
`_classify_seed_pattern()` route the question via keyword tables (`skill_learning.py:160-183`)
to a `question_class`, with no embedding search (`_retrieve_prompt_skills` short-circuits to `[]`
in symbolic mode, `skill_learning.py:690-691`).

An opt-in `memory_mode in {"dense","hybrid"}` path does real embedding retrieval:
`SkillRetrievalIndex` (`skill3d/core/skill_retrieval.py:374-472`) embeds via
`SentenceTransformerBackend` wrapping `Qwen/Qwen3-Embedding-0.6B` (env `SKILL3D_RETRIEVAL_MODEL`,
default at `skill_retrieval.py:32`), falling back to a deterministic MD5 hashing-bag-of-words
backend if the model fails to load. Retrieval is cosine similarity over per-unit
`retrieval_text` embeddings, reranked by a hand-weighted linear combination (0.45 semantic +
0.20 class match + 0.15 tool compat + 0.10 success rate + 0.05 reward avg + 0.05 freshness −
0.15 failure penalty, `skill_retrieval.py:691-748`), then diversity-filtered with an MMR-style
greedy pass (`filter_similar_skill_candidates`, `:577-688`).

`test/test_skill3d_selector.py`, despite its name, does **not** test this retrieval mechanism —
it exercises the downstream rule-based tool-call selector/gate from §1 (`_select_tool_calls`,
`_missing_must_try_tools`) given an already-built skill bundle. The actual retrieval-mode logic
(symbolic vs. dense) has no dedicated test coverage.

## 5. Tool Service Contracts

All seven expert services are **Flask** apps (not FastAPI), each bound `host="0.0.0.0"`, none
with authentication or CORS lockdown:

| Tool | Endpoints | Response shape | Metric status |
| --- | --- | --- | --- |
| Depth-Anything-3 (`depth_server.py`) | `/health`, `/test`, `/infer` | depth map + `depth_min/max`, **`depth_units` ("meters"/"relative")**, **`is_metric` bool**, `model_variant`, optional confidence npz | Self-reports metric vs. relative honestly (`_prediction_is_metric`, `depth_server.py:41-45`) |
| SAM3 (`sam3_server.py`) | `/health`, `/infer` | masks + per-mask `score` | N/A (segmentation); one of only two services with a native confidence score |
| GroundingDINO (`grounding_dino_server.py`) | `/health`, `/test`, `/infer`, `/infer_video` | boxes + raw (uncalibrated) detection logit as "confidence" | N/A |
| Pi3 (`pi3_server.py`) | `/health`, `/test`, `/infer` | PLY point cloud + `camera_poses [x,y,z]` | **No `is_metric`/`units` field anywhere** — point cloud/pose scale is never labeled, unlike the depth service |
| SwinIR (`swinir_server.py`) | `/health`, `/test`, `/infer` | enhanced image | N/A (appearance only) |
| Orient-Anything (`orient_anything_server.py`) | `/health`, `/test`, `/infer` | `azimuth/elevation/rotation` (degrees) + softmax `confidence` over discretized angle bins | Code is labeled "v1" throughout despite README calling it "v2" — version/doc mismatch |
| moondream (`md_server.py`) | `/health`, `/test`, `/infer` | caption/query/detect/point | **Not in README's tool table**; proxies every image to Moondream's *cloud* API rather than running locally, a different trust boundary than its six siblings |

**Confirmed metric-honesty bug**: `skill3d/tools/depth_tool.py:418-422` and `:430-434`
unconditionally emit the word "Metric" in the natural-language `description` string ("Metric
point-to-point measurement...", "Metric point queries...") even when `depth_units == "relative"`
and `is_metric == False` (the default when a non-metric/`mono` checkpoint is loaded, per
`depth_server.py:34-38`). The raw numeric values are correctly unit-suffixed elsewhere
(`_format_depth_value`, `depth_tool.py:308-312`, checks `units == "meters"` before appending
`" m"`), and `is_metric`/`depth_units` are present and correct elsewhere in the same response —
but the `description` field, which is the primary channel an LLM agent reads, asserts metric
scale regardless of actual model output. This is the one confirmed instance of a silent
relative→metric claim; Pi3's scale ambiguity (above) is an omission rather than a false claim.

Additional risk items: SAM3's argparse port default (20040) and Pi3's (20021) both disagree with
README's documented ports (20020/20030); `pi3_client.py`'s own hardcoded default
(`localhost:30030`) disagrees with both. SwinIR's `/infer` accepts an unauthenticated,
unvalidated `model_path` field passed straight to `torch.load()` (`swinir_server.py:526`,
`_resolve_weight_path`, `:92-120`) — a local-file-load surface given `torch.load`'s
pickle-deserialization risk. `supervision_tool.py` and `yoloe_tool.py` (`skill3d/tools/`) import
a `skill3d/external_experts/supervision/` module that **does not exist** in the repo — dead code,
mock-only.

## 6. Training

`train/train_sft.sh` and `train/train_grpo.sh` are thin wrappers around **ms-swift**
(`ms_swift` in `requirements.txt:30`), not custom training loops.

- **SFT** (`swift sft`): full fine-tune (`TRAIN_TYPE=full`), `NUM_EPOCHS=1`, `LR=5e-6`,
  `MAX_LENGTH=32768`, ViT frozen (`FREEZE_VIT=true`), DeepSpeed ZeRO-2 by default, registers a
  custom special-token file via `--new_special_tokens` (`train_sft.sh:24,123`).
- **GRPO** (`swift rlhf --rlhf_type grpo`): runs on an SFT checkpoint (`MODEL_PATH` globs a prior
  SFT run, `train_grpo.sh:21-29`), `LR=1e-6`, `NUM_GENERATIONS=8`, `RL_BETA=0.05`. Reward is a
  **weighted sum of three named, external reward functions**:
  `--reward_funcs external_r1v_acc external_agentic_skill3d_format external_skill3d_tool_use
  --reward_weights 0.6 0.2 0.2` (`train_grpo.sh:60-62,147-148`) — i.e. 60% answer correctness,
  20% format compliance, 20% tool-use behavior, by name/weight only. The implementation lives in
  `--external_plugins "${project_dir}/plugin/plugin_all_angles.py"`
  (`train_grpo.sh:144-145`), together with the `spagent_tool_call_scheduler` multi-turn
  scheduler — **this file is absent from the repo** (confirmed via `git ls-files` and `find`; only
  `test/test_plugin_all_angles_toolcall_repair.py` references it). The actual reward computation
  cannot be verified from this checkout.

`train/system_prompt/` ships three files: `system_prompt_skill3d_agentic.txt` (the one actually
wired into `train_grpo.sh`, defining the `<think>/<skill_choice>/<tool_call>/<answer>` grammar and
a closed six-tool set) plus two **unreferenced ablation prompts**,
`system_prompt_grpo_all_angles.txt` (adds `moondream_tool` and a coarse-to-fine viewpoint-search
policy) and `system_prompt_grpo_wotool.txt` (a 2-line no-tool baseline).
`train/special_tokens/skill3d_agentic_spatial.txt` defines the 8 paired tags
(`<think>`, `<tool_call>`, `<skill_choice>`, `<answer>` and closes) registered into the tokenizer
before SFT.

Base models are Qwen3-VL-4B/8B-Instruct, linked only via HF `tree/main` — no commit/revision pin
anywhere. Per the README's own disclaimer, the repo does not ship: base checkpoints, SFT/GRPO
dataset JSONL, generated Scene Memory/Skill Library, running tool-service weights, and —
confirmed missing — `plugin/plugin_all_angles.py`.

## 7. Licensing and Redistribution

Top-level `LICENSE` is verbatim, unmodified Apache-2.0, but the copyright-holder placeholder was
never filled in and no `NOTICE` file exists — a paperwork gap, not a legal blocker.

| Component | License | Commercial use |
| --- | --- | --- |
| `third_party/GroundingDINO`, `third_party/SwinIR` | Apache-2.0 | OK |
| `third_party/Orient-Anything` | CC BY 4.0 | OK (attribution) |
| `third_party/sam3` | **Meta's custom "SAM License"** (gated, export-control/acceptable-use clauses). `pyproject.toml:18` mislabels it `MIT` — **wrong metadata**, controlling doc is the `LICENSE` file | VERIFY / RESTRICTED |
| `skill3d/external_experts/Pi3` (vendored `dinov2/` subtree) | Apache-2.0 code (Meta headers) | OK — code only, see weights below |
| Depth-Anything-3 **DA3METRIC-LARGE** checkpoint (the one README tells users to fetch) | Apache-2.0 | OK |
| Other Depth-Anything-3 variants (GIANT/NESTED/LARGE) | CC BY-NC 4.0 | RESTRICTED if swapped in |
| GroundingDINO SwinB checkpoint | Apache-2.0 | OK |
| **Pi3 `model.safetensors` checkpoint** (README-directed download) | **CC BY-NC 4.0**, non-commercial research/education only per model card | **RESTRICTED — the single clearest redistribution blocker**, since README's own instructions fetch this exact file |
| **SAM3.1 checkpoint** | Meta's custom "SAM License", HF tag `license: other`, gated | VERIFY / RESTRICTED |
| SwinIR checkpoint | Apache-2.0 | OK |
| Orient-Anything v2 checkpoint | CC BY 4.0 | OK (attribution) |
| VSI-Bench, BLINK, CV-Bench datasets | Apache-2.0 (HF tags) | OK, eval-only anyway |
| MMSI-Bench dataset | CC BY 4.0 (HF tag) | OK (attribution), eval-only |
| `lhy-zju/Skill-3D` release (splits, SFT/GRPO files, trained checkpoints) | Apache-2.0, explicit on the dataset card | OK |

Anyone reusing this commercially must swap out the Pi3 checkpoint and clear SAM3.1's licensing
terms; everything else in the documented, README-specified stack is either permissive or
attribution-only.

## 8. Code Quality and Risks

- **Secrets**: none found hardcoded (`grep`-verified, excluding `third_party/`); all key/token
  references are legitimate `os.getenv` lookups (e.g. `skill3d/vllm_models/qwen.py:9`).
  `.gitignore` excludes checkpoints and generated memory JSON but has **no `.env`/`credentials*`
  pattern** — a gap, not a current leak.
- **Error handling**: `agent.py`'s tool-call retry/recovery subsystem
  (`_is_retryable_tool_exception`, `agent.py:921`; `_call_tool_with_recovery`, `:1076`) is
  deliberately engineered — classifies retryable vs. fatal HTTP/network errors, backs off with
  jitter, honors `Retry-After`, returns a structured error result rather than crashing. The Flask
  servers (spot-checked `depth_server.py`, `sam3_server.py`, `md_server.py`) uniformly validate
  payloads (400) and wrap inference in try/except with traceback logging (500) — consistent
  enough across independently-vendored servers to suggest Skill-3D retrofitted this rather than
  inheriting it as-is.
- **Vendored vs. original**: 414 Python files total; **360 (~87%) are vendored ML model code**
  under `third_party/` and `skill3d/external_experts/`; **43 (~10%) are original Skill-3D logic**,
  concentrated in `skill3d/core/` (~10,362 lines across 11 files — agent loop, retry machinery,
  skill learning/retrieval, tool normalization).
- **Test coverage**: only 2 of 6 files under `test/` are real automated `unittest` suites
  (`test_skill3d_selector.py`, `test_plugin_all_angles_toolcall_repair.py` — the latter targets a
  module absent from the repo); 1 is an assertion script not wired into any runner; 3 are
  manual/live-service smoke scripts. No `pytest.ini`/`conftest.py` anywhere. Of `skill3d/core/`'s
  11 files, only 4 are touched at all, and narrowly — `skill_retrieval.py`, `prompts*.py`,
  `data_collector.py`, `skill.py`, `skill_normalization.py` have zero test references despite
  being load-bearing for the memory/skill system in §3–4.
- **Fork divergence**: `git rev-parse HEAD upstream/main origin/main` all resolve to the same
  commit `e379d64` — this clone is a **pure mirror of upstream, zero fork-specific commits**.
  Every finding above describes upstream `skill-3d/Skill-3D` behavior, not weave-logic-ai work.

## 9. Reusable Parts

**Worth lifting cleanly**, given they are original, reasonably self-contained, and not tied to
the benchmark harness:
- The **tool-call normalization/repair chain** (`skill3d/core/tool_normalization.py`) — a
  well-tested, defensive JSON/envelope/name/argument repair pipeline that is broadly applicable
  to any Hermes/ReAct-XML-style tool-calling agent, independent of 3D spatial reasoning.
- The **tool retry/recovery subsystem** in `agent.py` (`_is_retryable_tool_exception`,
  `_call_tool_with_recovery`) — a clean, generic pattern for calling flaky HTTP tool services.
- The **skill/lesson data model** (§3: `SkillMemoryUnit`, `success_workflows`/`failure_lessons`
  keyed by `sequence_key`, `fallback_tools` next-best-tool mapping) as a *design pattern* — the
  concept of turning successful tool sequences into reusable workflows and failed sequences into
  keyed lessons-with-fallbacks is generalizable, even though the concrete schema is tied to
  Skill-3D's question-class taxonomy.
- The **dense retrieval + rerank + MMR-diversity-filter pipeline** in `skill_retrieval.py`, as a
  retrieval design (embedding + hand-weighted rerank + duplicate suppression), reusable for any
  skill/memory library needing top-k selection over heterogeneous, partly-stale entries.

**Benchmark-specific scaffolding, not worth lifting as-is**:
- All vendored `third_party/` and `skill3d/external_experts/` model code (87% of the repo) — this
  is upstream Depth-Anything-3/SAM3/GroundingDINO/Pi3/SwinIR/Orient-Anything source with thin
  Flask wrappers, not Skill-3D's contribution, and carries the licensing constraints in §7.
- The **symbolic classification tables** (`_seed_class_keywords`, `skill_learning.py:160-183`)
  and the 8 hardcoded seed skills (`skill_normalization.py:10-111`) are hand-tuned to the four
  benchmark question taxonomies (VSI-Bench/BLINK/CV-Bench/MMSI-Bench) and would need to be
  rebuilt for a different domain.
- `train/` shell scripts are ms-swift CLI glue specific to this repo's checkpoint/dataset layout
  and reference a missing external plugin — not runnable standalone.
- `examples/evaluation/` and `skill3d/utils/download_*.py` are benchmark-dataset-download/eval
  glue with no reuse value outside this project's evaluation harness.

## 10. Rewrite Inventory (Python → Rust)

Scope: every **original**, non-vendored module under `skill3d/` (core, tools, models,
vllm_models, utils, top-level files), plus `scripts/`, `train/`, `test/`, and
`examples/evaluation/` — 51 Python files, 4 shell scripts, and 6 training-asset files (prompts +
special tokens), ~14,700 lines total. Out of scope: `skill3d/external_experts/*_server.py`/
`*_client.py` (thin but still Skill-3D-authored HTTP glue, ~14 files/~3,500 lines — a rewrite
replaces these with new Rust HTTP clients against the same server contracts documented in §5, not
a line-for-line port) and all of `third_party/`/vendored model internals (§7 licensing applies if
any of that code is kept).

### `skill3d/core/` — the orchestration engine (critical path, highest risk)

| Path | Lines | Responsibility | External deps | Difficulty | Reason |
| --- | --- | --- | --- | --- | --- |
| `skill3d/core/agent.py` | 3,068 | `SPAgent` main loop, tool-call selector/scorer (§1), retry/recovery (§8), format validation | stdlib only (`json`, `re`, `ast`, `asyncio`, `ThreadPoolExecutor`, `logging`) | **Hard** | Largest file, most control-flow branches, every other module's entry point |
| `skill3d/core/skill_learning.py` | 4,186 | `AdaptiveSkillManager`: 4-layer memory, skill-management decisions, dynamic skill card synthesis (§3) | stdlib (`json`, `re`, `hashlib`, `shutil`, `tempfile`, `datetime`) | **Hard** | Largest file overall, deeply stateful, zero test coverage (§8) to pin semantics during a port |
| `skill3d/core/skill_retrieval.py` | 783 | `SkillMemoryUnit`, `SkillRetrievalIndex`, embedding backends, rerank + MMR diversity filter (§4) | `numpy`; optional `sentence-transformers` (Qwen3-Embedding-0.6B) | **Hard** | Needs a Rust embedding path (candle/ONNX port, or an HTTP delegate) plus a faithful port of the hand-weighted rerank formula |
| `skill3d/core/prompts_with_skills.py` | 649 | Skill-augmented system prompt assembly: tool cards, problem card, ranked skill candidates, scene-memory block, skill-choice protocol (§4) | stdlib | **Hard** | Must be string-exact (see preserve-list below), many nested compaction helpers |
| `skill3d/core/tool_normalization.py` | 481 | Tool-call JSON/envelope/name/argument repair chain (§2) | stdlib (`ast.literal_eval`, `re`) | **Hard** | Many hand-tuned edge cases; `ast.literal_eval` has no direct Rust equivalent, needs a bespoke Python-literal-ish parser |
| `skill3d/core/data_collector.py` | 560 | Rollout → SFT/GRPO export (JSONL/ShareGPT formats) | stdlib (`json`, `shutil`, `uuid`) | Moderate | Straightforward I/O + schema mapping, no subtle control flow |
| `skill3d/core/prompts.py` | 214 | Base (no-skill) system prompt | stdlib | Moderate | Must be string-exact, but no branching logic |
| `skill3d/core/tool.py` | 138 | `Tool` ABC + `ToolRegistry` | stdlib | Trivial | Small interface + list/dict registry |
| `skill3d/core/skill_normalization.py` | 115 | 8 hardcoded seed skills | stdlib | Trivial | Static data, direct transcription |
| `skill3d/core/model.py` | 98 | `Model` ABC | stdlib | Trivial | Two-method interface |
| `skill3d/core/skill.py` | 41 | `Skill` dataclass | stdlib | Trivial | 4-field struct |
| `skill3d/core/__init__.py` | 29 | Package exports | — | Trivial | Re-exports only |

### `skill3d/tools/` — per-expert tool wrappers (mock/real switch, image pre/post-processing)

| Path | Lines | Responsibility | External deps | Difficulty | Reason |
| --- | --- | --- | --- | --- | --- |
| `skill3d/tools/depth_tool.py` | 625 | Depth tool wrapper: point/point-pair metric description, unit handling | `cv2`, `numpy` | Moderate | Needs `image`/`ndarray` crate equivalents; contains the confirmed "Metric" bug (§5) — fix, don't port |
| `skill3d/tools/pi3_tool.py` | 571 | Pi3 wrapper: viewpoint args, disk cache of rendered views | stdlib | Moderate | Contains the disk-cache key bug (§5, drops `rotation_reference_camera`/`camera_view`) — fix, don't port |
| `skill3d/tools/moondream_tool.py` | 236 | moondream wrapper, restricted to `task="point"` | stdlib | Moderate | Thin, but talks to a cloud API (§5) — carry the trust-boundary disclosure forward |
| `skill3d/tools/swinir_tool.py` | 238 | SwinIR wrapper, client-side crop_box/crop_margin logic | `PIL` | Moderate | Needs `image` crate for the crop/margin math |
| `skill3d/tools/orient_anything_tool.py` | 218 | Orient-Anything wrapper, visualization overlays | `PIL.ImageDraw` | Moderate | Needs a 2D-draw crate for the overlay |
| `skill3d/tools/segmentation_tool.py` | 197 | SAM3 wrapper | stdlib | Moderate | Straightforward request/response shaping |
| `skill3d/tools/detection_tool.py` | 195 | GroundingDINO wrapper | stdlib | Moderate | Straightforward request/response shaping |
| `skill3d/tools/yoloe_tool.py` | 178 | Wrapper for a non-existent backing server (§5) | stdlib | Trivial | Dead code — drop, don't port |
| `skill3d/tools/supervision_tool.py` | 154 | Wrapper for a non-existent backing server (§5) | stdlib | Trivial | Dead code — drop, don't port |
| `skill3d/tools/__init__.py` | 28 | Package exports | — | Trivial | Re-exports only |

### `skill3d/models/` + `skill3d/vllm_models/` — LLM client wrappers

| Path | Lines | Responsibility | External deps | Difficulty | Reason |
| --- | --- | --- | --- | --- | --- |
| `skill3d/vllm_models/gpt.py` | 391 | OpenAI-compatible client: base64 image encoding, retry logic | `openai` SDK | Moderate | `async-openai` crate covers the wire protocol, but retry/backoff logic needs re-implementing |
| `skill3d/vllm_models/qwen.py` | 224 | Qwen OpenAI-compatible client | `openai` SDK | Moderate | Same pattern as `gpt.py` |
| `skill3d/models/qwen_vllm_model.py` | 172 | `Model`-interface subclass over `qwen_vllm.py` | stdlib | Trivial–Moderate | Thin adapter |
| `skill3d/models/qwen_model.py` | 168 | `Model`-interface subclass over `qwen.py` | stdlib | Trivial–Moderate | Thin adapter |
| `skill3d/models/gpt_model.py` | 161 | `Model`-interface subclass over `gpt.py` | stdlib | Trivial–Moderate | Thin adapter |
| `skill3d/vllm_models/qwen_vllm.py` | 104 | Local vLLM OpenAI-compatible client | `openai` SDK | Trivial | Minimal wrapper, no retry logic |
| `skill3d/models/__init__.py` | 15 | Package exports | — | Trivial | Re-exports only |

### `skill3d/utils/`, root scripts, `test/`, `examples/evaluation/`, `scripts/`, `train/`

| Path | Lines | Responsibility | External deps | Difficulty | Reason |
| --- | --- | --- | --- | --- | --- |
| `skill3d/utils/utils.py` | 515 | VSI-Bench MRA metric, answer normalization, JSON-tag extraction (`parse_json`), box drawing | stdlib, `numpy` | Moderate | Worth porting only if the eval harness is kept; string/regex-heavy |
| `skill3d/utils/generate_angles.py` | 504 | Pi3 batch view pregeneration for training data | `requests`, threading | Moderate | Training-time-only tooling, not agent runtime |
| `skill3d/utils/download_cvbench.py` | 432 | CV-Bench dataset downloader | `pandas`, HF `datasets` | Trivial | Benchmark data-prep glue, off critical path — recommend leaving as Python or dropping |
| `skill3d/utils/download_Omni-Perspective.py` | 359 | Omni-Perspective dataset downloader | `pandas`, HF `datasets` | Trivial | Same as above |
| `skill3d/utils/download_vsibench.py` | 345 | VSI-Bench dataset downloader | `pandas`, HF `datasets` | Trivial | Same as above |
| `skill3d/utils/download_mmsi.py` | 290 | MMSI-Bench dataset downloader | `pandas`, HF `datasets` | Trivial | Same as above |
| `skill3d/utils/download_erqa.py` | 258 | ERQA dataset downloader | `pandas`, HF `datasets` | Trivial | Same as above |
| `skill3d/utils/extract_error_cases.py` | 205 | Pull BLINK entries for incorrect predictions | `pandas`, `argparse` | Trivial | Analysis script, off critical path |
| `skill3d/utils/download_mindcube.py` | 183 | MindCube dataset downloader | `pandas`, HF `datasets` | Trivial | Off critical path |
| `skill3d/utils/download_blink.py` | 172 | BLINK dataset downloader | `pandas`, HF `datasets` | Trivial | Off critical path |
| `skill3d/utils/weave_integration.py` | 171 | Optional Weights & Biases "Weave" tracing hooks | `weave` (optional) | Trivial | Drop or replace with a Rust tracing backend |
| `skill3d/utils/download_vlm4d.py` | 177 | VLM4D dataset downloader | `pandas`, HF `datasets` | Trivial | Off critical path |
| `skill3d/utils/re_export_simple_format.py` | 90 | Re-export collected training data in simple format | stdlib | Trivial | Thin wrapper over `data_collector.py` |
| `skill3d/utils/cvbench_img.py` | 66 | Rebuild PNG folders from CV-Bench parquet | `pandas`, `PIL` | Trivial | Off critical path |
| `skill3d/__init__.py` | 59 | Package exports, legacy `workflows` deprecation shim | stdlib | Trivial | Re-exports only |
| `skill3d/tool_definition_examples.py` | 466 | Example custom-tool definitions (docs) | stdlib | Trivial | Drop or fold into Rust doc examples |
| `skill3d/quick_start.py` | 340 | Demo/example script | stdlib | Trivial | Drop or fold into Rust doc examples |
| `test/test_skill3d_selector.py` | 322 | Selector-gate `unittest` suite (§1, §4) | stdlib | Moderate | Port **first**, ahead of `agent.py` — only executable spec of selector behavior |
| `test/run_tool_normalization_checks.py` | 116 | Assertion-script coverage of tool-name/arg normalization (§2) | stdlib | Moderate | Port as golden-fixture tests before rewriting `tool_normalization.py` |
| `test/test_orient_anything_service.py` | 107 | Manual live-service smoke script | `requests`, `argparse` | Trivial | Not automated; low priority |
| `test/test_pi3_llm.py` | 107 | Manual live-service smoke script | `requests` | Trivial | Not automated; low priority |
| `test/test_swinir_service.py` | 97 | Manual live-service smoke script | `requests`, `argparse` | Trivial | Not automated; low priority |
| `test/test_plugin_all_angles_toolcall_repair.py` | 91 | `unittest` suite for the **missing** `plugin/plugin_all_angles.py` scheduler (§6) | stdlib | Moderate | Documents intended repair surface of an unshipped module; port the assertions, not the missing import |
| `examples/evaluation/skill3d_evaluation.py` | 1,198 | Main VSI/BLINK/CV-Bench/MMSI evaluation driver: multi-worker, tag parsing, metric scoring | stdlib, `numpy` | Hard | Only in scope if the eval harness itself is rewritten; long and stateful |
| `examples/evaluation/evaluate_img_alltools.py` | 263 | Single-image all-tools evaluation entry point | stdlib | Moderate | Smaller driver variant of the above |
| `scripts/run_skill3d_qwen_sft_inference.sh` | 172 | Inference launch script for local vLLM/Qwen checkpoints | shell, `swift`/`vllm` CLIs | Moderate | Reimplement as Rust CLI subcommand + process spawn, or keep as an external launch script |
| `scripts/run_skill3d_gpt54_inference.sh` | 146 | Inference launch script for OpenAI-compatible API models | shell | Moderate | Same pattern as above |
| `scripts/common_env.sh` | 57 | Shared env var defaults (tool ports, memory paths) | shell | Trivial | Direct transcription into Rust config defaults |
| `scripts/vllm_start.sh` | 57 | vLLM server launch wrapper | shell, `vllm` CLI | Trivial | Thin launch wrapper, likely stays a shell/ops script regardless of rewrite |
| `train/train_grpo.sh` | 180 | ms-swift GRPO launch: hyperparameters, reward weights, `SPAGENT_*` knobs (§6) | shell, `ms-swift` CLI | Moderate | CLI glue; the logic worth preserving is the flag/env contract, not the shell itself |
| `train/train_sft.sh` | 145 | ms-swift SFT launch: hyperparameters, special-token registration (§6) | shell, `ms-swift` CLI | Moderate | Same — preserve the flag/env contract |
| `train/system_prompt/system_prompt_skill3d_agentic.txt` | 39 | Live GRPO system prompt (wired into `train_grpo.sh`) | — | Trivial | Plain text asset — copy verbatim, see preserve-list item 1 |
| `train/system_prompt/system_prompt_grpo_all_angles.txt` | 51 | Unreferenced ablation prompt (viewpoint-search variant) | — | Trivial | Copy verbatim if the ablation is kept |
| `train/special_tokens/skill3d_agentic_spatial.txt` | 7 | The 8 special-token strings (§6) | — | Trivial | Copy verbatim, byte-for-byte — see preserve-list item 3 |
| `train/system_prompt/system_prompt_grpo_wotool.txt` | 2 | Unreferenced no-tool ablation prompt | — | Trivial | Copy verbatim if the ablation is kept |

### Behaviors a rewrite must preserve exactly

The checkpoints (Qwen3-VL-4B/8B SFT/GRPO fine-tunes, §6) were trained against specific literal
strings, a specific parser, and specific gating logic; drifting any of the following will
silently degrade tool-use accuracy rather than raise an error, because the model was never shown
the different behavior:

1. **Prompt formats** — the exact text produced by `create_system_prompt()`
   (`skill3d/core/prompts.py:12-79`) and `create_system_prompt_with_skills()`
   (`skill3d/core/prompts_with_skills.py:336+`, including the `_TOOL_CARD_SUMMARIES` /
   `_TOOL_PARAMETER_HINTS` compact tool-card text and the `<skill_candidates>` block format). The
   model was SFT/GRPO-tuned to expect this phrasing and structure token-for-token.
2. **`<tool_call>` grammar** — the `<think>/<skill_choice>/<tool_call>/<answer>` tag protocol
   (`prompts.py:43-61`) and its format-violation checks (`_response_format_errors`,
   `agent.py:2579-2602`). SFT/GRPO training data was produced under this exact grammar.
3. **Normalization/repair rules** — the full `tool_normalization.py` fallback chain (§2):
   `parse_tool_call_block()` (`:15-47`), `normalize_tool_call_payload()` (`:173-245`),
   `normalize_tool_name()` (`:252-324`), `normalize_tool_arguments()` (`:340-481`). Training data
   was filtered through this exact repair logic (`skill3d/core/data_collector.py`), so a Rust
   parser that is stricter or laxer will accept or reject a different distribution of model
   outputs than the checkpoint was tuned against.
4. **Special tokens** — the 8 literal strings in `train/special_tokens/skill3d_agentic_spatial.txt`
   (`<think>`, `</think>`, `<tool_call>`, `</tool_call>`, `<skill_choice>`, `</skill_choice>`,
   `<answer>`, `</answer>`) were registered as atomic tokenizer tokens before SFT
   (`train/train_sft.sh:24,123`). Any Rust-side tokenizer/generation config must treat these as
   single indivisible tokens exactly as spelled, not sub-word sequences.
5. **`learned_skills.json` and `memory/` schemas** — `skill_management.
   success_workflows`/`failure_lessons` (`skill_learning.py:2091-2151`), `trajectories`,
   `skill_memory.skills[]`, and the `statics/skill3d_shared/{memory,progressive_skills}`
   directory layout (`README.md:279-297`). Any existing generated memory, or the 8 static seed
   skill cards, must remain readable/writable in the same shape, or a Rust agent starts cold and
   loses accumulated learning.
6. **Selector rules** — `_validate_tool_call()` (`agent.py:536-627`) and `_score_tool_call()`
   (`agent.py:629-686`), including the prerequisite-tool gating (e.g. `detect_objects_tool` before
   `depth_estimation_tool`), the repeated-`pi3_tool`-view rejection, and the
   `_question_signals()` heuristics (`agent.py:360-439`) that drive scoring. This logic determines
   which model-proposed tool calls ever execute — a laxer or stricter port changes what evidence
   the agent gathers even if the model's raw output is unchanged.
7. **Retry semantics** — `_is_retryable_tool_exception()` (`agent.py:921`) and
   `_call_tool_with_recovery()` (`agent.py:1076`): which HTTP status codes/exception types are
   retried vs. raised immediately, the exponential-backoff-with-jitter and `Retry-After` honoring,
   and the bounded `max_attempts`/`max_total_wait` limits. Reproducing this loosely changes how
   often transient tool-service failures surface as agent-visible errors vs. get silently retried.
8. **Env var contract** — `SKILL3D_SKILL_STORAGE_PATH`, `SKILL3D_HIERARCHICAL_MEMORY_DIR`,
   `SKILL3D_RETRIEVAL_MODEL`, `RETRIEVAL_TOP_K`, and the `SPAGENT_*` knobs consumed at both
   inference time and by `train/train_grpo.sh` (§6) — renaming or dropping these breaks existing
   deployment scripts and the (currently unshipped) GRPO plugin's expectations.
