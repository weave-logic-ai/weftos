# Skill-3D → WeftOS spatial: Rust rewrite plan

**Date:** 2026-09-28
**Status:** Research plan with decisions recorded 2026-09-28 (§9); board tickets filed
**Author role:** world-builder (Urth)
**Subject:** `~/dev/Skill-3D` (weave-logic-ai fork of `skill-3d/Skill-3D`, Apache-2.0, arXiv 2606.07436). The fork is a pure mirror of upstream at `e379d64` ([code-review.md](./code-review.md) §8)
**Scope:** Rewrite Skill-3D's original logic in Rust inside WeftOS crates. Keep Python only where model inference or training has no realistic Rust path yet.
**Paper text license:** The paper is CC BY-NC-SA 4.0 and the repo is Apache-2.0. Reusing code is fine. Do not copy paper text or figures into WeftOS docs; paraphrase and cite.
**Inputs (not duplicated here):**
- [code-review.md](./code-review.md): §7 licensing go/no-go, §10 Rewrite Inventory (per-module lines, deps, difficulty), §10 "Behavior a rewrite must preserve exactly".
- [papers/paper-skill-3d.md](./papers/paper-skill-3d.md) and the per-reference `papers/analysis/NNN-*.md`.
- Cluster syntheses:
  - [experts-and-frontier-models.md](./papers/experts-and-frontier-models.md): per-model Rust feasibility and license matrix. It is authoritative; §3.3 below is a summary.
  - [tool-agents.md](./papers/tool-agents.md): tool-layer design, applied in §3.5.
  - [benchmarks-and-rl.md](./papers/benchmarks-and-rl.md)
  - [skills-memory.md](./papers/skills-memory.md): skill-library design space and recommendations (§2 Skill Library rows).
  - [spatial-vlm-3d-embodied.md](./papers/spatial-vlm-3d-embodied.md): spatial VLMs and agentic 3D baselines; metric-honesty audit (§3) and patterns to reuse (§4).
**Canon:** ADR-008 (WeftOS cloud-side for Mentra), ADR-056, ADR-076 (MCP profiles), ADR-077, ADR-078, ADR-079, ADR-088, ADR-093, ADR-096, [urth-applicability.md](../spatial-intelligence-2026/urth-applicability.md), [dashboard-portfolio-goals-2026-09.md](../../plans/dashboard-portfolio-goals-2026-09.md) (Urth chain R1 → R2 → R3 → OSM/DEM pilot)
**ruv grounding:** The `search_ruvnet` brain tool could not be loaded in this session (ToolSearch was disabled). ruv claims come from local source instead: `~/.cargo/registry/.../ruvector-sona-0.2.1/src/{reasoning_bank,types}.rs`, agentic-flow 3.0.0-alpha.2 `dist/reasoningbank/core/{distill,judge}.js` (weftos `node_modules`), and `~/dev/ruflo/v3/@claude-flow/memory/src/{learning-bridge,rvf-learning-store}.ts`.

## 0. Bottom line

Skill-3D is a **tool-use policy learner**. It learns which perception tool to call, in what order, and when to stop, and it keeps failures as lessons. It is not a world model. In the paper, Scene Memory stores rollouts together with their scene context, tool evidence and failure patterns ([paper-skill-3d.md](./papers/paper-skill-3d.md)). In code it is JSON files plus a hash of tool names (`skill_learning.py:3948`), scene identity is a dataset filename (`pi3_server.py:39`), and the paper's "scene signature" is a **question-type taxonomy, not a geometric scene embedding**. The paper covers indoor scenes only (Limitations, line 722).

About 10k lines of original Python logic sit in `skill3d/core/`; about 87% of the repo is vendored model code (review §8). The rewrite targets the 10k lines. Those lines become Rust in WeftOS. The six expert models are handled one by one: ONNX via `ort` where exports exist, candle where a port exists, and a Python sidecar behind a Rust-defined contract elsewhere. SFT/GRPO training stays in Python. Rust produces the trajectory datasets and consumes the trained weights.

Two changes matter more than the port itself:
1. Scene Memory becomes **Urth regions**: BVH Event and Object leaves keyed by `region/urth/…`.
2. Every tool output carries a **provenance envelope**, so monocular and scale-free geometry can never pass as metric.

## 1. What Skill-3D adds and what WeftOS already has

| Capability | Skill-3D | WeftOS today | Verdict |
|---|---|---|---|
| Bounded ReAct loop (3 turns, answer-first stop, rule-based tool selector/gate, info-gain scoring, retry/backoff) | `SPAgent.solve_problem` (`agent.py:1292`), `_select_tool_calls`, `_call_tool_with_recovery` | clawft agent loop (general-purpose) | **Rewrite** as a spatial specialization; reuse the clawft-llm providers |
| `<tool_call>` normalization and repair (JSON, literal_eval, name aliasing, arg coercion) | `tool_normalization.py` (481 lines; review calls it the best reusable part) | `clawft-llm/src/hermes.rs` extracts `<tool_call>` blocks but has no repair chain | **Port** into `clawft-llm` (it is generic) |
| Success workflows and failure lessons with `fallback_tools` | `success_workflows` / `failure_lessons` (`skill_learning.py:2091-2151`) | `skill_autogen.rs` (repeated tool sequence ≥3 → pending SKILL.md); `ruvector-sona` ReasoningBank weights by `reward.max(0.0)`, so failures carry no weight and no text | **Gap. Rewrite** as a generic outcome ledger. agentic-flow `core/distill.js` (separate success and failure templates) is the reference design |
| Skill retrieval: the paper's pipeline is dense embedding → hand-weighted rerank → MMR filter (top-k 6), with skills injected as cards cited through `<skill_choice>`. Removing retrieval costs **5.8 points**, the largest single ablation (Table 3). The code's default is symbolic; dense is opt-in (review §4) | `skill_retrieval.py:374-748` | ECC HNSW + `SonaSkillReranker` | **Reuse ours**; port the rerank weights, MMR (k = 6) and the card / `<skill_choice>` citation protocol. Run dense retrieval by default |
| Geometry memory across questions | None | BVH + chain + regions + `VectorRef` | **WeftOS supplies this** |
| Metric honesty | DA3 reports `is_metric`/`depth_units`, but `depth_tool.py:418-434` always says "Metric" in the LLM-facing text (confirmed bug). Pi3 has no scale field | Doctrine only. `WmObjectPayload` has `confidence` but no provenance | **Fix by construction** (envelope, §3.2) |
| SFT/GRPO distillation | ms-swift wrappers. Reward plugin `plugin/plugin_all_angles.py` is **missing from the repo** (review §6) | MetaHarness receipts; no VLM fine-tuning | **Stays Python.** Rust owns data out and weights in (§3.4) |

