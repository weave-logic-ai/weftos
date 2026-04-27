# agent-core v1 — clawft port of openclaw, end-to-end through WeftOS

| | |
|---|---|
| **Status** | Active |
| **Drafted** | 2026-04-27 |
| **Branch** | `development-0.7.0` |
| **Supersedes** | `docs/plans/chat-agent-v1.md` §14 (commits 1-9) |
| **Origin** | Six-panel symposium 2026-04-27 (Cartographer, Architect, Weaver-specialist, Kernel-specialist, Researcher, ruv-researcher, Reviewer) |

## Lineage

clawft (a.k.a. nanoclaw) is the Rust port/rewrite of **openclaw** (the
Node.js reference at `~/dev/openclaw-aws-workshop`, now a deployment
guide; the upstream project shape it pinned). The in-tree agent module
headers — `crates/clawft-core/src/agent/loop_core.rs:4`,
`crates/clawft-core/src/agent/context.rs:5`,
`crates/clawft-core/src/agent/memory.rs:3` — cite **nanobot** (Python)
as the immediate Rust-port source. Both lineages are honored: the wire
shape (OpenAI-compatible `agent.chat`) is openclaw's; the loop and
context machinery is the Rust translation of nanobot.

The `agent.chat` RPC, today (commit `e6f8c816`), runs a
**vertical-slice spike** that inlined a fresh ~360-line tool loop in
`crates/clawft-weave/src/daemon.rs::handle_agent_chat` rather than
calling the existing 9,781 LoC at `crates/clawft-core/src/agent/`.
This plan finishes the wiring so the agent core actually drives chat
across the panel, the CLI, and (in time) voice and other channels.

## End-state acceptance criteria

The plan is **done** when all of these are true:

1. `agent.chat` RPC delegates to `clawft-service-agent::AgentService::dispatch` — no inlined tool loop in `daemon.rs`.
2. The dispatch path runs through `clawft-core::agent::AgentLoop::handle_turn` (extracted from `process_message:323`), so the CLI (`weft agent`) and the panel share one execution core.
3. Tool catalog lives in `clawft-tools`; the LLM only ever sees tools the `ToolRegistry` registered for the active identity. No hand-rolled tool JSON in the daemon.
4. Every tool call passes through `clawft-kernel::GovernanceGate::check` (`crates/clawft-kernel/src/gate.rs:381`) with an `EffectVector`. `Defer` and `Deny` decisions are visible in the chat UX.
5. Per-conv state (history, mutex, cancel token) is owned by `AgentService`. Five panels in five conversations are fully parallel; same conversation in two panels is serialized.
6. Conversation turns are persisted as substrate JSONL at `substrate/<node>/derived/chat/<conv_id>/turns/<ulid>`, gated by a `chat` `DerivedWriteGrant` (`crates/clawft-kernel/src/node_registry.rs:78`). Heartbeat publishes to `…/status`.
7. `IdentityLoader` reads `.clawft/SOUL.md` + `.clawft/IDENTITY.md`, with a `BINDING_THREAD_EXCERPT` compile-time pin and SHA-256 hash on the loaded contents. Sandbox hard-denies writes to those paths.
8. ContextRouter is live at v1 (LLM classifier) with v2 (embedding retrieval over `ruvector-diskann@2.1`) shipped behind a feature flag and gated on a 7-day fallback-rate metric. v0 → v1 → v2 → v2.5 → v3 phasing per `docs/research/rvf-context-router.md` is honored; no skipping.
9. `OPENROUTER_API_KEY` path stays working end-to-end. Local llama-server still works when the env is unset.
10. Cancel works: `agent.chat.cancel { conv_id }` aborts the in-flight loop within one tool-call boundary.
11. Boot order is correct: kernel → identity grants → LLM → **agent service** → terminal → UI sentinels. Shutdown drains in-flight loops before `supervisor().shutdown_all`.
12. The `chat-agent-v1.md` plan §2-D1 promise ("reuse `loop_core::run_tool_loop`") is fulfilled and the cutover commit (D3) is named in git history.

## Phase A — Cleanup & seams (no behavior change)

Goal: kill drift, fix bugs, reach a clean spike that's safe to keep
running while the real wiring lands.

