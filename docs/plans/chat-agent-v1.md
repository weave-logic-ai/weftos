# Plan: WeftOS Concierge Chat Agent (v1)

**Status:** Draft, pending RVF research + multi-expert review
**Target branch:** `development-0.7.0`
**Owner:** —
**Last updated:** 2026-04-26

## 1. Goal

Make the chat panel a real conversational interface to WeftOS rather than a stateless `llm.prompt` round-trip. The agent — the **WeftOS Concierge** — runs on the daemon side, wraps the kernel, and is reachable from:

- The WASM panel hosted in Cursor / VSCode (primary v1 target).
- The CLI (`weaver chat`, follow-up).
- Voice I/O (phase 2; pipeline already partly wired via `clawft-tools::voice_*`).

Concretely: when the user opens the panel and asks "what is this project about?", the agent reads `CLAUDE.md`, enumerates `agents/`, lists relevant skills, and answers from real context — not from base-model priors.

## 2. Locked architectural decisions

These come out of the planning conversation; they are not up for debate without an explicit revisit.

| # | Decision | Why |
|---|---|---|
| 1 | Agent loop runs on the daemon. | Reuses `clawft-core::agent::loop_core::run_tool_loop`, which is production-ready. WASM panel is correctly modeled as a viewer. |
| 2 | WASM panel uses one new RPC: `agent.chat`. `llm.prompt` stays as the dumb no-tools sibling. | Clean separation; doesn't churn the existing surface. |
| 3 | Tool surface = filesystem ops via `clawft-tools` registry, sandboxed to a workspace root. | All tools already exist (`read_file`, `write_file`, `edit_file`, `list_directory`, `exec_shell`, `web_fetch`, `web_search`, `memory_*`, `voice_*`). |
| 4 | Workspace root = daemon CWD for v1. Configurable in `.clawft/config.json` later. | Smallest viable scope. |
| 5 | History stored per-turn in substrate at `substrate/<node>/derived/chat/<conv_id>/turns/<ulid>`. | Substrate-first per WeftOS conventions; ULID is sortable; per-turn entries are enumerable via `substrate.list`. |
| 6 | Default identity is the **Weaver** (concierge), loaded from `.clawft/SOUL.md` + `.clawft/IDENTITY.md`. | Identity already articulated in `docs/skills/clawft/{SOUL,IDENTITY}.md`. |
| 7 | Defaults live in `docs/skills/clawft/`; `weaver init` materializes them into `.clawft/` for runtime modification. Loader reads from `.clawft/` only. | Clean separation between canonical templates and per-instance state. |
| 8 | Self-update via append-only `.clawft/SOUL.journal.md`; promotion to `SOUL.md` requires explicit `weaver soul promote` with diff review. Sections marked `<!-- core: do-not-edit -->` are immutable. | Prevents personality drift; binding thread can't be self-erased. |
| 9 | Two routers, distinct concerns:<br>(a) **Model-tier router** = existing `TieredRouter`, untouched.<br>(b) **Context router** = new, picks per-turn skills/agents/tools. | Existing `TieredRouter` already handles complexity → model tier; context routing is orthogonal. |
| 10 | Context router v1 = LLM classifier on free tier. v2 = RVF model (design TBD; see `docs/research/rvf-context-router.md`). Behind a `ContextRouter` trait so v2 swaps in without rewiring. | Ship v1 fast; don't paint into a corner. |
| 11 | History format: one substrate value per turn, `{ role, content, tool_calls?, tool_call_id?, ts, turn_id }`. ULID-keyed. | Mirrors Claude Code's per-turn JSONL. |
| 12 | Voice = phase 2; same `agent.chat` underneath, just different I/O. Tools (`voice_listen`, `voice_speak`, `audio_*`) already exist. | Out of v1 scope but doesn't constrain it. |
| 13 | `Turn.content` is a `TurnContent` enum (`Text \| Audio \| Mixed`) from day 1, even though v1 only constructs `Text`. | Migrating substrate-stored turns later is worse than the optionality cost now (system-architect C5). |
| 14 | Permission is **server-resolved from authenticated channel mapping**, never client-supplied. `AgentChatParams` has no `permission` field. New channel `vscode_panel` (level 1 / user) added to `.clawft/config.json`. | Client-trusted permission is self-elevation (governance B2). |
| 15 | Panel uses `Command::Raw { method: "agent.chat" }`. No new `Command` variant. Panel **never** imports `clawft-core::*`; rehydrate via `substrate.list` + `substrate.read` over the existing RPC bridge. | Zero-touch path; matches every other RPC; preserves wasm32 cleanliness (k3-apps B1, B2). |
| 16 | `weaver init` is **extended**, not duplicated. Existing `crates/clawft-weave/src/commands/init_cmd.rs` writes `weave.toml` + `.weftos/runtime/`; we add `.clawft/SOUL.md`, `.clawft/IDENTITY.md`, `.clawft/SOUL.journal.md`, optionally `.clawft/config.json` to the same flow. | `.weftos/` and `.clawft/` are distinct namespaces; `.weftos/` is project runtime, `.clawft/` is agent self-state (weaver B1, B2). |
| 17 | Tool calls go through `gate.check` per K2 D7. Sandbox allowlist is the inner layer; gate is the outer. New module `clawft-core::agent::effects` maps tool name → `EffectVector` (5D). | Defense-in-depth (governance B1, C10). |
| 18 | Concurrent `agent.chat` to the same `conv_id` is serialized by a `DashMap<ConvId, Mutex<()>>` inside `ConversationStore`. `llama-server` semaphore is not enough — race is between `load_history` and `append_turn`. | Logical race exists at substrate layer even though writes are atomic (kernel R3). |
| 19 | Vertical-slice spike commit (0) before the trait-and-module commits. ~600 LoC, end-to-end demo answering "what is this project about?" in Cursor. | De-risks RPC naming, permission mapping, allowlist, panel rehydrate before any router/journal/promote machinery (system-architect R1). |

## 3. What's reused (no rebuild)

