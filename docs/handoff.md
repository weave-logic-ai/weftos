# Session handoff — 2026-04-26 (late evening)

Pick-up doc for the next session. Reflects `development-0.7.0` at
commit `e6f8c816`, two new commits on top of the evening's egui-0.34
+ agent-orphans batch:

- `1fe04e5b` `docs(plan): chat-agent v1 plan + RVF context-router research`
- `e6f8c816` `feat(spike): vertical-slice agent.chat — concierge demo`

This session was a single arc: design → research → multi-expert
review → spike. No code shipped beyond the spike; the production
machinery (commits 1-9 of the plan) is queued for next session.

The full-workspace `cargo test --workspace` ran green this time
(exit 0). The `clawft-kernel hnsw_eml` benchmark tests that have
deadlocked previously did finish — they're slow, not stuck. Targeted
tests still recommended for fast iteration:

```bash
cargo test -p clawft-core -p clawft-weave -p clawft-gui-egui --lib
```

---

## What's new this session

### Commit 1 — `docs(plan): chat-agent v1 plan + RVF context-router research` (`1fe04e5b`)

Two design artifacts that scope the WeftOS Concierge chat-agent
work — the agent that lets the user actually have a conversation
with WeftOS through the WASM panel in Cursor.

`docs/plans/chat-agent-v1.md` (~744 lines):
- 19 sections, decisions locked, file-level scope, commit boundaries.
- Vertical-slice spike (commit 0, this session) inserted before the
  trait-and-module commits (1-9, next session) so the user-visible
  win lands first and de-risks the wire path.
- Phased router rollout: **v0 NullRouter → v1 LLM classifier → v2
  embedding retrieval → v2.5 hybrid → v3 MicroLoRA**, with concrete
  promotion gates (e.g. v2 → v2.5 needs fallback rate < 25% over
  7 days). No skipping.
- Substrate per-turn JSONL at
  `substrate/<node>/derived/chat/<conv_id>/turns/<ulid>`. Read path:
  `substrate.list` is authoritative; `substrate.subscribe` is
  best-effort tail (kernel fanout drops on overflow).
- Identity loader with append-only `SOUL.journal.md` + binding-thread
  hash pin (compile-time `const`) + sandbox hard-deny on
  `.clawft/SOUL.md` / `IDENTITY.md` paths even under writable roots.
- `gate.check` + `EffectVector` mapping per K2 D7 defense-in-depth
  (sandbox is the inner allowlist; gate is the outer 5D evaluation).
- Per-conv `DashMap<ConvId, Mutex<()>>` serializes concurrent
  `agent.chat` calls — `llama-server` semaphore doesn't cover the
  load_history → append_turn race.
- `TurnContent` enum (`Text | Audio | Mixed`) from day 1 for voice
  forward-compat; v1 only constructs `Text` but storage shape is
  ready, no substrate migration later.
- Heartbeat to `derived/chat/<conv>/status` with `{phase, tool,
  arg_preview, iter, max_iter}` fixes the dead-spinner UX without
  adding a streaming RPC.

`docs/research/rvf-context-router.md` (~949 lines, by ruv-researcher):
- Inventory of relevant ruv ecosystem packages (`ruvllm`, `ruvector`,
  SONA, MicroLoRA adapters, HNSW routers).
- Four routing-architecture options compared with latency / accuracy
  trade-offs.
- Hard contract with `TieredRouter`: context router emits
  `complexity_hint ∈ [-0.3, +0.3]` (clamped in code), writes into
  the existing `ChatRequest.complexity_boost` field, **never picks
  a model, never escalates a tier**.
- 11-pattern HNSW cap in `ruvllm-wasm` v2.0.1 documented — only
  good for archetype routing (5-7 task types feeding
  `TaskProfile.task_type`), not the primary skill index (we have
  35+ skills today).
- Embedder default: local ONNX MiniLM with API fallback +
  `HashEmbedding` floor (three-level degradation; ~12ms p50 local).
- SOUL.journal as preference data is gated by shadow-mode + WITNESS
  audit before any closed-loop training to production weights.

### Commit 2 — `feat(spike): vertical-slice agent.chat — concierge demo` (`e6f8c816`)

Smallest end-to-end path that lets the panel ask "what is this
project about?" and get a real answer from the daemon-side
concierge. Replaces the panel's chat wire from `llm.prompt` to
`agent.chat` without changing the existing `llm.prompt` RPC.

**`clawft-core::agent::identity`** (new, 159 lines):
- `IdentityLoader` reads `.clawft/SOUL.md` and `.clawft/IDENTITY.md`,
  with a `docs/skills/clawft/` fallback for the spike (post-spike
  the loader will require `weaver init`-seeded files).
