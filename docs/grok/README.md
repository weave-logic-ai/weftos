# Ruflo on Grok Build

Host integration so Ruflo runs **at least as well as Claude Code**, with stronger multi-agent isolation.

| Artifact | Purpose |
|----------|---------|
| [`.grok/config.toml`](../../.grok/config.toml) | Project MCP (`weftos` tool surface + optional ruflo) |
| [Grok ↔ WeftOS MCP guide](../guides/grok-weftos-mcp.md) | Attach Grok Build to `weft mcp-server` (ADR-075) |
| [`.grok/rules/ruflo-grok.md`](../../.grok/rules/ruflo-grok.md) | Host doctrine + tool map |
| [`.grok/agents/`](../../.grok/agents/) | Pipeline roles (architect/coder/tester/reviewer) |
| [`.grok/skills/agent-teams-grok/`](../../.grok/skills/agent-teams-grok/) | Named teams skill |
| [`.grok/skills/handoff/`](../../.grok/skills/handoff/) | Session handoff → **`docs/handoff.md`** |
| [`scripts/grok-team-bus.mjs`](../../scripts/grok-team-bus.mjs) | Host-agnostic mailbox (ADR-320) |
| [ADR-320](https://github.com/ruvnet/ruflo/blob/main/v3/docs/adr/ADR-320-grok-host-agnostic-agent-teams.md) | Architecture decision (upstream Ruflo; the file lands with ruvnet/ruflo PR 3512) |

## Setup (once per machine)

1. **Scaffold** (any repo): `npx -y ruflo@3.32.38 init --grok` (use local CLI after build for newest templates).
   Pin matches `package.json` / `.mcp.json` (WEFT-684). Never `ruflo@latest` — it owns the AgentDB schema.
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
npx --no-install ruflo daemon start
npx --no-install ruflo doctor
# or, without a local install: npx -y ruflo@3.32.38 doctor
```

6. Optional **RuvNet Brain** grounding (not installed by default):

```bash
npx ruvnet-brain@latest
# Uncomment [mcp_servers.ruvnet-brain] in .grok/config.toml
# init --grok expands $HOME → absolute paths. **Required:** env KB_DIR =
# absolute path to ~/.cache/ruvnet-brain/kb (without it: passages not found).
# Raise tool_timeout_sec (e.g. 600) for cold BGE model download.
```

7. **Local team_* without npm publish** (monorepo only):

```bash
cd v3 && pnpm --filter @claude-flow/cli build
# Point [mcp_servers.ruflo] at node + ABS path to bin/cli.js (see config.toml comments)
# Restart Grok → grok mcp doctor ruflo should report ~336 tools
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
- [x] MCP `team_*` tools inside Ruflo server (ADR-320; local build / next publish)
- [x] `npx ruflo init --grok` productization
- [x] Brain MCP template: `KB_DIR` + timeouts + `$HOME` expand on init
- [x] Conformance bench (`scripts/bench-grok-host-conformance.mjs` — teams/swarm/hive/learning/neural/CLI)
- [ ] Publish release with team_* on pinned `npx ruflo@3.32.38` (bump pin deliberately; never `@latest`)
)