| Component | Path | Notes |
|---|---|---|
| Tool implementations | `crates/clawft-tools/src/{file_tools,shell_tool,web_fetch,web_search,memory_tool,voice_*,audio_*,delegate_tool}.rs` | Workspace-bounded. Feature-gated. |
| Tool registry | `crates/clawft-core/src/tools/registry.rs` (`ToolRegistry`) | Populated by `clawft_tools::register_all`. |
| Tool loop | `crates/clawft-core/src/agent/loop_core.rs::run_tool_loop` | Has max_iterations, post-write verification, hallucination detection, result truncation. |
| Sandbox | `crates/clawft-core/src/agent/sandbox.rs` | Per-tool allowlist enforcement, decision logging. |
| Tiered model router | `crates/clawft-core/src/pipeline/tiered_router.rs` | Free/standard/premium/elite tiers. Already wired to `.clawft/config.json`. |
| LLM client (with tools) | `crates/clawft-service-llm/src/client.rs::complete_with_tools` | Just landed in a7e848cd. |
| LLM adapter | `crates/clawft-core/src/pipeline/service_llm_adapter.rs` | Bridges service-llm to pipeline traits. |
| Permissions config | `.clawft/config.json` (already populated) | zero_trust / user / admin levels with per-level tool allowlists. |
| Substrate read/list/subscribe | `crates/clawft-weave/src/daemon.rs` | Already proxied to the WASM panel. |
| Identity content | `docs/skills/clawft/{SOUL,IDENTITY}.md`, `agents/clawft/CLAWFT.md` | Used as templates for `weaver init`. |

## 4. New components

| Component | Where | Purpose |
|---|---|---|
| `ContextRouter` trait + `LlmClassifierRouter` | `crates/clawft-core/src/agent/context_router.rs` (new) | Per-turn skill/agent/tool selection. v1 calls a free-tier LLM. |
| `IdentityLoader` + `SoulJournal` | `crates/clawft-core/src/agent/identity.rs` (new) | Resolves SOUL/IDENTITY from `.clawft/`; manages append-only journal. |
| `SystemPromptBuilder` | `crates/clawft-core/src/agent/system_prompt.rs` (new) | Composes identity + workspace + skills + tools into the system prompt. |
| `ConversationStore` | `crates/clawft-core/src/agent/conversation.rs` (new) | Substrate-backed per-turn log. Read + write. |
| `agent.chat` RPC handler | `crates/clawft-weave/src/daemon.rs` (new function `handle_agent_chat`) | Wraps the pipeline; sends/receives via substrate. |
| `agent.chat` protocol types | `crates/clawft-weave/src/protocol.rs` (new structs) | `AgentChatParams`, `AgentChatResult`, `ToolCallSummary`. |
| `weaver init` subcommand | `crates/clawft-weave/src/commands/init_cmd.rs` (new) | Materializes `.clawft/` from defaults. |
| `weaver soul promote` subcommand | `crates/clawft-weave/src/commands/soul_cmd.rs` (new) | Diff-review and promote journal entries to SOUL.md. |
| `agent.chat` allowlist | `extensions/vscode-weft-panel/src/extension.ts` | Added to `ALLOWED_METHODS` + per-method timeout. |
| Chat panel rewire | `crates/clawft-gui-egui/src/explorer/chat.rs` | Switch from `llm.prompt` to `agent.chat`; rehydrate from substrate. |

Total new modules: 7. Total touched modules: 4. No deletions.

## 5. RPC surface: `agent.chat`

```rust
// crates/clawft-weave/src/protocol.rs

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentChatParams {
    /// Existing conversation; None to start a new one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    /// User turn content.
    pub content: String,
    // NOTE: no `permission` field. Permission is resolved server-side
    // from the authenticated channel; client-supplied permission would
    // be self-elevation (governance B2). The channel mapping in
    // `.clawft/config.json` `routing.permissions.channels` is the
    // single source of truth, with a new `vscode_panel: { level: 1 }`
    // entry covering the WASM-panel callers.
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentChatResult {
    pub conversation_id: String,
    pub turn_id: String,             // ULID of the assistant turn
    pub assistant_text: String,
    pub finish_reason: String,
    pub tool_calls: Vec<ToolCallSummary>,
    pub usage: UsageStats,
    /// Optional reasoning trace from ContextRouter (debug aid).
    pub context_decision: Option<ContextDecisionSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallSummary {
    pub name: String,
    pub arguments_preview: String,    // truncated
    pub result_preview: String,       // truncated
    pub success: bool,
    pub duration_ms: u64,
}
```

**Behaviour:**

1. Resolve permission level **server-side** from authenticated channel mapping in `.clawft/config.json` (no client param).
2. If `conversation_id` is None, mint a ULID; else acquire the per-conv mutex (`DashMap<ConvId, Mutex<()>>` in `ConversationStore`) and load history via `substrate.list` + `substrate.read`. Per-conv mutex serializes between `load_history` and `append_turn`.
3. Append the user turn to substrate.
4. `IdentityLoader::current()` reads `.clawft/SOUL.md` + `.clawft/IDENTITY.md`. **Rejects load** if the binding-thread hash-pinned excerpt (compile-time `const` in `clawft-core::agent::identity`) is missing or modified.
5. `ContextRouter::route()` produces a `ContextDecision` with `complexity_hint ∈ [-0.3, +0.3]` (clamp asserted).
6. Set `request.complexity_boost = decision.complexity_hint` (existing `ChatRequest` field; no new wiring).
7. `SystemPromptBuilder::build()` assembles the system prompt with tools **filtered by permission**.
8. `TieredRouter::route()` picks the model tier (existing path; consumes `complexity_boost`).
9. `run_tool_loop()` runs to completion or `max_tool_iterations`. **Per-iteration:**
   - Before each tool call: `gate.check(agent_id, "agent.chat.tool_call", effect_for_tool(name))` — see §5.5. Denial → tool result `"denied: <reason>"`, loop continues.
   - After each tool call: publish heartbeat to `substrate/<node>/derived/chat/<conv_id>/status` (`{ phase, tool, arg_preview, iter, max_iter }`) — see §11.5.
   - Conversation-level cost circuit breaker: see §5.6.
10. Append the assistant turn (and any intermediate `role: "tool"` turns) to substrate. Each turn is a separate ULID-keyed value at `derived/chat/<conv_id>/turns/<ulid>`.
11. Return `AgentChatResult`. Status sentinel reset to `{ phase: "idle" }` (one write at end, never per-iteration overwrite — kernel B2).

**Error semantics:** strings via `Response::error(...)`, mirroring `llm.prompt` (`daemon.rs:2077`). Honors the `llm` control flag. Typed error variants tracked as v1.1 follow-up (weaver C1).

**Error categories (informational, all flatten to strings in v1):**

- `invalid params: ...` — schema parse failure.
- `agent: identity load failed: ...` — SOUL.md missing or binding-thread hash mismatch.
- `agent: gate denied tool call <name>: ...` — capability denial.
- `agent: tool loop iteration cap reached` — `max_tool_iterations` exhausted.
- `agent: cost cap reached for conversation <id>` — circuit breaker.
- `agent: substrate write failed: ...` — propagated kernel errors.
- `agent.chat: <inner>` — fallback for unclassified failures (mirrors `llm.prompt: <inner>`).

