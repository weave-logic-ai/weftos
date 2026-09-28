# Team bus: Codex and WeftOS hosts — design and build notes

Input: `docs/research/team-bus-codex-weftos-hosts.md` (hosts research), the
measured CLI flags (codex-cli 0.157.1, grok 1.0.41, `weft`), and the Ruflo
worktree `~/dev/ruflo-wt-team-bus-hosts` on branch `feat/team-bus-codex-weftos`.
The upstream-facing decision is recorded in that worktree as
"Amendment (2026-09-28) — Codex and custom command hosts" in
`v3/docs/adr/ADR-320-grok-host-agnostic-agent-teams.md`. This file holds the
build notes. Read the amendment first.

## 1. Decisions

| # | Decision | Why |
|---|----------|-----|
| D1 | **Codex stop signal: the runner parses `codex exec --json` plus the exit code. It is the only default.** The outcome is `turn.completed` + exit 0 → done; `turn.failed`, a non-zero exit or a timeout → failed. The native `SubagentStop` hook is opt-in only (`init --codex --team-hooks`). | This path writes nothing to `~/.codex/config.toml` or to the `[hooks.state]` trust ledger, and it needs no `/hooks` approval. The probe's old permanent skip (research §1.6) does not apply to it. Native delegation is prompt-triggered and not deterministic, so it cannot be the default. |
| D2 | Ruflo gets a generic **command host** (`.claude-flow/team-hosts.json`). It does **not** get a `weftos` host. | `weft agent -m "<task>"` fits the generic `<bin> <flags> <prompt>` shape exactly. Ruflo stays neutral, and WeftOS owns its host entry in its own repo. |
| D3 | WeftOS is **both** a spawnable exec host (via D2) and an MCP-client participant (`weft mcp add ruflo`). It needs no Rust changes for the MVP. | Research §2.1–2.3: the MCP client stack and the clean `-m` stdout contract already exist. |
| D4 | The stop signal for exec hosts is the runner observing process exit. It is not a host hook. | WeftOS has no agent-turn stop event (research §2.3). A Codex exec child needs no hook either. |
| D5 | The runner posts the child's final message to `next` (or `lead`) itself, then calls `team_on_stop`. | The bus advances even if a sandboxed child cannot or does not call `team_*`. Codex's read-only sandbox blocks the CLI-fallback bus, and whether Codex exec auto-approves MCP tool calls is unverified. |
| D6 | The shared seam is a `TeamHostAdapter` interface in `mcp-tools/team-hosts/`. | Keeps grok, claude, codex and command out of one growing `buildSpawnPlan`. |
| D7 | The runner reuses `@claude-flow/codex`'s process and env code by extracting two exported functions. It does not import `DualModeOrchestrator`. | Research §1.5: the orchestrator coordinates through AgentDB memory, which is a different model. Only the spawn/timeout/stdin-EOF logic and the secret-stripping env builder are reusable. The CLI already bundles `@claude-flow/codex`. |
| D9 | **Ruflo's `team-tools.ts` handlers are the only writer of `.claude-flow/teams/` and the mailbox.** `team.json` gains `schemaVersion: 1`; v0 is read and normalized, and a newer version is refused. The Ruflo Grok template bus and WeftOS's `scripts/grok-team-bus.mjs` both become shims over `ruflo team <verb> --params`. | Two writers with different locking and status rules would corrupt each other. The measured diff is in §2.10. |
| D8 | Model calls in the live probe become opt-in (`--execute`, `--live`). | Today `probe-host-live.mjs` runs a `PING` model turn by default unless `--no-execute` is passed. That breaks the no-model-calls-without-a-flag rule. **This changes the Grok probe's default, so it needs confirmation.** |

## 2. Ruflo side (owned by the Ruflo change; one squashed commit upstream)

Paths are relative to the worktree root.

### 2.1 Adapter seam — `v3/@claude-flow/cli/src/mcp-tools/team-hosts/`

- `types.ts` defines `TeamHostAdapter`, `SpawnContext` and `ExecSpec`:

  ```ts
  interface ExecSpec {
    command: string;
    args: string[];
    promptVia: 'stdin' | 'arg';
    closeStdin: boolean;
    passEnv: string[];
  }
  ```