## 2. Mapping

| Skill-3D | WeftOS home |
|---|---|
| Scene Memory, spatial half (**geometric key**) | `SpatialService` region query over BVH: Object leaves plus observation Event leaves, with optional `VectorRef` into `VISUAL_FEATURES` / `LANGUAGE_FEATURES`. Key `(region_id, T_ecef_local, capture_device, camera stats)` replaces the filename and tool hash. Skill-3D has no equivalent: its "scene signature" is a question-type taxonomy |
| Scene Memory, episodic half (**semantic index**) | A separate ECC HNSW index over question text, question class and evidence summaries, pointing at chain-anchored rollout episodes. The geometric key selects *where*; the semantic index selects *which past reasoning applies*. The two are never merged into one key |
| Skill Library | SKILL.md (skills_v2) under `.clawft/skills/spatial/`, created only through the `skill_autogen` pending → approve path (ADR-080). Stats go in frontmatter |
| Lessons | A generic `lesson` record in the outcome ledger: `failed_tool`, `error_type`, `next_best_tool`, `region_class`. Retrieved alongside skills. Never zero-weighted |
| Ledger write ops | Use Memory-R1's ADD / UPDATE / DELETE / NOOP as the ledger's only write vocabulary, kept separate from the task-skill loop ([skills-memory.md](./papers/skills-memory.md) rec. 1, 5) |
| Skill trust | Map Xu & Yan's 4-tier, provenance-linked trust onto the existing governance gate (`skill_autogen` pending → approve, capability tokens); budget for about 26% of external skills being vulnerable (rec. 3) |
| Retrieval at scale | Add a capability-tree index over the flat ECC HNSW before the library passes a few hundred skills; use DAG orchestration for multi-skill tasks, per AgentSkillOS (rec. 4) |
| Curation benchmark | Once R0.4 lands, benchmark SkillOpt's validated add/delete/replace edits against Skill-3D's rule-based promote/merge on the R0.7 held-out set (rec. 7) |
| Tool services | `PerceptionBackend` trait. Implementations are native (`ort`/candle), sidecar HTTP, or replay. Exposed as the kernel `PerceptionService` and as MCP tools (§6) |
| Tool outputs | `SPATIAL_OBSERVATION` Event leaves with `ObservationProvenance`. Objects are created only through the Graph Views F9 gate |
| Closest prior art | RieMind's persistent typed 3D scene graph (tool-agents ref 49) is the nearest published analog to Urth-backed Scene Memory |
| Object references in dialogue | Chat-Scene's durable per-object IDs map onto BVH `LeafId`, which agents cite across turns (spatial-vlm synthesis §4) |
| BVH-backed QA tool | SpatialRGPT's "region query → grounded relative-geometry answer" shape is the template for `spatial_region_query` answers |
| Agent context packing | GPT4Scene-style BEV plus consistent object IDs, once Urth leaf IDs exist; bounded like F10 packs |
| Reward / verdict | Production: measured ground truth (yardstick, tape, stereo, survey) or explicit human confirmation. A benchmark answer key is never a production reward |

## 3. Architecture and crate placement

```
 hosts: Claude Code · Grok · Codex ──MCP──► weft mcp-server --profile …,spatial
                                              │
 clawft-spatial-reasoner (loop, gate, lint, rollout export) ◄─ clawft-llm (providers + hermes repair)
      │ skills/lessons ◄─► clawft-core agent::skill_ledger ─► skills_v2 SKILL.md · ECC HNSW · SONA rerank
      │ perceive
      ▼
 clawft-perception: PerceptionBackend ── Replay | Ort | Candle | Sidecar(HTTP → Python)
      │ ObservationProvenance (weftos-leaf-types)
      ▼
 clawft-kernel PerceptionService → SpatialService (R2) → Event leaves + chain → Graph View → F9 → Object leaves
      ▲
 capture ingest (phone ADR-077 · MentraOS glasses §7) → weftos-perception-types::CaptureFrame
```

### 3.1 Crates