## 5.5 Gate integration & EffectVector mapping (governance B1 / C10)

New module `crates/clawft-core/src/agent/effects.rs`:

```rust
use clawft_kernel::governance::EffectVector;

/// Maps a tool name to its 5D effect vector for gate.check evaluation.
/// Conservative defaults; wider effects gate at threshold 0.8.
pub fn effect_for_tool(name: &str) -> EffectVector {
    match name {
        "read_file" | "list_dir" | "list_directory" =>
            EffectVector { risk: 0.1, privacy: 0.3, security: 0.1, novelty: 0.0, fairness: 0.0 },
        "write_file" | "edit_file" =>
            EffectVector { risk: 0.4, privacy: 0.2, security: 0.3, novelty: 0.1, fairness: 0.0 },
        "web_fetch" | "web_search" =>
            EffectVector { risk: 0.3, privacy: 0.4, security: 0.4, novelty: 0.2, fairness: 0.0 },
        "exec_shell" | "spawn" =>
            EffectVector { risk: 0.7, privacy: 0.3, security: 0.7, novelty: 0.3, fairness: 0.0 },
        _ => EffectVector::default_conservative(),
    }
}
```

**Per-tool-call flow inside `run_tool_loop`:**

1. `gate.check(agent_id, "agent.chat.tool_call", effect_for_tool(name))` →
2. If `Allow` → invoke tool through sandbox (which runs its own per-tool allowlist).
3. If `Deny` → return synthetic tool result `denied: <reason>` to the LLM; loop continues.
4. After 3 denials in one turn → escalate via `EscalateToHuman` (full implementation v1.1; v1 just stops the loop with `agent: gate denied tool calls 3x; halting`).

This is **defense in depth** per K2 D7. Sandbox allowlist is the inner layer (per-tool `is_tool_allowed` check); gate is the outer layer (capability + effect-vector evaluation). Both must allow for the call to proceed.

## 5.6 Cost circuit breaker (governance B3 — minimal v1, full v1.1)

`.clawft/config.json` `routing.permissions.<level>` gains `cost_budget_per_conversation_usd`:

- `zero_trust`: 0.05
- `user`: 0.25
- `admin`: 5.00

Inside `run_tool_loop`, between iterations, sum the conversation's actual LLM spend (already tracked by `pipeline::cost_tracker`). If it exceeds the per-conversation cap, return `agent: cost cap reached for conversation <id>` and halt the loop. The user can start a new conversation.

v1 implementation: minimal — just the per-conversation cap check between iterations. v1.1: refined budget windowing, soft warnings at 80%, separate caps for read-only vs write tools, integration with daily/monthly budgets already in config.

## 6. Substrate schema

```
substrate/<daemon-node>/
├── ui/chat                                # existing sentinel, untouched
├── derived/agent/identity                 # { soul_hash, identity_hash, last_loaded } — per kernel C1
└── derived/chat/
    └── <conversation_id>/                  # ULID
        ├── meta                            # { created_at, model, identity_hash, channel } — written ONCE
        ├── status                          # { phase, tool?, arg_preview?, iter?, max_iter? } — heartbeat
        └── turns/
            ├── 01HQK...A                   # { role, content, tool_calls?, tool_call_id?, ts, turn_id }
            ├── 01HQK...B
            └── ...
```

### 6.1 Read/write contract (resolves kernel B1, B2)

**Authoritative read path: `substrate.list` + `substrate.read`.** Subscribers tolerate gaps.

The kernel substrate fanout uses bounded mpsc(256) per subscriber with `try_send` and **silent drop on full** (`crates/clawft-kernel/src/substrate_service.rs:615`, `:639-660`). A long tool sequence can publish 20+ values per turn rapidly; subscribers will lose lines without backpressure to the writer. Implications:

- **Rehydrate** (panel reopens conversation) **must** use `substrate.list("derived/chat/<conv_id>/turns/")` followed by per-turn `substrate.read`. Authoritative — never skips a turn.
- **Live tail** via `substrate.subscribe` is **best-effort**. Used for the "still working: tool 4/8" UX heartbeat (§11.5), not for reconstructing conversation state. The panel must reconcile against the rehydrate path on focus or after any paint that observes a dropped tick.
- Status sentinel writes are **start-of-turn + end-of-turn only** (one transition `idle → working`, one `working → idle`). Per-iteration heartbeat writes go to `status` as overwrites by design — losing intermediate ticks is acceptable; what matters is the final reset.

### 6.2 Path conventions (resolves §15.3, §15.9)

- `derived/chat/<conv_id>/...` for conversation log — symmetric with `derived/transcript/`, `derived/terminal/`, `derived/classify/`.
- `derived/agent/identity` for the identity projection — projection of `.clawft/SOUL.md` + `IDENTITY.md`, fits `derived/` semantics. **Not** a new top-level `agent/` namespace (kernel C1).
- `ui/chat` (existing) stays as the sentinel mount; not touched.

### 6.3 Conversation listing

`substrate.list("substrate/<node>/derived/chat/")` returns all conversations. v1: panel uses this only on first selection to find the most-recent. Sidebar listing UI deferred to v1.1.

## 7. Identity loader & SOUL journal

### 7.1 Layout

```
docs/skills/clawft/SOUL.md           # canonical template (read-only)
docs/skills/clawft/IDENTITY.md       # canonical template (read-only)
.clawft/SOUL.md                      # runtime identity (mutable, agent + user edit)
.clawft/IDENTITY.md                  # runtime identity (mutable)
.clawft/SOUL.journal.md              # append-only self-observation log
```

### 7.2 `weaver init`

- If `.clawft/SOUL.md` does not exist: copy from `docs/skills/clawft/SOUL.md`.
- Same for `IDENTITY.md`.
- Create empty `SOUL.journal.md` with header (`# Weaver Self-Observation Journal\n\n_Append-only. Promote via \`weaver soul promote\`._\n`).
- If `.clawft/config.json` does not exist: copy a default template (we already ship one in the repo as a fixture; check during implementation).
- `--force` overwrites existing files (with a confirmation prompt).
- Idempotent by default.

### 7.3 SOUL.journal.md format

```markdown
# Weaver Self-Observation Journal

_Append-only. Promote via `weaver soul promote`._

## 2026-04-27T14:32:18Z [conversation 01HQK...]
**Observation:** the user prefers terse responses; I drifted into bullet-list mode three times in this conversation.
**Suggested update:** in SOUL.md "Direct and Honest" section, add: prefer narrative paragraphs over bullet-lists unless the content is genuinely list-shaped.
**Source:** turn 01HQK...A → 01HQK...C (correction)

## 2026-04-27T15:01:44Z [conversation 01HQM...]
...
```