- `grok.ts` moves `grokSpawnArgs`, `roleConstraint` and the Grok protocol lines out of `team-tools.ts` without changing them. `stopIdentity` reads `SUBAGENT_NAME`, `subagentName`, `agentName`, `agent`, `description` and `toolInput.description`, and strips a `role:` prefix.
- `claude.ts` produces `{ taskType, note }` as today.
- `codex.ts` builds the exec plan (see the amendment and §2.2). Its `stopIdentity` tolerates Claude-shaped field names, plus `agent_type`, `agent_id` and `name`. The real field names are **unverified**, so this is a live-probe item (§4.3).
- `command.ts` loads and validates `.claude-flow/team-hosts.json` and substitutes placeholders. It rejects: labels or commands outside `^[A-Za-z0-9._/-]+$`; any placeholder that does not fill a whole argv element; `promptVia: "arg"` with anything other than exactly one `{prompt}`; unknown keys. `kind` must be `exec`.
- `index.ts` holds the registry `getAdapter(label)`: built-ins first, then `team-hosts.json` labels, which resolve to the command adapter. An unknown label is an error, not a silent Grok fallback.
- `team-tools.ts`: `buildSpawnPlan` becomes about 20 lines. It builds the common protocol once, loops over `hosts`, and calls `adapter.plan()`. Keep the file under 500 lines. It is 664 today, so the move brings it down.

Common protocol lines (host-neutral): who you are and your team; read `team_inbox` first; the next agents; "your final reply is delivered to <next or lead> as your handoff"; then the task. Grok's lines about depth-1 nesting and `spawn_subagent` move into `grok.ts`.

### 2.2 Codex exec plan details

Built by `codex.ts` from the measured 0.157.1 flags only:

```
codex exec --sandbox <read-only|workspace-write> [--worktree] --skip-git-repo-check
           --json -o {resultFile} -C {cwd}
           -c mcp_servers.{mcpServer}.env.CLAUDE_FLOW_CWD="{teamRoot}"
           [-m <model>]  -        # prompt over stdin, then EOF
```

- Read-only roles (researcher, architect, reviewer, security) get `read-only`. Other roles get `workspace-write`, and `--worktree` is added when the role's isolation is `worktree`.
- `-m` is added only when `team_spawn` is given a `model`.
- Never emit `--full-auto` (it does not exist in 0.157.1) or `--dangerously-bypass-*`.
- `{mcpServer}` is resolved by the runner through `codex mcp list --json`. It picks the first enabled `ruflo` or `claude-flow`. If neither is found, the `-c` pair is dropped and a warning is recorded.
- `passEnv` defaults to `CODEX_HOME`, `OPENAI_API_KEY` and `OPENAI_BASE_URL`. The orchestrator's sensitive-name filter would otherwise strip `OPENAI_API_KEY`, and a key-auth Codex child would fail to log in.
- `advisory` carries `capability_mode`, `isolation`, and a note that parallelism is process-level: start several `ruflo team run` processes.

### 2.3 Extraction in `@claude-flow/codex` (enables D7)

New `v3/@claude-flow/codex/src/dual-mode/process.ts`, exported from the `./dual-mode` entry:

- `runHeadlessProcess({ command, args, cwd, env, stdinText?, timeoutMs, maxOutputBytes }) → Promise<{ code, stdout, stderr, timedOut, ms }>`. This is the body of `executeHeadless` from `spawn` onward, including `stdin.end()` after the optional write. It resolves with the exit code instead of rejecting on a non-zero exit.
- `buildWorkerEnvironment(base, { principalId, dbPath?, envelope?, passEnv? })`. This is the strip loop plus the `FORCE_COLOR`, `PRINCIPAL_ID` and `ENVELOPE` settings, with `passEnv` re-added after stripping. Token minting stays in the orchestrator.
- `DualModeOrchestrator.executeHeadless` and `workerEnvironment` call these, and their behavior is unchanged. `tests/dual-mode.test.ts`, `tests/dual-mode-stdin-2947.test.ts` and `scripts/audit-codex-integration.mjs` must still pass. That audit fails CI on regressions here.

