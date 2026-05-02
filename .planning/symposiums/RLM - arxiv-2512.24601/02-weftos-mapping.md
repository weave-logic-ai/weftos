# arXiv:2512.24601 "Recursive Language Models" — WeftOS Mapping

**Paper:** Zhang, Kraska, Khattab — *Recursive Language Models* (MIT CSAIL, Jan 2026). An LLM inference scaffold that loads the prompt into a persistent REPL as variable `P`, exposes only metadata (length, prefix, accessors) to a *root* LM, and lets the root emit Python that slices/filters `P`, stores intermediates in REPL variables, and recursively invokes a *sub-LM* over programmatic slices via `sub_RLM(prompt)`. Termination: root sets `Final`. Only constant-size `stdout` metadata is appended to root history.

This document maps the paper's primitives onto WeftOS. No adoption recommendations — mapping only.

---

## 1. Mapping Table

| Paper primitive | WeftOS subsystem (primary) | Secondary | Notes |
|---|---|---|---|
| Persistent REPL environment `E` (Python) | `clawft-kernel::wasm_runner` + `clawft-core::agent::sandbox` | `clawft-plugin` | Closest existing execution surface is the WASM runner and agent sandbox. Python is not in-tree; a REPL is a new runtime leaf. |
| Prompt-as-variable `P` (metadata-only to root) | `clawft-substrate` | `clawft-weave` (JSON-RPC) | Substrate already models pathed typed state with `substrate.read` (value + tick + sensitivity) and `substrate.subscribe`. A long prompt binds to a path like `substrate/prompt/<session-id>`. |
| `sub_RLM(prompt)` (symbolic recursion handle) | `clawft-llm::router` + `pipeline::tiered_router` | `agent::loop_core` | `TieredRouter` already routes by `complexity ∈ [min,max]`. A sub-call is a tier-1/tier-2 dispatch; router owns provider, loop owns lifecycle. |
| Root LM history compaction (stdout metadata only) | `pipeline::assembler` | `agent::context` | Metadata-only appending is a policy variant of the existing assembler. |
| Root/sub-call bifurcation (heavy root, cheap subs) | `pipeline::tiered_router` | `intelligent_router` | ADR-026 ladder (Booster / Haiku / Sonnet) already encodes this split. |
| REPL state as symbolic intermediates | `clawft-substrate` (KV paths) | `kernel::persistence` | Intermediates live on `substrate/session/<id>/var/<name>` — survive restart, observable via subscribe. |
| Trajectory trace (code, stdout, sub-call fanout) | ExoChain (Stream channel) | `core::session_indexer` | Trace shape matches Stream channel's causally-linked event model. |
| Cost / budget tracking per trajectory | `pipeline::cost_tracker` | `pipeline::rate_limiter` | Exists; RLM adds heavy-tail variance as a new distribution shape. |
| Recursion bound / iteration cap | `agent::loop_core` (max-iter) | `kernel::governance` | Max-depth and max-iter are governance-shaped rules; loop_core has iteration caps. |
| Chunking / slicing of `P` by code | `substrate::delta` + `substrate::projection` | `plugin-treesitter` | Projections already expose sub-views; tree-sitter handles structured slicing. |
| Regex / keyword filtering by priors | `clawft-plugin` tools | `core::tools` | Tool plane (regex, BM25, embeddings) is the "filter without reading" layer. |
| Native-recursive fine-tuned model | `clawft-llm::local_provider` | — | No training infra in-tree; externally trained, loaded as provider. |
| Stitched long-output assembly | `clawft-surface::builder` | `core::artifact_store` | Surface builds structured outputs; artifact store persists them. |
| Symbolic handle to prompt (no copy into window) | `clawft-substrate` path + `clawft-weave` | — | Substrate path *is* the symbolic handle. |
| REPL-necessary / sub-calls-optional (Observation 2) | `kernel::wasm_runner` (env) | `pipeline::tiered_router` (delegation) | Two layers already live in distinct crates. |
| Trajectory as training signal | ExoChain Stream → offline consumer | `clawft-services` | Signed Stream tier is the natural export source; no in-process training. |

---

## 2. Architecture Alignment

**Reinforcing assumptions.** The paper's strongest prior — *the LM should hold a handle to data, not the data itself* — is what WeftOS's substrate already bakes in. `substrate.read` returns a path-addressed value plus tick and sensitivity; clients subscribe to paths they never materialize in full. An RLM's "prompt as REPL variable" is structurally the same move with a narrower domain. The `substrate_service().read()` seam in `clawft-weave/src/daemon.rs` already takes `(actor_id, path)` — the identity needed for per-actor prompt visibility.

A second alignment is the 3-tier model router (ADR-026, `pipeline/tiered_router.rs`). The paper splits work between a *root* model and cheaper *sub-call* models (GPT-5 root, GPT-5-mini sub) with the same rationale WeftOS uses for Booster/Haiku/Sonnet: cost concentration on genuine reasoning. `TieredRouter::matches_complexity` is the decision surface an RLM sub-call hits; `fallback` corresponds to the paper's "no recursion" ablation.

