---
name: agent-teams-grok
description: >
  Run Ruflo-style named Agent Teams on Grok Build using the host-agnostic team bus
  (scripts/grok-team-bus.mjs). Use when coordinating multi-agent pipelines, feature
  development with architect→coder→tester→reviewer, or replacing Claude SendMessage.
---

# Agent Teams on Grok (ADR-320)

Claude `SendMessage` / TeammateTool are **not** available. Prefer **Ruflo MCP** `team_*` tools; CLI bus is the fallback.

## Preferred: MCP tools

| Tool | Purpose |
|------|---------|
| `team_create` | Create team + topology |
| `team_plan` | Ordered pipeline steps |
| `team_spawn` | Register agent; returns **spawn plan** for `spawn_subagent` |
| `team_send` / `team_broadcast` | Handoff messages |
| `team_inbox` | Drain/peek mailbox |
| `team_status` / `team_on_stop` / `team_shutdown` | Lifecycle |

Discover: `search_tool` query `team_create`. Call via `use_tool` as `ruflo__team_create` etc.

## Quick start (CLI fallback)

```bash
# 1. Create team
node scripts/grok-team-bus.mjs create --name feature-x --topology hierarchical

# 2. Register plan
node scripts/grok-team-bus.mjs plan --team feature-x \
  --steps '["architect","developer","tester","reviewer"]'

# 3. Register agents (prints spawnPlan JSON for spawn_subagent)
node scripts/grok-team-bus.mjs spawn --team feature-x --agent architect --role architect \
  --prompt "Design <feature>. Handoff to developer." --next developer
node scripts/grok-team-bus.mjs spawn --team feature-x --agent developer --role developer \
  --prompt "Implement from architect design." --next tester
node scripts/grok-team-bus.mjs spawn --team feature-x --agent tester --role tester \
  --prompt "Test implementation." --next reviewer
node scripts/grok-team-bus.mjs spawn --team feature-x --agent reviewer --role reviewer \
  --prompt "Review code and tests."
```

## Lead (you) then

1. Parse each `spawnPlan.host.grok` from stdout.
2. Call **`spawn_subagent`** in **one message** for all agents (`background: true`).
3. Use `spawnPlan.prompt` as the child prompt; set `subagent_type`, `capability_mode`, `isolation` from the plan.
4. Prefer agent types under `.grok/agents/` when available (`ruflo-architect`, `ruflo-coder`, …).
5. `grok-team-on-stop.cjs` marks the teammate idle when Grok fires `SubagentStop` (or PostToolUse on the wait tool). If `lastStopAt` is missing, the lead still calls `on-stop`. Then spawn/resume the next plan step.
6. Synthesize results; `shutdown` the team.

## Defaults (better than Claude shared-tree teams)

| Role | capability_mode | isolation |
|------|-----------------|-----------|
| architect / reviewer | read-only / plan | none |
| developer / tester | all | **worktree** |

## Messaging

```bash
node scripts/grok-team-bus.mjs send --team feature-x --to developer \
  --from architect --summary "design" --message "..."
node scripts/grok-team-bus.mjs inbox --team feature-x --agent developer
node scripts/grok-team-bus.mjs status --team feature-x
```

## Also use Ruflo MCP when live

- `memory_search` / `memory_store` with namespace `team:feature-x`
- `swarm_init` for coordination records (then **you** still execute)

## Anti-patterns

- Waiting for `swarm start` to write code
- Inventing SendMessage
- Parallel coders on the same tree without worktrees
