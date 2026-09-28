# Codex and WeftOS as Agent-Teams hosts — research for a PR #3512 follow-up

Scope: ruvnet/ruflo PR #3512 (ADR-320, "Grok Build host surface +
host-agnostic team bus") moved Ruflo's Agent Teams bus into Ruflo as nine
MCP tools (`team_create/spawn/send/inbox/broadcast/plan/status/on_stop/
shutdown`) with a filesystem mailbox under `.claude-flow/teams/<team>/`. Only
a Grok adapter ships today. This doc answers two questions: (1) what a Codex
adapter needs, (2) what WeftOS needs to participate as a host/teammate.

Facts are cited with `file:line` or a URL. Recommendations are marked as such
and are not verified against the actual PR diff — only against the
`feat/team-bus-codex-weftos` worktree. Anything not directly observed is
marked **unverified**.

## Question 1: Codex

### 1.1 How a Codex lead starts parallel children today

Two independent mechanisms exist; current Ruflo code only uses the second.

**A. Native Codex "Agents" (subagents).** `codex features list` on this
machine (codex-cli 0.157.1) reports `multi_agent stable true` — enabled by
default. Agent definitions are standalone TOML files at
`~/.codex/agents/*.toml` (personal) or `<project>/.codex/agents/*.toml`
(project-scoped) — live example:
`/Users/mathewbeane/weftos/.codex/agents/world-builder.toml`, shape `name`,
`description`, `developer_instructions`, plus optional `model`,
`model_reasoning_effort`, `sandbox_mode`, `mcp_servers`, `skills.config` (per
learn.chatgpt.com/docs/agent-configuration/subagents, fetched 2026-09-28).
Global tuning lives under `[agents]` in `config.toml` (this machine's has no
`[agents]` block, so it sits at unverified defaults). Spawning is
prompt-driven: the lead delegates via natural language and Codex's own
runtime does the fan-out and join. `codex agents` (confirmed via `--help`)
browses agent sessions on the shared local app-server daemon.
**Unverified**: the exact internal tool-call schema used to spawn a
subagent — the fetched doc describes it in prose only.

**B. `codex exec` subprocess fan-out.** What all existing Ruflo Codex code
uses. `codex exec [PROMPT]` (confirmed via `--help`) is the non-interactive
entrypoint; prompt is positional or read from stdin. `codex exec resume
[SESSION_ID|--last] [PROMPT]` and `codex exec fork` both exist. In
`~/dev/ruflo-wt-team-bus-hosts/v3/@claude-flow/codex/src/dual-mode/orchestrator.ts:224-233`,
worker args are built as `['exec', '--sandbox', readOnly?'read-only':
'workspace-write', '--skip-git-repo-check', ...(model?['-m',model]:[]),
enhancedPrompt]` and spawned via Node `child_process.spawn` — plain OS child
processes, not the native Agents feature. `.claude/agents/dual-mode/
codex-coordinator.md:10` and `codex-worker.md:10` document the same pattern
via shell `codex exec --sandbox workspace-write --skip-git-repo-check
"<prompt>" &` / `wait`. `orchestrator.ts:251` closes stdin
(`proc.stdin?.end()`) as a fix for upstream issue #2947: `codex exec` blocks
in `resolve_root_prompt` waiting for stdin EOF if left open — a real gotcha
for any code driving `codex exec` as a subprocess.

**Recommendation**: decide explicitly which mechanism a Codex team-bus
adapter targets. Native Agents (A) is cheaper to wire but spawning is
delegated through the model's own prompt-following, not a deterministic call.
`codex exec` fan-out (B) is what current Ruflo Codex code already
implements — reusing it is lower-risk than inventing a third convention.

### 1.2 Lifecycle hook / completion notification and its payload

Three distinct signals exist; none has a fully documented payload schema.