The agent writes journal entries as a side effect of `agent.chat` when it observes patterns worth recording. It does NOT write to `SOUL.md` directly.

### 7.4 `weaver soul promote`

- Reads `.clawft/SOUL.journal.md`.
- For each unpromoted entry, shows it alongside the relevant `SOUL.md` section.
- User picks per entry: **integrate** (apply suggested update), **append** (add to a "Recent learnings" tail section), **discard**, **skip** (leave unpromoted).
- Honors `<!-- core: do-not-edit -->` markers — refuses to touch sections inside them. Such suggestions are auto-discarded with a warning.
- After promotion, archives processed entries to `.clawft/SOUL.journal.archive.md`.
- `--dry-run` shows what would happen without writing.

### 7.5 Loader

```rust
pub struct IdentityLoader {
    soul_path: PathBuf,
    identity_path: PathBuf,
    journal_path: PathBuf,
}

pub struct Identity {
    pub soul: String,
    pub identity: String,
    pub hash: String,                // sha256 of soul+identity, for substrate meta
}

impl IdentityLoader {
    pub fn current(&self) -> Result<Identity>;
    pub fn append_journal(&self, entry: &JournalEntry) -> Result<()>;
}
```

Hot reload: re-read on each `agent.chat` call (cheap; both files are small).

### 7.6 Binding-thread integrity (resolves governance C8)

The binding thread (`an agent must not diminish human capability...`) is a **compile-time pinned excerpt** in `clawft-core::agent::identity`:

```rust
/// Hash-pinned excerpt from SOUL.md. If this string is not found in
/// the loaded SOUL.md, IdentityLoader::current() returns
/// ClawftError::IdentityCorrupt and the agent refuses to run.
const BINDING_THREAD_EXCERPT: &str = "an agent must not diminish human capability";
```

Behaviour:
- `IdentityLoader::current()` substring-matches `BINDING_THREAD_EXCERPT` against the loaded SOUL.md. Missing → load fails; agent does not start (or, mid-session, the next `agent.chat` returns `agent: identity load failed: binding thread missing`).
- This is v1's defense. Recompile-from-source attack is out of scope.
- v1.1 promotes this to a governance rule `soul.binding_thread_intact` evaluated by `gate.check` (governance recommendation).

### 7.7 Sandbox hard-deny on identity files (resolves governance R5)

`crates/clawft-core/src/agent/sandbox.rs` adds an explicit denylist:

- `.clawft/SOUL.md` — hard-deny on write/edit, even if path falls within a writable root.
- `.clawft/IDENTITY.md` — same.

Read of these files is allowed (the agent reflects on its own identity). Writes go through journal-only path. Anti-test: tool call `write_file { path: ".clawft/SOUL.md", content: "..." }` returns `denied: identity files are read-only to the agent` and the loop continues.

### 7.8 Identity drift mid-conversation (resolves §15.8, system-architect C7)

**Decision: record-and-warn.** The conversation `meta` records the identity hash at conversation start. If a turn loads with a different hash:

- The new hash is written to a `derived/chat/<conv_id>/identity_drift` substrate path with `{ from_hash, to_hash, at_turn, ts }`.
- The next `AgentChatResult` carries an `identity_drift: Option<DriftNote>` field surfaced by the panel as a one-line muted warning above the assistant bubble.
- No automatic reset; the user can start a new conversation if they want a clean slate.

Freeze-per-conversation was considered but rejected: the user should be able to edit SOUL.md and have it take effect immediately, with visibility into when it happened.

## 8. Context router v1

### 8.1 Trait

```rust
// crates/clawft-core/src/agent/context_router.rs

#[async_trait]
pub trait ContextRouter: Send + Sync {
    async fn route(&self, request: &ContextRequest) -> ContextDecision;
}

pub struct ContextRequest {
    pub user_turn: String,
    pub history_summary: Option<String>,    // last N turns condensed
    pub permission: PermissionLevel,
    pub available_skills: Vec<SkillManifest>,
    pub available_agents: Vec<AgentManifest>,
}

pub struct ContextDecision {
    pub skills: Vec<SkillRef>,              // by ID; loader resolves to content
    pub agents: Vec<AgentRef>,
    /// Bias for `TieredRouter`. Clamped to [-0.3, +0.3] in code.
    /// Written into `ChatRequest.complexity_boost` before TieredRouter
    /// runs. Context router NEVER picks a model and NEVER escalates a
    /// tier on its own — that's TieredRouter's job. Hard contract per
    /// `docs/research/rvf-context-router.md`.
    pub complexity_hint: f32,
    /// Decision confidence in [0, 1]. v2 embedding retrieval falls back
    /// to v1 LLM classifier when confidence < 0.45.
    pub confidence: f32,
    pub reasoning: String,                  // for debug + journal
}
```

### 8.2 v1 implementation: `LlmClassifierRouter`

- Holds an `LlmClient` configured for the free tier.
- Builds a classifier prompt: lists available skills (id + 1-line description), available agents (id + role), and asks the model to emit a JSON decision.
- Constrains output via JSON-mode if the model supports it; otherwise grammar-checks the response.
- Falls back to a "load nothing extra" decision on parse failure (the always-loaded `.clawft/skills/*` set is the floor).
- Cache by user-turn hash for repeat queries within a session.

### 8.3 Skills/agents discovery

- `.clawft/skills/*` — always loaded (v1 floor: claude-code, claude-flow).
- `.claude/skills/*` — candidate set, loaded when ContextRouter selects them.
- `agents/*/*.md` — header (frontmatter + first paragraph) loaded as part of the catalog; full content fetched only when an agent is selected.
- `.claude/agents/*` — same pattern, larger catalog.

The catalog is built once at daemon boot, refreshed on filesystem change (existing `notify` watcher infrastructure in `clawft-core`).

### 8.4 Phased rollout (per `docs/research/rvf-context-router.md`)

The trait ships in commit (2). Implementations land progressively:

| Phase | Implementation | Ships when | What it does |
|---|---|---|---|
| **v0** | `NullRouter` | commit (2) | Returns floor decision (`.clawft/skills/*` always-loaded; empty agents; complexity_hint=0; confidence=1). Validates the trait + plumbing without intelligence. |
| **v1** | `LlmClassifierRouter` | commit (2), behind feature flag | Free-tier LLM call per turn. Generates labeled training data for v2/v3. |
| **v2** | `EmbeddingRouter` | follow-up plan after v1 stabilizes | Dense retrieval over hand-authored skill descriptors. Uses **existing** `clawft-kernel` HNSW or workspace `diskann 2.1` (1.0 recall, 90 µs search). **No new ruv crates yet.** Falls back to v1 when confidence < 0.45. |
| **v2.5** | `HybridRouter` | when v2 fallback rate stabilizes < 25% and tool-match implicit feedback ≥ 0.6 over a week | EmbeddingRouter + SONA rerank + LLM fallback. |
| **v3** | `MicroLoraRouter` | when v2.5 has stable preference signal from `SOUL.journal` | MicroLoRA adapter trained on logged decisions + journal preferences, with mandatory shadow-mode + WITNESS audit before promotion to production weights. |