### 2.4 CLI command — `v3/@claude-flow/cli/src/commands/team.ts`

Register it in `commands/index.ts`.

`team <create|spawn|send|inbox|broadcast|plan|status|on-stop|shutdown> --params '<json>'`: look up the matching `team_*` handler in `teamTools`, call it in-process, and print its result as JSON. Exit 1 when `success === false`. This is the public, non-MCP interface that the shims (D9) call. Map `on-stop` to `team_on_stop`.

It also has two more subcommands, `run` and `hook-stop`.

`team run --team T --agent A [--host L] [--timeout 1800000] [--max-output 1048576] [--dry-run] [--json]`:

1. Load `team.json` and the member. The member must be registered with a stored `spawn[L]` of kind `exec`. The default L is `team.host`.
2. Pick `runId = run_<ts>_<rand>`. `resultFile = .claude-flow/teams/T/runs/A-<runId>.last.txt`. `cwd` is the project root. `teamRoot` is `getProjectCwd()`.
3. Resolve placeholders. For Codex, resolve `{mcpServer}`.
4. With `--dry-run`, print `{command, args, cwd, passEnvNames}` and exit 0.
5. Mark the member `running` (with `runId`), then call `runHeadlessProcess`.
6. Get the result text from `resultFile` if it exists, otherwise from stdout. When the plan has `events: "codex-jsonl"`, parse stdout line by line; lines that are not JSON are ignored. Take the thread id from `thread.started` and record it on the member for a later `codex exec resume`. Take the terminal event from the last `turn.completed` or `turn.failed` (`turn.failed.error.message` becomes `reason`). The Codex plan sets `events: "codex-jsonl"`; command hosts have no `events` field.
7. Write `runs/A-<runId>.json` with `{runId, host, code, timedOut, ms, resultFile, threadId?}`.
8. Call the `team_send` handler directly (in-process, no MCP round trip) with `from: A`, `type: "result"`, to each `next` or to `lead`, and the result truncated to 64 KB with a pointer to the run file.
9. Call the `team_on_stop` handler with `outcome`, `runId` and `reason`. Plain exec hosts: `done` if and only if code 0 and no timeout. `codex-jsonl`: `done` if and only if code 0, no timeout, and the terminal event is not `turn.failed`. An exit 0 with no terminal event is `done`, and `warnings: ["noTerminalEvent"]` goes into the run file.
10. Exit with the child's code.

`team hook-stop --host H`: read stdin (300 ms cap, as in the Grok hook), run `getAdapter(H).stopIdentity(payload)`, and resolve the team as payload team, then `TEAM_NAME`, then the single active team. If there is **more than one active team and no explicit team, do nothing**. The Grok hook currently picks `names[0]`, which is a mis-advance risk worth fixing here too. Call `team_on_stop`. Always exit 0.

### 2.5 `team_on_stop` and team.json writes

These are in `team-tools.ts`:

- New optional inputs: `outcome`, `runId`, `reason`. For `failed`, set the step status to `failed`, set `member.status = 'failed'`, do not advance, and return `assign.hint` as "retry with `ruflo team run` or reassign".
- Dedupe: if `runId` equals `member.lastStopRunId`, return `{ duplicate: true }` without changes.
- Normalize an `agent` containing `:` to the part after the last `:`, then run `safeName`.
- Add a `withTeamLock(name, fn)` helper: `openSync(team.json.lock, 'wx')`, retrying every 25 ms for up to 2 s, then remove it; treat a stale lock older than 10 s as broken. `saveTeam` writes `team.json.tmp` and then `renameSync`s it. Wrap every read-modify-write handler (spawn, plan, on_stop, shutdown) in the lock.
- Also fix `templates/grok/scripts/grok-subagent-stop-hook.mjs` and its `scripts/` copy: strip the `role:` prefix, and do not guess among several active teams. These are small edits, and the bug is real (research §1.4 and the current `description` fallback).

### 2.6 `init --codex` additions (in `@claude-flow/codex`)

