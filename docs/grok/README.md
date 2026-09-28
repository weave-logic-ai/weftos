# Ruflo on Grok Build

Host integration so Ruflo runs **at least as well as Claude Code**, with stronger multi-agent isolation.

| Artifact | Purpose |
|----------|---------|
| [`.grok/config.toml`](../../.grok/config.toml) | Project MCP: `weftos` tool surface + **`ruflo` = local `feat/grok-host` CLI** |
| [Grok ↔ WeftOS MCP guide](../guides/grok-weftos-mcp.md) | Attach Grok Build to `weft mcp-server` (ADR-075) |
| [`.grok/rules/ruflo-grok.md`](../../.grok/rules/ruflo-grok.md) | Host doctrine + tool map |
| [`.grok/agents/`](../../.grok/agents/) | Pipeline roles (architect/coder/tester/reviewer) |
| [`.grok/skills/agent-teams-grok/`](../../.grok/skills/agent-teams-grok/) | Named teams skill |
| [`.grok/skills/handoff/`](../../.grok/skills/handoff/) | Session handoff → **`docs/handoff.md`** |
| [`scripts/grok-team-bus.mjs`](../../scripts/grok-team-bus.mjs) | Host-agnostic mailbox (ADR-402) |
| [ADR-402](https://github.com/ruvnet/ruflo/blob/main/v3/docs/adr/ADR-402-host-agnostic-agent-teams.md) | Architecture decision (upstream Ruflo PR [#3512](https://github.com/ruvnet/ruflo/pull/3512) + [#3513](https://github.com/ruvnet/ruflo/pull/3513): Grok/Claude bus, then Codex + generic command hosts) |

## Setup (once per machine)

1. **Harness is a local Ruflo checkout**, not npm. `package.json`
   `weftos.rufloPinNote` is the source of truth: `~/dev/ruflo` branch
   `feat/grok-host` (rebuild via `~/dev/ruflo/scripts/use-local-ruflo.sh`).
   As of 2026-09-28 the Agent Teams bus below (ADR-402, upstream
   `ruvnet/ruflo` PR #3512 + #3513) is not on `feat/grok-host` yet. If you
   need it before that lands, build a local Ruflo checkout that includes
   PR #3512 + #3513 and point the local, gitignored
   `.claude-flow/ruflo-cli-path` at its `cli.js` — do **not** repoint
   `.grok/config.toml` at a personal path; that file is tracked. Never
   `ruflo@latest` — it owns the AgentDB schema (WEFT-684) and lacks `team_*`.
2. **Trust the folder** in Grok (`/hooks-trust` or launch with `--trust`) — already required for project hooks.
3. **Restart Grok** (or reload MCPs) so `.grok/config.toml` is picked up.
4. Confirm:

```bash
grok mcp list
# expect: ruflo (project)
grok mcp doctor ruflo
# published: ~327 tools; local CLI build with team_*: ~336 tools
```

5. Optional daemon (project-local install preferred after `npm ci`):

```bash
ruflo daemon start
ruflo doctor
# PATH ruflo must be the grok-host link (use-local-ruflo.sh), not a registry copy.
```

6. Optional **RuvNet Brain** grounding (not installed by default):

```bash
npx ruvnet-brain@latest
# Uncomment [mcp_servers.ruvnet-brain] in .grok/config.toml
# init --grok expands $HOME → absolute paths. **Required:** env KB_DIR =
# absolute path to ~/.cache/ruvnet-brain/kb (without it: passages not found).
# Raise tool_timeout_sec (e.g. 600) for cold BGE model download.
```

7. **team_* without npm publish** — already the WeftOS Grok default once
   `feat/grok-host` carries ADR-402 (see step 1 for the interim path):

```bash
~/dev/ruflo/scripts/use-local-ruflo.sh
# .grok/config.toml [mcp_servers.ruflo] → …/v3/@claude-flow/cli/bin/cli.js
# Restart Grok → grok mcp doctor ruflo should list
# team_create … team_shutdown, team_run and team_trust_host
```

## Agent Teams (no Claude SendMessage)

Prefer **MCP** when available (in-session after local MCP or published team_*):

```
team_create → team_plan → team_spawn → spawn_subagent(plan) → team_send / team_inbox → team_on_stop → team_shutdown
```

CLI fallback (always works without MCP team_*):

```bash
node scripts/grok-team-bus.mjs create --name feature-x
node scripts/grok-team-bus.mjs plan --team feature-x \
  --steps '["architect","developer","tester","reviewer"]'
node scripts/grok-team-bus.mjs spawn --team feature-x --agent architect --role architect \
  --prompt "Design …" --next developer
# Use printed spawnPlan.host.grok → spawn_subagent (parallel, background)
```

See skill `agent-teams-grok`.

## Remotes (this fork)

| Remote | URL |
|--------|-----|
| `origin` | `git@github.com:weave-logic-ai/ruflo.git` |
| `upstream` | `git@github.com:ruvnet/ruflo.git` |

Feature work lands on `feat/grok-host` (or similar), not upstream `main` force-pushes.

## Status

- [x] Project MCP config + Grok rules
- [x] Host-agnostic team bus MVP (CLI)
- [x] Grok agents + skill
- [x] MCP `team_*` tools inside Ruflo server (ADR-402; local build / next publish)
- [x] `npx ruflo init --grok` productization
- [x] Brain MCP template: `KB_DIR` + timeouts + `$HOME` expand on init
- [x] Conformance bench (`scripts/bench-grok-host-conformance.mjs` — teams/swarm/hive/learning/neural/CLI)
- [ ] Publish a grok-host release that carries `team_*` so machines without
      `~/dev/ruflo` can pin npm; until then **feat/grok-host is the product path**
)
