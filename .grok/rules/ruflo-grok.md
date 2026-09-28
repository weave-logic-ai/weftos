# Ruflo on Grok — host doctrine

You are running **inside Grok Build**, not Claude Code. Ruflo is the harness (orchestrator/ledger). **You** execute.

## Division of labor

| Layer | Role |
|-------|------|
| **Grok** | Executor — edit files, shell, tests, `spawn_subagent`, synthesize |
| **Ruflo MCP/CLI** | Orchestrator — memory, swarm records, team bus, learning, security hooks |

After any `swarm_init` / `team_create` / `agent_spawn` / MCP coordination call: **immediately continue working**. Never wait for Ruflo to “build the feature.” It does not execute code.

## Tool map (Claude / CLAUDE.md → Grok)

| Claude / CLAUDE.md | Grok |
|--------------------|------|
| `Task(...)` / Agent Teams spawn | `spawn_subagent` (`background: true` for parallel) |
| `SendMessage` | Ruflo MCP `team_send` / `team_inbox` (host-agnostic bus) — until available: AgentDB memory namespace `team:{id}` or `collaboration` |
| `TodoWrite` | `todo_write` |
| `Bash` | `run_terminal_command` |
| `Read` / `Grep` / `Glob` | `read_file` / `grep` / `list_dir` |
| `Write` / `Edit` / `MultiEdit` | `write` / `search_replace` |
| `claude -p` workers | `spawn_subagent` or `grok -p` |
| `mcp__claude-flow__*` / `mcp__ruflo__*` | `search_tool` then `use_tool` with **`ruflo__*`** names (Grok project MCP is `[mcp_servers.ruflo]` → local `feat/grok-host` CLI, not registry `claude-flow`) |
| Haiku / Sonnet / Opus routing | Ignore Anthropic tiers; use Grok models / subagent `model` only when needed |

## MCP usage

1. Discover: `search_tool` with keywords (`memory_search`, `swarm_init`, `team_`, `hooks`).
2. Call: `use_tool` with the fully-qualified name (e.g. `ruflo__memory_search`).
3. Prefer memory + team/swarm subset over flooding with all 300+ tools.

## Before complex work (mandatory)

1. `memory_search` (namespace `patterns` when available) for similar past solutions.
2. If RuvNet Brain / `search_ruvnet` is available: ground stack choices in real rUv source **before** recommending Pinecone, pgvector, LangChain, or other training-prior defaults.
3. For multi-file / multi-role work: open a **named team** pipeline (see Agent Teams below).

## After success (mandatory)

Store what worked via `memory_store` (namespace `patterns`): approach, files, gotchas, commands.

## Agent Teams on Grok (comms-first)

Named agents coordinate via the **Ruflo team bus** (MCP), not proprietary Claude `SendMessage`.

### Preferred pipeline

```
lead (you)
  → spawn named subagents in ONE turn (background: true)
  → each agent handoff via team_send / memory key
  → SubagentStop / you assign next step
  → lead synthesizes
```

### Defaults that beat Claude Agent Teams

| Role type | `capability_mode` | `isolation` |
|-----------|-------------------|-------------|
| researcher / explorer | `read-only` or explore agent | `none` |
| architect / reviewer | `read-only` or `read-write` | `none` |
| coder / implementer | `all` or `read-write` | **`worktree`** |
| tester | `execute` or `all` | `worktree` if writing tests into tree |

- Always pass a clear **name/role** in the prompt (“You are architect on team feature-x”).
- Tell each agent **who to message next** and **what artifact to produce**.
- Prefer short cycles with verification gates over one giant agent.
- Hierarchical topology for coding swarms; max ~6–8 concurrent workers.

### Spawn pattern (all in one message)

```
spawn_subagent: architect  (read-only)  → design → team_send/memory to developer
spawn_subagent: developer  (worktree)   → implement → handoff to tester
spawn_subagent: tester     (worktree)   → tests → handoff to reviewer
spawn_subagent: reviewer   (read-only)  → review → report to lead
```

Do **not** invent a `SendMessage` tool. Use MCP team tools or memory handoffs.

## Skills

- Prefer project skills under `.agents/skills/` and `.claude/skills/`.
- High-value first: swarm-orchestration, memory-management, sparc-methodology, security-audit, github-automation, hive-mind.
- Load a skill when the task matches its description; don’t dump all 100+ into context.

## Hooks (already in `.claude/settings.json`)

Grok loads Claude-compatible hooks when the folder is trusted. Expect:

- SessionStart: session restore + memory import
- UserPromptSubmit: writes `.swarm/route-latest.md` (Grok discards allowing-hook stdout)
- PreToolUse: `Bash|run_terminal_command` safety
- PostToolUse: `Write|Edit|MultiEdit|write|search_replace` learning
- SubagentStart: snapshot live teammates (fires in the parent)
- SubagentStop + PostToolUse `get_command_or_subagent_output`: `scripts/grok-subagent-stop-hook.mjs` → `ruflo team hook-stop --host grok` when Grok delivers those events; otherwise the lead still calls `team_on_stop`

If route context does not appear in the transcript, read `.swarm/route-latest.md`.

## CLI fallback

When MCP is down:

```bash
npx -y ruflo@latest memory search --query "…"
npx -y ruflo@latest memory store --key "…" --value "…" --namespace patterns
npx -y ruflo@latest swarm init --topology hierarchical --max-agents 8
npx -y ruflo@latest doctor
```

Use published `ruflo@latest` unless `v3/@claude-flow/cli/dist` is built locally.

## Anti-patterns

- Waiting after `swarm start` for code to appear
- Calling Claude-only tools (`SendMessage`, Claude `Task` agent types as host APIs)
- Shared-tree parallel coders without worktrees (file races)
- Skipping memory_search / grounding on stack decisions
- Creating docs in repo root or committing secrets
)