- Returns `{ soul, identity, hash, source }`. `source` lets the
  daemon log warn when running on the docs fallback.

**`clawft-weave::daemon::handle_agent_chat`** (new, ~360 lines):
- Builds an identity-aware system prompt: SOUL + IDENTITY +
  workspace context + tool intro.
- Exposes two read-only built-in tools — `read_file` and
  `list_directory` — bounded to the daemon CWD via
  `canonicalize` + prefix check (rejects `../../../etc/passwd`).
- Runs a tool-call loop against `LlmClient::complete_with_tools`
  (max 10 iterations); each iteration appends the assistant
  tool-use turn and the tool-result turn for OpenAI-compat shape.
- New protocol types: `AgentChatParams`, `AgentChatResult`,
  `AgentChatToolCall`, `AgentChatMessage`. No `permission` field
  on params (server-resolved per governance review).
- Honors the existing `llm` control flag — disabling LLM
  fast-fails `agent.chat` the same way as `llm.prompt`.

**`extensions/vscode-weft-panel`**:
- `agent.chat` allowlisted with a comment block matching existing
  per-section commentary.
- Reuses the existing 300s `LLM_TIMEOUT_MS` bucket (same per-method
  timeout policy as `llm.prompt` from `1bbd6f0d`).

**`clawft-gui-egui::explorer::chat`**:
- `Command::Raw { method }` switched from `llm.prompt` to
  `agent.chat`.
- `build_request_params` no longer sends `system` — the daemon-side
  concierge owns the system prompt, no panel-side identity injection.
- `on_response_ok` accepts both `assistant_text` (new) and
  `completion` (legacy) so the daemon and wasm bundle can roll
  independently.

**What this spike is NOT yet** (per plan §14 commits 1-9):
- No `gate.check` / `EffectVector` evaluation per tool call.
- No `SOUL.journal` append, no `weaver soul promote`.
- No `ContextRouter` (system prompt is fixed).
- No substrate-backed conversation history (panel sends full
  history each turn).
- No per-conversation cost circuit-breaker.
- Tool surface hardcoded to `read_file` + `list_directory` (not the
  full `clawft-tools` registry).
- No heartbeat to `derived/chat/<conv>/status` (spinner stays).
- No identity-drift surface; no binding-thread hash pin.

---

## Validation gates passed

- `scripts/build.sh check` — clean.
- `scripts/build.sh clippy` — clean (1m 40s).
- `scripts/build.sh native-debug` — clean (3m 0s); `weft` 253 MB,
  `weaver` 296 MB.
- `scripts/build.sh test` (workspace) — exit 0.
- `extensions/vscode-weft-panel`: `npm run compile` (tsc) — clean.
- `extensions/vscode-weft-panel/scripts/build-wasm.sh` — fresh
  bundle at `webview/wasm/clawft_gui_egui_bg.wasm` (artifact
  gitignored; rebuild locally).
- `cargo install --path crates/clawft-weave --force` — release
  binary `weaver` installed at `~/.cargo/bin/weaver` (5m 20s).

---

## Design notes worth knowing

### Five-expert review consolidated (plan §18)

The plan was reviewed by ruv-researcher (RVF), then by
clawft-kernel-specialist, clawft-weaver-specialist,
clawft-governance-specialist, clawft-k3-apps-specialist, and
system-architect concurrently. **Eight blockers** caught and fixed
before code; key calls:

- `weaver init` collision: must extend
  `crates/clawft-weave/src/commands/init_cmd.rs`, not duplicate.
  `.weftos/` and `.clawft/` are distinct namespaces.
- Substrate fanout drops on overflow: rehydrate via `substrate.list`
  is authoritative; subscribe is best-effort. Status writes are
  start/end transitions, not per-iteration.
- Client-trusted `permission` param is self-elevation: server
  resolves from authenticated channel mapping; new `vscode_panel`
  channel at level 1 (user) lands with commit (5).
- No `gate.check` on tool calls is a defense-in-depth gap: K2 D7
  requires both gate (outer) and sandbox (inner) allow.
- Cost budget is per-LLM-call, not per-conversation: a confused
  loop on user permission can burn the daily budget in one turn.
  Minimal per-conv cap in commit (6); full circuit-breaker v1.1.
- `TurnContent` enum from day 1: voice + streaming need it later;
  migrating substrate-stored turns is worse than the optionality
  cost now.
- Vertical-slice spike commit (0) inserted: validates RPC naming,
  permission mapping, allowlist, panel rehydrate before any
  router/journal/promote machinery (~600 LoC vs ~3000).

### Two-registry boundary documented