| # | Commit | Files | Exit criteria |
|---|---|---|---|
| **A1** | `chore: openrouter takeover (env-driven LlmConfig + dotenvy)` | `crates/clawft-service-llm/src/{client,lib}.rs`, `crates/clawft-core/src/pipeline/service_llm_adapter.rs`, `crates/clawft-weave/{Cargo.toml,src/daemon.rs,src/main.rs}`, `Cargo.lock` | `cargo test -p clawft-service-llm` green (24/24); panel chat works against OpenRouter when `OPENROUTER_API_KEY` is set; local llama-server path unchanged when unset |
| **A2** | `feat(weave): "chat" derived-write grant + conv_id param` | `crates/clawft-weave/src/daemon.rs:209` (add `"chat"` to grants array); `crates/clawft-weave/src/protocol.rs::AgentChatParams` adds `pub conv_id: String` with `#[serde(default)]` and ULID fallback | Spike still works; substrate publish to `derived/chat/...` succeeds for grant-covered topics |
| **A3** | `fix(plugin): canonicalize-prefix sandbox path safety` | `crates/clawft-plugin/src/sandbox.rs:235`; lift `resolve_workspace_path` from spike's `daemon.rs` into the plugin | Tests prove `..` traversal, symlink, and Windows `\\?\` cases all reject correctly; `is_path_readable` and `is_path_writable` use canonicalize + prefix check |
| **A4** | `refactor(weave): route agent.chat tools through clawft-tools::register_all` | `crates/clawft-weave/src/daemon.rs` (delete `build_concierge_tools`, `execute_concierge_tool`, `concierge_read_file`, `concierge_list_directory`); add `into_service_llm_tool` adapter; `crates/clawft-weave/Cargo.toml` adds `clawft-tools` dep | Net `-150 / +60` LoC; demo unchanged; `ReadFileTool`/`ListDirectoryTool` from `crates/clawft-tools/src/file_tools.rs:151,382` are the only path |

## Phase B — Trait seams in `loop_core`

Goal: prepare `loop_core` for RPC reuse without changing any caller.

| # | Commit | Files | Exit criteria |
|---|---|---|---|
| **B1** | `feat(core): ContextRouter trait + NullRouter` | new `crates/clawft-core/src/agent/context_router.rs`; plumbing in `loop_core` to call it (no-op via `NullRouter`) | `cargo test -p clawft-core` green; `ContextDecision { skills, complexity_hint, tool_subset }` shape matches `chat-agent-v1.md:558-583` and `rvf-context-router.md:579-582` |
| **B2** | `feat(core): EffectGate + ConversationSink traits + in-memory impls` | `crates/clawft-core/src/agent/{effects,gate,sink}.rs`; `loop_core::run_tool_loop` calls `gate.check` before `tools.execute`; `loop_core` calls `sink.append_turn` after each turn | All existing tests pass against `NoopGate` + `InMemorySink`; `effect_for_tool(&str, &Value) -> EffectVector` static table populated |
| **B3** | `refactor(core): AgentLoop::handle_turn(req) -> reply` | `crates/clawft-core/src/agent/loop_core.rs:269` `run()` becomes thin wrapper over a new `pub async fn handle_turn(&self, msg: InboundMessage) -> Result<OutboundMessage>` extracted from `process_message:323` | 12 existing `#[tokio::test]` blocks in `loop_core.rs:1153-2354` still pass |

## Phase C — Service materialization & substrate

Goal: production-shaped daemon service. Spike still alive behind a
feature flag.

| # | Commit | Files | Exit criteria |
|---|---|---|---|
| **C1** | `feat(crate): clawft-service-agent skeleton` | new `crates/clawft-service-agent/`: `AgentService { llm, substrate, identity_loader, tool_registry, agent_loop, conv_locks: DashMap<ConvId, Mutex<()>>, cancel_tokens: DashMap<ConvId, CancellationToken> }`, `dispatch`, `cancel`, `shutdown(deadline)` | Crate builds; unit tests cover dispatch + cancel + per-conv lock semantics; per-conv DashMap behaviour matches `chat-agent-v1.md:439-476` |
| **C2** | `feat(weave): DAEMON_AGENT OnceLock + service flag + boot order` | `crates/clawft-weave/src/daemon.rs:39` add static; `:435` register flag; init between LLM (`:512`) and terminal (`:518`); shutdown before `supervisor().shutdown_all` (`:1070`); add `agent.chat.cancel` RPC | `agent-core-chat` feature flag (off): spike runs. (on): service runs. Both work; `weaver --version` boots cleanly under both |
| **C3** | `feat(service-agent): substrate ConversationStore + heartbeat` | `crates/clawft-service-agent/src/substrate_sink.rs` implements `ConversationSink` against `Arc<SubstrateService>`; per-conv tokio interval task publishes status; `TurnContent::{Text \| Audio \| Mixed}` enum (only `Text` constructed today) | `substrate.list derived/chat/<conv>/turns/` returns turns; panel reload rehydrates from substrate; per-turn ULIDs are monotonic |

