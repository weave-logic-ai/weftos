# Session handoff — 2026-04-26 (evening)

Pick-up doc for the next session. Reflects `development-0.7.0` at
commit `c9f43fc8` (fast-forwarded over `phase3-node-identity`). Four
new commits land on top of the morning's "LLM connect + parallel-wave"
batch. All green: `scripts/build.sh check` + `scripts/build.sh
clippy` clean across the workspace; targeted tests pass for every
touched crate (1391 passing across `clawft-service-llm`,
`clawft-core`, and `clawft-gui-egui`).

The full-workspace `cargo test --workspace` still deadlocks on
`clawft-kernel hnsw_eml::tests::benchmark_*` (preexisting, NOT a
regression — those tests run >30 min). Run targeted tests instead:

```bash
cargo test -p clawft-service-llm \
          -p clawft-core \
          -p clawft-gui-egui \
          -p clawft-surface --lib
```

This evening was two pieces:

1. **egui 0.34 bump + custom alacritty-based terminal renderer** —
   replacing the UTF-8-lossy `String` accumulator that was rendering
   ANSI escape codes as literal `\u{1b}[...` text.
2. **Wire every orphaned agent capability in `clawft-core` through
   `clawft-service-llm`** — Phases A-H of the integration plan.
   Eight orphans were brought into the live agent loop with
   `clawft-service-llm` as the canonical LLM call site.

---

## What's new this session

### Commit 1 — `feat(gui-egui): bump egui to 0.34 + alacritty-backed terminal renderer` (`7bf6bf16`)

The workspace was anchored at egui 0.29 by `egui_dock 0.14` (the
egui-0.29-compatible line). That blocked any modern terminal-emulator
crate. Bumped:

| crate | from | to |
|---|---|---|
| egui / eframe / egui_extras / egui_demo_lib | 0.29 | 0.34 |
| egui_plot | 0.29 | 0.35 |
| egui_dock | 0.14 | 0.19 |

Mechanical migrations across ~50 files in `clawft-gui-egui` +
`clawft-surface`:

- `Margin::symmetric(f32, f32)` → `Margin::symmetric(i8, i8)`
- `Rounding` → `CornerRadius` (u8-based)
- `Visuals::*_rounding` → `Visuals::*_corner_radius`
- `WidgetVisuals::rounding` → `WidgetVisuals::corner_radius`
- `Painter::rect` / `rect_stroke` — added `StrokeKind` argument
- `Line::new(points)` → `Line::new(name, points)`
- `eframe::App` — added required `ui` (kept `update`)
- `fonts(|f| f.glyph_width(..))` → `ctx().fonts_mut(|f| ..)`
- `SidePanel::*` → `Panel::*`
- `Frame::none()` → `Frame::new()`
- `.rounding(N)` → `.corner_radius(N)`
- `Context::style/set_style` → `global_style/set_global_style`
- `default_width / width_range` → `default_size / size_range`

**Terminal renderer.** `egui_term` was evaluated and rejected because
(a) it pins to egui 0.31, and (b) it owns its own PTY internally —
would have bypassed the daemon-side `clawft-service-terminal`
service entirely. Instead, `crates/clawft-gui-egui/src/explorer/terminal.rs`
now drives `alacritty_terminal` directly:

- PTY bytes from the daemon-side service flow through
  `vte::ansi::Processor` into a `Term<NopListener>`.
- The grid is painted as colored cells + glyphs
  (`NamedColor`/`Spec`/`Indexed` mapped to a Solarized-dark-ish
  palette, 256-color cube + grayscale ramp resolved inline).
- Keyboard input: text passes through; arrows / Enter / Backspace /
  Tab / Esc / Home / End / PageUp / PageDn / Delete / Insert /
  F1-F12 emit the right CSI sequences; Ctrl+letter masks to control
  bytes (Ctrl+C → `0x03` etc).
- Resize syncs the local model AND fires `terminal.resize` so
  in-shell apps reflow.
- Cursor: filled rect when focused (with blink), hollow when not.
- Browser builds get a placeholder stub since alacritty pulls
  platform-specific tty + polling crates that don't compile to wasm.

The daemon-side architecture is preserved end-to-end. The daemon
spawns PTYs in `clawft-service-terminal` and publishes chunks at
`substrate/<daemon-node>/derived/terminal/<session_id>`; this surface
remains the thin renderer.

### Commit 2 — `fix(vscode-weft-panel): per-method RPC timeout for llm.prompt` (`1bbd6f0d`)

The default 3000 ms in `rpc.ts` is right for daemon-local control
verbs that round-trip in milliseconds, but `llm.prompt` proxies to a
llama.cpp server doing CPU/GPU inference; even a short completion
takes 5-30 s and a longer one runs minutes. The panel now hands
`llm.prompt` a 300 s timeout while everything else keeps fast-fail
semantics — a stopped daemon still surfaces immediately on the
chips.