- `generators/agents-md.ts` gets a new exported `renderTeamBusSection()`, included by the `default`, `full` and `enterprise` templates. `minimal` gets a 3-line pointer to the skill. The content is the lead loop and the child rules in the amendment. Use no personal paths, and use only `ruflo team run` and `team_*` names.
- `v3/@claude-flow/codex/.agents/skills/agent-teams/SKILL.md` is packaged, added to `BUILT_IN_SKILLS` and `ALL_AVAILABLE_SKILLS` in `templates/index.ts`, and added to the `minimal` and `default` defaults. `retainCanonicalPackagedSkills` then keeps it.
- `initializer.ts` gets a new `mergeTeamStopHook()`, which runs **only when `teamHooks: true`** (the `init --codex --team-hooks` flag, threaded through `CodexInitOptions`). By default no hook is written. It reads `.codex/hooks.json` or starts from `{hooks:{}}`. If no existing command contains `team hook-stop`, it appends `{hooks:[{type:"command", command:"npx -y ruflo@latest team hook-stop --host codex", timeout:30}]}` under `SubagentStop`. On Windows it uses the `cmd /c` form, mirroring `getRufloMcpServerConfig`. It writes with a 2-space indent and never removes or reorders existing entries. It pushes the `/hooks` trust instruction into `warnings`, the same channel the plugin activation message uses. It never touches `~/.codex/config.toml`.
- MCP registration: no change. `registerMCPServer` already adds `ruflo` when it is missing.
- `commands/init.ts` (cli): add one bullet to the Codex "Next steps" list, "Teams: `ruflo team run`; see AGENTS.md → Agent Teams".

### 2.7 Tests (vitest, Ruflo)

- `cli/__tests__/team-tools.test.ts`: keep the existing test **unchanged** as the Grok regression guard. Add:
  - the default `hosts` for a `codex` team, which yields `host.codex` + `host.claude`;
  - the Codex plan for a read-only role (`--sandbox read-only`, ends in `-`, `promptVia: stdin`, `closeStdin: true`, no `--worktree`);
  - the Codex plan for a write role with worktree isolation;
  - no plan contains `bypass` or `full-auto`;
  - a command host from a temp `team-hosts.json`, with the valid case plus rejects for a shell metacharacter in `command`, `{prompt}` embedded in a larger string, two `{prompt}`s, and an unknown label;
  - `team_on_stop` with `failed` (no advance), a duplicate `runId` (no-op), and `role:agent` normalization;
  - 10 concurrent `team_on_stop` calls on different agents, after which team.json parses and every member is marked.
- `cli/__tests__/team-hosts-stop-identity.test.ts`: fixture payloads per adapter, including Grok's `description` form.
- `cli/__tests__/team-run.test.ts`: register a command host whose command is `process.execPath` with args `["-e", "process.stdout.write('ok:'+process.argv[1])", "{prompt}"]`. Check that `team run` writes the run file, delivers a `result` message to `next`, advances the plan, and exits 0. Add a failing variant (exit 3, which yields `failed` and no advance), a timeout variant, and `--dry-run` (no process started).
- `codex/tests/process.test.ts`: `runHeadlessProcess` stdin write-then-EOF, timeout, output cap; `buildWorkerEnvironment` strips `FOO_TOKEN` but keeps a `passEnv` key.
- `codex/tests/initializer.test.ts`: `mergeTeamStopHook` on an empty dir, on an existing hooks.json with other events (preserved), and on a second run (idempotent).
- `codex/tests/generators.test.ts`: the AGENTS.md default contains the team section, and the skill payload validates.

### 2.8 Bench and probe

- `scripts/bench-host-conformance.mjs` is the renamed generalization. `bench-grok-host-conformance.mjs` stays as a thin alias that forwards `--host grok`, the same pattern as `probe-grok-host-live.mjs`. The `--host grok|codex|command|all` flag selects domains. Existing Grok domains and their 91/92 result must not regress. Reports go to `docs/benchmarks/host-conformance-<host>-latest.{md,json}`; keep the existing Grok report path for the alias. New domains, all without model calls:
  - `host-plan`: `mcp exec team_spawn` per host, with shape assertions as in the unit tests.
  - `runner`: `team run` against the fake command host in a temp project.
  - `codex-init`: `init --codex` into a temp dir with `--skip-mcp`. If the initializer has no such option, run `mergeTeamStopHook` and the generators directly via a small import script. Assert the AGENTS.md section, the skill dir and the hooks.json entry. It must not call `codex mcp add` or `codex plugin add` against the real user config.