**Hard rules from research:**

1. The 11-pattern HNSW cap in `ruvllm-wasm` v2.0.1 is real. Use it **only** for archetype routing (5-7 task types feeding `TaskProfile.task_type`), never as the primary skill index — we have 35+ skills today and growing.
2. Embedder default: local ONNX MiniLM, with API fallback, with `HashEmbedding` floor. Three-level degradation. p50 ~12ms local; ~150ms with API fallback.
3. No new ruv crates land in the workspace before v2.5. v0/v1/v2 use what's already there.
4. SOUL.journal mining as preference data is gated by shadow-mode + human approval. No closed loop to weights without explicit promotion.
5. Mandatory observability before any phase past v0 (see §13).

## 9. SystemPromptBuilder

```rust
pub struct SystemPromptBuilder {
    identity: Identity,
    workspace: WorkspaceContext,
    decision: ContextDecision,
    tools: Vec<ToolDescriptor>,
}

impl SystemPromptBuilder {
    pub fn build(&self) -> String;
}
```

**Order of assembly (top to bottom):**

1. **Identity** — full SOUL.md + IDENTITY.md content. Binding thread is in here.
2. **Workspace** — CWD, branch, ahead/behind from main, summary of `git status` (file count by category, no diffs).
3. **Loaded skills** — full markdown content of each, deduplicated, with `## Skill: <id>` headers.
4. **Loaded agents** — header + role only (full content reserved for `delegate_tool` calls when implemented).
5. **Tool descriptions** — auto-derived from the registry; one block per tool with name, args, return shape.
6. **Conversation hints** — turn count, recent tool-call summary if relevant.

**Token budget:** check assembled prompt length against the resolved tier's `max_context_tokens` (from `.clawft/config.json`); if over, drop loaded skills in reverse priority until it fits, then drop agent headers.

## 10. ConversationStore

```rust
use dashmap::DashMap;

pub struct ConversationStore {
    substrate: Arc<dyn SubstrateClient>,
    node_id: String,
    /// Per-conversation mutex serializing load_history → append_turn
    /// against concurrent agent.chat calls on the same conv_id
    /// (kernel R3). The llama-server semaphore does not save us here.
    conv_locks: DashMap<String, Arc<tokio::sync::Mutex<()>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TurnContent {
    Text(String),
    Audio(AudioRef),    // path to substrate-stored audio chunk
    Mixed(Vec<TurnPart>),
}

pub struct Turn {
    pub turn_id: String,            // ULID
    pub role: String,               // user | assistant | tool | error
    pub content: TurnContent,
    pub tool_calls: Option<Vec<ToolCallSummary>>,
    pub tool_call_id: Option<String>,
    pub ts: i64,                    // unix epoch ms
}

impl ConversationStore {
    pub async fn create(&self, meta: ConversationMeta) -> Result<String>;
    /// Acquires the per-conv mutex; releases on drop of the returned guard.
    pub async fn lock_conversation(&self, conv_id: &str) -> ConversationGuard;
    pub async fn append_turn(&self, conv_id: &str, turn: Turn) -> Result<String>;
    pub async fn load_history(&self, conv_id: &str) -> Result<Vec<Turn>>;
    pub async fn list_conversations(&self) -> Result<Vec<ConversationSummary>>;
    /// Heartbeat write — overwrites the `status` sentinel. Best-effort
    /// semantics on subscriber side (§6.1).
    pub async fn publish_status(&self, conv_id: &str, status: HeartbeatStatus) -> Result<()>;
}
```

Backed by `substrate.publish` / `substrate.read` / `substrate.list`. Errors propagate; no silent swallowing.

### 10.1 Boundary vs `clawft-core::agent::memory` (resolves system-architect B1)

`agent/memory.rs` already exists. Its job: long-term agent memory (`MEMORY.md` append-only facts, `HISTORY.md` session summaries) under `~/.clawft/workspace/memory/`. **Different concern.** ConversationStore is per-conversation per-turn; MemoryStore is cross-conversation distilled facts.

Documented boundary: at end-of-conversation (or on a periodic timer), a future `MemoryConsolidator` reads ConversationStore and emits distilled facts into `MemoryStore` (v1.5+). They never write the same paths. This module spec lands in `crates/clawft-core/src/agent/learning/` (system-architect C6).

### 10.2 `TurnContent` enum from day 1 (resolves system-architect C5)

`Turn.content` is `TurnContent::Text(String) | Audio(AudioRef) | Mixed(Vec<TurnPart>)` from commit (4). v1 only constructs `Text`. Voice (phase 2) and streaming (phase 2) construct `Audio` and `Mixed` without migrating substrate-stored turns. Migration cost later >> optionality cost now.

## 11. Chat panel changes

Touch `crates/clawft-gui-egui/src/explorer/chat.rs`:

### 11.1 Wire path (locked per k3-apps B1)

Stay on `Command::Raw { method: "agent.chat", params, reply }`. **No new `Command` variant.** The panel never imports `clawft-core::*`; rehydrate is via `substrate.list` + `substrate.read` over the existing RPC bridge (k3-apps B2). This matches every other RPC in the panel (substrate.list, terminal.spawn, control.set_enabled).

### 11.2 ChatView state

- Add `conversation_id: Option<String>` to `ChatView`.
- Add `tool_name: Option<String>`, `tool_args: Option<String>`, `tool_result: Option<String>` to `ChatMessage`.
- Filter `role == "tool"` out of any wire payload (mirror the existing `error` filter at `chat.rs:158`).

### 11.3 Selection & rehydrate (k3-apps R1 — v1, not v1.5)

Mirror the terminal pattern at `explorer/mod.rs:240-241`. The `Explorer` owns a `HashMap<String, String>` mapping sentinel path → most-recent `conversation_id`. On chat-sentinel selection:

- If the cache has a `conversation_id` for this sentinel path: fire `substrate.list("derived/chat/<conv_id>/turns/")` then a batched `substrate.read` per turn. Render in chronological order.
- If not, and `meta.most_recent` is available via `substrate.list("derived/chat/")`: adopt it.
- Otherwise: leave `conversation_id` None; the daemon mints one on first user turn.