| Crate | New/existing | Contents |
|---|---|---|
| `weftos-leaf-types` | existing | `spatial::observation`: `ObservationProvenance { source_model, weights_sha256, license, metric_basis, scale_source, sigma_m, frame, intrinsics_source, capture_ref }`, `MetricBasis` enum, `SPATIAL_OBSERVATION` tag (additive, ADR-056 §3), optional `provenance` on `WmObjectPayload` |
| `weftos-perception-types` | **new**, serde-only, WASM-safe | Request/response contracts for depth, segment, detect, reconstruct, orient, and restore. Also `CaptureFrame`, `CaptureDevice`, `Intrinsics`, `ImuWindow`, `ConsentState`. This is the Rust-defined sidecar contract |
| `clawft-perception` | **new** | `PerceptionBackend` trait; `ReplayBackend` (recorded fixtures); `SidecarBackend` (reqwest, retry/backoff/Retry-After ported from `agent.py:921-1150`); `OrtBackend` (feature `ort`, workspace `ort 2.0.0-rc.12`); `CandleBackend` (feature `candle`). Envelope enforcement; LLM-facing text generated from the envelope, never hard-coded |
| `clawft-llm` | existing | Extend `hermes.rs` with the normalization/repair chain; port Skill-3D's repair cases as golden tests |
| `clawft-core` | existing | `agent::skill_ledger`: generic success-workflow / lesson ledger keyed by `(question_class, tool_sequence)`, promotion into skills_v2 through the `skill_autogen` pending gate, reranker port (weights + MMR) |
| `clawft-spatial-reasoner` | **new** | Bounded loop, tool selector/gate, scene-task context, Urth question classes (rebuilt, not the benchmark keyword tables), honesty answer-lint, rollout recorder, ms-swift SFT/GRPO JSONL export, `weft spatial score` CLI |
| `clawft-kernel` | existing | `PerceptionService` (ServiceRegistry, `ecc`); capture ingest RPC `capture.ingest`; consent and region-policy checks |
| `clawft-cli` (`mcp_server.rs`) | existing | New ADR-076 profile `spatial` (§6) |
| `clawft-android-edge` | existing | Reuse Ed25519 identity + `pair_store` for glasses and phone capture nodes (Android arm64) |

### 3.2 Honesty contract

- The failure this contract prevents has a textbook example: SpatialVLM (ref 6) builds "metric-space" spatial VQA from monocular RGB with no scale anchor ([spatial-vlm-3d-embodied.md](./papers/spatial-vlm-3d-embodied.md) §3).
- `metric_basis ∈ {survey, stereo, lidar_tof, yardstick, mono_predicted, scale_free, none}`.
- DA3METRIC-LARGE outputs canonical depth; meters require focal length (model card: "multiplying by focal length gives metric depth"). With **predicted** intrinsics the result is `mono_predicted` with a wide `sigma_m`. With calibrated intrinsics it is still `mono_predicted`, but `sigma_m` comes from calibration (R3.4).
- Pi3 is always `scale_free` (the upstream README calls Pi3X metric output "approximate"). It gains a `scale_source` only from a same-session yardstick or stereo pair.
- SwinIR is appearance only. Any downstream output inherits `restored_input: true` and cannot be geometric evidence.
- GroundingDINO, SAM, and Orient produce semantic evidence only; GroundingDINO "confidence" is an uncalibrated logit (review §5).
- Space outside observed frusta stays `unobserved`. F9 promotes to `WM_OBJECT` only with `survey|stereo|lidar_tof|yardstick` evidence, or with two agreeing `mono_predicted` observations plus a calibrated `sigma_m` for that scene class.
- Answer lint: any answer in meters that rests only on `scale_free` evidence fails.

### 3.3 Expert models: Rust inference paths