- `scripts/probe-host-live.mjs`:
  - Model turns are opt-in (D8). `--execute` runs one turn per host, and `--live` implies `--execute` and adds the team round-trip. `--no-execute` is accepted as a no-op for back-compat.
  - Codex `discover`: read project `.codex/hooks.json` and assert the `SubagentStop` → `team hook-stop` entry. Also check `.agents/skills/agent-teams/SKILL.md`. It remains a SKIP when the project was not initialized for Codex.
  - Codex `connect`: unchanged. The `codex mcp list` handshake stays a SKIP.
  - Codex `execute` (`--execute`): `codex exec --ephemeral --sandbox read-only --skip-git-repo-check --json -` with the prompt `Reply with exactly: PING` on stdin. Pass if a `turn.completed` event arrives. No hook trust is involved, which removes the old permanent skip.
  - Codex `live` (`--live`): a temp team with plan `[child]`, `team_spawn` with `hosts:["codex"]` and a read-only role, then `ruflo team run`. Assert that the run file exits 0, the lead inbox has a `result` message, and plan index = 1. Record (warn, not critical) whether the child called any `team_*` tool, which answers the MCP-approval unknown.
  - Command host `live` (`--live --host command`, optional `--command-label <label>`): the same round-trip using a label from the project's `team-hosts.json`. This is how WeftOS gets verified live without Ruflo knowing about it.

### 2.10 Canonical format (D9): measured diff and required changes

Diff of WeftOS `scripts/grok-team-bus.mjs` against Ruflo `team-tools.ts` and Ruflo `scripts/grok-team-bus.mjs` (identical to its template copy):

| Area | WeftOS script | Ruflo `team-tools.ts` | Risk |
|---|---|---|---|
| Paths | `.claude-flow/teams/<t>/team.json`, `.claude-flow/swarm/mailbox/<agent>/<prio>_<id>.json`, `archive/` | same | none |
| `team.json` top level | `id, name, topology, maxAgents, status, createdAt, host, members, plan` | same keys; `maxAgents` clamped 1–50; `host` from input (the script hardcodes `grok`) | low |
| Message | `id, teamId, from, to, summary, content, type, priority, timestamp` | identical | none |
| inbox/archive | identical drain/peek | identical | none |
| `team_plan` object steps | stored **verbatim** (`: s`) — may lack `id`, `agent` or `status`, and step 0 is not set to `ready` | normalized to `{id, agent, status}`, with step 0 set to `ready` | **medium**: a Ruflo `on_stop` against a verbatim step with no `agent` never advances |
| `member.spawn` | flat v0 Grok plan `{grok:{subagent_type,capability_mode,isolation,background}, claude}` | `{grok:{contract,spawn,advisory}, claude}` (and after this change `codex` and `<label>` with `exec`) | low for reads; `team run` must refuse v0 |
| After this change | no lock, whole-file `writeFileSync` | `O_EXCL` lock plus tmp+rename, `failed`/`running` statuses, `lastStopRunId` | **high**: the script's unlocked read-modify-write can overwrite a concurrent locked write (a lost update), and its `on-stop` advances a `failed` step |