**`notify` (config.toml top-level key).** Confirmed live at
`~/.codex/config.toml:1-3`: an argv array, e.g. `notify =
["/path/to/SkyComputerUseClient", "turn-ended"]`. Codex appends a JSON
payload as an argument or via stdin at turn completion (per
learn.chatgpt.com/docs/config-file/config-reference, fetched 2026-09-28); the
exact schema is **not officially documented** (upstream issue
openai/codex#21990, "Document hooks payload schema"). This machine's config
fires per-turn, not per-subagent.

**Native Hooks (`hooks.json` + `[hooks]` in config.toml).** Event types per
the same docs page: `SessionStart`, `SessionEnd`, `PreToolUse`, `PostToolUse`,
`SubagentStart`, `SubagentStop`, `UserPromptSubmit`, `Stop`, `Interrupt`.
Confirmed live: `/Users/mathewbeane/weftos/.codex/hooks.json` already wires
several of these (including `SubagentStart` and `Stop`) to
`.claude/helpers/hook-handler.cjs`/`auto-memory-hook.mjs`, using the same
Claude-Code-shaped `{"hooks":{"EventName":[{"matcher":...,"hooks":[...]}]}}`
schema — ADR-320 itself notes "Hooks | Claude-compat + SubagentStop",
consistent with this. Codex trust-hashes each hook individually:
`~/.codex/config.toml`'s `[hooks.state]` table records a `trusted_hash` per
`<path>:<event>:<index>:<subindex>` key (including entries for this repo's
`hooks.json:subagent_start:0:0` and `...:stop:0:0`) — the ledger
`--dangerously-bypass-hook-trust` bypasses. `SubagentStop` is distinct from
whole-session `Stop` and is the direct analog of what
`scripts/grok-subagent-stop-hook.mjs` already wires to `team_on_stop` for
Grok. **Unverified**: the exact JSON field names on `SubagentStop`/`Stop`
stdin payloads. `grok-subagent-stop-hook.mjs:51-58` defensively reads several
possible field names for exactly this reason; a Codex hook script should
adopt the same pattern.

**`codex exec --json` event stream** (separate from hooks/notify — structured
stdout for a process driving one `codex exec` call). Per
takopi.dev/reference/runners/codex/exec-json-cheatsheet and upstream issue
#41216 (fetched 2026-09-28): `thread.started`, `turn.started`,
`turn.completed` (carries `usage`), `turn.failed` (carries `error`), and
`item.started`/`updated`/`completed` (final answer =
`item.completed`/`agent_message`). `orchestrator.ts:255-277` does **not** use
`--json` today — it buffers raw stdout/stderr and treats exit code 0 as
success. A future adapter could use `--json` for a structured completion
signal without touching hooks.

### 1.3 MCP server configuration for Codex

Two config scopes, both observed live. User-level: `~/.codex/config.toml`
(entries: `mcp_servers.claude-flow`, `.ruvnet-brain`, `.ruvector`,
`.node_repl`, `.computer-use`). Project-level: `<project>/.codex/config.toml`,
loaded only for trusted projects — confirmed live:
`/Users/mathewbeane/weftos/.codex/config.toml:9-23` registers
`[mcp_servers.claude-flow]` (`command="npx"`,
`args=["--no-install","@claude-flow/cli","mcp","start"]`, plus
`CLAUDE_FLOW_*` env vars), and `~/.codex/config.toml`'s
`[projects."/Users/mathewbeane/weftos"] trust_level = "trusted"` confirms it
actually loads.

Table shape: `[mcp_servers.<name>]` with `command`, `args`,
`[mcp_servers.<name>.env]`, `startup_timeout_sec`, `tool_timeout_sec`,
`enabled`, `required`; HTTP transport uses `url`/`bearer_token_env_var`
instead (confirmed by the live `cloudflare` entry in `codex mcp list --json`,
`"transport":{"type":"streamable_http","url":"..."}`). CLI management
(confirmed via `--help`): `codex mcp list [--json]`, `get`,
`add <name> (--url <URL> | -- <COMMAND>...) [--env K=V]`, `remove`,
`login`/`logout`.

Ruflo's own generator, `v3/@claude-flow/codex/src/mcp-config.ts`
(`getRufloMcpServerConfig`/`renderMcpServerToml`/`getRufloMcpAddCommand`),
emits `codex mcp add ruflo -- npx -y ruflo@latest mcp start`. Note:
`.grok/config.toml` (WeftOS) deliberately avoids `npx ruflo@latest` for its
own `[mcp_servers.ruflo]`, pointing at a local `feat/grok-host` checkout
instead — published tarballs don't carry `team_*` until a grok-host
release — the same caveat applies to any Codex generator using
`ruflo@latest` before a release ships `team_*`.

### 1.4 Shape a `team_spawn` "codex" plan should take, and stop → `team_on_stop`

**Fact**: `buildSpawnPlan()` (`team-tools.ts:201-251`) only emits
`host.grok` and `host.claude` branches today — no `host.codex` branch exists,
so `team_spawn` cannot currently produce a Codex-shaped plan. Grok's shape,
for comparison (`team-tools.ts:227-250`): `{teamId, name, role, prompt, next,
host: {grok: {contract, spawn: {description, background, isolation},
advisory: {capability_mode, subagent_type, note}}, claude: {taskType,
note}}}`.

**Recommendation**, following §1.1's conclusion to target `codex exec`
fan-out: `host.codex.spawn` should carry the literal argv, e.g.
`{command:"codex", args:["exec","--sandbox", isolation==='worktree'?
'workspace-write':'read-only', "--skip-git-repo-check", "-m", model, prompt]}`,
with an advisory that parallelism is process-level (`&`/`wait` or `spawn`),
not a Codex-native concept. If native Agents (§1.1-A) is targeted instead,
`host.codex.spawn` would describe an on-disk `.codex/agents/<role>.toml` to
write, with an advisory that the lead's own prompt (not the plan) triggers
delegation.

**Stop signal → `team_on_stop`**: directly analogous to
`scripts/grok-subagent-stop-hook.mjs` (lines 19-78): reads hook JSON from
stdin, extracts an agent name from several possible fields, resolves the
active team from `.claude-flow/teams/*/team.json`, and runs `spawnSync(node,
[bus, 'on-stop', '--team', team, '--agent', agent])` against
`scripts/grok-team-bus.mjs`, failing open. A Codex equivalent would register
under Codex's native `Stop`/`SubagentStop` event (§1.2) in
`.codex/hooks.json` instead of Grok's registration path — the `hooks.json`
schema itself needs no change since Codex already accepts the
Claude-compatible format; only the registration location and the hook-trust
ledger (`[hooks.state]`, which Grok does not require) differ.

### 1.5 What the dual-mode orchestrator already does (reuse, don't duplicate)

`v3/@claude-flow/codex/src/dual-mode/orchestrator.ts` (833 lines, tested via
`tests/dual-mode.test.ts` and `tests/dual-mode-stdin-2947.test.ts`) already
solves problems a new adapter would otherwise re-solve: correct `codex exec`
argv construction including the stdin-EOF fix for issue #2947 (line 251,
omitting it hangs a worker); dependency-level scheduling with a writer cap
(`buildDependencyLevels`/`partitionLevel`, lines 403-483 — topological levels
by `dependsOn`, batched by `maxConcurrent`/`maxWriters`, with worktree
isolation validation); timeout + output-size bounding per worker (lines
204-284, `setTimeout`→`SIGTERM`, capped stdout/stderr); environment
sanitization + per-worker identity (`workerEnvironment`, lines 600-653 —
strips secret-shaped env vars, sets `CLAUDE_FLOW_PRINCIPAL_ID=agent:<id>`,
and behind `isMcpCallerAuthEnabled()` mints a signed TTL'd `InvocationToken`
per worker, one per spawn since a worker can't re-request one, rationale at
lines 617-640); and an optional policy preflight (`authorizeWorker`, lines
531-557, via `npx ruflo@latest policy evaluate`) enforcing that a worker's
capability envelope never exceeds its parent's (`resolveWorkerEnvelope`,
lines 572-598).