## Phase D — Identity, gate, cutover

Goal: turn the seams into a real agent. Delete the spike.

| # | Commit | Files | Exit criteria |
|---|---|---|---|
| **D1** | `feat(core): identity-aware system prompt + binding-thread hash` | `crates/clawft-core/src/agent/system_prompt.rs`; SHA-256 hash on `Identity` (replacing `len(soul)+len(identity)` placeholder at `identity.rs:38-39`); `BINDING_THREAD_EXCERPT` compile-time const; sandbox hard-deny on `.clawft/SOUL.md`/`IDENTITY.md` | `loop_core` system prompt is built from `Arc<dyn IdentityProvider>`; spike fallback to `docs/skills/clawft/` removed in F1 |
| **D2** | `feat(weave): per-tool gate.check with EffectVector` | `effect_for_tool` static table in `crates/clawft-tools/src/effects.rs` (or `clawft-core::agent::effects`); `AgentService` calls `gate.check(agent_id, format!("tool.{name}"), ev)` (`crates/clawft-kernel/src/gate.rs:381`) before each dispatch; `Defer`/`Deny` surfaced as tool results | Tools with `EffectVector::risky` deny in restricted environments; witness chain entries appear (`crates/clawft-kernel/src/chain.rs:1009`); `Permit { token }` accompanies the tool execute call |
| **D3** | `feat(weave): default agent-core-chat on; delete spike inline loop` | Flip flag default; delete `handle_agent_chat` body (~360 LoC) and helpers; `agent.chat` becomes one-line `DAEMON_AGENT.dispatch(params).await` | Net delete ~360 LoC; CLI `weft agent` and panel `agent.chat` go through identical code; **end-state criteria 1–7 above are met** |

## Phase E — Router phasing

Goal: chat-agent-v1.md §16's locked v0 → v3 sequence, gated on metrics.
No skipping.

| # | Commit | Files | Exit criteria |
|---|---|---|---|
| **E1** | `feat(core): LlmClassifierRouter (v1)` | `crates/clawft-core/src/agent/context_router.rs` adds `LlmClassifierRouter`; writes clamped `complexity_hint ∈ [-0.3, +0.3]` into `ChatRequest.complexity_boost` (`crates/clawft-core/src/pipeline/tiered_router.rs:585`) | v0 `NullRouter` → v1 swap is a config flip; metric harness logs fallback rate to substrate |
| **E2** | `feat(core): EmbeddingRouter (v2)` | uses workspace-pinned `ruvector-diskann@2.1` (already at workspace `Cargo.toml:171`); `crates/clawft-core/src/embeddings/api_embedder.rs` + `hash_embedder.rs` floor (no new deps) | v1 → v2 promotion gate: 7-day fallback rate < 25% before flip per `chat-agent-v1.md:393-405` |
| **E3** | `feat(core): HybridRouter (v2.5) plumbing` | rerank step deferred (placeholder); router selects between archetype-routing (would use `ruvllm-wasm` 11-pattern HNSW once stabilized) and skill retrieval (diskann) | v3 (MicroLoRA) explicitly deferred until `ruvllm` lifts the 11-pattern HNSW cap (`docs/research/rvf-context-router.md:118-128`) |

## Phase F — Identity tooling, journal, witness

Goal: identity becomes operable, not just loaded.

