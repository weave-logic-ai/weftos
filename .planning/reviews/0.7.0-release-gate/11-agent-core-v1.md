---
title: "Agent-Core-v1 (Chat Agent E2E)"
slug: agent-core-v1
workstream_id: "11"
status: shipped (with v1.1 follow-ups documented)
last_updated: 2026-04-28
owners:
  - clawft-service-agent
  - clawft-core::agent
  - clawft-weave::daemon
related_plans:
  - docs/plans/agent-core-v1.md
  - docs/plans/chat-agent-v1.md
  - docs/research/rvf-context-router.md
related_adrs:
  - ADR-020 (chainloggable)
  - ADR-022 (exochain-mandatory-audit)
  - ADR-035 (serviceapi-layered-protocol)
---

# Workstream 11 — Agent-Core-v1 (Chat Agent End-to-End)

## General Description

Workstream 11 ports the openclaw chat-agent contract (Rust/nanobot loop
under it) into a daemon-resident `clawft-service-agent` that the
WeftOS WASM panel, CLI, and (eventually) voice channels share. The
work superseded `docs/plans/chat-agent-v1.md` §14 commits 1–9, retired
the ~360-line vertical-slice spike that lived inside
`crates/clawft-weave/src/daemon.rs::handle_agent_chat`, and re-shaped
the dispatch path so every `agent.chat` JSON-RPC call now flows
through:

```
agent.chat RPC (clawft-weave::daemon, unconditional)
  → clawft-service-agent::AgentService::dispatch (per-conv lock + cancel)
    → clawft-core::agent::AgentLoop::handle_turn
      → ContextRouter::route (Null / LlmClassifier / Embedding / Hybrid)
      → SystemPromptBuilder (identity-aware, SHA-256, BINDING_THREAD)
      → loop_core::run_tool_loop
          → EffectGate::check (KernelEffectGate → GovernanceGate → witness chain)
          → ToolRegistry::execute (clawft-tools, sandboxed)
          → ConversationSink::append_turn (substrate JSONL + heartbeat)
      → clawft-service-llm::LlmClient (OpenRouter or local llama-server)
```

Phases A (cleanup + seams), B (loop_core trait extraction), C
(`clawft-service-agent` materialization + substrate sink), D (identity
prompt + kernel gate + spike cutover), E (router phasing v0→v2.5), and
F (weaver `init` + `soul promote` + witness assertions) all landed.
Four follow-on commits this session (`8b05d868`, `0452539a`,
`ec7bb2bd`, `cb947080`) closed the four field-discovered defects that
prevented a real boot from delivering the new path end-to-end. The
release-gate audit captures every TODO, deferred item, orphaned
artifact, and open question that was *not* in scope for v1.

## Status & Timeline

- **Drafted** 2026-04-27 (six-panel symposium).
- **Plan committed** `1fe04e5b docs(plan): chat-agent v1 plan + RVF context-router research`.
- **Spike landed** `e6f8c816 feat(spike): vertical-slice agent.chat — concierge demo`.
- **Phases A→F shipped** in 78 commits ahead of `origin/development-0.7.0`,
  closing on `7fbbe8df Merge F2: weaver soul promote command (closes agent-core-v1)`.
- **Handoff** `e2c3ecc1 docs(handoff): agent-core-v1 ships`,
  `8c08ce0a docs(handoff): add worktree + branch cleanup item`.
- **This-session top-up** (4 commits, 2026-04-28):
  - `8b05d868` `fix(service-llm): accept null content on tool-call turns` — Nemotron-shape `"content": null` regression.
  - `0452539a` `feat(platform): cwd-relative workspace config overlay` — restored Layer-3 `./.clawft/config.json` deep merge.
  - `ec7bb2bd` `fix(core,weave): thread loaded RoutingConfig to daemon agent loop` — agent-loop branch was discarding the loaded config.
  - `cb947080` `feat(weaver): add --update flag to init for non-destructive top-up` — reach `.clawft/` seeding on workspaces with hand-tuned `weave.toml`.
