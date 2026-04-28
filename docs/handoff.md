# Session handoff — 2026-04-27 (late evening) — agent-core-v1 SHIPS

The full **agent-core-v1** plan at `docs/plans/agent-core-v1.md`
landed across this session. All 12 end-state acceptance criteria
are met. Spike is gone; `agent.chat` runs through
`clawft-core::agent::AgentLoop::handle_turn` end-to-end with
kernel-backed `GovernanceGate::check`, substrate-backed
`ConversationSink`, identity-aware system prompt, and the v0→v2.5
context router phasing in place.

## What landed (78 commits ahead of origin/development-0.7.0)

| Phase | Scope | Commits |
|---|---|---|
| Plan + handoff | `docs/plans/agent-core-v1.md` (167 lines), bug-hunt notes | 2 |
| **A** | OpenRouter takeover, `chat` derived-write grant, `conv_id`, canonicalize sandbox, tools-registry route | 4 + ride-along `fix(ci)` |
| **B** | `handle_turn` extracted from `process_message`; `ContextRouter`/`EffectGate`/`ConversationSink` traits; sandbox-test repair | 3 + 1 fix |
| **C** | `clawft-service-agent` crate skeleton; `DAEMON_AGENT` OnceLock + service flag + boot order + `agent.chat.cancel`; substrate `ConversationSink` + heartbeat | 3 |
| **D** | Identity-aware system prompt + SHA-256 hash + `BINDING_THREAD_EXCERPT`; per-tool `gate.check` via `KernelEffectGate`; cutover (~360 LoC spike deleted, feature default on) | 3 |
| **E** | `LlmClassifierRouter` (v1); `EmbeddingRouter` (v2, `ruvector-diskann@2.1`); `HybridRouter` (v2.5 plumbing); E2 import fix | 3 + 1 fix |
| **F** | `weaver init` seeds `.clawft/`; `WitnessRecord` chat-path tests; `weaver soul promote` | 3 |

## Test totals after F2 + final fix

```
cargo test --lib -p clawft-core -p clawft-weave -p clawft-service-agent \
                  -p clawft-service-llm -p clawft-tools -p clawft-plugin
clawft-core         1218
clawft-plugin         82
clawft-service-agent  15  (+ 7 dispatch + 11 substrate + 3 witness = 36 total)
clawft-service-llm    24
clawft-tools         152
clawft-weave          58  (+ integration suites: ~30)
─────────────────────────
                    1549 lib tests, 0 failed
```

`scripts/build.sh check`, `scripts/build.sh clippy`, and
`cargo build -p clawft-weave --no-default-features --features
cluster,ecc,exochain,mesh` (the `agent-core-chat` feature off path)
all return exit 0.

## End-state acceptance criteria — all met

1. ✅ `agent.chat` delegates to `AgentService::dispatch` (no inline loop in daemon).
2. ✅ Dispatch runs through `AgentLoop::handle_turn` (B3 extraction).
3. ✅ Tool catalog from `clawft-tools::register_all` (A4).
4. ✅ Per-tool `gate.check` with `EffectVector` via `KernelEffectGate` (D2). Defer/Deny → structured tool-result JSON.
5. ✅ Per-conv `DashMap<ConvId, Mutex<()>>` + cancel tokens on `AgentService` (C1).
6. ✅ Substrate JSONL at `derived/chat/<conv_id>/turns/<ulid>` + heartbeat at `…/status` (C3); `chat` grant (A2).
7. ✅ `IdentityLoader` reads `.clawft/`, SHA-256 hash, `BINDING_THREAD_EXCERPT` compile-time pin, sandbox hard-deny (D1, F1).
8. ✅ Router phasing: `null` → `llm-classifier` → `embedding` → `hybrid`, locked seam at `ChatRequest.complexity_boost`. v3 (MicroLora) deferred per ruv-researcher pin.
9. ✅ `OPENROUTER_API_KEY` path live; local llama-server unchanged when env unset (A1).
10. ✅ `agent.chat.cancel` aborts in-flight loops (C2).
11. ✅ Boot order: kernel → grants → LLM → agent service → terminal → UI sentinels (C2).
12. ✅ `chat-agent-v1.md` §2-D1 promise fulfilled; cutover commit named in git history (D3).

## Known follow-ups (none blocking)