**Architectural mismatch worth flagging**: the orchestrator's own
coordination (`initializeSharedMemory`/`collectSharedMemory`, lines 127-151,
488-501) shells out to `npx ruflo@latest memory init/store/list` — shared
AgentDB memory, not MCP — while ADR-320's team bus uses mailbox files under
`.claude-flow/teams/` with `team_on_stop`-driven pipeline advance. These are
two unconnected coordination mechanisms for the same `codex exec` primitive.
A team-bus adapter should decide whether to ride on top of
`DualModeOrchestrator` (reuse spawn/timeout/env, replace memory coordination
with mailbox/`team_on_stop` calls) or bypass it and duplicate only argv
construction — wrapping `executeHeadless`/`workerEnvironment` directly is the
lower-duplication path.

Lower priority: `v3/@claude-flow/codex/src/initializer.ts` generates
`AGENTS.md`/skills/`config.toml` for `init --codex`;
`v3/@claude-flow/codex/agents/*.yaml` look stale — stubs unrelated to Codex's
real native `.codex/agents/*.toml` format. **Unverified** whether anything
still reads `agents/*.yaml`.

### 1.6 `scripts/probe-host-live.mjs`'s existing Codex adapter, verbatim

Lines 305-355, the `codex` entry in `adapters`: `discover()` (307-309)
immediately records a `skip` — "Codex CLI has no inspect --json for rules,
agents, skills, or hooks" — never reading `.codex/hooks.json` or
`.codex/agents/`. `connect()` (310-350) runs `codex mcp list --json`, finds a
server named `ruflo` or `claude-flow`, records `mcp:configured` as a critical
pass but `mcp:handshake` as an explicit `skip` (`codex mcp list` shows
configuration only, never a live handshake); if the server's args include a
local `cli.js` path it checks `REQUIRED_TOOLS = [team_create, memory_store,
memory_retrieve, hooks_route, swarm_init, neural_status]` against real tool
names via `node <cli.js> mcp tools`. On this machine's real config
(`args=[...,"mcp","start"]`, no `cli.js` literal) that step is skipped —
`mcp:configured` PASSes but every `server-tool:*` check SKIPs, never
verifying `team_*` is exposed. `execute()` (351-354) **does nothing but
record two hardcoded skips**: "Codex can bypass hook trust, but this adapter
does not yet know a safe temp hook file Codex will load"; "Headless codex
exec is implemented only once a hook install path is confirmed." Key signal:
the probe's own authors declined to fake a passing `execute()`, leaving an
honest permanent skip until someone solves getting a hooks config trusted
without polluting `~/.codex/config.toml`'s trust ledger — the same problem a
Codex team-bus stop-hook registration will hit.