`ChatView::close()` on selection move drops `pending` but does not clear `conversation_id` from the Explorer cache.

### 11.4 Tool-call rendering (k3-apps R2)

Extend `paint_bubble()` (chat.rs:360-432) with a `"tool"` arm. Use `egui::CollapsingHeader::new(format!("tool: {}", name))` collapsed by default; expanded shows `arguments_preview` and `result_preview` in monospace. Failures render in muted red. No new role variant beyond "tool".

### 11.5 Heartbeat-driven status label (k3-apps R3)

Plan §6 already provisions `derived/chat/<conv_id>/status` for heartbeat. The panel:

- Subscribes via `substrate.subscribe` (existing allowlist; `extension.ts:58`) to that path on selection.
- Replaces the static spinner at `chat.rs:299-308` with a status-driven label: `format!("{}: {}", phase, tool.unwrap_or_default())`.
- Tolerates dropped ticks (best-effort per §6.1) — final reset to `phase: "idle"` is the authoritative end signal.

This converts the "long blank wait" UX into "tool 4/8: read_file" without adding a streaming RPC.

### 11.6 Streaming UX

Real token streaming is phase 2 (`agent.chat_stream`). The heartbeat above mitigates the perceptual gap.

## 12. Extension changes

`extensions/vscode-weft-panel/src/extension.ts`:

### 12.1 Allowlist & timeout

- Add `"agent.chat"` to `ALLOWED_METHODS` (line 39 area), with a comment block matching the existing per-section commentary (e.g., the `terminal.spawn` block at lines 76-82). The allowlist doubles as documentation (weaver C2).
- **Reuse the existing `LLM_TIMEOUT_MS = 300_000`** (extension.ts:263). Add `agent.chat` to the same `if (method === "llm.prompt")` check at line 265 — no new constant. 300s upper bound is correct; the heartbeat (§11.5) handles UX.

### 12.2 Conversation-id persistence across hot-reload (k3-apps C1)

The hot-reload watcher at extension.ts:220-239 reassigns `panel.webview.html` on bundle change, destroying wasm memory and `ChatView.conversation_id` with it.

Fix: stash conversation IDs in `vscode.ExtensionContext.workspaceState` keyed by sentinel path. Flow:

- When the panel posts `{ type: "ready" }` (extension.ts:96-100), the host responds with `{ type: "conversation-restore", entries: { "<sentinel-path>": "<conv_id>", ... } }`.
- The wasm panel's `WebviewReadyMessage` handler in `crates/clawft-wasm/src/lib.rs` populates the `Explorer`'s conv-id cache from this message.
- When the panel reports a new conversation ID via a `{ type: "conversation-update" }` postMessage, the host writes it to `workspaceState`.

Survives `cargo build` of the bundle without losing the active conversation.

### 12.3 No webview-side TypeScript logic

The TS extension is still a dumb proxy. All conversation reasoning happens daemon-side; all UI happens in wasm. The TS adds two lines: the allowlist entry and the timeout key.

## 13. Test plan

### Unit tests
- `IdentityLoader` — round-trip read; missing files → init guidance; immutable-core marker recognition.
- `SoulJournal` — append-only invariant; promote dry-run produces correct diffs.
- `ContextRouter` LLM-classifier path — given mocked LLM responses, assert correct decisions; fallback path on parse failure.
- `SystemPromptBuilder` — order, dedup, token-budget truncation.
- `ConversationStore` — round-trip via mock substrate client.
- Permission filtering — `user`-level request can't access tools outside `user.tool_access`.

### Integration tests
- `agent.chat` end-to-end with a mock LLM: user turn → response with tool call → tool execution → second LLM call → final text response → all turns landed in substrate.
- `agent.chat` honoring the `llm` control flag (disabled → clean error).
- `weaver init` on an empty `.clawft/` → all expected files present.
- `weaver soul promote --dry-run` round-trip.

### E2E manual
- Start daemon (`scripts/build.sh native-debug && ./target/debug/weaver daemon`).
- Run `weaver init` in a fresh dir.
- `extensions/vscode-weft-panel/scripts/build-wasm.sh`.
- Open WASM panel in Cursor.
- Ask: "what is this project about?" → assistant should `read_file CLAUDE.md`, `list_directory agents/`, and answer with real specifics.
- Ask: "what's in the SOUL?" → assistant reads `.clawft/SOUL.md`.
- Reload panel → conversation rehydrates from substrate.
- Edit `.clawft/SOUL.md`, ask another question → identity reflects edit on next turn.

### Anti-tests (must not happen)
- Agent never writes `.clawft/SOUL.md` directly; sandbox hard-deny enforced (governance R5).
- Agent never writes `.clawft/IDENTITY.md` directly; sandbox hard-deny enforced.
- Agent cannot read files outside the workspace root (test with `../../../etc/passwd`).
- Tool loop respects `max_tool_iterations` (test with a tool that always wants more).
- ContextRouter never sets `complexity_boost` outside `[-0.3, +0.3]` (clamp asserted).
- ContextRouter has no path that calls `TieredRouter` directly or sets a model.
- Panel build fails to compile if it imports `clawft-core::*` (k3-apps B2). Verify with `cargo tree --package clawft-gui-egui` filtered to wasm32 target.
- `agent.chat` rejects an `AgentChatParams` JSON payload that includes a `permission` field (governance B2 — schema-strict).
- Identity load fails when `BINDING_THREAD_EXCERPT` is removed from `.clawft/SOUL.md` (governance C8 v1).
- Conversation cost circuit-breaker triggers at `cost_budget_per_conversation_usd` (governance B3).
- Concurrent `agent.chat` calls on the same `conv_id` do not interleave turn writes (kernel R3 — assert via observable turn ordering).

### Observability (mandatory per research §recommendation 7)
Before any phase past v0 ships, these surfaces must exist:

- `weft routing trace [--conversation <id>]` — replay last N routing decisions: input turn, decision (skills+agents+complexity_hint+confidence), reasoning, latency, fallback path taken.
- `weft routing replay <decision_id>` — re-run a logged decision through the current router (regression check after a router change).
- `weft status` extension: p50/p99 router latency, fallback rate (% of v2 decisions that fell back to v1 LLM), tool-match implicit feedback rate.
- Substrate projection: `substrate/<node>/agent/routing/recent` — last 100 decisions for live observability.

These commands are commit (5) scope — they ship with the `agent.chat` RPC, not deferred.

## 14. Commit boundaries

Each commit must pass `scripts/build.sh check` and ship its own tests.