A third alignment is the ExoChain Stream channel. RLM trajectories are long, causally-linked event sequences — *root emits code*, *REPL executes*, *sub-LM called on slice k*, *stdout metadata returned*. Stream is already shaped for high-rate causal emission, and dual-signing (ML-DSA-65 + Ed25519) cleanly preserves sub-call provenance.

**Forced reconsiderations.** WeftOS's current agent loop (`agent/loop_core.rs`) is a flat tool-ReAct loop: one model, one iteration, one tool per step. The paper demands *programmatic* (not verbalized) sub-model invocation. `loop_core` has no equivalent of "the model emits Python that calls `sub_LM(...)` inside a loop over 10k slices." The design tension is whether the REPL lives *inside* an agent iteration or *above* it (the agent loop becomes the REPL driver).

The substrate's sensitivity model is stressed. Paths carry a sensitivity label; a prompt may contain secrets the root LM is trusted with but a cheaper sub-model is not. The paper's "root hands a slice to sub-LM" requires either per-slice sensitivity inheritance or explicit declassification at the sub-call boundary. Neither exists today.

Finally, the paper assumes Python + unsandboxed REPL persistence across iterations. WeftOS's execution surfaces (wasm_runner, agent sandbox) are intentionally not persistent Python. Anything RLM-shaped needs a new runtime leaf under `clawft-kernel` or a deliberate policy of treating the substrate as the "REPL" (paths as variables, projections as slices, JSON-RPC as exec — code-free).

---

## 3. Integration Sketches

### 3.1 RLM-as-substrate-consumer (prompt-as-path)

```
          +-----------------------+         +----------------------------+
  user -->|  weaver JSON-RPC      |  bind   |  clawft-substrate          |
  prompt  |  substrate.publish    |-------->|  path=substrate/prompt/<s> |
          |  path, value=<big>    |         |  value stored, tick = T0   |
          +-----------------------+         +----------------------------+
                   |                                   ^
                   | invoke                            | substrate.read(path, offset, len)
                   v                                   | substrate.subscribe(path-prefix)
          +-----------------------+                    |
          |  clawft-core          |                    |
          |  agent::loop_core     |--- Tool: slice ----+
          |  (root model, tier 3) |
          +-----------------------+
                   |
                   | pipeline::tiered_router.route(complexity=low)
                   v
          +-----------------------+        (recursive; bounded depth/fanout)
          |  clawft-llm::router   |-----> provider (tier-2 model) --+
          +-----------------------+                                 |
                   ^                                                |
                   +------- sub-call result (string) ---------------+
```

Touchpoints:
- **JSON-RPC:** `substrate.publish` already exists; would need a read variant returning `(length, prefix, sha256)` without the body — call it `substrate.describe` (new verb) — to mirror the paper's "metadata only to root" rule.
- **Trait:** `clawft-core::agent::loop_core::AutoDelegation` is the closest existing extension point for "loop asks for a sub-call"; the sub-call target is `TieredRouter::route(complexity)`.
- **Crate boundary:** `clawft-llm` stays purely about provider routing; `clawft-core` owns the recursion bookkeeping; `clawft-substrate` owns the prompt bytes.

### 3.2 Trajectory emission to ExoChain Stream

```
 root iter k                                   Stream channel (hot: Arrow IPC)
 +---------------+   code_emit   +---------+   +-------------------------------+
 |agent::loop_core|-------------->|chain_   |-->|event: rlm.root.code           |
 +---------------+               |event    |   |  sess=<s>, depth=0, tick=T    |
         |                        |         |   |  payload_ref=substrate/...    |
         | exec(code, state)      +---------+   |  (ML-DSA-65 + Ed25519 signed) |
         v                                       +-------------------------------+
 +---------------+                                         ^
 |kernel::wasm_  |   stdout_meta_append                    |
 |runner / sbx   |-----------------------------------------+
 +---------------+                                         |
         |                                                 |
         | if code contains sub_RLM() call:                |
         v                                                 |
 +---------------+   sub-call                              |
 |pipeline::     |   (complexity, prompt-slice-path)       |
 |tiered_router  |-----------------------------------------+  event: rlm.sub.call
 +---------------+                                            event: rlm.sub.return
```