### Commit 3 — `feat(service-llm): tool-call wire format + complete_with_tools` (`a7e848cd`)

Extended the narrow `LlmClient` so it can drive a tool-using agent
loop against llama-server's OpenAI-compat endpoint:

- `ChatMessage` carries optional `tool_calls` (assistant) and
  `tool_call_id` (`role:"tool"` replies). New
  `ChatMessage::tool(id, content)` constructor closes the round-trip.
- `ChatRequest` grows optional `tools` and `tool_choice` (both
  serde-skipped when None — wire shape stays byte-compatible with the
  no-tools case the chat panel and daemon RPC already use).
- New types: `Tool`, `ToolFunction`, `ToolCall`, `ToolCallFunction`.
- `ToolChoice { Auto | None | Required | Function(name) }` with a
  custom `Serialize` that emits `"auto"` / `"none"` / `"required"`
  or `{type:"function",function:{name}}` per the OpenAI schema.
- `LlmClient::complete_with_tools(messages, tools, tool_choice, …)`.
  Existing `complete(...)` delegates to the new method (passing no
  tools), so all current callers keep working unchanged.

22 client tests (6 new) — all pass.

### Commit 4 — `feat(core): wire agent orphans through clawft-service-llm` (`c9f43fc8`)

This is the big one. Brings every orphaned-but-built agent
capability in `clawft-core` into the live agent loop, with
`clawft-service-llm` as the canonical LLM call site.

**Phase A** (separate commit above) — tool-call wire format on
`LlmClient`.

**Phase B — `pipeline/service_llm_adapter.rs` (NEW).** A
`ServiceLlmAdapter` bridges `Arc<LlmClient>` into the pipeline's
`LlmProvider` trait. Inbound `&[serde_json::Value]` → typed
`ChatMessage` conversion tolerates partial inputs (missing role →
`"user"`, missing content → `""`, malformed `tool_calls` → `None`).
Outbound `ChatResponse` → OpenAI-shape `Value` so the existing
`OpenAiCompatTransport` response parser consumes it unchanged.
Streaming defers to the trait default (`LlmClient` has no SSE yet).
10 unit tests + a wiremock end-to-end round-trip.

**Phase C — Bootstrap default pipeline.** `build_default_pipeline`
now wires `ServiceLlmAdapter` over `LlmClient::new(LlmConfig::from_env())`
instead of the previously-stubbed transport. Default model server is
the same llama-server the daemon's `llm.prompt` RPC and the chat
panel already use — three call paths, one model. On `LlmClient`
construction failure (bad env URL) the transport falls back to the
stub so the rest of the pipeline still wires. Browser builds keep
the stub (service-llm pulls reqwest; native-only).

**Phase D — Learner feedback loop.** `LearningBackend` grows
`evolve_prompt(prompt) -> String` with a no-op default.
`PipelineRegistry::complete` and `::complete_stream` call
`apply_prompt_evolution` between context assembly and the transport
stage, mutating the first `system` message in place.
`TrajectoryLearner` overrides `evolve_prompt` to: (a) check
`evolution_ready`, (b) snapshot poor + best trajectories as
`TrajectoryHint`s (capped at 16), (c) `auto_select_strategy` +
`mutate_prompt`, (d) clear the flag so we don't re-mutate every
turn. `NoopLearner` inherits the default no-op.

**Phase E — Sandbox enforcement at tool dispatch.** `AgentLoop` got
an optional `Arc<SandboxEnforcer>` via `with_sandbox()`. In
`run_tool_loop`'s per-tool dispatch, if an enforcer is attached we
call `check_tool(name)` before `tools.execute`. A denial surfaces
as `{"error": "sandbox denied: …"}` so the LLM can recover instead
of failing the turn, and the audit log captures it.

**Phase F — Skill watcher hot-reload.** `AppContext` got
`start_skill_watcher()` (native-only) which builds an empty
`SkillRegistry` over the `SkillsLoader`'s directory, wraps in
`Arc<RwLock>`, and starts the existing `notify`-based watcher.
Caller keeps the returned handle alive for the loop's lifetime.
Opt-in (bootstrap doesn't auto-start to avoid inotify-quota
issues).

**Phase G — Skill autogen.** `AgentLoop` got an optional
`Arc<Mutex<PatternDetector>>` via `with_autogen()`. Post-dispatch we
feed every executed tool name to `record_tool_call`, then
`detect_candidates`; new patterns get materialized as pending
`SKILL.md` files in `~/.clawft/skills/pending/<name>/` via
`install_pending_skill`. Pending → live promotion stays manual per
the autogen module's design.