- **`chain.append` RPC**: F2's `weaver soul promote` writes a witness payload to `<workspace>/.weftos/audit/soul-promote.log` (JSONL) plus a `tracing::info!(target = "chain_event", …)` event because the daemon doesn't expose a public `chain.append` RPC yet. Source has a `TODO(agent-core-v1.1)` to switch when the wire ships.
- **Defer UX**: D2 surfaces `Defer { reason }` as a structured tool-result `{ "deferred": true, "reason": ... }` so the LLM can re-plan. Real interactive defer (panel-side prompt-and-resume) is v1.1.
- **Per-user agent_ids**: chat is single-tenant (one `concierge-bot` principal registered at boot per D2). Per-user agent_ids ship in a future phase.
- **Agent-side journal write**: F2 lands the operator side of `weaver soul promote`; the agent's self-observation write path (during chat turns) is deferred. With an empty journal the command exits cleanly.
- **C3 monotonic-ULID test flake**: `append_turns_are_monotonic` occasionally fails when two appends land in the same ms. Pre-existing from C3; not a new issue.
- **v3 `MicroLoraRouter`**: explicitly deferred until `ruvllm-wasm` lifts the documented 11-pattern HNSW cap (`docs/research/rvf-context-router.md:118-128`). E3's `HybridRouter` left a `TODO(agent-core-v1 phase E3+)` marker.

## Architectural shape post-F2

```
agent.chat RPC  (clawft-weave/src/daemon.rs, unconditional)
      │
      ▼
clawft-service-agent::AgentService::dispatch
      │  per-conv DashMap<Mutex>, CancellationToken,
      │  AgentChatParams → InboundMessage
      ▼
clawft-core::agent::AgentLoop::handle_turn
      │  ContextRouter::route (NullRouter / LlmClassifier /
      │     Embedding / Hybrid based on Config.routing.context_router)
      │  SystemPromptBuilder (identity-aware, SHA-256, BINDING_THREAD)
      ▼
clawft-core::agent::loop_core::run_tool_loop
      │  for each tool call:
      │    EffectGate::check (KernelEffectGate → GovernanceGate
      │       → witness chain entry)
      │    ToolRegistry::execute (clawft-tools)
      │  ConversationSink::append_turn (SubstrateConversationSink
      │       → derived/chat/<conv>/turns/<ulid>)
      ▼
clawft-service-llm::LlmClient
      │  OpenRouter (OPENROUTER_API_KEY) or local llama-server
      ▼
LLM
```

## Branch status