Touchpoints:
- **Message:** three new event kinds on the Stream channel — `rlm.root.code`, `rlm.sub.call`, `rlm.sub.return` — carrying substrate path refs instead of inlined payloads (respects the channel's k_level observe-only rule in v1; no governance implications).
- **Crate:** `clawft-core::chain_event` already emits chain events; the new kinds slot into its enum.

### 3.3 "REPL-as-substrate" (no new runtime)

```
 +---- root model iteration k -----------------------------+
 |                                                          |
 |   pipeline::assembler ----metadata-only history--------> |
 |          ^                                               |
 |          | read(path=substrate/sess/<s>/var/*, meta)     |
 |          |                                               |
 |   tool: substrate.project(path, op={head,regex,slice,len}|
 |          |                                               |
 |          v                                               |
 |   +--------------------------+                           |
 |   | clawft-substrate         |                           |
 |   |   projection.rs          |<-- write var binding ---+ |
 |   |   delta.rs               |--- fan out via subscribe  |
 |   +--------------------------+                           |
 |          |                                               |
 |          v  if slice passes to sub-LM:                   |
 |   pipeline::tiered_router.route(low-complexity)          |
 +----------|-----------------------------------------------+
            v
     provider call, result written back to
     substrate/sess/<s>/var/out_k via substrate.publish
```

Touchpoints:
- **Trait:** the paper's REPL is replaced by substrate + projections; no Python runtime enters the workspace. The "variable" is a path; the "exec" is a JSON-RPC verb.
- **New verb:** `substrate.project(path, op)` — would extend `clawft-weave/src/protocol.rs` alongside the existing `substrate.read` / `publish` / `subscribe`.
- **Crate boundary:** `clawft-substrate::projection.rs` already exists and is the natural home for `head`, `slice`, `regex_find`, `line_chunks`.

---

## 4. Divergences

**Determinism.** WeftOS's kernel tick is a 1 ms beat with p95 ≈ 24 μs calibration and a hash-verified resource tree. RLMs are non-deterministic at the trajectory level: different root runs emit different code, fan out different sub-calls, produce different intermediates. A substrate that takes an RLM trajectory as the source of state transitions loses the reproducibility the resource tree assumes. Any "RLM writes to substrate" edge needs isolation — writes to a scratch namespace the hashed tree does not cover, or trajectories post-compacted into a single deterministic commit.

**Governance.** The governance engine (22 rules, threshold 0.7, genesis-anchored) validates explicit, enumerable state transitions. An RLM root emits Python that performs arbitrary computation over the prompt; the paper gives no mechanism for pre-declaring a trajectory's action space. Mapping this requires either (a) gating every substrate write emitted by a trajectory through the 22-rule check — costly, could stall long trajectories — or (b) moving governance to the *trajectory commit* boundary, a different policy model than today's per-delta evaluation.

**Ordering / causality.** ExoChain Stream treats causally-linked events as an append-only signed sequence. RLM sub-calls have *tree* causality (root → sub_k → possibly deeper), not linear. The paper caps at depth one in evaluations, but the primitive allows arbitrary depth. Stream's current model assumes linear causality within a channel; depth-unbounded fanout requires per-trajectory sub-channels or a richer parent-pointer scheme than the 0.7-era design specifies.

**Threat model.** The RLM scaffold runs untrusted sub-LM output back into the root's decision loop — a prompt-injection surface. WeftOS's threat model assumes signed state transitions and authenticated actors; sub-LM outputs are neither. The substrate `sensitivity` tag has no per-call downgrade/upgrade semantics at the model-call boundary today.

**Execution surface.** Python is not in the tree; WASM and tool-invocation are. A faithful Python REPL is a new runtime dependency; the substrate-projection analog (sketch 3.3) avoids that but sacrifices arbitrary user code — exactly the paper's stated expressiveness lever. Trade unresolved.

**Cost variance.** `cost_tracker` enforces per-call budget caps. Figure 3 shows RLM p95 cost ~3× median; cost_tracker is not structured around heavy-tail distributions and the tiered router's `fallback` does not cover "trajectory budget exhausted mid-run."

**Output assembly.** `clawft-surface::builder` composes outputs from structured fragments. The paper's long-output pattern concatenates unstructured LM-generated strings inside the REPL. Surface builder has no ingestion path for "accumulated REPL variable" — an adapter layer would be needed.

---

## Subsystem Touch Summary

Ranked by surface area touched in the mapping above:

1. **clawft-substrate** — prompt-as-handle, projections, intermediate variables. Tight fit.
2. **clawft-core pipeline/agent** — tiered routing, assembler, agent loop, complexity classifier. Tight fit for sub-call dispatch; strained for recursion bookkeeping.
3. **ExoChain (Stream channel)** — trajectory event emission. Clean fit as observer; strained if used as source-of-truth for tree causality.
4. **clawft-kernel (wasm_runner / governance)** — REPL runtime surrogate, iteration/recursion caps. Strained: no Python, governance is per-delta not per-trajectory.
5. **clawft-llm** — provider routing for sub-calls. Unchanged role.
6. **clawft-weave JSON-RPC** — one or two new verbs (`substrate.describe`, `substrate.project`) or a new `rlm.*` namespace.
7. **clawft-surface / GUI / CLI** — trajectory visualization, Explorer panel alignment. Observer role only.

Not touched: `clawft-channels`, `clawft-security` (beyond the sensitivity-label question), `eml-core`, `exo-resource-tree` (except the determinism concern in §4), `clawft-plugin-oauth2`, any of the provider-specific plugin crates.
