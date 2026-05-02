# arXiv:2512.24601 — Adoption Candidates for WeftOS

**Paper:** Zhang, Kraska, Khattab. *Recursive Language Models* (RLMs). MIT CSAIL, Jan 2026.
**Reviewer role:** adoption-candidates specialist (panel of 4).
**Branch context:** `development-0.7.0`, v0.6.19 shipped.
**Deliverable:** ranked list of concrete changes WeftOS could ship that adopt ideas from the paper. Cost / value ordered. Nothing speculative-only.

---

## TL;DR

The paper is unusually pragmatic. Its core claim — *don't paste the prompt into the model's context; put it in a REPL variable and let the model write code against it, including recursive sub-LLM calls* — is a pattern, not a new architecture. WeftOS already has most of the substrate (WASM sandbox with fuel/memory limits, `delegate_task` tool that invokes a sub-LLM with tool access, an HNSW-indexed resource tree, a token-budget assembler that is essentially the paper's "bad Algorithm 2"). Three candidates stand out: a thin RLM scaffold in the weave, promoting the resource tree to be the "context variable" by default, and replacing `read_file` with a paginated/programmatic handle. The rest of the paper is either redundant with what we have or not worth the infrastructure.

---

## Ranked candidates

### 1. RLM scaffold around `delegate_task` / auto-delegation — **ship first**

- **What it adopts.** §2 (Algorithm 1) plus Appendix C.1 system prompt. Treat the user prompt and any bulky attachment as a variable in a scratch environment; expose an `llm_query` function to the root model; iterate until a `FINAL()` marker is emitted. The paper's key observation (§4, Obs. 2) is that even *without* sub-calling, offloading the prompt to a variable beats compaction on long inputs. That ablation alone is ROI-positive.
- **Where it lands.** New module `crates/clawft-core/src/agent/rlm.rs`, plus a thin glue crate extension `crates/clawft-tools/src/rlm_tool.rs` exposing `llm_query` as a registry tool. Wires into `loop_core.rs:run_auto_delegation` as an alternate path selected by routing heuristic. Reuses `clawft_services::delegation::claude::ClaudeDelegator` for sub-calls; on local providers reuses `clawft_llm::router::LlmRouter`.
- **Effort.** 1.5–2 weeks (10–14 engineer-days). Rust port of ~250 lines of Python REPL-loop logic plus the `FINAL_VAR`/`FINAL` parser. The sub-call primitive already exists; the novelty is the loop + "metadata-only back to root" discipline + variable store.
- **Value.** Directly addresses a current pain: reading a 200KB source file via `read_file` dumps the whole thing into context and evicts prior reasoning. The paper's OOLONG result (+28% vs. base, $0.43 vs. $1.31 summary-agent cost) is precisely the regime WeftOS hits when a user asks questions over a large `crates/clawft-kernel/src` slice. Also gives us a principled answer to "why is the model losing the plot after three tool calls" that our current `TokenBudgetAssembler` (`crates/clawft-core/src/pipeline/assembler.rs:20`) cannot: that assembler *truncates*, which is §2's dismissed Algorithm 2 verbatim.
- **Blocking ADRs / decisions.**
  - ADR: "Variable store semantics" — is the persistent environment per-session, per-request, or per-agent? Per-session is right; it lines up with the existing `SessionStore`.
  - ADR: "FINAL marker vs. explicit tool call" — the paper notes (Appendix B) that textual `FINAL()` tags are brittle. We already have structured tool calls; prefer a `finalize(answer)` tool over regex-parsed tags.
  - Routing decision: when does the auto-delegation router pick the RLM path? Proposed gate: `context_type in {large_file, doc_corpus, arrow_chunk}` OR `token_estimate > 0.7 * provider.max_context`.
- **Prototype step-1 (tomorrow).** In `crates/clawft-tools/src/` add `rlm_tool.rs` that registers two tools: `llm_query(prompt: String, context_refs: Vec<String>) -> String` (calls the existing router with a sub-model) and `finalize(answer: String)`. Add one integration test in `crates/clawft-core/tests/` that feeds a 10-line OOLONG-style query against a fixture. No loop yet, no variable store — just prove the sub-call round-trips through the registry. Commit message: `feat(rlm): scaffold llm_query + finalize tools (phase 1 of RLM-0001)`.

---

### 2. Resource tree promoted to "prompt variable" — **half-built, gold**

- **What it adopts.** §2 design choice #1 ("symbolic handle to P without copying it into the context window") and §4.1's "filtering input information using code execution based on model priors." The paper's insight is that the model must be able to poke at the prompt *by reference*, not be fed it. WeftOS's `exo-resource-tree` (HNSW-indexed resource tree, `crates/exo-resource-tree/src/tree.rs`, plus `kernel/src/tree_manager.rs` — 1,945 lines already written) is exactly the data structure the paper wishes it had. We just haven't wired it into the root-model prompt as the default container.
- **Where it lands.** `crates/clawft-kernel/src/tree_view.rs` (277 lines) grows a thin "handle" API: `Handle::metadata() -> {kind, size, children_count, hnsw_topk(query)}`. The root model sees handles, not content. New tools: `tree.peek(path, offset, limit)`, `tree.search(query, top_k)`, `tree.slice(path, range)`. Register in `crates/clawft-tools/src/lib.rs:114` alongside the existing `spawn_tool` block.
- **Effort.** 1 week (5–7 days). Most of the work is designing the handle protocol and deciding which existing tools deprecate in its favor. HNSW is already exposed via `hnsw_service.rs` (1,022 lines, operational).
- **Value.** Solves three pain points at once: (a) `read_file` no longer nukes context; (b) the HNSW service finally has a first-class agent-facing API (currently it's a kernel internal with no direct tool binding I could find); (c) the Explorer panel MVP being drafted right now (`.planning/explorer/PROJECT-PLAN.md`) gets a free agent-side mirror — humans and agents browse the same tree with the same primitives. That's a meaningful unification.
- **Blocking ADRs / decisions.**
  - ADR: "Tree handle as the default context container" — does every inbound message bind its attachments to a tree node? Recommend yes; it makes ExoChain provenance trivial.
  - Decision: replace `read_file` or coexist? Coexist for six weeks, then deprecate. Large reads (>100KB) should *require* `tree.peek` with a range.
- **Prototype step-1.** Add `tree.peek(path: String, offset: u64, limit: u64) -> {bytes, eof, size}` to `clawft-tools` backed by `tree_manager::get_node`. Write one golden test: agent given 5MB fixture, answers a question with 3 `tree.peek` calls totaling <20KB read. Commit: `feat(tools): tree.peek for paginated resource access (RLM-0002 phase 1)`.

---

### 3. Recursive sub-agent depth-1 in the weave — **natural fit**

- **What it adopts.** §2 design choice #3 (symbolic recursion) and Observation 2 (sub-calls matter for information-dense tasks, +10–59% gain). WeftOS's auto-delegation (`loop_core.rs:447`, `run_auto_delegation`) already calls Claude as a sub-agent once per inbound message. The paper's contribution is letting the root model issue sub-calls *programmatically, in a loop, with results as variables*. We have the plumbing; we just don't let the model drive it.
- **Where it lands.** Extends candidate #1's `rlm.rs` with a sub-call registry that reuses `DelegateTaskTool` (`crates/clawft-tools/src/delegate_tool.rs`, 185 lines). Hard-capped at `max_recursion_depth = 1` per the paper's Section 6 recommendation and enforced via governance. Sub-call gets a fresh tool set minus `delegate_task` itself to prevent runaway recursion (the existing `mcp_tools.rs:347` already has this "preventing recursive delegation" note — confirms we thought about it).
- **Effort.** 3–5 days *if* #1 lands first. Standalone it's ~1.5 weeks because you'd be building the loop twice.
- **Value.** Unlocks the paper's F1 58% vs. 0.1% result on OOLONG-Pairs — tasks that require pairwise aggregation across an input. WeftOS-shaped examples: "across these 30 commits, find pairs whose diffs contradict each other", "for every pair of ExoChain channels, list the causal links." These are hard-impossible today.
- **Blocking ADRs / decisions.**
  - Governance ADR: sub-calls need effect-vector accounting. Do we charge the sub-call's tool effects against the root agent's budget or the sub-agent's? Recommend the root's — prevents jailbreak via recursion.
  - Budget decision: cap on number of sub-calls per root iteration. Paper had to patch Qwen3-Coder's prompt to stop it launching thousands; we'll hit the same wall. Propose: `max_subcalls_per_iter = 32`, circuit-breaker on cost.
- **Prototype step-1.** In `rlm.rs`, add a `SubCallBudget { max_depth: 1, max_calls: 32, token_cap: 200_000 }` and plumb it through `llm_query`. Governance rule that rejects depth-2 attempts. Test: one fixture that issues 40 sub-calls and verifies the 33rd through 40th are rejected with a clean error the root can see. Commit: `feat(rlm): bounded recursive sub-calls with governance gate (RLM-0003)`.

---

### 4. Metadata-only history discipline for the agent loop — **cheap, high-value refactor**

- **What it adopts.** §2 footnote 1: *only constant-size metadata about stdout is appended to M's history for the next iteration*. This is the single most important anti-context-rot trick in the paper and it's a ~100-line change.
- **Where it lands.** `crates/clawft-core/src/agent/loop_core.rs`. When a tool returns >N bytes, store the result under a handle in the session's variable store, and append only `{tool, handle, bytes, sha256, head_preview[256]}` to the model-facing history. Today we append the full tool output (or truncate, which is worse).
- **Effort.** 3–4 days. Main risk is retroactively fixing agents that assume full outputs — most don't; they already call `read_file` → `grep`-style chains.
- **Value.** Hong et al.'s "context rot" is the exact failure mode users hit after 5–10 tool calls in a long conversation. This change buys us a ~3–5x effective context without touching any model. Cost-free; complements candidate #2 naturally.
- **Blocking ADRs / decisions.**
  - ADR: "Tool output budget" — what's N? Propose 4KB inline, 4KB–128KB handle-with-preview, >128KB handle-only. Matches §2's "short prefix and length" recipe.
  - Decision: can this ship before candidate #1? Yes — it's independently useful.
- **Prototype step-1.** Feature flag `rlm-handles` in `clawft-core`. Add `ToolResult::{Inline(Vec<u8>), Handle { id: String, size: u64, preview: String }}` in `tools/registry.rs`. Convert `shell_tool.rs` first (its stdout is the most bloated). Commit: `feat(agent): handle-based tool results for long outputs (RLM-0004)`.

---

### 5. Native-RLM fine-tuning recipe for the local model path — **speculative-adjacent, ship only if #1–#4 land**

- **What it adopts.** §4 Observation 6 and Appendix A. With 1,000 distilled trajectories and 48 H100-hours, Qwen3-8B gained 28.3% on long-context tasks. WeftOS has a local-provider story (`crates/clawft-llm/src/local_provider.rs`) and there's ongoing interest in edge-resident models (see `crates/clawft-edge-bench`).
- **Where it lands.** Not a code change — a training track. Would sit under `.planning/sparc/weftos/` as an RFC. Artifact: a fine-tuned adapter shipped alongside the local provider, selected when the user enables `recursive_native` in config.
- **Effort.** 2–4 weeks of engineer time + ~$200–$500 compute. Most of that is trajectory collection from production RLM runs (requires #1 to be shipping telemetry).
- **Value.** Real but contingent. Only matters if a meaningful fraction of users run the local provider for long-context work. Today that's a minority use case.
- **Blocking ADRs / decisions.**
  - Decision: do we even want to ship fine-tuned artifacts, or stay model-agnostic? This is a product call, not a technical one.
  - ADR: trajectory collection + privacy. We'd be harvesting tool-call sequences from users; opt-in only, ExoChain-logged.
- **Prototype step-1.** Skip until #1 ships and we have >1,000 real RLM trajectories to distill. Putting this ahead of #1 is cart-before-horse.

---

## Would not ship

### ✗ BM25 retrieval baseline integration

The paper benchmarks against CodeAct + BM25 (§3.2) and the baseline loses to RLM by 30+ points on most tasks. I looked for BM25 in the codebase — we don't have it, and the paper's result is that adding BM25 wouldn't close the gap anyway. Our HNSW + embeddings path is already stronger than BM25 for semantic retrieval; adding BM25 would be work for a baseline the paper already debunked. **Veto.**

### ✗ Python REPL as the execution environment

The paper uses a Python REPL because it's easy. For WeftOS, embedding CPython (or pyo3) adds a huge dependency, a new sandbox boundary, and a second scripting language alongside our WASM runner. WASM already runs arbitrary guest code under fuel + memory limits (`crates/clawft-kernel/src/wasm_runner/`). The "REPL" can be a WASI-WASM module that exposes `llm_query` / `tree.peek` as host imports. Skip Python.

### ✗ Async sub-calls as a first-class feature (yet)

Paper §6 flags synchronous sub-calls as their main runtime bottleneck. Tokio makes this nearly free for us once the loop exists. But: adopting this before candidate #1 ships is a speculative optimization. Mark it as a natural follow-on, not a standalone adoption candidate.

---

## Rough cost if we ship everything above "would not ship"

| # | Candidate | Effort | Dep |
|---|---|---|---|
| 1 | RLM scaffold around `delegate_task` | 10–14 days | — |
| 2 | Resource tree as prompt variable | 5–7 days | parallel with #1 |
| 3 | Bounded recursive sub-calls | 3–5 days | after #1 |
| 4 | Metadata-only history discipline | 3–4 days | independent |
| 5 | Native-RLM fine-tuning | 2–4 weeks | after #1+telemetry |

**Critical-path total (excluding #5):** ~4–5 engineer-weeks for a solid first release of RLM-shaped long-context handling in WeftOS. Roughly half the exochain-logging project's 12-week estimate, and this one has a visible user-facing win (stop losing the plot on big codebases) from day one.

If we pick only one thing: **#4 (metadata-only history)**. 3 days, no new concepts, measurable reduction in context rot. If we pick two: **#4 + #2 (tree as variable)**. That's a compound week of work that makes WeftOS's existing HNSW-indexed resource tree actually visible to the root LLM, which it effectively isn't today.

---

## Fit-to-current-fronts cross-check

- **Explorer panel MVP** (`.planning/explorer/PROJECT-PLAN.md`): candidate #2 makes the human-facing tree browser share primitives with the agent-facing one. Worth coordinating the ADRs.
- **ExoChain logging redesign** (`.planning/symposiums/exochain-logging/`): candidate #4's handle-based tool results want their own `chain.tool.result.handle` event kind. Coordinate with Phase 1 of that project; we'd be adding one EVENT_KIND either way.
- **ESP32 sensor bridge** (`.planning/sensors/`): no direct overlap. Sensor data volume isn't in the regime where RLMs beat base LLMs (§4 Obs. 3: "the base LM outperforms RLM in the small input context regime"). Leave that front alone.
- **v0.7.0 development branch**: candidates #1 and #4 can land in 0.7.0. Candidate #2 probably wants to ride with the Explorer panel MVP on 0.7.x. Candidate #3 is 0.7.x. Candidate #5 is 0.8+.

---

## Notes on what the paper does *not* give us

- No new algorithm for HNSW / vector routing — ours is already more sophisticated.
- No governance or sandboxing model — the paper hand-waves sandboxing; we have wasmtime + constitutional governance.
- No multi-agent coordination — sub-calls are depth-1 child LLMs with no state sharing. Our mesh is orthogonal.
- No persistence strategy — the REPL is per-request. We already have session + ExoChain.

The paper's reach ends at the single-agent long-context inference loop. That's exactly where WeftOS has a weakness (context rot on multi-file codebase tasks), which is why three of the five candidates above are genuinely high-ROI. Past that frontier the paper stops being useful for us.