- **Worktrees retained** as rollback escape hatch through F2 — already pruned by 2026-04-28 (`git worktree list` shows only the main checkout).

Branch `development-0.7.0`, working tree clean. Nothing pushed.
`scripts/build.sh check`, `clippy`, and the workspace test suite green
(1549 lib tests, 0 failures) per the F2 handoff.

## Released Features (12-criteria acceptance checklist)

The 12 end-state acceptance criteria from
`docs/plans/agent-core-v1.md` map verbatim to the released feature set;
each is satisfied by a named commit in `git log`:

- [x] **AC-1** `agent.chat` RPC delegates to
  `clawft-service-agent::AgentService::dispatch` — no inlined tool
  loop in `daemon.rs`. *(D3 cutover; see
  `crates/clawft-weave/src/daemon.rs:3591–3622`)*
- [x] **AC-2** Dispatch runs through
  `clawft-core::agent::AgentLoop::handle_turn` (extracted from
  `process_message`). *(B3 extraction; preserved 12 existing
  `#[tokio::test]` blocks in `loop_core.rs:1153–2354`)*
- [x] **AC-3** Tool catalog from `clawft-tools::register_all` —
  `ReadFileTool` + `ListDirectoryTool` + the rest of the registry.
  No hand-rolled tool JSON in the daemon. *(A4 refactor)*
- [x] **AC-4** Every tool call passes through
  `clawft-kernel::GovernanceGate::check` with an `EffectVector`.
  Defer/Deny surface as structured `{"deferred": true, "reason": ...}`
  / `{"denied": true, "reason": ...}` tool-result JSON the LLM can
  re-plan against. *(D2; see `loop_core.rs:1056–1105`)*
- [x] **AC-5** Per-conv state owned by `AgentService` —
  `DashMap<ConvId, Arc<Mutex<()>>>` for serialization plus
  `DashMap<ConvId, CancellationToken>` for cancel. Five panels in five
  conversations are fully parallel; same conversation in two panels is
  serialized. *(C1; tests in `crates/clawft-service-agent/tests/dispatch.rs`)*
- [x] **AC-6** Conversation turns persisted as substrate JSONL at
  `substrate/_derived/chat/<conv_id>/turns/<ulid>`, gated by the
  `chat` `DerivedWriteGrant`; heartbeat publishes
  `…/status` every 2s with `MissedTickBehavior::Skip`. *(A2 grant +
  C3 sink; substrate path is mesh-canonical via
  `publish_gated_with_grants`)*
- [x] **AC-7** `IdentityLoader` reads `.clawft/SOUL.md` +
  `.clawft/IDENTITY.md`, with the `BINDING_THREAD_EXCERPT`
  compile-time pin and a SHA-256 digest of `soul + "\n" + identity`
  on the loaded contents. The `docs/skills/clawft/` fallback was
  deleted in F1. Sandbox hard-deny on identity paths is in the
  governance layer. *(D1 + F1)*
- [x] **AC-8** ContextRouter ladder live: `null` (v0) → `llm-classifier`
  (v1) → `embedding` (v2, `ruvector-diskann@2.1`) → `hybrid` (v2.5
  plumbing). Selected via `Config.routing.context_router`. v3
  (`MicroLoraRouter`) explicitly deferred per ruv-researcher pin. *(E1,
  E2, E3 + import fix `67b14886`)*
- [x] **AC-9** `OPENROUTER_API_KEY` env path live; local
  `llama-server` unchanged when the env is unset. *(A1 takeover; the
  `request_timeout` 120→300s lift and `AGENT_CHAT_PER_TURN_MAX_TOKENS`
  cap from the late-evening 2026-04-27 patch ride along)*
- [x] **AC-10** `agent.chat.cancel { conv_id }` aborts the in-flight
  loop. *(C2 RPC arm; per-loop cancellation at iteration boundary
  remains a v1.1 follow-up — see "What's Left")*