- Working tree: clean.
- `git status -sb`: `## development-0.7.0...origin/development-0.7.0 [ahead 78]`.
- Three locked agent-core/* worktrees retained from this session's parallel work (D1, D2, F2). Safe to `git worktree remove` once you've verified the merges.
- Nothing pushed yet.

---

# Session handoff — 2026-04-27 (early morning)

Follow-on debug session on top of the previous handoff (preserved
below). The chat-agent vertical-slice spike was tried for real, hung
on the first query, and root-caused. A small observability + config
patch is staged (uncommitted) on `development-0.7.0`. The user has
rebuilt the kernel and is about to restart Cursor to pick up the new
daemon binary.

## The bug — `agent.chat` hung on first real query

Symptom: panel showed `error: agent.chat: llm http transport: error
sending request for url (http://127.0.0.1:8111/v1/chat/completions)`
after a long spinner. Daemon log showed only the
identity-fallback WARN at handler entry, then silence; llama-server
slots were idle when checked mid-hang.

Root cause (math, not deadlock):

- `LlmClient.request_timeout` defaulted to **120 s**
  (`crates/clawft-service-llm/src/client.rs:55`).
- `LlmConfig.default_max_tokens` = **512**.
- Qwen3.6-35B IQ2_XXS sustained generation ≈ 4 tok/s under the
  spike's prompt shape (cold first turn; reasoning_content on the
  wire eating budget).
- 512 tokens × 250 ms ≈ **128 s of generation alone**, already
  past the 120 s reqwest timeout. Add prompt processing of the
  ~13 KB SOUL+IDENTITY system prompt + tool catalog + history and
  every iteration was guaranteed to hit the wall.
- Panel-side `LLM_TIMEOUT_MS` is 300 s — so the daemon was failing
  *before* the panel would have. Panel surfaced the transport
  error verbatim.

Contributing (not the cause, but they made the fail mode invisible):

- Zero progress logging in the tool loop
  (`crates/clawft-weave/src/daemon.rs:2197-2258`). No `info!`
  around `complete_with_tools`, no per-iteration trace.
- No heartbeat to `derived/chat/<conv>/status` — explicitly
  deferred per plan §14 commit (6).
- The handoff's "first turn likely 5-30 s" estimate was wildly
  optimistic for Qwen 35B at IQ2_XXS with reasoning_content on.

## Patch staged on `development-0.7.0` (uncommitted)

Five files, ~80 LoC. All gates clean.

**`crates/clawft-service-llm/src/client.rs`**:
- `LlmConfig.request_timeout` default 120 s → **300 s** (matches
  panel `LLM_TIMEOUT_MS`).
- New `ChatUsagePromptDetails { cached_tokens: u32 }`, attached as
  `usage.prompt_tokens_details` on `ChatUsage`. Lets us see slot
  prefix-cache hit counts.
- New `ChatTimings { predicted_per_second, prompt_per_second }`,
  attached as `timings: Option<ChatTimings>` on `ChatResponse`.
  Lets us see real sustained throughput per call.
- Both fields are `#[serde(default)]` / `Option`, so non-llama-server
  backends keep deserializing fine.

**`crates/clawft-service-llm/src/lib.rs`**:
- Re-export `ChatTimings`, `ChatUsagePromptDetails`.

**`crates/clawft-core/src/pipeline/service_llm_adapter.rs`**:
- Two test-mock construction sites updated for the new
  `ChatResponse.timings: None` field and `ChatUsage.. .Default::default()`
  spread. Tests still pass.

**`crates/clawft-weave/src/daemon.rs`**:
- New `AGENT_CHAT_PER_TURN_MAX_TOKENS: u32 = 256` const, passed in
  place of `p.max_tokens` to every `complete_with_tools` call. Caps
  per-iter generation at ~64 s @ 4 tok/s (cold) or ~10 s @ 25 tok/s
  (sustained) — both safely under the 300 s timeout. Model can keep
  calling tools across iterations if it needs more output.
- `info!` at handler entry (msg_count, identity_source,
  per_turn_max_tokens).
- Per-iter `info!` after every `complete_with_tools` returns Ok:
  `iter, elapsed_ms, prompt_tokens, cached_tokens,
   completion_tokens, predicted_per_sec, tool_calls`. One line per
  iteration in `kernel.log` — debugging future hangs is now trivial.
- `warn!` on transport errors (with iter + elapsed) and on
  `max_iterations` cap (with elapsed).

## Validation gates

- `scripts/build.sh check` — clean (41 s).
- `scripts/build.sh native-debug` — clean (1 m 25 s); `weft` 253 MB,
  `weaver` 296 MB.
- `cargo test -p clawft-service-llm --lib` — **22 / 22** pass.
- `cargo test -p clawft-core --lib` — **1141 / 1141** pass.

## Daemon

User rebuilt the kernel and is restarting Cursor at handoff time.
Next session should:

1. Confirm `weaver --version` shows the post-patch build.
2. Open the Cursor panel, ask "what is this project about?".
3. `tail -f .weftos/runtime/kernel.log | grep "agent.chat"` and
   expect one `info!` line per loop iteration.

## Open questions the new logs will answer in one chat cycle

1. **Does Qwen3.6 hybrid arch honor slot prefix cache?** Iter 2+
   should report `cached_tokens ≈ prompt_tokens` of iter 1
   (strictly-extending prefix). If `cached_tokens` stays at 0
   across iters, the hybrid arch isn't reusing the slot cache and
   we should reorganize the prompt (smaller system prompt, tools
   moved to messages, or skip tool catalog reuse).
2. **What's the real sustained throughput** under the spike's
   actual prompt shape? `predicted_per_sec` per iter tells us
   whether the 25 tok/s claim with `--spec-type ngram-simple`
   holds, or whether we're durably at 4 tok/s and need to revisit
   speculation tuning / reasoning_format / quant.

If `cached_tokens` stays at 0, candidate follow-ups:

- Add `--reasoning-format none` to the llama-server start script —
  stops reasoning_content from burning the per-turn token budget,
  ~2-3× speedup on tool-call turns.
- Move tools out of the `tools:` field into a static system-prompt
  block (some hybrid models prefix-cache plaintext better than the
  structured tools block).

## Architecture note (carried from this session's Q&A)

WeftOS does **not** require running as wasm in Cursor. The egui GUI
is dual-target:

- `crates/clawft-gui-egui/src/main.rs` — eframe native window
  (`fn main() -> eframe::Result<()>`).
- `[[bin]] name = "weft-gui-egui"` at
  `crates/clawft-gui-egui/Cargo.toml:18-21`,
  `required-features = ["native"]`.
- `weft-demo-lab` and the `workshop-watcher` example use the same
  surface natively.

Build it standalone:

```bash
cargo build -p clawft-gui-egui --features native --bin weft-gui-egui
./target/debug/weft-gui-egui
```

Note: `scripts/build.sh native` only builds `weft` + `weaver` today.
If we want `weft-gui-egui` as a first-class artifact, it's a one-line
addition to the script (deferred — user is staying with the Cursor
panel for the chat demo).

User is keeping the **Cursor panel path** for now because that's
where `LLM_TIMEOUT_MS`, hot-reload watcher, allowlist, and demo
muscle memory already live. Native eframe path remains a fallback
if webview indirection becomes the bottleneck again.

---

# Session handoff — 2026-04-26 (late evening)

Pick-up doc for the previous session. Reflects `development-0.7.0` at
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
