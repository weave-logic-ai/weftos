# arXiv:2512.24601 "Recursive Language Models" — Symposium Synthesis

**Authors:** four-specialist panel synthesis (paper-summary / weftos-mapping / adoption-candidates / gaps-and-risks)
**Date:** 2026-04-23
**Status:** **Option D selected (2026-04-23)** with cross-cutting channel-generality constraint (§4.5); Q1–Q8 in §5 still need user answers before implementation begins
**Siblings:** `01-paper-summary.md` · `02-weftos-mapping.md` · `03-adoption-candidates.md` · `04-gaps-and-risks.md`

---

## TL;DR

Zhang, Kraska, Khattab (MIT CSAIL, Jan 2026) propose keeping the prompt *out* of the LLM's context window and exposing it as a variable inside a REPL the model drives with code plus recursive sub-calls; they report 91.3% vs. 0.0% on 6M-token BrowseComp-Plus and +28–58 point gains on information-dense tasks. The pattern maps cleanly onto WeftOS's substrate (paths as handles) and tiered router (root/sub-call split), **but** naively copying Algorithm 1 as a Python REPL at the root of the agent loop bypasses the `GateBackend::check()` capability gate, flattens three governance classes into one string variable, and re-introduces the chain-flood the ExoChain channel-split redesign exists to prevent. **Adopt the pattern, reject the implementation.** Ship the two low-risk ideas (metadata-only tool history; substrate paths as prompt handles) before the two high-risk ones (a REPL, recursive sub-calls); decline the fine-tune track entirely for now.

---

## 1. Paper in one page

**What it is.** An inference-time scaffold around any base LLM `M` with context size `K`. Given prompt `P` where `|P| >> K`, the scaffold binds `P` as a variable inside a persistent Python REPL, registers an `llm_query(...)` function that calls a smaller sub-LM, and runs a loop: (1) feed `M` only metadata about `P` (length, short prefix, stdout from the last turn); (2) let `M` emit Python that slices / filters / chunks `P` and optionally fires `llm_query` inside loops or list comprehensions; (3) execute the code in the REPL, persist variables; (4) append only constant-size stdout metadata to `M`'s history; (5) terminate when `M` emits a `FINAL(x)` or `FINAL_VAR(name)` sentinel.

**Three design choices** (paper §2) distinguish it from a CodeAct-style agent: the prompt never enters `M`'s context; the answer returns through a REPL variable (so output isn't output-token-bounded); and sub-calls are *symbolic* — invoked inside code over programmatic slices, not verbalized as ReAct steps.

**Headline results** (Table 1, §4):

| Benchmark | Length / complexity | Baseline (GPT-5) | RLM(GPT-5) |
|---|---|---|---|
| S-NIAH (RULER) | 2¹³–2¹⁸ tok, O(1) | degrades past 2¹⁴ | stays high |
| CodeQA (LongBench-v2) | 23K–4.2M tok, multi-file | 24.0% | **62.0%** |
| BrowseComp-Plus 1K docs | 6M–11M tok, multi-hop | 0.0% (OOC) | **91.3%** |
| OOLONG trec_coarse | 131K tok, O(N) | 44.0% | 56.5% |
| OOLONG-Pairs | 32K tok, O(N²) F1 | 0.1% | **58.0%** |

Median cost per query is *lower* than base GPT-5 (no prompt-ingest tax) but the 95th percentile is a heavy tail. Ablating sub-calls (REPL-only) keeps the scaling wins on S-NIAH / CodeQA / BrowseComp-Plus but loses 10–59 points on OOLONG / OOLONG-Pairs — so "REPL handles scale, sub-calls handle density."

**What the paper does not address.** No threat model for `exec()`-ing model-generated code over adversarial input; no sandbox spec; no replay semantics; no cost-tail cancellation; no multi-tenant consistency; depth is capped at 1 and never tested deeper; the `FINAL` sentinel is brittle (~16% / ~13% training-turn errors); short-prompt regime slightly loses to the base model (break-even length unstated).

---

## 2. Where the panel converged