| # | Commit | Files | Exit criteria |
|---|---|---|---|
| **F1** | `feat(weaver): init seeds .clawft/SOUL.md + IDENTITY.md + journal grant` | `crates/clawft-weave/src/commands/init_cmd.rs`; new substrate topic `soul_journal` with `DerivedWriteGrant` | `weaver init` produces a runnable agent with no `docs/skills/clawft/` fallback; D1 fallback path deletable |
| **F2** | `feat(weaver): soul promote command` | `crates/clawft-weave/src/commands/soul_cmd.rs`; reads `SOUL.journal.md` (substrate-backed), prints diff, applies on confirmation; appends witness chain entry | Agent can self-observe drift; human can promote it explicitly; `chain.rs:1009` append is testable |
| **F3** | `feat(service-agent): WitnessRecord assertions in chat path tests` | integration test under `crates/clawft-service-agent/tests/`: drive a chat turn, assert chain entries for `gate.check` decisions and `soul promote` | Audit trail is testable, not just live |

## Risk register

1. **`process_message` extraction (B3) breaks 12 existing async tests in `loop_core.rs`.** Budget one commit's worth of test repair; do not bundle with the trait seam commits.
2. **`OPENROUTER_API_KEY` runtime swap.** Today `daemon_llm()` captures the client once at boot. Once `AgentService` holds an `Arc<LlmClient>`, runtime env changes go stale. Fix by wrapping in `Arc<RwLock<LlmClient>>` and refreshing on `control.set_enabled("llm", _)` cycles. Ships in C2.
3. **`ConversationSink` vs `agent/memory.rs` confusion.** `memory.rs` (461 LoC) is the cross-conversation distilled-facts store; `ConversationSink` is per-turn substrate. They never share a path (per `chat-agent-v1.md:348-355`). Document this clearly in C3's commit message.
4. **Gate `Defer` UX.** `Defer { reason }` requires a human-in-the-loop hook (`crates/clawft-kernel/src/gate.rs:14-34`). v1 lands as "tool result becomes the defer reason; loop continues" (model can re-plan). Real interactive `Defer` is a v1.1 follow-up needing panel UI.

## Sequencing & parallelism

Execution uses `git worktree` per parallelizable commit. Worktrees
branch off `development-0.7.0` as `agent-core/<commit-id>` (e.g.
`agent-core/a3-sandbox`), land green, and merge back via `--no-ff` on
completion of each phase.

| Phase | Sequential | Parallel-able in worktrees |
|---|---|---|
| A | A1 → A2 | A3 + A4 after A2 |
| B | B3 first (loop_core extraction) | B1 + B2 after B3 |
| C | C1 → C2 → C3 | none — daemon.rs touches |
| D | D1 → D2 → D3 | D1 + D2 if `loop_core` touchpoints don't overlap |
| E | E1 → E2 → E3 | none — same module |
| F | F1 → F2 → F3 | F1 + F3 |

Every commit must:

- pass `scripts/build.sh check`
- pass `scripts/build.sh clippy` (warnings-as-errors)
- pass `scripts/build.sh test` for affected crates
- preserve the OpenRouter takeover (commit A1) — `OPENROUTER_API_KEY` env path stays live
- preserve the panel UX (panel asks → daemon answers) until D3 cutover, after which the daemon answers via `loop_core` instead of inlined spike

## Cutover semantics (D3)

D3 is a single commit and the only behavior change in the whole arc.
Until D3, both the spike (default) and the agent-core path (feature
flag `agent-core-chat`) work. D3 deletes the spike and flips the
default. After D3, `daemon.rs::handle_agent_chat` is one line:

```rust
DAEMON_AGENT.get().expect("agent service wired at boot")
    .dispatch(params).await
```

Rollback strategy: if D3 regresses a release-blocking flow, revert D3
alone — the spike's source has already been deleted, but C2 left the
spike path behind a feature flag specifically so the revert is `git
revert D3` and a flag flip, not a re-implementation.

## Cross-references

- `docs/plans/chat-agent-v1.md` — predecessor; §14 commits 1-9 are subsumed by Phases A-D.
- `docs/research/rvf-context-router.md` — locks router phasing and the `complexity_hint` clamp.
- `docs/handoff.md` — bug hunt + OpenRouter takeover from sessions 2026-04-26/27.
- `~/dev/openclaw-aws-workshop/docs/integration-architecture.md` — the wire shape.
- `crates/clawft-core/src/agent/loop_core.rs` — the production loop being adopted.
- `crates/clawft-weave/src/daemon.rs::handle_agent_chat` — the spike being retired in D3.
- Symposium panelists (2026-04-27): findings live in conversation; key cites are inline in this doc.