Required Ruflo-side changes (in the §2.5 lock work):
- Export `TeamState`, `TeamMember`, `PlanStep`, `TeamMessage` and `TEAM_SCHEMA_VERSION = 1` from `team-tools.ts`, and stamp `schemaVersion` on every save.
- `loadTeam` normalizes v0 data: it fills step `id`/`agent`/`status` by the `team_plan` rule, and it leaves unknown member fields untouched. Every handler preserves unknown keys (already true today, because handlers mutate the parsed object; add a test).
- `loadTeam` returns an error for `schemaVersion > TEAM_SCHEMA_VERSION`, and handlers must not write in that case.
- Turn `templates/grok/scripts/grok-team-bus.mjs` (and the repo's `scripts/` copy) into a shim. It keeps the same flags and help, and each verb calls `ruflo team <verb> --params <json>` via `spawnSync`. The binary resolves as `RUFLO_CLI` (a path to cli.js, run with node) → local `node_modules/.bin/ruflo` → `ruflo` on PATH → `npx -y ruflo@latest`. It prints the handler JSON unchanged. It has no file I/O of its own.
- Tests: `cli/__tests__/team-format.test.ts` covers the v0 fixture (`__tests__/fixtures/team-v0/`: a team.json with a verbatim object step lacking `status`, a flat v0 `spawn`, and one mailbox message): `team_status` reads it, `team_on_stop` advances it, the result has `schemaVersion: 1` and keeps an unknown key, `schemaVersion: 2` is refused without a write, and `team run` refuses the v0 member. `cli/__tests__/grok-bus-shim.test.ts` runs the shim with `RUFLO_CLI` set to the built CLI through a create → plan → spawn → send → inbox → on-stop → status sequence, and asserts the final team.json equals the result of the same sequence run through the handlers (after dropping timestamps and ids).

### 2.9 Upstream hygiene gate (run before the squash)

```
git diff <base>..HEAD | grep -nE 'WEFT-|weftos|clawft|/Users/|mathewbeane|feat/grok-host|feat/team-bus|aepod|weavelogic' \
  && echo FAIL || echo clean
```

The only allowed hits are none. The amendment does not name WeftOS, and the ADR's existing "Deciders" line predates this change, so leave it. Use `myagent` as the example command host in docs and tests. Squash to one commit whose message describes the Codex adapter, the command host, and the runner.

**Also flag to the maintainer:** `v3/docs/adr` has two ADR-320 files (`ADR-320-mcp-composition-inspector-channel-guardrails.md` from an earlier upstream PR). This change does not renumber either. The PR description should mention the collision.

## 3. WeftOS side (owned by this repo; follows the repo rules: `scripts/build.sh`, dashboard board ticket, never master)

No Rust changes are needed for the MVP.

1. **Host entry.** Add `.claude-flow/team-hosts.json`:

   ```json
   { "hosts": { "weft": { "kind": "exec", "command": "weft",
       "args": ["agent", "-m", "{prompt}"], "promptVia": "arg",
       "passEnv": ["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "OPENROUTER_API_KEY"],
       "isolation": "none" } } }
   ```

   Add `--trust-project-skills` to `args` only if team children need workspace skills; that is a security choice (SEC-SKILL-05), so leave it out by default. The `passEnv` list must match the providers `.clawft/config.json` actually uses. Check the `.env` shadow gotcha before trusting env-based keys.
2. **Participant (MCP client).** Document and script the registration:
   `weft mcp add ruflo --env CLAUDE_FLOW_MCP_TOOLS=team --env CLAUDE_FLOW_CWD=<repo root> --internal-only false -- node <ruflo-cli>/bin/cli.js mcp start`.
   Use a local Ruflo checkout until a published release carries `team_*`, the same caveat `.grok/config.toml` already records. `CLAUDE_FLOW_MCP_TOOLS=team` keeps the ToolRegistry from filling with 300+ Ruflo tools. Put the steps in `docs/guides/` (the existing `grok-weftos-mcp.md` guide is the natural home) instead of a new top-level doc.
3. **Existing CLI bus becomes a shim (D9).** Replace the body of `scripts/grok-team-bus.mjs` with the same shim Ruflo ships (§2.10). It keeps the WeftOS flag set and delegates every verb to `ruflo team <verb> --params`. `RUFLO_CLI` defaults to the pinned local checkout's `v3/@claude-flow/cli/bin/cli.js`, read from an env var or `.claude-flow/ruflo-cli-path`, and never from a tracked absolute path (per the rufloPinNote rule). **Delete the independent writer.** If Ruflo cannot be resolved, the shim exits 2 with a message; it does not write a divergent format. The Grok `subagent_type` names (`ruflo-architect` …) move into the spawn prompt path of Ruflo's Grok adapter if they are still wanted; they are advisory only in 1.0.41, so dropping them is acceptable.
   - Point `scripts/grok-subagent-stop-hook.mjs` and `.claude/helpers/grok-team-on-stop.cjs` at `ruflo team hook-stop --host grok` (this carries the `role:` prefix fix and the several-active-teams rule), keeping the fail-open exit 0.
   - **Interop test**, `scripts/grok-team-bus.interop.test.mjs` (`node --test`, the same convention as `scripts/ticket-steward.test.mjs`). In a temp project with `CLAUDE_FLOW_CWD` set to it:
     - (a) The shim runs create → plan → spawn, then Ruflo `ruflo team send` writes a message, the shim `inbox` drains it, Ruflo `on-stop` advances, and the shim `status` shows index 1. Assert the team.json has `schemaVersion: 1`, and that its key set matches the `TeamState` keys (from a golden list in the test).
     - (b) An old-format team.json (a copy of the pre-change output of this script, checked in under `scripts/fixtures/team-v0/`) is advanced correctly by the shim's `on-stop` and stamped v1.
     - (c) 8 parallel `on-stop` calls (the shim and Ruflo mixed) leave team.json parseable, with every member idle.
     - When `RUFLO_CLI` cannot be resolved the test calls `t.skip('ruflo CLI not resolvable')`, so it reports SKIP and not PASS. Run it with `node --test scripts/grok-team-bus.interop.test.mjs`.
4. **Dead link.** Fix `docs/grok/README.md`'s `../../v3/docs/adr/...` link (research §2.4). It should point at the upstream ADR URL.
5. **Optional later (separate tickets, not blocking):**
   - `weft agent --json`, for a structured result and usage;
   - `weft agent --session <id>`, for stage continuity like `codex exec resume`;
   - an agent-turn-complete event on the kernel bus, which would give native-hook parity.

   None of these is required, because the runner uses process exit and stdout.

## 4. Build order

1. `@claude-flow/codex` §2.3 extraction plus its tests. Confirm the dual-mode tests and `audit-codex-integration.mjs` are green.
2. The adapter seam §2.1, moving the Grok code with **no behavior change**. The existing Grok test must stay green before continuing.
3. `team_on_stop`, the lock and the canonical format §2.5 + §2.10: schemaVersion, v0 normalization, the Grok bus shim, `ruflo team <verb>` (move this part of §2.4 up), and the format and shim tests.
4. The Codex adapter §2.2 and the command adapter with `team-hosts.json` validation, with their unit tests.
5. The `ruflo team run` / `hook-stop` command §2.4, with the fake-host runner tests.
6. `init --codex` additions §2.6, with the initializer and generator tests.
7. The bench generalization and new domains, then the probe changes §2.8. Run the bench for `--host all`, and the probe with `--no-execute` equivalents (the default) for grok, codex and command.
8. Live, flag-gated, on this machine, and only with the lead's approval because they spend model calls:
   - `probe-host-live.mjs --host codex --execute`
   - `probe-host-live.mjs --host codex --live`
   - `probe-host-live.mjs --host grok --execute`
9. The hygiene gate §2.9, then the squash to one commit on the feature branch. Run the Ruflo build, typecheck, lint and tests before committing.
10. WeftOS side §3 — the shim and the interop test first (it can run as soon as step 3 is built, against the worktree CLI via `RUFLO_CLI`), then the host entry and the MCP registration. Do this on a WeftOS feature branch with a dashboard board ticket, then `probe-host-live.mjs --host command --command-label weft --live` from the WeftOS checkout, with approval.

### 4.3 Unknowns the live runs must settle

- Whether `codex exec` in `read-only` or `workspace-write` auto-approves MCP tool calls. D5 makes correctness independent of this; the probe records it.
- The payload field names for Codex `SubagentStop`. This matters only for the opt-in `--team-hooks` path; the default runner path does not depend on it.
- Where `codex exec --worktree` puts the worktree, and whether the `--json` stream reports its path. The run file records it when it is present.
- Whether `weft`, when started as an MCP client, passes its env through to the Ruflo child. If it does not, the `CLAUDE_FLOW_CWD` in the `weft mcp add --env` registration covers it.