All four specialists independently landed on the same three observations:

- **The substrate is already the "prompt variable."** Paths in `clawft-substrate` (`weft://…`) plus `exo-resource-tree`'s HNSW index are structurally what the paper reinvents as `context: str` + regex. We have better than the paper's primitive and it is already structured, hashable, and checkpointable (mapping §2.1, candidates #2, gaps §2.1).
- **The tiered router is already the root/sub-call split.** ADR-026's Booster / Haiku / Sonnet ladder and `pipeline::tiered_router.rs` encode the same cost-concentration move the paper makes between GPT-5 root and GPT-5-mini sub (mapping §1, candidates #1, §2).
- **Metadata-only tool history is a cheap, independent win.** The paper's §2 fn.1 discipline ("only constant-size metadata about stdout re-enters history") is ~100 lines in `agent/loop_core.rs` and directly addresses the Hong-et-al. context-rot failure mode we hit today (candidates #4 — flagged by everyone).

Where they diverged:

| Axis | 01 summary | 02 mapping | 03 candidates | 04 gaps | Resolution |
|---|---|---|---|---|---|
| Is the REPL adoptable? | describes it | "new runtime leaf" | "use WASI-WASM, skip Python" | "prompt=data=code is policy regression" | **No Python REPL in-tree.** Substrate+projections is the REPL analog; if an execution surface is needed it is WASI-WASM, not CPython. |
| Effort for a minimum RLM scaffold | n/a | n/a | **10–14 engineer-days** | "every `llm_query` needs gate wrapping; cancels the speed argument" | **The 10–14 day number is wrong if it excludes gating and isolation.** Real floor is ~4–5 weeks (see §3). |
| Sub-calls worth it? | +10–59 points on dense tasks | "sub-call target is `TieredRouter::route()`" | "3–5 days after #1 lands" | "governance bypass, chain flood, cost-tail = deadline-miss" | **Yes, but only with per-sub-call `GateBackend::check()` and a per-trajectory budget.** Budget caps are new work, not bundled in the 3–5 day figure. |
| Fine-tune a native RLM? | Qwen3-8B +28.3% at $200–$500 compute | "no training infra in-tree" | "#5, speculative-adjacent, only after #1–#4" | "collides with tiered-router model story" | **Decline for now.** Revisit once there are >1000 real RLM trajectories to distill from. |

---

## 3. The coherent adoption story

**One path, phased:** adopt the *pattern* (prompt-as-handle, metadata-only history, optional bounded recursion) on top of the primitives WeftOS already has; do **not** adopt the *implementation* (persistent Python REPL with unrestricted `llm_query`).

**Resolving the cross-panel tension.** The adoption-candidates specialist estimated **10–14 engineer-days** for candidate #1 ("RLM scaffold around `delegate_task` / auto-delegation"). The gaps-and-risks specialist argued no adoption works without runtime isolation, per-sub-call capability gating, per-trajectory cost-tail cancellation, and a decision about how trajectories interact with the four-channel ExoChain split. The two are not compatible.

**We rule for gaps-and-risks on this.** The 10–14 day figure covers the Rust port of the REPL loop + `FINAL`/`FINAL_VAR` parser + one integration test. It *excludes*:

- wrapping `llm_query` as a gated tool so sub-calls pass through `GateBackend::check()` (otherwise the root LLM can issue `llm_query(substrate["/secrets/..."])` and evade governance — gaps §3.1);
- per-trajectory cost + depth + fanout caps with a clean cancellation path that does not starve the 1 ms ECC tick (gaps §3.3, §3.9);
- routing sub-call events to Stream or a new `reasoning` channel with window-anchoring rather than one chain event per sub-call (gaps §3.2);
- a decision on whether the "REPL" is a WASI-WASM module, a Rust-native symbolic-recursion harness, or just the substrate + projections (candidates "would not ship" + gaps §3.7).

With those four items the real critical-path number for a safely-shippable RLM-flavoured loop is the candidates specialist's own **4–5 engineer-week** aggregate for #1 + #2 + #3 + #4 — which is consistent once the gating, budgeting, channel routing, and runtime-surface decisions are made. The correct re-reading of the adoption-candidates doc is "the cheap pieces are #4 and #2; the expensive piece labelled '10–14 days' is actually ~3 weeks once the gate, budget, and channel work is counted in."

**The recommended path.** Ship **#4 (metadata-only history)** and **#2 (substrate paths as prompt handles)** first — together they are ~1.5 weeks, touch no new runtime, deliver most of the paper's "scale" wins (paper §4 Obs. 2 confirms the REPL-only ablation keeps the scaling benefit), and are independently useful to the Explorer panel MVP. Defer **#1 (RLM loop) + #3 (bounded recursion)** to a second phase gated on user answers to §5. **Decline #5 (native fine-tune)** for now.

---

## 4. Decision matrix

Five concrete options. Pick one.

| # | Option | Effort | Value captured vs. paper | New risk surface | Open questions it still leaves |
|---|---|---|---|---|---|
| **A** | **Decline.** No adoption. | 0 | 0% | none | none — but current context-rot pain on >200KB reads persists |
| **B** | **#4 only — metadata-only history in `loop_core.rs`.** Tool outputs >4KB stored as handles, history gets `{tool, handle, bytes, sha256, preview[256]}`. | 3–4 days | ~25%. Buys ~3–5× effective context on long tool chains. No recursion, no REPL. | Tiny — one flag, one ToolResult variant. Reuses existing session store. | Q3, Q6 |
| **C** | **B + #2 — substrate paths as prompt handles.** New `substrate.describe` / `substrate.project` verbs; new `tree.peek` / `tree.search` / `tree.slice` tools; root model sees handles, not content. | 1.5–2 weeks (B + 5–7 days) | ~55%. Covers the REPL-only ablation regime from paper §4 Obs. 2 — which keeps the scaling wins on S-NIAH / CodeQA / BrowseComp-Plus. No `llm_query`, no recursion, no new runtime. | Low. Extends existing JSON-RPC surface; all reads go through existing capability checks. | Q1, Q3, Q5 |
| **D** ✅ **SELECTED 2026-04-23** | **C + bounded recursive sub-calls.** `llm_query` wrapped as a gated tool at `clawft-kernel/src/chain.rs`; per-trajectory budget `{max_depth:1, max_subcalls:32, token_cap:200K}`; sub-call events land on Stream via a new `RollingWindowAnchor` aggregator (reuse the Phase 2 mechanism from the exochain-logging synthesis); no Python. Rust-native loop driver in `crates/clawft-core/src/agent/rlm.rs`. | 4–5 weeks (C + 2.5–3 weeks) | ~85%. Also captures the OOLONG-Pairs regime (symbolic recursion; paper's 0.1% → 58.0% F1 jump). Leaves out the paper's "unbounded Ω(|P|²) fanout" which we do not want anyway. | Medium. New trajectory state to replicate (or explicitly not) across Fabric; cost-tail cancellation semantics; prompt-injection surface on `llm_query` inputs. | Q2, Q4, Q7 |
| **E** | **Full RLM with a Python / WASI-WASM REPL at the agent-loop root.** Faithful to paper §2. | 8–12+ weeks | ~95% (+ the fanout we explicitly don't want) | High. New runtime leaf; `exec()` over adversarial input; governance gate bypass unless every host import is individually gated; collides with the mutually-exclusive `native` / `browser` feature split (MEMORY.md); fine-tune track (#5) becomes reachable. | Q1–Q8 all live |

**Default recommendation:** **Option C** now, **Option D** after §5 is answered. Option E is not recommended.

**User decision (2026-04-23): Option D,** with the channel-generality constraint in §4.5 baked into every ADR and interface from day one.

---

## 4.5. Cross-cutting constraint: channel-generality

**User directive (2026-04-23):** "Make sure we are considering this for all channel data like this, not just audio."

The RLM adoption path and every primitive it introduces must treat **substrate paths as the unit of abstraction**, not any specific modality. No audio-specialized code. No mic-specialized code. No tof-specialized code. If a primitive works for `substrate/sensor/mic`, it works for `substrate/sensor/tof`, `substrate/kernel/log`, `substrate/chain/tail`, `substrate/mesh/status`, and every substrate path a future sensor or subsystem emits to.

This constraint cuts across the whole Option D stack:

| Primitive | Channel-generality requirement |
|---|---|
| `substrate.describe(path)` | Returns a shape-schema plus metadata (`bytes`, `sample_rate?`, `frame_count?`, `last_tick`, `updated_at`, `characterization`) — schema-match is data-driven, not per-modality. No special case for mic. |
| `substrate.project(path, op)` | The `op` vocabulary (`window`, `stride`, `reduce`, `filter`, `regex`, `tail`) applies uniformly. A window over an audio buffer and a window over a kernel-log tail use the same call. |
| `tree.peek` / `tree.search` / `tree.slice` | Recursive over any subtree. The root LLM can `tree.search("substrate/sensor/")` and get mic+tof+future sensors back as a flat handle list without knowing what any of them are. |
| `llm_query(handle, prompt)` | Accepts any substrate handle. The sub-model reasons about whatever the handle points at. Specialization happens only in how the *viewer* renders the final value (see Explorer panel pattern registry), not in how the agent loop handles it. |
| Viewer / renderer registry | Shared with `.planning/explorer/PROJECT-PLAN.md`. A viewer is matched by schema signature, not by path. Adding ToF temperature humidity or an accelerometer sensor later = add one viewer, zero changes to the RLM loop. |
| Trajectory event schema (ADR-0009) | `rlm.sub.call` events carry the substrate path they operated on as a first-class field. No channel-specific event types. Querying "which trajectories touched `substrate/sensor/*`" is a single filter, not a schema-per-sensor join. |

**Implication for Q6:** Option D's work and the Explorer panel MVP must land as a single coordinated stream, not two parallel ones — they share `substrate.describe` / `substrate.project` / `tree.*` and the viewer registry. ADR-0006 covers both consumers. Expect the Explorer panel to ship as Option D's first visible artifact, since it's the human-facing side of the same primitives the agent uses.

**Implication for the mic-adapter refactor** (currently in the working tree): the `mic.rs` race fix that closed this session's open loop is the *last* mic-specialized path in the substrate publish side. From here forward, new sensors land on `substrate/sensor/<name>` with no code in `crates/clawft-substrate/` specialized for them — the Explorer and RLM viewers read them through the generic primitives above.

---

## 5. Open questions for the user

Answer inline under each. Deduplicated from gaps-and-risks' original ten. **With Option D selected, Q2, Q4, Q5, Q7 are on the critical path — ADR-0008 and ADR-0009 cannot start until they have answers.** Q6 is effectively pre-answered by §4.5's Explorer-coordination directive but still worth user confirmation.

**Q1. Is the REPL a substrate pattern or a reasoner pattern?** If substrate + HNSW + projections already satisfy "prompt as environment," Option C is sufficient and there is no need for a Python / WASI REPL ever. If a persistent executable environment is required (e.g., for multi-step numeric work inside a trajectory), that is net-new runtime engineering and Option D/E territory.
**Answer:**

**Q2. Where does `llm_query` live, and is every sub-call gated?** Options: (a) registered as a tool in `clawft-tools`, hits `GateBackend::check()` like any other tool call — safe but slower; (b) bypasses the gate for perf, never — this is a hard "no" from gaps §3.1. Confirm (a). Also: do sub-call events emit on Stream (cheap, window-anchored) or Diag (truncated) or a new `reasoning` channel?
**Answer:**

**Q3. Tool output budget thresholds.** Candidate #4 proposes `≤4KB inline / 4KB–128KB handle-with-preview / >128KB handle-only`. Confirm, or set different numbers. These values land in `crates/clawft-core/src/agent/loop_core.rs` and bind the ExoChain event payload discipline downstream.
**Answer:**

**Q4. Per-trajectory budget + cancellation primitive.** If an RLM-ish trajectory runs hot (p95 tail), what cancels it? Per-trajectory `{depth:1, subcalls:32, tokens:200K, wall_clock:N sec}`? Does cancellation raise a clean error the root LLM sees, or hard-abort the session? This interacts with the 1 ms ECC tick — the reasoning loop must not co-host with the kernel's tokio runtime (gaps §3.3).
**Answer:**

**Q5. Replayability contract for RLM-derived decisions.** If a trajectory output is fed into anything governance-bearing (a chain event, a tool exec, a peer-facing state change), do we (a) snapshot `{model_id, seed, prompt_hash, sub-call trace}` to Governance, (b) forbid RLM outputs from triggering governance-bearing actions, or (c) accept non-replayable decisions inside a clearly labeled k_level? Ties into the exochain-logging symposium's envelope v2 and K-level assignments.
**Answer:**

**Q6. Does this ride with the Explorer panel MVP or land ahead of it?** Candidate #2's `tree.peek` / `tree.search` / `tree.slice` agent-facing tools share primitives with the human-facing Explorer panel in `.planning/explorer/PROJECT-PLAN.md`. Coordinating ADRs saves duplicate work; decoupling ships sooner. Which?
**Answer:**

**Q7. Mesh behavior for in-flight trajectories.** Is a running RLM trajectory pinned to its origin node (simple; loses on peer failure), or does it replicate state via Fabric's `SyncStreamType::Chain` (audit-clean; breaks Fabric rate assumptions — gaps §3.8)? Default recommendation: pin to origin for v1, revisit when the mesh is load-bearing for reasoning.
**Answer:**

**Q8. Do we reserve space in the roadmap for the native-RLM fine-tune (candidate #5)?** Not a code decision — a product one. Implies trajectory collection telemetry, opt-in user data harvesting, a model artifact pipeline, and $200–$500 compute per fine-tune run. If the answer is "never," we can drop the telemetry plumbing; if "eventually," we should bake `rlm.trajectory.complete` into Option D's event schema now.
**Answer:**

---

## 6. Artifacts to produce (Option D — all four ADRs active)

All four ADRs must land before implementation begins. Write ADR-0006 and ADR-0007 first; they unblock the Explorer panel MVP as a side effect. ADR-0008 and ADR-0009 are blocked on Q2, Q4, Q5, Q7 answers.

- **ADR-0006:** Substrate as Prompt Handle — `substrate.describe` + `substrate.project` verbs, `tree.peek` / `tree.search` / `tree.slice` tool contracts. **Channel-general per §4.5.** Shared consumer: Explorer panel from `.planning/explorer/PROJECT-PLAN.md`.
- **ADR-0007:** Metadata-Only Tool History — thresholds, `ToolResult::{Inline, Handle}` enum, migration from existing full-output appending in `loop_core.rs`. Settle Q3 thresholds inline.
- **ADR-0008:** Bounded Recursive Sub-Calls — `llm_query` as a gated tool, per-trajectory budget, cancellation semantics. **Blocked on Q2, Q4.**
- **ADR-0009:** RLM Trajectory Event Schema — which ExoChain channel carries `rlm.root.code` / `rlm.sub.call` / `rlm.sub.return`, k_level assignments, RollingWindowAnchor aggregator. Channel-general event shape per §4.5: substrate path is a first-class field, not a type discriminator. **Blocked on Q2, Q5, Q7.**

All four coordinate with the exochain-logging symposium's Envelope v2 (`01-exochain-specialist.md`) and the Explorer panel MVP (`.planning/explorer/PROJECT-PLAN.md`).

---

**RESUME →** answer Q1–Q8 inline in this file. As soon as Q3 and Q6 are answered (both low-contention), ADR-0006 and ADR-0007 can start in parallel — they deliver the Explorer panel MVP as a side effect and pay down the original mic-gauge-silent problem that triggered today's session. ADR-0008 and ADR-0009 start once Q2, Q4, Q5, Q7 are green. Do not write RLM code until ADR-0008 lands — gap §3.1 makes ungated `llm_query` a non-starter.