`clawft_kernel::ToolRegistry` (kernel-side WASM/builtin tool dispatch
for kernel agent loop) and `clawft_core::tools::ToolRegistry`
(agent-side LLM tool-call registry consumed by `run_tool_loop`) are
distinct registries serving different code paths. Both constructed
in the daemon. No collision; documented as "two registries, two
layers" in the plan.

### `ConversationStore` vs `agent::memory.rs` boundary

`memory.rs` manages cross-conversation distilled facts
(`MEMORY.md` append-only + `HISTORY.md` session summaries) under
`~/.clawft/workspace/memory/`. `ConversationStore` (commit 4) is
per-conversation per-turn substrate log. They never write the same
paths. A future `MemoryConsolidator` (Phase 4) bridges them at
end-of-conversation.

---

## Daemon

Restarted this session. Old daemon (PID 97887, started 17:01) was
running the binary built before today's chat-agent work. Stopped via
SIGTERM, then `cargo install --path crates/clawft-weave --force`
replaced `~/.cargo/bin/weaver` with a fresh release build, then
`weaver kernel start` (backgrounds by default).

```
Current daemon PID:      66815
Socket:                  /home/aepod/dev/clawft/.weftos/runtime/kernel.sock
Log:                     /home/aepod/dev/clawft/.weftos/runtime/kernel.log
Binary:                  /home/aepod/.cargo/bin/weaver (post-spike)
Services registered:     6
```

The new daemon advertises `agent.chat` in the dispatch table at
`crates/clawft-weave/src/daemon.rs:3110`. The WASM panel's
hot-reload watcher (`extension.ts:220`) will detect the new bundle
and reload with a `$(sync) WeftOS: reloaded wasm bundle` toast.

---

## Next session — commits 1-9 of the plan

Plan: `docs/plans/chat-agent-v1.md` §14. Approximate scope:

| # | Commit | Crate | LoC |
|---|---|---|---|
| 1 | identity loader + binding-thread integrity + SoulJournal | clawft-core | ~450 |
| 2 | ContextRouter trait + NullRouter + LlmClassifierRouter | clawft-core | ~500 |
| 3 | SystemPromptBuilder + permission-filtered tool descriptors | clawft-core | ~300 |
| 4 | ConversationStore (substrate-backed, per-conv mutex, TurnContent enum) | clawft-core | ~450 |
| 5 | EffectVector mapping (effect_for_tool table) | clawft-core | ~120 |
| 6 | agent.chat — full handler with gate-check, cost circuit-breaker, heartbeat | clawft-weave | ~600 |
| 7 | extend init_cmd to seed .clawft/ identity files | clawft-weave | ~150 |
| 8 | allowlist + workspaceState conv-id stash | vscode-weft-panel | ~80 |
| 9 | full chat panel — Command::Raw, rehydrate, tool role, heartbeat label | clawft-gui-egui | ~300 |

Total: ~3,050 LoC + ~600 tests. PR boundary at end of (9).

Deferred to v1.1 (separate plan):
- `weaver soul promote` subcommand.
- `weft routing trace` / `replay` + p99 / fallback-rate metrics.
- Full per-conversation cost cap circuit-breaker integration.
- Multi-conversation sidebar UI.
- Typed error variants for `agent.chat`.
- Health surface registration (`weft status` shows agent.chat).
- Governance rule `soul.binding_thread_intact`.
- After-3-denials → `EscalateToHuman`.

---

## Open loops (carrying forward)

These persist from the morning handoff:

- **Live verify with a running llama-server.** Now that the chat
  panel calls `agent.chat`, the user-visible acceptance check for
  this session is: open the WASM panel in Cursor, click into the
  chat sentinel, ask "what is this project about?", and verify the
  concierge reads `CLAUDE.md` + `agents/` and answers from real
  context. First turn likely 5-30s. The daemon log
  (`.weftos/runtime/kernel.log`) shows the tool-call sequence.
- **VSCode panel — Apr 25 user brief items:** inline-streaming
  (needs `agent.chat_stream`, phase 2), provider switcher in chip
  strip, multi-conversation thread (deferred to v1.1 sidebar).
- **Mesh canonical write gate** soak test still wanted.
- **Doc/UX polish pass** before master merge: README + ADR-001
  appendix entries.

---

## Branch state

```
development-0.7.0  e6f8c816 feat(spike): vertical-slice agent.chat — concierge demo
                   1fe04e5b docs(plan): chat-agent v1 plan + RVF context-router research
                   10b91fb4 docs(handoff): 2026-04-26 evening — egui 0.34 + agent orphans wired
                   c9f43fc8 feat(core): wire agent orphans through clawft-service-llm
                   ...
```

Nothing pushed. The branch is 36 commits ahead of `origin/development-0.7.0`.
Ready to push when you decide.