**Major restructure per system-architect R1 + R3**: insert commit (0) as a vertical-slice spike. Defer `weaver soul promote` and `weft routing trace/replay` to v1.1 (those exist for steady-state debugging; nothing to debug on day 1).

| # | Commit | Crates touched | Lines (rough) | Demo |
|---|---|---|---|---|
| **0** | `feat(spike): vertical-slice agent.chat ("what is this project about?")` | clawft-core, clawft-weave, vscode-weft-panel, clawft-gui-egui | ~600 | Open panel in Cursor → ask question → real answer reading CLAUDE.md and `agents/`. |
| 1 | `feat(core): identity loader + binding-thread integrity + SoulJournal` | clawft-core | ~450 | (no end-user demo; expands commit 0's minimal loader) |
| 2 | `feat(core): ContextRouter trait + NullRouter + LlmClassifierRouter` | clawft-core | ~500 | (NullRouter folded into trait file; v1 = LlmClassifier per system-architect R2) |
| 3 | `feat(core): SystemPromptBuilder + permission-filtered tool descriptors` | clawft-core | ~300 | |
| 4 | `feat(core): ConversationStore (substrate-backed, per-conv mutex, TurnContent enum)` | clawft-core | ~450 | |
| 5 | `feat(core): EffectVector mapping (effect_for_tool table)` | clawft-core | ~120 | |
| 6 | `feat(weave): agent.chat — full handler with gate-check, cost circuit-breaker, heartbeat` | clawft-weave | ~600 | Same demo as (0), now exercising the full path. |
| 7 | `feat(weave): extend init_cmd to seed .clawft/ identity files` | clawft-weave | ~150 | `weaver init` now also writes `.clawft/SOUL.md` etc. |
| 8 | `feat(extension): allowlist agent.chat + workspaceState conv-id stash` | vscode-weft-panel | ~80 | |
| 9 | `feat(gui-egui): full chat panel — Command::Raw, rehydrate, tool role, heartbeat label` | clawft-gui-egui | ~300 | |
| 10 | `chore(extension): rebuild webview/wasm bundle` | webview/wasm | (artifact) | |

Approximate total: ~3,050 LoC of new code + ~600 LoC of tests. PR boundary at end of (9). (10) is a release/handoff commit.

**Vertical-slice spike commit (0) scope** (system-architect R1):

- New `agent.chat` RPC, **no gate-check** (fast-fail on permission via channel mapping only), no cost circuit-breaker, no journal, no soul-promote.
- `IdentityLoader` minimal: read `.clawft/SOUL.md` + `IDENTITY.md` if present, else fall back to `docs/skills/clawft/{SOUL,IDENTITY}.md` for the spike only (post-spike, the loader requires `weaver init` to have run).
- `NullRouter` always: empty additional skills/agents; existing pipeline + tool loop run with a fixed always-loaded skills set.
- No substrate rehydrate: each conversation starts fresh.
- Minimal `SystemPromptBuilder`: identity + workspace + tool descriptors. No filtered-by-permission gating yet.
- Panel uses `Command::Raw { method: "agent.chat" }`; allowlist + 300s timeout in extension.

Goal: prove the wire path end-to-end and demo the user-visible win. Risk-isolating, not feature-complete. Commits (1) - (9) backfill the production-grade machinery.

**v1.1 (separate plan, separate PR):**
- `weaver soul promote` subcommand (350 LoC + tests).
- `weft routing trace` / `weft routing replay` observability (200 LoC).
- Per-conversation cost cap with circuit breaker (governance B3 — minimal version in (6); full version v1.1).
- Multi-conversation sidebar UI in panel.
- Typed error variants for `agent.chat`.

Phase 2 (voice) is its own plan; not part of this commit chain.

## 15. Risks & open questions — review status

Expert review is complete. Findings inline at the relevant sections. Status of each pre-review question below:

1. ~~**`agent.chat` naming.**~~ **Resolved (weaver R1):** keep `agent.chat`. Existing `agent.*` namespace at `daemon.rs:3097-3308` already covers register/spawn/stop/restart/inspect/list/send. `agent.chat` is the only name that fits.

2. ~~**Tier-hint interaction with TieredRouter.**~~ **Resolved by research:** ContextRouter writes `complexity_hint ∈ [-0.3, +0.3]` into the existing `ChatRequest.complexity_boost` field; TieredRouter consumes it as part of its existing complexity calculation. Context router never picks a model, never escalates a tier. Hard contract — enforced by clamping in `ContextDecision::clamp_hint()` and asserted in tests.

3. ~~**Substrate path.**~~ **Resolved:** `derived/chat/<conv_id>/...` for conversations; `derived/agent/identity` for identity projection (kernel C1). Not a new top-level `agent/` namespace.

4. ~~**Workspace boundary.**~~ **Resolved (kernel R2):** new `agent.workspace_root: PathBuf` in `clawft-types::config`, defaulting to daemon CWD. Feeds `Sandbox::readable_paths` / `writable_paths`. **Distinct from** `skills_dir` (catalog root). Don't widen `skills_dir`.

5. ~~**Default permission for the WASM panel.**~~ **Resolved (governance B2):** add `vscode_panel: { level: 1 }` to `.clawft/config.json` `routing.permissions.channels`. `permission` field stripped from `AgentChatParams` — server-resolved only.

6. ~~**Hot-reload of SOUL.md.**~~ **Resolved:** re-read each turn. Plus binding-thread hash check (§7.6) and identity-drift surface (§7.8).

7. ~~**Substrate publish backpressure.**~~ **Resolved (kernel B1):** documented in §6.1. Rehydrate via `substrate.list` is authoritative; subscribe is best-effort. Status sentinel writes one transition per turn.

8. ~~**Identity hash mid-conversation.**~~ **Resolved (system-architect C7):** record-and-warn. `identity_drift` field in `AgentChatResult`; panel surfaces a one-line muted warning.

9. ~~**Identity substrate projection.**~~ **Resolved:** `derived/agent/identity` (per kernel C1).

10. ~~**Concurrent conversations.**~~ **Resolved (kernel R3):** `DashMap<ConvId, Mutex<()>>` in `ConversationStore` serializes load_history → append_turn. Substrate writes to sibling paths under same conv_id are race-free at the kernel level.

11. ~~**`weaver init` collision.**~~ **Resolved (weaver B1):** EXTEND existing `init_cmd::run` (`crates/clawft-weave/src/commands/init_cmd.rs`). One bootstrap, two artifact roots (`.weftos/runtime/` + `.clawft/`). New flags: `--soul-only` (skip weave.toml regen), `--force` already exists.

12. ~~**Tool registry feature flags / permission visibility.**~~ **Resolved (governance §recommendation 4):** `SystemPromptBuilder` filters tool descriptions by permission. If LLM emits a denied tool name from training priors, the gate refuses, sandbox returns `denied: ...` as the tool result, the loop continues. After 3 denials in one turn → escalate via `EscalateToHuman` rather than silent loop-continue.

## 16. Phased rollout & promotion gates

Authoritative source: `docs/research/rvf-context-router.md`. Summary here for plan-local reference; do not duplicate detail.

**Promotion gates** (do not skip):

| From → To | Gate criteria |
|---|---|
| v0 → v1 | Trait + plumbing landed; observability commands shipping decisions to `substrate/<node>/agent/routing/recent`. |
| v1 → v2 | v1 has logged ≥ 1,000 decisions over real usage; labels are diverse enough to seed embedding descriptors; observability shows a stable p99 latency baseline. |
| v2 → v2.5 | v2 fallback rate (LLM rescue when confidence < 0.45) stabilizes below 25% over a 7-day window AND tool-match implicit feedback ≥ 0.6 over the same window. |
| v2.5 → v3 | v2.5 has accumulated SOUL.journal preference signal sufficient for MicroLoRA training (researcher's spec); shadow-mode comparison shows v3 outperforms v2.5 in offline replay; WITNESS audit greenlights production weights. |

**No skipping.** Each phase generates the data the next phase needs. Skipping v1 leaves v2 with no labels; skipping v2 leaves v2.5 with no retrieval baseline; skipping v2.5 leaves v3 with no preference signal.

Trait shape (§8.1) is fixed for v0–v2.5. v3 may need additive fields; if so, that's a non-breaking trait extension, not a redesign.

## 17. Phase 2 hooks (out of v1 scope, not in commit chain)

### v1.1 (next plan, follows v1)
- `weaver soul promote` subcommand with diff-review and core-marker enforcement.
- `weft routing trace` / `replay` observability commands + p99 / fallback-rate metrics in `weft status`.
- Per-conversation cost cap with full circuit-breaker integration (v1 ships minimal version).
- Multi-conversation sidebar UI in panel (substrate.list listing).
- Typed error variants for `agent.chat` (replaces v1's string format).
- Governance rule `soul.binding_thread_intact` evaluated at gate.check time (replaces v1's load-time hash pin alone).
- Health surface registration: `agent.chat` registers a `SystemService` impl tracking last-completion-time so `weft status` shows it (kernel C2).
- After 3 gate denials in one turn → escalate via `EscalateToHuman` (governance recommendation 4 full implementation).

### Phase 2 (voice + streaming)
- **Voice in:** mic capture → `audio_transcribe` tool → user-turn synthesis → `agent.chat`. Existing tools already exist. `TurnContent::Audio` populated.
- **Voice out:** `agent.chat` result → `audio_synthesize` → `voice_speak`. Same.
- **Streaming:** `agent.chat_stream` RPC, connection-takeover pattern, mirrors `substrate.subscribe`. Per-token deltas. The `TurnContent::Mixed` enum carries token + audio fragments.

### Phase 3 (router evolution)
- **EmbeddingRouter (v2):** dense retrieval over hand-authored skill descriptors using existing `clawft-kernel` HNSW or workspace `diskann 2.1`. Per research §recommendation 2.
- **HybridRouter (v2.5):** retrieval + SONA rerank + LLM fallback.
- **MicroLoraRouter (v3):** trained adapter from logged decisions + journal preferences. Shadow-mode + WITNESS audit.

### Phase 4 (memory + delegation)
- **MemoryConsolidator** (`crates/clawft-core/src/agent/learning/`): periodic distillation from `ConversationStore` → `MemoryStore` (`MEMORY.md` / `HISTORY.md`). Closes the boundary system-architect B1 raised.
- **Skills auto-promotion:** after enough successful uses of a `.claude/skills/*` skill, promote to `.clawft/skills/` for faster routing.
- **Cross-agent delegation:** `delegate_tool` already exists; chat agent spawns specialist agents from `agents/` profiles.

## 18. Review process — DONE

- ✅ ruv-researcher: `docs/research/rvf-context-router.md` written (949 lines). Integrated into §8.4 and §16.
- ✅ clawft-kernel-specialist: substrate fanout, ToolRegistry boundary, workspace concept, concurrency, identity projection, health surface.
- ✅ clawft-weaver-specialist: RPC naming locked (`agent.chat`), `weaver init` collision identified (extend not duplicate), serde attributes, timeout reuse (300s).
- ✅ clawft-governance-specialist: gate-check integration (K2 D7), permission resolution (server-side from channel), per-conv cost cap, sandbox hard-deny on identity files, binding-thread integrity, EffectVector mapping.
- ✅ clawft-k3-apps-specialist: panel wire path locked (Command::Raw), no-clawft-core-import contract, rehydrate-on-reselect promoted to v1, tool-role rendering, heartbeat-driven status label, hot-reload conv-id stash.
- ✅ system-architect: ConversationStore vs memory.rs boundary documented, IdentityLoader vs agents.rs distinct, vertical-slice spike commit (0) inserted, NullRouter folded into trait file, soul-promote and observability moved to v1.1, TurnContent enum from day 1, learning/ docking module spec.

**Integration complete.** All 12 pre-review open questions resolved. Two cross-reviewer conflicts surfaced (see §15.5 — superseded by current plan):

1. ~~Cost cap timing — governance v1 vs system-architect v1.1.~~ **Compromise locked:** minimal per-conversation cap in v1 (commit 6, ~30 LoC); full circuit breaker, soft warnings, integration with daily/monthly budgets in v1.1.
2. ~~Timeout — weaver 300s vs k3-apps 180s.~~ **Resolved:** 300s upper bound (matches existing `LLM_TIMEOUT_MS`); heartbeat (§11.5) handles UX so practical wait feels short.

**Status: ready for code.**

## 19. Done criteria

- [ ] `weaver init` materializes `.clawft/SOUL.md`, `.clawft/IDENTITY.md`, `.clawft/SOUL.journal.md`, optionally `.clawft/config.json`.
- [ ] `agent.chat` RPC round-trips via the WASM panel in Cursor.
- [ ] User asks "what is this project about?" and gets a real, project-specific answer.
- [ ] Conversation persists across panel close/reopen (substrate rehydrate).
- [ ] Identity edits in `.clawft/SOUL.md` take effect on the next turn.
- [ ] `weaver soul promote --dry-run` shows a sensible diff for a synthetic journal.
- [ ] `scripts/build.sh gate` passes on the final commit.
- [ ] WASM bundle is rebuilt and the hot-reload watcher confirms.
- [ ] Phase 2 (voice) and v2 (RVF) are unblocked — clear extension points exist for both.