Workspace today: `ort = "2.0.0-rc.12"` (workspace; used by `clawft-voice-tts`, `clawft-voice-onnx`, `clawft-kernel` optional); `candle-core`/`candle-nn 0.8` (optional in `weftos-worldmodel-impls`); `ndarray 0.16`; `image 0.25` (png only). No `burn` and no `tract`. **Recommendation: `ort` first** (in the workspace, with a CoreML execution provider for the Mac; enabling that ort feature is a spike). Use candle only for DINOv2-family ports. Do not add burn: there are no ports to reuse. The MIT crate [`usls`](https://github.com/jamjamjon/usls) already wraps GroundingDINO, Depth Anything v1–v3, SAM/SAM2/SAM3-Image, DINOv2, and Swin2SR on ONNX Runtime. Use it as a reference for pre- and post-processing; depending on it risks an `ort` version clash.

Summary only; the authoritative matrix is [papers/experts-and-frontier-models.md](./papers/experts-and-frontier-models.md) §1–§4. That synthesis says no SAM 3 ONNX export was found, but the community exports cited below do exist. None is official, and none is verified for SAM3.1 parity, so the recommendation is unchanged: sidecar.

| Model (weights license per review §7) | ONNX today | candle today | Path | Difficulty |
|---|---|---|---|---|
| **DA3METRIC-LARGE** (Apache-2.0). The license varies by checkpoint: Large/Giant/Nested are CC BY-NC, so pin the exact checkpoint tag | Community exports ([MoonCodeMaster](https://github.com/MoonCodeMaster/Depth-Anything-3-Onnx), [onnx-community small](https://huggingface.co/onnx-community/depth-anything-v3-small), [ros2 TRT node](https://github.com/ika-rwth-aachen/ros2-depth-anything-v3-trt)); metric-large export unverified | DA v2 only | `ort` | Easy–medium. First native port |
| **GroundingDINO SwinB** (Apache-2.0) | Community exports ([X-AnyLabeling](https://github.com/CVHub520/X-AnyLabeling/blob/main/tools/onnx_exporter/export_grounding_dino_onnx.py), [hpc203](https://github.com/hpc203/GroundingDINO-onnxrun)); dict-output export pitfalls | none | `ort` + `tokenizers` (BERT text side) | Medium |
| **SAM3.1** (Meta "SAM License", gated; `pyproject.toml` mislabels it MIT) | No official export; community exports ([vietanhdev](https://huggingface.co/vietanhdev/segment-anything-3-onnx-models), [wkentaro/sam3-onnx](https://deepwiki.com/wkentaro/sam3-onnx)) of SAM3, not verified for 3.1 multiplex; encoder ~1.8 GB + text ~1.6 GB | SAM v1 only | **Sidecar, research-only** until the license is cleared. Default segmenter: GroundingDINO boxes → SAM2 via `ort` (license check in R2.2) | Hard (license) |
| **Pi3** (code BSD-3; **weights CC BY-NC 4.0**) | None found | none | **Sidecar, research-only.** About 21 s per 7-frame reconstruction (experts synthesis §4), so not real-time. Default multi-frame geometry stays the splat pipeline (COLMAP / known camera stats); DA3's multi-view mode on an Apache checkpoint is the license-clean substitute to evaluate first | Hard (license + large multi-frame transformer) |
| **SwinIR** (Apache-2.0) | Community exports (unverified parity, `analysis/031-swinir.md`) | none | `ort`, or Swin2SR via the usls pattern. Low priority: appearance only | Easy–medium |
| **Orient-Anything**: the vendored code is **V1** (DINOv2 + MLP) despite the README's "v2" (review §5). V2 is VGGT-based, about 5 GB. **V2 license conflict:** review §7 and the GitHub page footer say CC BY 4.0, but the experts synthesis §2 found no LICENSE file and flags possible inherited NC terms. **Treat as non-commercial until verified** (R2.2) | None found | DINOv2 exists | V1: candle port (DINOv2 backbone + MLP head). V2: sidecar | V1 medium; V2 hard |

The reasoner VLM (Qwen3-VL 4B/8B, Apache-2.0) is **not** ported. It is an LLM provider call through `clawft-llm`: an API, or local llama.cpp through `~/llm`. Whether Qwen3-VL runs under llama.cpp or MLX on the Mac is a spike. vLLM is CUDA-only.

### 3.4 Training boundary

SFT/GRPO **stays Python** (ms-swift, DeepSpeed, vLLM rollouts; the paper used 4× RTX PRO 6000, about 3 h SFT and 28 h RL).

The GRPO reward plugin is referenced but missing from the repo, and the repo's SFT defaults differ from the paper's Table C.4 (learning rate, effective batch size). **The reward spec is therefore the paper's equations**, not the scripts: `R = R_ans + R_fmt + R_tool` with `R_tool = R_exec − |A|/B` (Eq. 1–2, C.1–C.5; summarized in [paper-skill-3d.md](./papers/paper-skill-3d.md)).

We add two terms of our own:
- **Latency/cost term:** the paper prices tool *count* but not tool *cost*, and Pi3 is about 20× slower than the single-frame experts. Our term weights each call by measured latency and compute class, taken from the envelope.
- **Honesty term:** from `weft spatial score`.

**Rust side:**
- `clawft-spatial-reasoner` exports SFT messages JSONL (the `<think>/<skill_choice>/<tool_call>/<answer>` grammar and special tokens from `train/special_tokens/`) and GRPO JSONL from **WeftOS-native** rollouts. Oracle rows are refused unless `--allow-oracle-sft`.
- `weft spatial score` exposes the honesty lint plus tool-efficiency scoring as a CLI. The Python reward plugin calls it, so the honesty term has one implementation.
- Trained weights come back as GGUF (or MLX) artifacts registered in `~/llm`, served via llama.cpp, and consumed through `clawft-llm`. Promotion happens only with a MetaHarness receipt (ADR-096).

### 3.5 Tool-layer design (from [tool-agents.md](./papers/tool-agents.md))

The ADOPT set is TIGeR (ref 20, calibrated metric tool API), RieMind (ref 49, persistent typed 3D scene graph) and HuggingGPT (ref 53, structured JSON task graph). They beat Skill-3D wherever they separate geometry from appearance, because their return types trace back to calibration or ground truth. RieMind's scene graph is the closest prior art to Urth-backed Scene Memory. These rules apply to `weftos-perception-types`, `clawft-perception` and `clawft-spatial-reasoner`:

- **Typed calls by default:** serde structs following HuggingGPT's task record (`task`, `args`, `dep: Vec<TaskId>`), with resource handles (not re-encoded pixels) passed between steps.
- **Code escape hatch:** a narrowly scoped, capability-gated `code_executor` for VADAR-style composition when no fixed tool fits. Off by default, sandboxed, and its outputs are always `scale_free` unless they cite a metric input.
- **Non-coercible units:** `MetricDistance`, `RelativeDepth` and `PixelBox` are separate types with no `From` conversions between them. The cautionary cases are ViperGPT's `compute_depth` and ByDeWay's depth layers (relative depth passed off as distance).
- **Staged verification:** TIGeR-style `Result` chain: format → tool-call validity → parameter checks → execution → unit/provenance check. It wraps the ported repair chain (R0.3).
- **Plan vs execution:** `Plan` and `ExecutionTrace` are distinct types. Rollout export (R0.6) and the lesson ledger read the trace, never the plan.

## 4. Phased tasks

**[S]** = research spike, **[B]** = build item. "Mac" = M5 Max, 128 GB unified memory, no CUDA. VRAM figures are the Skill-3D README estimates: DA3 10–14 GB, SAM3 14–20, GroundingDINO 3–5, Pi3 18–28, SwinIR 2–6, Orient 3–6. Every build item passes `scripts/build.sh test` and `clippy`.

### Phase 0: Rust core, no GPU, no Urth dependencies → Milestone M1

M1: the Rust reasoner answers held-out spatial questions from **recorded** tool outputs, records successes and lessons, retrieves them on the next run, and exports training JSONL. No GPU and no R1/R2 required.

| ID | Task | Kind | Acceptance | Deps | Compute |
|---|---|---|---|---|---|
| R0.1 | `weftos-perception-types` + `ObservationProvenance`/`SPATIAL_OBSERVATION` in `weftos-leaf-types` | B | CBOR/JSON round-trip; old payloads decode; `MetricBasis` required on every response type | none | none |
| R0.2 | Fixture source: check whether the Apache `lhy-zju/Skill-3D` release trajectories carry tool observations; else record one pass of the Python sidecars into replay fixtures | S | ≥200 fixtures with envelopes (Pi3 → `scale_free`); provenance of each fixture noted | R0.1 | none, or one remote GPU pass |
| R0.3 | Port the normalization/repair chain into `clawft-llm::hermes` | B | Skill-3D repair cases pass as Rust golden tests; no regression in existing Hermes tests | none | none |
| R0.4 | `clawft-core::agent::skill_ledger` + reranker port + promotion through `skill_autogen` pending | B | A seeded failure produces a retrievable lesson; success promotes only after approval; failures are not zero-weighted | none | none |
| R0.5 | `clawft-spatial-reasoner` loop + gate + answer lint on `ReplayBackend` | B | Runs a fixture set end to end via an API or local LLM; lint catches meters-from-`scale_free` | R0.1–R0.4 | Mac |
| R0.5c | **Compat mode** for the released Skill-3D 4B/8B checkpoints: prompt strings, the `<think>/<skill_choice>/<tool_call>/<answer>` grammar, the 8 special tokens, and the repair chain preserved byte-for-byte per review §10 "preserve exactly"; one-shot importer from `learned_skills.json` into the ledger | B | Golden prompt snapshots match the Python output; the released 4B checkpoint scores within ±3 pts of Python on the same replay fixtures. Native mode (Urth question classes, envelope-aware prompts) is a separate profile. The `SKILL3D_*` / `SPAGENT_*` env vars are deliberately **not** preserved (Rust config instead) | R0.3, R0.5 | Mac (llama.cpp if Qwen3-VL runs there) or remote |
| R0.6 | Rollout export + `weft spatial score` | B | SFT/GRPO JSONL validates against ms-swift's dataset loader in a Python smoke test; oracle rows refused | R0.5 | none |
| R0.8 | Think3D (ref 93, Skill-3D's primary baseline) scale audit: does its Pi3X point-cloud tool claim metric scale, and on what basis? | S | Written verdict with source lines; if unverified, Think3D-derived loop outputs are tagged `scale_free` in the reasoner | none | none |
| R0.7 | Held-out real set: 30–50 questions with tape or yardstick ground truth, **scene-disjoint** (at least two captured rooms that never appear in skill construction or training). The benchmark splits are disjoint by question, not by scene, so the paper's gains may include scene leakage | B | No held-out scene id occurs in any skill, lesson or training rollout (checked by a test); answers keyed to measured values | camera-stats capture | Mac |

### Phase 1: Urth wiring → Milestone M2 (needs R1 and R2)

M2: perception outputs become Event leaves in a region, and the agent answers a repeat question from BVH memory without re-running perception.

| ID | Task | Kind | Acceptance | Deps | Compute |
|---|---|---|---|---|---|
| R1.1 | `PerceptionService` in `clawft-kernel`; writes `SPATIAL_OBSERVATION` leaves via `SpatialService` | B | Observation visible in a region query with `chain_seq ≠ 0` (ADR-069 direction) | **R1, R2**, R0.1 | Mac |
| R1.2 | Reasoner tools `region.query` / `region.similar` | B | Repeat question cites leaf ids, with no second perception call | **R2**; **R3** for similar | Mac |
| R1.3 | F9 promote rule (§3.2) in Graph View binding | B | A lone Pi3 cloud cannot mint `WM_OBJECT`; yardstick + mono can | R4 (F2/F9), R1.1 | Mac |

### Phase 2: native inference (runs alongside Phase 1)

| ID | Task | Kind | Acceptance | Deps | Compute |
|---|---|---|---|---|---|
| R2.1 | `SidecarBackend` + Python sidecars patched to emit the Rust contract (fork, no upstream push) | B | Contract tests against all six; SwinIR `model_path` → `torch.load` surface removed; bind 127.0.0.1 | R0.1 | Mac / remote |
| R2.2 | License gate: go / no-go per weight file; SAM2 license check as the default segmenter | S | Table merged into code-review.md; research-only models are blocked from default config | review §7 | none |
| R2.3 | `OrtBackend`: DA3METRIC-LARGE (CoreML EP spike) | B | Parity vs sidecar within tolerance on fixtures; p50 latency and memory on the Mac recorded | R2.1 | Mac |
| R2.4 | `OrtBackend`: GroundingDINO SwinB, SAM2, SwinIR/Swin2SR | B | Same parity/latency gates | R2.3 | Mac |
| R2.5 | `CandleBackend`: Orient-Anything V1 | S | Parity or a documented no-go | R2.1 | Mac |
| R2.6 | Pi3, SAM3.1, Orient V2 remain sidecars behind the contract; watch for permissive replacements | S | Documented; research-only flag enforced | R2.2 | remote CUDA |

### Phase 3: MentraOS glasses capture (§7)

| ID | Task | Kind | Acceptance | Deps | Compute |
|---|---|---|---|---|---|
| R3.1 | Intrinsics calibration per resolution mode (640×480, 720p, 1080p, stream) | S | Stored `Intrinsics { source: calibrated }` with reprojection error; nominal-only modes flagged | device | Mac |
| R3.2 | Capture transport spike: direct on-glasses daemon (camera REST `:8089` + IMU) vs MentraOS cloud `AppSession` (`requestPhoto` / managed stream) | S | Measured end-to-end latency and clock-offset error for both; recommendation recorded | R0.1 | Mac + glasses |
| R3.3 | `capture.ingest` RPC: `CaptureFrame` with device identity (Ed25519 via `clawft-android-edge`), device/receive timestamps, intrinsics, IMU window, `ConsentState` | B | Frames without consent state or region policy are rejected; raw pixels are never in the chain (hashes only) | R3.1, R3.2, R1.1 | Mac |
| R3.4 | Egocentric pilot: walk-through → multi-frame reconstruction (sidecar Pi3 research-only, or splat pipeline) + DA3 on calibrated intrinsics; calibration of `sigma_m` vs tape | S | Error table with n and CI per scene class; IMU gyro-gated blur rejection reduces reprojection error | R3.3, R2.3 | mixed |
| R3.6 | Keyframe triage before Urth ingest (SpatialPrompting: VL-similarity + pose spread + sharpness, plus gyro-gated blur rejection) | B | Measured reduction in frames ingested at equal or better reconstruction error on R3.4 sessions | R3.3 | Mac |
| R3.7 | Candidate: "look here next" capture guidance for MentraOS wearers (CoV coarse-to-fine viewpoint selection + SpatialPrompting); prompts target `unobserved` or low-evidence regions via the HUD | S | Guided sessions reach the same region coverage with fewer frames than unguided ones; prompts never claim geometry | R3.6, R1.2 | Mac + glasses |
| R3.5 | Bystander privacy: person/face redaction before persistence; region `capture_policy` | B | Redaction runs before any store; shared-space capture blocked without recorded consent | R3.3, R2.4 | Mac |

### Phase 4: agent-host surface (§6; P4.1–P4.2 can start after R0.4)

| ID | Task | Kind | Acceptance | Deps | Compute |
|---|---|---|---|---|---|
| R4.1 | MCP profile `spatial` in `weft mcp-server` | B | `tools/list` under `--profile spatial` matches the catalog rows; default profile unchanged | R0.4 (skill tools), R1.2 (region tools) | none |
| R4.2 | Canonical wrappers (skills, agents) + sync to Claude and Codex | B | The grok-claude-sync report shows no spatial drift; Codex TOML generated | R4.1 | none |
| R4.3 | Rollout hooks per host | B | A real session's spatial tool calls land as an episode with `verdict: pending`; the verdict is set only by a measured value or user confirmation | R4.1 | none |
| R4.4 | Host parity test | S | The same scripted question on all three hosts yields the same tool calls and episode shape, or documented differences | R4.2, R4.3 | none |

### Phase 5: pilot region and distillation

| ID | Task | Kind | Acceptance | Deps | Compute |
|---|---|---|---|---|---|
| R5.1 | Pilot: R0.7 questions on the licensed OSM/DEM pilot site with captured L4 rooms (phone + glasses); compare no-skill, skill, and skill + region memory | S | MetaHarness receipt with CIs; zero honesty-lint violations | **R1–R3 + pilot**, R3.4 | mixed |
| R5.2 | Distillation (Python): SFT → GRPO on exported WeftOS rollouts; reward calls `weft spatial score` | S | Beats the prompt-only teacher on R0.7 with no rise in violations | R5.1, R0.6 | remote, 4× 96 GB class, about 31 h |
| R5.3 | Serve distilled weights via `~/llm` + `clawft-llm` | S | Runs on the Mac; promotion only through ADR-096 | R5.2 | Mac |

Critical path: R0.1 → R0.4/R0.5 (**M1**) → R1/R2 (Urth chain) → R1.1/R1.2 (**M2**) → R3.3/R3.4 → R5.1.

## 5. Risks

| Risk | Detail | Mitigation |
|---|---|---|
| Licensing | Pi3 weights are CC BY-NC 4.0 ("the single clearest redistribution blocker", review §7). SAM3.1 is under the custom SAM License, gated, and mislabeled MIT in `pyproject.toml`. Non-metric DA3 variants are CC BY-NC. Orient V1 is CC BY 4.0. Orient V2 has conflicting reports (CC BY 4.0 per the review, but no LICENSE file per the experts synthesis), so treat it as non-commercial until verified. DA3METRIC-LARGE, GroundingDINO, SwinIR, datasets, and the `lhy-zju/Skill-3D` release are Apache/CC-BY | R2.2 gate; `license` in every envelope; research-only models blocked by default; weights are never vendored |
| Compute | All six sidecars need about 50–80 GB peak. On the Mac, SAM3 and Pi3 MPS support is unverified and vLLM does not run | Native `ort` for the four permissive models; remote CUDA for sidecars and training; lazy loading; per-question tool budget |
| Rewrite cost | The 3k-line agent and 4k-line skill-learning files encode many heuristics without tests (review §8: 2 real test suites) | Port behavior from fixtures, not line by line; M1 parity against recorded Python rollouts; per-module sizing in review §10. The released checkpoints are tied to exact prompt strings and the parser, hence the separate compat mode (R0.5c) |
| Benchmark overfitting | Keyword tables and seed skills are tuned to VSI/BLINK/CV/MMSI phrasing; oracle mode injects answers; reported gains are indoor-only | R0.7 real held-out set is the acceptance metric; Urth question classes rebuilt; oracle rows quarantined |
| Honest-geometry violations | "Metric" text bug; Pi3 scale omission; predicted intrinsics; SwinIR hallucination; egocentric monocular capture | §3.2 envelope + lint + F9 rule; calibrated intrinsics (R3.1); `sigma_m` from measurement (R3.4) |
| Privacy | Glasses capture bystanders in shared spaces | R3.5 redaction before persistence, consent state, region policy, capture indicator |
| Host drift | Three hosts, three config formats | One canonical source + sync (§6.3); parity test R4.4 |

## 6. Agent-host integration surface

### 6.1 Host-neutral core: the WeftOS MCP server

`weft mcp-server` is already the tool surface for Grok (`.grok/config.toml` `[mcp_servers.weftos]`, ADR-075/076). ADR-076 profiles exist today: `control`, `workspace`, `media`, `default`, `full`. Add a **`spatial`** profile with these tools:

- `spatial_region_query` — BVH query over a region; returns leaves with `metric_basis`.
- `spatial_region_similar` — R3.
- `spatial_observe` — runs perception; governed write of Event leaves.
- `spatial_skill_retrieve` / `spatial_skill_record` — the Skill-3D loop. Records land as pending.
- `spatial_verdict` — attaches a measured value or user confirmation to an episode.
- `spatial_capture_list` — capture sessions with consent state.

Per host:
- **Claude Code:** `.mcp.json` lists only `claude-flow` today. Add `weftos` with `weft mcp-server --profile default,spatial` (`weft` on PATH; no absolute paths, per WEFT-684).
- **Grok:** extend the existing `args`.
- **Codex:** `.codex/config.toml` `[mcp_servers.weftos]` (the table shape is documented in `docs/research/team-bus-codex-weftos-hosts.md` §1.3).

### 6.2 Thin per-host wrappers

| Artifact | Purpose |
|---|---|
| Skill `spatial-reasoning` | Honest-geometry rules (§3.2), the retrieve → act → record → verdict loop, and when to answer "unobserved" |
| Skill `urth-capture` | Phone and glasses capture with consent, calibration mode, and region anchoring |
| Agent `spatial-reasoner` | Answers spatial questions only through the `spatial_*` tools; cites leaf ids and `metric_basis` |
| Agent `scene-curator` | Reviews pending skills and lessons and F9 candidates; `world-builder` keeps doctrine ownership |
| Hooks | `PostToolUse` on `spatial_*` → append a rollout step; `Stop`/`SubagentStop` → close the episode as `verdict: pending`. Hooks record only; they never judge success |

Hook support per host (from repo docs):

| Host | Hooks | Source |
|---|---|---|
| Claude Code | Full: `PreToolUse`, `PostToolUse`, `SessionStart/End`, `Stop`, `SubagentStop`, `UserPromptSubmit`, `PreCompact` wired in `.claude/settings.json` via `.claude/helpers/hook-handler.cjs` | repo |
| Codex | `.codex/hooks.json` uses the Claude-shaped schema; events include `PreToolUse`, `PostToolUse`, `SubagentStart/Stop`, `Stop`. Each hook is trust-hashed. The payload schema is **undocumented** (openai/codex#21990) | `team-bus-codex-weftos-hosts.md` §1.2 |
| Grok | `.grok/hooks/ruflo-team.json` wires `SubagentStop` only. Whether Grok fires `PostToolUse` for MCP tools, and with what matcher syntax, is **unknown, verify** (R4.3). Folder trust is required (`docs/grok/README.md`) | repo |

Agent formats differ:
- Claude: `.claude/agents/*.md` with `name`, `description`, optional `tools`.
- Grok: `.grok/agents/*.md` adds `prompt_mode`, `permission_mode`, and `agents_md`.
- Codex: `.codex/agents/*.toml` (`name`, instructions, `model_reasoning_effort`, `sandbox_mode`, `mcp_servers`, `skills.config`; e.g. `.codex/agents/world-builder.toml`).
- Codex skill-file support beyond `skills.config`: **verify**.

### 6.3 Single source of truth

Author canonically in `.grok/skills/spatial-*` and `.grok/agents/{spatial-reasoner,scene-curator}.md`. `world-builder` already lives in `.grok/agents`, and grok → claude is the helper's default direction. Mirror with `node ~/.claude/helpers/grok-claude-sync.cjs apply`: skills copy mechanically, and agents need frontmatter adaptation per the skill's Step 2. **Extend that helper** with a `--codex` target that renders `.codex/agents/*.toml` and an AGENTS.md pointer, rather than building a third mechanism. The helper lives in `~/.claude/helpers/` (global, outside this repo), so the change needs the user's sign-off. Add a repo drift check (`scripts/`) that fails when the rendered Claude or Codex copies do not match the canonical source.

## 7. MentraOS glasses as a capture source

**Grounding:** `~/dev/mentra/MENTRA_LIVE_DEVICE_PROFILE.md`, `docs/COGNITIVE_EDGE_ARCHITECTURE.md`, `~/.claude/agents/mentraos/{mentraos-app-developer,mentraos-asg-developer}.md`, `~/.claude/skills/mentraos-{app,device}`, ADR-008.

**What the device captures:**
- **Device:** Mentra Live, Android 11, MT6761.
- **Camera:** Camera2 via `CameraNeo`, and an on-device camera REST server on `:8089` (verified healthy). SDK `requestPhoto` sizes are `small` 640×480, `medium` 720p, `large` 1080p, and `full`. RTMP streaming defaults to 1920×1080 at a configurable bitrate; managed cloud streaming is 720p (HLS/DASH/WebRTC). Frame rate is not stated in the local docs: **verify**. `ro.mtk_cam_stereo_camera_support=1` is a vendor flag, not evidence of a second camera, so treat the device as **monocular**. Intrinsics are not documented, so calibrate (R3.1).
- **IMU:** accelerometer 50–200 Hz, gyroscope 10–400 Hz, game rotation vector and gravity up to 200 Hz. The ASG protocol has `ImuStreamConfig` and `HeadGestureConfig`.
- **Missing sensors:** no magnetometer, no GPS. Yaw drifts, and there is no geo anchor.
- **Location:** the SDK's `VPS_COORDINATES` (lat/lon) source and accuracy are unknown: **verify**.
- **Audio** exists but is out of scope here.

**Latency paths:**
- **(a) MentraOS cloud:** glasses → phone → Mentra cloud → `AppSession` WebSocket / `/photo-upload` webhook → a TypeScript app server (the SDK is TS) → `capture.ingest`. Frames transit Mentra's cloud.
- **(b) Direct:** a Rust arm64 daemon on the glasses (per the cognitive-daemon plan) reads `:8089` and the IMU and sends over LAN WebSocket, about 1–5 ms per the device profile, with a WiFi power-save sawtooth unless a WiFi lock is held.

WeftOS itself never runs on the glasses (ADR-008).

**Contract:** `CaptureFrame { device: CaptureDevice { kind: MentraLive, node_id (Ed25519), asg_client_version, mode }, t_device_ns, t_received_ns, clock_offset_ns ± err, image_hash + blob_ref, resolution, intrinsics: Option<Intrinsics { source: calibrated | nominal | predicted }>, imu_window (±50 ms), gravity, region_hint, consent: ConsentState }`.

For which experts a single head-mounted RGB camera needs, see [experts-and-frontier-models.md §4](./papers/experts-and-frontier-models.md). The **minimum license-clean glasses stack is GroundingDINO + DA3METRIC-LARGE**: both Apache-2.0, both on an `ort` path, and depth is metric only when intrinsics are calibrated. Pi3, at about 21 s per 7 frames, is offline-only for glasses sessions.

**Good uses:**
- Multi-frame reconstruction as the wearer moves (always `scale_free` without a scale source).
- Appearance order and route timelines. The egocentric sequence is the evidence.
- Gravity-aligned floor and wall hypotheses from the IMU.
- Gyro-gated frame selection that drops motion-blurred frames.

**Honesty limits:**
- No metric scale from a single glasses camera: DA3 on calibrated intrinsics is `mono_predicted` at best.
- Motion blur and rolling shutter.
- IMU double-integration gives no usable translation scale.
- No geo anchor, so glasses sessions must attach to a surveyed or phone-anchored site region.

**Privacy and consent:**
- Every frame carries `ConsentState`.
- Regions carry a `capture_policy`. Private spaces the owner controls are allowed. Shared spaces require recorded consent or are blocked.
- The shutter sound (`sound: true`) and LED indicator stay on during capture.
- Persons and faces are redacted before persistence. People are anonymous tracks, never identities.
- Raw frames live under capability ACLs with retention limits; the chain stores hashes only.
- Path (a) sends frames through a third-party cloud, which is part of the consent notice.

## 8. Open decisions (recommendations)

1. **Rust inference runtime.** Recommend `ort` as primary (already in the workspace, with the CoreML EP), candle only for DINOv2-family ports, no burn. Model-by-model order: DA3-metric → GroundingDINO → SAM2 → SwinIR → Orient V1.
2. **Pi3 and SAM3.1.** Recommend research-only sidecars, off by default (non-commercial and custom-license weights). Default geometry stays the splat pipeline with known camera stats; the default segmenter is GroundingDINO + SAM2, pending the R2.2 license check.
3. **Glasses transport.** Recommend the direct on-glasses Rust daemon over LAN for the pilot: lower latency, and frames stay on our network. Use the MentraOS cloud `AppSession` path only for sessions whose consent notice covers the third-party cloud.
4. **Canonical source for host wrappers.** Recommend authoring in `.grok/` and extending `grok-claude-sync` with a Codex target (needs sign-off, since the helper is global), instead of a new generator.
5. **Monocular promotion and distillation timing.** Recommend no monocular-only promotion to Object leaves (§3.2), and no distillation spend until R5.1 shows lift on our own held-out set.

## 9. Decisions recorded 2026-09-28

These supersede the recommendations in §8 and the rows they name.

1. **Model runtime: `~/llm` owns the models.** The Rust service does not embed its own vision
   runtime. It calls `~/llm`'s served models through their contracts and follows its rules:
   weights on `/Volumes/ai-models`, one heavy model resident at a time, fixed ports, and `bin/pull`,
   `bin/modelstore` and `bin/monitor`. The eikon pattern (`~/llm/eikon`, `bin/eikon`) is the model:
   Apple Vision first, one Qwen3-VL call per batch on `:8093`, and specialists only on request. This
   replaces R2.3–R2.5 (the `OrtBackend`/`CandleBackend` rows).
   **Segmentation is SAM 3.1, owned by `~/llm`.** `mlx-community/sam3.1-bf16` is already in
   `~/llm/docs/models/registry/image-embed.yaml` (~3.5 GB). The missing piece is a runner: a thin
   `bin/` wrapper over the installed mlx-vlm `Sam3Predictor.predict` (`mlx_vlm/models/sam3/generate.py`),
   like `bin/whisper`, returning masks, boxes, scores and ids. Eikon's segment skill calls that
   wrapper. That is `~/llm`'s work; WeftOS consumes the masks and does not grow its own SAM stack.
   **Grounding DINO** stays in the catalog as the permissive fallback for the day the SAM license
   blocks something. It is not built, and it gets no Eikon stage.
   **Metric depth** (DA3METRIC-LARGE) is still not served by `~/llm`; it remains a request there.
2. **Pi3** is research-only and off by default (non-commercial weights; R2.6). **SAM 3.1** is the
   segmenter, served by `~/llm` under Meta's SAM License; R2.2 still records its terms, and Grounding
   DINO is the documented fallback if those terms ever block a use.
3. **MentraOS capture runs on the WeftOS substrate as streams.** The glasses are a WeftOS node with
   their own Ed25519 identity (ADR-025, ADR-077 edge-node model). They publish signed values to
   per-node sensor paths following the journaled-sensor contract (`.planning/sensors/`,
   `clawft-substrate/src/sensor_paths.rs`): `substrate/<node-id>/sensor/camera/summary` beside
   frame and `sensor/imu/*` sibling paths. Camera topics are `Sensitivity::Capture`, so they need a
   per-goal ADR-012 `CapabilityGrant`, which is where bystander consent is enforced. Streams use
   `BufferPolicy::DropOldest` with a bounded window, are read under the ADR-057 ACL, and are joined
   by node `tick`. Open point: whether frame bytes ride in the substrate value or as a content-addressed
   reference with bytes over the ADR-077 QUIC chunk stream. Check the substrate size limits and
   the splat pipeline first. This replaces R3.2's transport choice and reshapes R3.3 into a substrate
   adapter.
4. **Host wrappers:** authored in weftos and rendered per host by `weftos init --claude|--grok|--codex`
   (agent directory ADR, D1/D2). This replaces R4.2's grok-claude-sync step.
5. **Metric scale comes from sensor fusion.** No single-camera estimate is promoted to a metric object
   on its own. The direction is fusion: IMU, multi-view geometry, calibrated intrinsics, depth sensors
   where present, and known references. R1.3's promote rule is the fusion gate.
6. **Training is on hold.** R5.2 and R5.3 wait. Every place that would need training data, a reward
   signal or fine-tuning goes into the standing "Skill-3D training needs" ticket instead of being built:
   the R0.6 rollout export, the R0.5c compat checkpoints, the R4.3 episode verdicts, and reward shaping
   such as the tool-cost term (§3.4) and a perception-consistency term.