### 1.7 Other findings worth flagging

- Per `search_ruvnet`: `.github/workflows/codex-integration-audit.yml` runs
  `scripts/audit-codex-integration.mjs` on push/PR touching
  `v3/@claude-flow/codex/**` or `.claude/agents/dual-mode/**` — a
  static-invariant guard (issue #1909), not a live/build test.
  `@claude-flow/codex` is at version `3.0.3` per `package.json` (snapshot ~7
  days stale; possibly outdated).
- **No Codex-equivalent conformance bench exists** —
  `scripts/bench-grok-host-conformance.mjs` covers only Grok; there's no
  automated acceptance bar for Codex beyond the stubbed
  `probe-host-live.mjs --host codex`.
- `scripts/audit-codex-integration.mjs` guards a *different* surface: that
  Ruflo's `codex` MCP backend uses the real `mcp-server` subcommand, that
  `DualModeOrchestrator.codexCommand` defaults to `'codex'` (guarding a past
  bug defaulting to `'claude'`), and that dual-mode markdown references
  `codex exec` not `claude -p`. Adjacent to the team-bus adapter work — read
  before touching `dual-mode/orchestrator.ts`, since it fails CI on
  regression.

## Question 2: WeftOS

### 2.1 Can a WeftOS agent call external MCP tools today?

**Yes, and no new Rust code is needed** — WeftOS already has a working MCP
client stack: `McpClient` (`crates/clawft-services/src/mcp/mod.rs:207-260`)
wraps a `Box<dyn McpTransport>` with JSON-RPC `send_raw`, `ping` (line 252),
and graceful shutdown (line 259); `McpClientPool`
(`crates/clawft-services/src/mcp/client.rs:188-241`) adds connection pooling,
TTL'd schema caching, health checks, auto-reconnect, and auto-discovery from
`~/.clawft/mcp/`; `McpServerManager`
(`crates/clawft-services/src/mcp/discovery.rs:1-40`) does live
add/remove/list/reload, with durable config under `tools.mcp_servers` in
`clawft.toml`/`config.json`. `MCPServerConfig { command, args, env, url,
internal_only }` (`crates/clawft-types/src/config/mod.rs:961-994`) supports
both stdio and streamable-HTTP transports; `internal_only: true` by default
means tools aren't auto-registered into the `ToolRegistry` unless explicitly
enabled — the flag WeftOS already uses for infrastructure MCP servers.
CLI: `weft mcp add <name> -- npx -y @example/mcp-server` (or `--url` for
HTTP), `weft mcp list`/`remove`
(`crates/clawft-cli/src/commands/mcp_cmd.rs:20-21,340`). `McpToolWrapper`
(`crates/clawft-cli/src/mcp_tools.rs:93-138,228-267`) implements the `Tool`
trait, namespacing MCP tool names as `{server}__{tool}` (mirroring Ruflo's
`ruflo__team_send` style) and registering them via `register_mcp_tools` —
gated `#[cfg(feature = "services")]`, but `services` ships by default
(`crates/clawft-cli/Cargo.toml:28`), so the real path is what a normal `weft`
build has.

**Conclusion**: a WeftOS agent could call a hypothetical Ruflo
`team_send`/`team_inbox` today via `weft mcp add ruflo -- node
/path/to/ruflo/v3/@claude-flow/cli/bin/cli.js mcp start` (or the equivalent
config entry), no WeftOS code change needed — config-only integration for
WeftOS-as-caller. **Recommendation**: set `internal_only: false` (or a
per-tool allowlist if one exists — **unverified**) so `team_*` tools land in
the `ToolRegistry` and are callable by the agent loop, not just discoverable.

### 2.2 Can WeftOS be spawned headless as a teammate?

**Yes**, with an existing, well-defined contract:
`crates/clawft-cli/src/commands/agent.rs:49-73` — `AgentArgs { message:
Option<String>, model, config, intelligent_routing, trust_project_skills }`;
`-m`/`--message` triggers non-interactive mode (lines 50-52). If a kernel
daemon is reachable the message routes through the `agent.chat` RPC
(`agent_daemon::run_single_message`, lines 118-125); otherwise it falls to
the in-process path (lines 234-236). Output contract, both paths: stdout
carries only the assistant's reply text — the "Engine: …" banner and
diagnostics go to stderr (`agent.rs:252-253`, `agent_daemon.rs:99-100`, with
an explicit comment "so `-m` stdout stays clean for scripting"). On success:
`println!("{}", msg.content)` to stdout, exit 0. On failure
(`metadata.error == true`, or no response): error text to stderr,
`anyhow::bail!` (`agent.rs:309-311`, `agent_daemon.rs:106-109`), propagating
out of `main()` as exit code 1. Output is plain text today, not JSON.
Confirmed via the CLI's own tests: `main.rs:807,813` —
`Cli::try_parse_from(["weft", "agent", "--message", "hello world"])`.

Command: `weft agent -m "<task>"`. Exit 0 + clean stdout on success, exit 1 +
stderr message on failure.

### 2.3 Does WeftOS have a finish/idle signal for `team_on_stop`?

**No native equivalent exists.** Checked `crates/clawft-core/src/agent_bus/*`,
`agent/loop_core.rs`, `agent/effects.rs`, `clawft-rpc/src/protocol.rs`,
`clawft-kernel/src/{weaver,app}.rs` — no hook fires on agent-turn completion
analogous to Claude/Grok's `SubagentStop`. The only `on_stop` in the Rust
codebase is `clawft-kernel/src/app.rs:186,1433,1540,1593,1792,1798`, a K3
app/service manifest hook (`hooks.on_stop: Option<String>`, e.g.
`"cleanup.sh"`) for kernel-managed service/container stops — process
supervision, unrelated to agent-turn lifecycle.

The closest usable signal is the CLI process's own exit: `weft agent -m
"..."` returning (exit 0/1) is the natural "done" event for a one-shot
spawn, the same shape Grok's `SubagentStop` hook consumes — but WeftOS emits
no separate event; a wrapping script would have to watch the process exit,
mirroring `scripts/grok-subagent-stop-hook.mjs`.

### 2.4 Existing WeftOS work on the Grok team bus — and a conflict to resolve

WeftOS already has its own filesystem-based ADR-320 implementation, built
before/independent of PR #3512. `scripts/grok-team-bus.mjs:1-8` —
"Host-agnostic Agent Teams bus (ADR-320) — filesystem MVP. Works from any
host (Grok, Claude, Codex) without proprietary SendMessage" — stores state at
`<project>/.claude-flow/teams/<team>/team.json` plus
`.claude-flow/swarm/mailbox/`, with commands `create`, `spawn`, `send`,
`inbox`, `status`, `plan`, `on-stop`, `shutdown` — a near 1:1 conceptual
mirror of Ruflo's new MCP tools, just filesystem-based.
`scripts/grok-subagent-stop-hook.mjs` and
`.claude/helpers/grok-team-on-stop.cjs` are hook scripts (stdin JSON from
`SubagentStop`/`SubagentStart`) calling `grok-team-bus.mjs on-stop --team <t>
--agent <a>`, fail-open. `.grok/config.toml:1-23` registers
`[mcp_servers.weftos]` (`weft mcp-server`, WeftOS's own MCP server, ADR-075)
alongside `[mcp_servers.ruflo]` (local `feat/grok-host` checkout) as two peer
servers Grok connects to; a comment warns against `npx ruflo@latest` there
since published tarballs don't carry `team_*` yet.
`.grok/skills/agent-teams-grok/SKILL.md` documents the intended precedence:
prefer Ruflo's MCP `team_*` tools when live, CLI bus as fallback.
`.metaharness/hosts/grok/README.md` states the split ("Grok Build = executor
… Ruflo/claude-flow = orchestrator … WeftOS Rust = product kernel, no Node
required at runtime") with a "Host contract (for future host-grok)"
checklist a `host-codex`/`host-weftos` would follow the same shape of.
`docs/grok/README.md` already marks "MCP `team_*` tools inside Ruflo server
(ADR-320; local build / next publish)" `[x]`, npm publish still `[ ]`.

**Conflict to flag**: `grok-team-bus.mjs` and Ruflo's new MCP-based bus both
use the path `.claude-flow/teams/`. **Unverified** whether their
`team.json`/mailbox schemas actually match — needs a side-by-side diff before
treating them as interoperable; don't assume a WeftOS-written mailbox entry
is readable by Ruflo's `team_inbox` or vice versa. Also: `docs/grok/README.md`
links `../../v3/docs/adr/ADR-320-grok-host-agnostic-agent-teams.md`, i.e.
`/Users/mathewbeane/weftos/v3/docs/adr/...` — no `v3/` directory exists in
the WeftOS repo. Dead relative link, pointing at a path that only exists in
the Ruflo checkout; worth fixing regardless of this effort.

### 2.5 Minimal changes needed, each side

**WeftOS side** (no Rust changes required): add a `[mcp_servers.ruflo]`-
equivalent config entry (or `weft mcp add ruflo -- ...`) so `team_*` tools
land in a WeftOS agent's `ToolRegistry`, matching `.grok/config.toml` for
Grok; add a thin wrapper script (not a code change) that runs `weft agent -m
"<task>"` as a background child and, on exit, calls `grok-team-bus.mjs
on-stop` or Ruflo's `team_on_stop` MCP tool directly, mirroring
`grok-subagent-stop-hook.mjs` but triggered by process exit; resolve whether
WeftOS's filesystem team bus and Ruflo's MCP-based one should share one
schema (recommended: Ruflo's MCP tools primary once published,
`grok-team-bus.mjs` as offline fallback, matching `SKILL.md`'s precedence for
Grok extended to WeftOS); fix the dead `v3/docs/adr/...` link in
`docs/grok/README.md`.

**Ruflo side** (from §1): add a `host.codex` branch to `buildSpawnPlan()`
(`team-tools.ts:201-251`), shaped around `codex exec` argv (§1.4), reusing
`dual-mode/orchestrator.ts`'s spawn/timeout/env logic (§1.5); register a
Codex `Stop`/`SubagentStop` hook analogous to `grok-subagent-stop-hook.mjs`,
solving the "safe temp hook file Codex will trust without polluting
`~/.codex/config.toml`" problem `probe-host-live.mjs`'s `execute()` stub left
unsolved (§1.6); no `host.weftos` branch exists today (**unverified whether
one is needed** — WeftOS can already call Ruflo's tools as an MCP client,
§2.1, and be spawned headless, §2.2, so a `team_spawn` plan for a WeftOS
teammate would look like `{command:"weft", args:["agent","-m", prompt]}`,
mirroring §1.4's `host.codex` shape rather than needing new capability); and
a Codex-equivalent conformance bench (extending
`bench-grok-host-conformance.mjs`) is needed before any adapter can claim
parity with Grok — none exists today (§1.7).

## Open / unverified items

- Exact JSON payload field names for Codex `SubagentStop`/`Stop` hooks, and
  the internal tool-call schema Codex uses when delegating to a subagent.
- Whether WeftOS's `grok-team-bus.mjs` mailbox schema matches Ruflo's new MCP
  mailbox schema byte-for-byte — needs a direct diff.
- Whether a per-tool allowlist (narrower than `internal_only`) exists in
  WeftOS's MCP client config.
- Whether anything in the Ruflo tree still reads
  `v3/@claude-flow/codex/agents/*.yaml` (appears stale), and whether the
  `@claude-flow/codex` package version (`3.0.3` per search_ruvnet) is current.