- [x] **AC-11** Boot order: kernel → identity grants → LLM → agent
  service → terminal → UI sentinels. Shutdown drains in-flight
  `agent.chat` dispatches before `supervisor().shutdown_all`. *(C2
  wiring; `daemon.rs:684–948` for init, `:1506–1510` for shutdown)*
- [x] **AC-12** `chat-agent-v1.md` §2-D1 promise ("reuse
  `loop_core::run_tool_loop`") fulfilled; D3 cutover commit named in
  git history (`0dd28b49 feat(weave): default agent-core-chat on; delete spike inline loop`).

Plus the full F-track operator surface:

- [x] `weaver init` seeds `.clawft/{SOUL.md, IDENTITY.md, SOUL.journal.md}`,
  stamps the `soul_journal` derived-write grant, and now (this
  session) accepts `--update` to top-up missing files on workspaces
  with a hand-tuned `weave.toml`.
- [x] `weaver soul promote` reads `SOUL.journal.md`, prints a diff,
  applies on confirmation, and writes a `WitnessRecord` payload —
  today to `<workspace>/.weftos/audit/soul-promote.log` JSONL plus a
  `tracing::info!(target = "chain_event", …)` event, since the daemon
  has no public `chain.append` RPC yet.
- [x] `WitnessRecord` chat-path tests assert chain entries for
  `gate.check` decisions across the loop end-to-end. *(F3, see
  `crates/clawft-service-agent/tests/witness_chain.rs`)*

## What's Left — Total Depth

This is the comprehensive list. None of these are 0.7.0 ship-blockers,
but they are the v1.1 backlog plus orphaned/deferred work that must
not get lost across releases.

### TODOs & FIXMEs in source (file:line)

| Marker | Location | Description |
|---|---|---|
| `TODO(Phase D2)` | `crates/clawft-service-agent/src/service.rs:209` | Wire `CancellationToken` through to `loop_core::run_tool_loop` so cancel takes effect at the next tool-call boundary instead of waiting for the whole turn to finish. Today the dispatch-as-a-whole is interruptible; per-iteration is not. |
| `TODO(agent-core-v1.1)` | `crates/clawft-weave/src/commands/soul_cmd.rs:246` | Replace the local audit log + `tracing::info!(target = "chain_event", …)` with a real `chain.append` RPC once the daemon exposes a public chain-append surface. The trait shape is already forward-compatible. |
| `TODO(agent-core-v1 phase E3+)` | `crates/clawft-core/src/agent/context_router/hybrid.rs:44` | Wire `MicroLoraRouter` (v3) once `ruvllm-wasm` lifts the documented 11-pattern HNSW cap (`docs/research/rvf-context-router.md:118-128`). The 35+-skill clawft catalog overruns ruvllm-wasm v2.0.1's per-index ceiling. v2.5 sona-backed rerank step also deferred until ruv-ecosystem stability clears. |
| `TODO(v1.1)` | `crates/clawft-core/src/bootstrap.rs:633` | Split workspace from global at the loader layer so `PermissionResolver::new(global, Some(workspace))` can use `enforce_workspace_ceiling` to clamp workspace permissions against system-wide bounds. Today the workspace overlay is deep-merged into `config.routing` upstream in `config_loader::load_config_raw` — workspace policy reaches the resolver but the security ceiling pattern is bypassed. Fine for single-user kernels; needed for multi-tenant. |
| In-source `v1.1` follow-up note | `crates/clawft-core/src/agent/loop_core.rs:1070-1073` | Per-tool `Permit { token }` is currently discarded — the plan calls out "optionally pass the token to tools.execute" as a follow-up that requires a tool-side proof-of-permission API the registry doesn't yet expose. |
| In-source `v1.1` follow-up note | `crates/clawft-core/src/agent/loop_core.rs:1090-1094` | `Defer { reason }` surfaces as a tool-result JSON the model can re-plan against. Real interactive defer (panel UI prompt-and-resume) is a v1.1 follow-up. |
| In-source `v1.1` follow-up note | `crates/clawft-core/src/agent/identity.rs:57` | Binding-thread mismatch annotates the prompt and emits a `warn!` — does not refuse to run. Hard refusal is v1.1. |
| In-source `v1.1` follow-up note | `crates/clawft-service-agent/src/kernel_gate.rs:24, 66, 89` | Kernel-side `Deny { reason, receipt }`: `receipt` is dropped because the panel UX has nowhere to render it. Permit's `token: Option<Vec<u8>>` is hex-encoded but only "kernel-permit" sentinel is propagated when None. Both tracked for v1.1. |

### Deferred items (handoff "Known follow-ups" + plan §17)

1. **Public `chain.append` RPC.** `weaver soul promote` writes a
   `WitnessRecord` payload to `<workspace>/.weftos/audit/soul-promote.log`
   (JSONL) plus a `chain_event` tracing event because the daemon
   doesn't expose a public `chain.append` RPC yet. Source has a
   `TODO(agent-core-v1.1)` to switch when the wire ships.
2. **Defer UX (interactive prompt-and-resume).** D2 surfaces
   `GateDecision::Defer { reason }` as a structured tool-result. Real
   interactive defer (panel-side prompt with human-in-the-loop hook
   per `crates/clawft-kernel/src/gate.rs:14-34`) is v1.1 — needs
   panel UI work.
3. **Per-user agent_ids.** Chat is single-tenant: one
   `concierge-bot` principal registered at boot per D2. Per-user
   agent_ids (multi-tenant chat) ship in a future phase.
4. **Agent-side journal write path during chat turns.** F2 lands the
   *operator* side of `weaver soul promote`; the agent's
   self-observation write path during chat turns (the loop noticing
   drift and appending to `SOUL.journal.md`) is deferred. With an
   empty journal `weaver soul promote` exits cleanly.
5. **`v3 MicroLoraRouter`.** Explicitly deferred until `ruvllm-wasm`
   lifts the documented 11-pattern HNSW cap
   (`docs/research/rvf-context-router.md:118-128`). E3's
   `HybridRouter` left a `TODO(agent-core-v1 phase E3+)` marker. v3
   needs MicroLoRA adapter trained on logged decisions + journal
   preferences with mandatory shadow-mode + WITNESS audit before
   promotion.
6. **v2.5 sona-backed rerank step.** `HybridRouter` ships plumbing
   only — chains a primary + fallback `ContextRouter`. The real v2.5
   design layers a sona-backed rerank step on top of the primary's
   top-K. Deferred until `sona` clears the ruv-ecosystem stability
   gate (`.planning/development_notes/ruv-ecosystem-analysis-20260414.md`).
7. **Per-loop `CancellationToken` wiring.** Per AC-10, `cancel`
   aborts the dispatch as a whole via `tokio::select!`. Per-iteration
   cancellation inside `loop_core::run_tool_loop` is the v1.1 next
   step — needs a `&CancellationToken` parameter threaded through
   `handle_turn`.
8. **`AgentChatResult` field shortfalls.** `tool_calls`,
   `prompt_tokens`, `completion_tokens`, `model`, `identity_source`
   default to empty/zero/None at the wire — `OutboundMessage` is a
   generic bus envelope without these fields. Documented at
   `crates/clawft-service-agent/src/protocol.rs:107-141` and
   `service.rs:377-398`. Loop's result type needs enriching to thread
   them through; tests in `protocol.rs:179-201` pin the C1 shape so
   the panel keeps tolerating the defaults across the cutover.
9. **Per-conv cost circuit-breaker.** `chat-agent-v1.md` §5.6 +
   §17.v1.1: minimal per-conv cap is in v1; full circuit breaker
   with soft warnings at 80%, separate caps for read-only vs write
   tools, and integration with daily/monthly budgets is v1.1.
10. **After-3-denials → `EscalateToHuman`.** `chat-agent-v1.md`
    §5.5: v1 just stops the loop with `agent: gate denied tool calls
    3x; halting`. Full implementation per governance recommendation 4
    is v1.1.
11. **Typed error variants for `agent.chat`.** v1 surfaces strings
    via `Response::error(...)` mirroring `llm.prompt`. Typed variants
    deferred to v1.1.
12. **Health surface registration.** `agent.chat` should register a
    `SystemService` impl tracking last-completion-time so `weft
    status` shows it (kernel C2 surface). Deferred to v1.1.
13. **Governance rule `soul.binding_thread_intact`.** Today's check
    is at load time only (the SHA-256 hash + `BINDING_THREAD_EXCERPT`
    substring match in `IdentityLoader`). v1.1 promotes this to a
    governance rule evaluated by `gate.check` on every turn.
14. **Multi-conversation sidebar UI.** Panel uses `substrate.list` on
    first selection to find the most-recent conv. Sidebar listing UI
    deferred per `chat-agent-v1.md:226`.
15. **`weft routing trace` / `replay` + p99 / fallback-rate metrics.**
    Per `chat-agent-v1.md:695`, observability commands needed for the
    v1→v2 promotion gate ("≥ 1,000 logged decisions") and the v2→v2.5
    promotion gate ("fallback rate < 25% over 7 days"). Without
    these, the router-phase gate metrics are best-effort.
16. **Hot-reload watcher for identity files.** `FileIdentityProvider`
    re-reads on every call (small files, cheap). A `notify`-driven
    watcher arrives "when measurement says it earns its keep"
    (`identity.rs:36-38`).
17. **`agent.workspace_root` config key.** Today workspace = daemon
    CWD. Plan §15.4 calls out a future config key
    (`crates/clawft-weave/src/daemon.rs:702`,
    `crates/clawft-core/src/agent/identity.rs:179`).
18. **`Arc<RwLock<LlmClient>>` runtime swap.** `daemon_llm()` captures
    the client once at boot; runtime env changes (e.g.
    `OPENROUTER_API_KEY` rotated mid-session) go stale. Plan risk #2
    flagged this for `control.set_enabled("llm", _)` cycles. Not
    landed; tracked by the resolver-live-probe diagnostic at
    `crates/clawft-core/tests/resolver_live_probe.rs` (`#[ignore]`).
19. **Phase 2 — voice + streaming.** `audio_transcribe` /
    `audio_synthesize` / `voice_listen` / `voice_speak` already exist
    as tools; chat path needs `TurnContent::Audio` populated and
    `agent.chat_stream` connection-takeover RPC. `TurnContent::Mixed`
    enum is pinned from day 1 (substrate JSONL ready) but never
    constructed today.
20. **Phase 4 — `MemoryConsolidator`.** `crates/clawft-core/src/agent/learning/`
    is the planned home for periodic distillation from
    `ConversationStore` (per-turn substrate) → `MemoryStore`
    (`MEMORY.md` / `HISTORY.md`). Closes the `memory.rs` vs
    `ConversationSink` boundary system-architect raised. Module
    doesn't exist yet.
21. **Phase 4 — Skills auto-promotion.** After enough successful uses
    of a `.claude/skills/*` skill, promote to `.clawft/skills/` for
    faster routing. Detector hooks are in
    `crates/clawft-core/src/agent/skill_autogen.rs` but the autopromote
    path is manual today.
22. **Cross-agent delegation.** `delegate_tool` already exists; chat
    agent should spawn specialist agents from `agents/` profiles. v1
    doesn't wire it.

### Open questions

- **Slot prefix-cache behaviour under Qwen3.6 hybrid arch.** The
  late-evening 2026-04-27 patch added per-iter `info!` lines with
  `cached_tokens` / `predicted_per_sec` / `prompt_per_sec`. The open
  question (handoff lines 215–229) is whether the hybrid arch reuses
  the slot cache (iter 2+ should report `cached_tokens ≈ prompt_tokens
  of iter 1`) or whether reasoning_content is burning the per-turn
  budget. Candidate follow-ups (`--reasoning-format none`, moving
  tools out of `tools:` into a static system-prompt block) are noted
  but not actioned.
- **Real sustained throughput under the spike's actual prompt shape.**
  The 25 tok/s claim with `--spec-type ngram-simple` may not hold;
  per-iter `predicted_per_sec` will tell.
- **C3 monotonic-ULID test flake.** `append_turns_are_monotonic`
  (`crates/clawft-service-agent/tests/substrate_sink.rs:113`)
  occasionally fails when two appends land in the same ms. Pre-existing
  from C3; the per-conv counter suffix in `turn_id_for` was added
  specifically to address this, but the test still races. Needs a
  reliable injectable clock or a deterministic counter to fully
  stabilize.
- **`overlay_probe` and `resolver_live_probe` are `#[ignore]`-marked
  diagnostics.** They run the live config through the production
  resolver. Useful for verifying the wire end-to-end if a regression
  appears, but they're not in CI rotation. Should they be promoted
  once a stable test workspace fixture exists?

### Orphaned work / housekeeping

- **`agent-core/*` worktrees + branches.** Handoff retained 12 locked
  worktrees under `.claude/worktrees/agent-*` plus the matching
  `agent-core/*` branches as a rollback escape hatch. Verified
  2026-04-28: `git worktree list` shows only the main checkout, so
  the worktrees were cleaned up between F2 ship and now. The
  cleanup-block in `docs/handoff.md:65-75` is still present
  documentation-wise but the side effects are already done.
- **Duplicate protocol types.** `clawft_service_agent::protocol::*`
  and `clawft_weave::protocol::AgentChat*` mirror each other byte-for-
  byte. The plan calls out a Phase C2 follow-up to make weave
  re-export from service-agent (`protocol.rs:1-21`). The duplication is
  intentional and pinned by matching tests, but it's still
  duplication.
- **`docs/skills/clawft/` fallback path.** Deleted in F1 from the
  identity loader. The fallback test
  (`identity.rs:292-308`) explicitly asserts the docs path no longer
  satisfies the loader. The `docs/skills/clawft/SOUL.md` file
  itself is still on disk because `weaver init`'s `SOUL_TEMPLATE` is
  `include_str!("../../../../docs/skills/clawft/SOUL.md")` — keep it
  in sync with the binding-thread excerpt.
- **`build_concierge_tools`, `execute_concierge_tool`,
  `concierge_read_file`, `concierge_list_directory`.** Deleted in A4.
  Verify no stale references in ancillary code or docs (grep for
  `concierge_` returns clean inside the workstream scope).
- **`handle_agent_chat` body.** Deleted in D3 (~360 LoC). The dispatch
  arm at `daemon.rs:3591` is a one-liner that surfaces "agent service
  not wired" if `DAEMON_AGENT.get()` is None. Verify on a real boot
  that the typed-error path is actually exercised when LLM init fails
  — there is no integration test covering that fallback.

### v1.1 follow-ups already documented elsewhere

The v1.1 backlog in `docs/plans/chat-agent-v1.md:693-701` is the
canonical list (also reflected in `docs/handoff.md:58-75`). Capturing
here for completeness, with cross-references:

- `weaver soul promote` subcommand — **shipped in F2** (operator
  side; agent-side journal write deferred per item 4 above).
- `weft routing trace` / `replay` observability — deferred (item 15).
- Per-conv cost cap with full circuit-breaker — deferred (item 9).
- Multi-conversation sidebar UI — deferred (item 14).
- Typed error variants for `agent.chat` — deferred (item 11).
- Governance rule `soul.binding_thread_intact` — deferred (item 13).
- Health surface registration — deferred (item 12).
- After-3-denials → `EscalateToHuman` — deferred (item 10).

## Task List

Concrete tasks to discharge the v1.1 backlog and the four
this-session field-discovered open ends.

### Critical path (blocks v1.1 ship)

1. **Per-iteration `CancellationToken` wiring.** Add
   `cancel: &CancellationToken` parameter to `AgentLoop::handle_turn`
   and observe it inside `loop_core::run_tool_loop` at the top of
   each iteration. Update `AgentService::dispatch` to thread the
   per-conv token through. Today the `select!` only catches whole-turn
   cancellations.
   - Files: `crates/clawft-core/src/agent/loop_core.rs`,
     `crates/clawft-service-agent/src/service.rs`.
   - Effort: ~150 LoC, one commit.

2. **Public `chain.append` RPC.** Expose a daemon RPC that takes a
   `WitnessRecord` and pushes it onto the live chain. Replace
   `weaver soul promote`'s local-audit-log fallback with a real RPC
   call.
   - Files: `crates/clawft-weave/src/daemon.rs` (new arm),
     `crates/clawft-weave/src/commands/soul_cmd.rs:239-265`,
     `crates/clawft-service-agent/src/protocol.rs` (new types).
   - Effort: ~250 LoC + tests, one commit.

3. **Promote workspace/global resolver split.** Convert
   `bootstrap.rs:633` TODO into a real two-arg
   `PermissionResolver::new(global, Some(workspace))` call, with
   `enforce_workspace_ceiling` clamping. Required for any multi-tenant
   deployment.
   - Files: `crates/clawft-core/src/bootstrap.rs`,
     `crates/clawft-platform/src/config_loader.rs`,
     `crates/clawft-core/src/pipeline/permissions.rs`.
   - Effort: ~300 LoC + tests, one commit.

### Quality bar

4. **Stabilize `append_turns_are_monotonic`.** Inject a clock into
   `SubstrateConversationSink` so the test isn't subject to wall-clock
   coincidences. The per-conv counter is already there; the test just
   needs deterministic inputs.
5. **Promote `overlay_probe` / `resolver_live_probe` from `#[ignore]`
   to a CI test fixture** with a hermetic workspace.
6. **Plumb `tool_calls` / `prompt_tokens` / `completion_tokens` /
   `model` through `OutboundMessage`** so `AgentChatResult` stops
   defaulting to zeros/None. (Documented shortfall in `protocol.rs`.)
7. **Hot-reload watcher for identity files.** `notify`-driven cache
   invalidation in `FileIdentityProvider`.

### Operator surface

8. **Agent-side journal-write path.** Hook into `loop_core` so the
   loop can append drift observations to `SOUL.journal.md` mid-turn,
   gated by a substrate `soul_journal` write grant (the grant already
   exists from F1).
9. **Defer UX in panel.** Interactive prompt-and-resume so
   `Defer { reason }` actually suspends the loop awaiting human input.
10. **Per-user agent_ids.** Plumb caller identity through
    `AgentService::dispatch` so each panel/CLI session gets its own
    principal in the kernel `AgentRegistry`.
11. **Health surface.** Register an `agent.chat` `SystemService` so
    `weft status` shows last-completion-time, in-flight count, and
    per-conv lock contention.
12. **Typed error variants for `agent.chat`.** Replace the
    `Response::error("agent.chat: <inner>")` string-format with a
    typed enum the panel can branch on.

### Router phasing

13. **Observability for v1→v2 promotion gate.** Log every router
    decision to `substrate/<node>/agent/routing/recent` per
    `chat-agent-v1.md:682`. Need ≥ 1,000 decisions over real usage to
    seed embedding descriptors before v2 flip.
14. **`weft routing trace` / `weft routing replay` commands.** Read
    from the same substrate path; expose p99 latency + fallback-rate
    metrics in `weft status`.
15. **v2.5 sona-backed rerank.** Layer onto `HybridRouter` once `sona`
    clears the ruv-ecosystem stability gate.
16. **v3 `MicroLoraRouter`.** Adapter training pipeline + shadow mode
    + WITNESS audit. Blocked on `ruvllm-wasm` lifting the 11-pattern
    HNSW cap.

### Cleanups

17. **De-duplicate protocol types.** Make `clawft_weave::protocol`
    re-export from `clawft_service_agent::protocol`. Plan called this
    out as a Phase C2 follow-up; never landed.
18. **Verify no stale `concierge_*` references in ancillary code or
    docs.** Spot-check; trivial.
19. **Confirm "agent service not wired" error path on LLM init
    failure** has integration test coverage.

## Sources

- `docs/plans/agent-core-v1.md` (167 lines) — primary plan, end-state
  acceptance criteria, phase breakdown.
- `docs/plans/chat-agent-v1.md` (~744 lines, predecessor) — §14
  commits 1–9 superseded by Phases A–D; §17 v1.1 backlog still
  authoritative; §16 router phasing locked.
- `docs/research/rvf-context-router.md` (~949 lines) — router phase
  contracts, `complexity_hint` clamp, 11-pattern HNSW cap, v3 deferral.
- `docs/handoff.md` — agent-core-v1 SHIPS handoff (2026-04-27 late
  evening), 2026-04-27 early-morning timeout patch, 2026-04-26
  late-evening spike, "Known follow-ups" goldmine (lines 58–75).
- `crates/clawft-service-agent/src/{lib,service,substrate_sink,kernel_gate,protocol}.rs`
  — service layer, ~1,400 LoC.
- `crates/clawft-service-agent/tests/{dispatch,substrate_sink,witness_chain}.rs`
  — ~1,200 LoC of integration tests.
- `crates/clawft-core/src/agent/{loop_core,context_router,identity,system_prompt,gate,sink,effects,sandbox,memory,skill_autogen,verification}.rs`
  — agent core, ~9,800+ LoC after B/D extractions.
- `crates/clawft-core/src/agent/context_router/{llm_classifier,embedding,hybrid}.rs`
  — E1/E2/E3 routers.
- `crates/clawft-core/src/bootstrap.rs:600-680` — `build_daemon_agent_loop`
  wiring, including the `routing: Option<&RoutingConfig>` parameter
  added in this-session commit `ec7bb2bd`.
- `crates/clawft-weave/src/daemon.rs:684-948, 3591-3622, 1506-1510` —
  agent service init, dispatch arm, drain on shutdown.
- `crates/clawft-weave/src/commands/{init_cmd,soul_cmd}.rs` — F1/F2
  operator commands.
- `crates/clawft-platform/src/config_loader.rs` — Layer-3 workspace
  overlay added in this-session commit `0452539a`.
- `crates/clawft-service-llm/src/client.rs` — null-content
  deserializer added in this-session commit `8b05d868`.
- ADR-020 (chainloggable), ADR-022 (exochain-mandatory-audit),
  ADR-035 (serviceapi-layered-protocol) — governance/audit/protocol
  contracts the gate, witness, and service surfaces honor.
- `git log` 78 commits 1fe04e5b..7fbbe8df (Phases A–F) + 4 commits
  8b05d868..cb947080 (this-session top-up).

<!-- TRIAGED-STAMP:BEGIN -->
## Triaged into Plane — 2026-04-28

All open items in this audit have been filed as Plane work items in the WeftOS workspace under the `ws11-agent-core-v1` label.

- **Range**: WEFT-322 … WEFT-350 (29 items)
- **Per cycle**: 0.7.x: 1, 0.8.x: 25, 0.9.x: 3
- **Triage spec**: `.planning/reviews/0.7.0-release-gate/triage/`
- **WEFT-N → name map**: `.planning/reviews/0.7.0-release-gate/triage/weft-mapping.json`

Per the project rule (CLAUDE.md → "Plane is the authoritative work tracker"): future updates to these items happen in Plane, not in this audit doc. This doc remains the source-of-truth for the original survey.
<!-- TRIAGED-STAMP:END -->