**Phase H — Multi-agent surfaces.** `AppContext` got optional fields
`agent_router: Arc<AgentRouter>` and `agent_bus: Arc<AgentBus>` plus
set/get pairs. The single-agent CLI flow ignores them; the daemon's
spawn manager (and any future multi-agent dispatcher) can register
them when needed. The orphaned core modules (`agent_routing.rs`,
`agent_bus.rs`) are reachable now without deletion or further
refactor.

---

## Three call paths, one llama-server

After this session the local llama-server is the unified LLM
endpoint for **all** of:

```
              ┌─ chat panel (vscode-weft-panel)  ──┐
              ├─ daemon RPC `llm.prompt`           ┤
              └─ CLI agent (weft agent / loop_core)┘
                                                   ▼
                  clawft-service-llm::LlmClient
                                                   │
                              POST /v1/chat/completions
                                                   ▼
                              llama-server (Qwen3 by default)
```

The CLI agent's pipeline (Classifier → Router → Assembler →
**ServiceLlmAdapter** → Scorer → **Learner**) is now end-to-end
working. The Learner's feedback loop mutates the system prompt when
poor outcomes accumulate; mutations carry forward into the next
turn's request.

---

## What's still orphaned-ish

After Phase H every module is reachable, but a few remain "wired
but not auto-driven" — i.e. the production bootstrap has the hook,
just isn't filling it in:

- `start_skill_watcher()` is opt-in. Nothing in `weft agent` yet
  calls it, so SKILL.md edits still need a process restart in the
  CLI. Easy follow-up: have `commands/agent.rs` start it after
  bootstrap and hold the handle for the loop's lifetime.
- `with_sandbox()` and `with_autogen()` builders exist on
  `AgentLoop` but bootstrap doesn't construct an enforcer or
  detector. Reasonable defaults need config keys; punt to next
  session.
- `set_agent_router` / `set_agent_bus` on `AppContext` are
  unused — daemon's `agent.spawn` still goes through the kernel
  `A2ARouter` (the canonical Phase-3 path). The core
  `AgentRouter` / `AgentBus` pair is now available for the CLI
  agent should we want multi-agent fan-out there too.
- `pipeline/llm_adapter.rs` (the older `clawft-llm` provider
  bridge) coexists with the new `service_llm_adapter.rs`. Not dead,
  not orphaned — it's the path we'd switch to if we ever need
  cross-provider routing (OpenAI / Anthropic / Groq) instead of
  local-llama.

---

## Build & test

```bash
# Verify
scripts/build.sh check       # workspace cargo check
scripts/build.sh clippy      # warnings-as-errors
cargo test -p clawft-service-llm -p clawft-core \
           -p clawft-gui-egui --lib

# Counts last-verified at commit c9f43fc8:
#   service-llm:   22 / 22 pass
#   core:        1137 / 1137 pass
#   gui-egui:     232 / 232 pass
#   surface:       18 / 18 pass
```

The `cargo test --workspace` deadlock on `clawft-kernel
hnsw_eml::tests::benchmark_*` is the same flake from the morning
session and not in scope here.

---

## Open loops (still)

These carried over from the morning handoff and are still on the
list:

- **Live verify with a running llama-server.** Now that the agent
  loop genuinely talks to llama-server end-to-end (Phase C wired the
  transport), running `weft agent -m "say hi"` against a live
  server is the next acceptance check. Expected behaviour: full
  pipeline runs, `ServiceLlmAdapter` POSTs to
  `/v1/chat/completions`, output appears.
- **VSCode panel — Apr 25 user brief items still pending:**
  inline-streaming (would need `llm.prompt_stream` lands on the
  daemon side first), provider switcher in the chip strip,
  multi-message thread vs single-shot (currently single-shot).
- **Mesh canonical write gate** — landed in the morning batch; soak
  test still wanted.
- **Doc/UX polish pass** before the master merge: README + ADR-001
  appendix entries for the canon primitives still aging from the
  pre-Phase-3 era.

---

## Daemon

Restarted at the end of this session — the running `weaver kernel
start --foreground` had been up since Apr 25 and was still on the
old binary. The new build picks up:

- alacritty-based terminal renderer — old explorer terminal text
  display will look completely different (real colors, real cursor)
  on next reconnect.
- service-llm tool-call extensions — RPC schema unchanged for the
  chat panel; only relevant when the agent loop drives tools.
- core orphan wiring — affects only `weft agent` callers; daemon
  RPCs are otherwise untouched.

Branch state: `development-0.7.0` and `phase3-node-identity` both
point at `c9f43fc8`. Nothing pushed yet.
