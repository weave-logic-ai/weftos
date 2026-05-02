# Skills and Agents

This guide covers the skills and agents systems: how to define, discover, and
use reusable prompt bundles (skills) and custom personas (agents).

---

## Skills

A **skill** is a reusable LLM instruction bundle with metadata. Skills package
a prompt template, variable declarations, and tool permissions into a single
unit that can be activated on demand.

### SKILL.md format

The preferred format is a single `SKILL.md` file with YAML frontmatter and a
markdown body containing the LLM instructions:

```markdown
---
name: research
description: Deep research on a topic
version: 1.0.0
variables:
  - topic
  - depth
allowed-tools:
  - WebSearch
  - Read
  - Grep
user-invocable: true
argument-hint: Search query or topic
---

You are a research assistant. Given a {{topic}}, perform deep research
at the requested {{depth}} level. Use WebSearch for current information
and Read/Grep for local files.
```

#### Frontmatter fields

| Field | Required | Description |
|-------|----------|-------------|
| `name` | yes | Skill identifier (must match directory name) |
| `description` | no | Human-readable summary shown in listings |
| `version` | no | Semantic version string |
| `variables` | no | Template variable names used in the body |
| `allowed-tools` | no | Tool allowlist (empty = all tools allowed) |
| `user-invocable` | no | Whether users can invoke via `/use` (default: false) |
| `disable-model-invocation` | no | Block LLM from invoking this skill (default: false) |
| `argument-hint` | no | Hint text for the slash-command argument |

Additional fields are preserved as metadata (e.g. `openclaw-category`,
`openclaw-license`). Both hyphenated (`allowed-tools`) and underscored
(`allowed_tools`) field names are accepted.

### Legacy skill.json format

The older format uses a `skill.json` metadata file and a separate `prompt.md`
for instructions:

```
my-skill/
  skill.json
  prompt.md
```

`skill.json`:

```json
{
  "name": "my-skill",
  "description": "A legacy skill example",
  "variables": ["topic"],
  "allowed_tools": ["Read"],
  "user_invocable": true
}
```

`prompt.md`:

```markdown
You are an assistant. Research the following topic: {{topic}}
```

When both `SKILL.md` and `skill.json` exist in the same directory, `SKILL.md`
takes precedence.

### Discovery chain

Skills are loaded from three levels, highest priority first:

1. **Workspace** -- `.clawft/skills/` in the project root (walks upward from cwd)
2. **User** -- `~/.clawft/skills/`
3. **Built-in** -- compiled into the binary

When the same skill name appears at multiple levels, the higher-priority
source wins. Each level is a directory containing skill subdirectories:

```
.clawft/skills/
  research/
    SKILL.md
  coding/
    SKILL.md
```

### CLI commands

```bash
# List all discovered skills with source annotation
weft skills list

# Show details of a specific skill (description, variables, instructions preview)
weft skills show research

# Install a skill from a local path into ~/.clawft/skills/
weft skills install /path/to/my-skill

# Remove a user-installed skill
weft skills remove my-skill

# Generate an Ed25519 signing keypair for skill publishing
weft skills keygen
```

`weft skills list` prints a table with columns: NAME, SOURCE (workspace /
user / builtin), FORMAT (SKILL.md / legacy), and DESCRIPTION.

For publishing signed skills and the trust-root model, see
[skill-signing.md](skill-signing.md).

### Hot-reload

The skill system watches the filesystem for changes. When a `SKILL.md` or
`skill.json` file is modified, the loader performs an atomic swap so that
in-flight skill invocations complete with the old definition while new
invocations use the updated version. No restart is required.

### MCP exposure

Loaded skills are automatically exposed as MCP tools via `SkillToolProvider`.
Any MCP client connected to `weft mcp-server` can invoke skills as tools.

### Slash-command framework

Skills with `user-invocable: true` contribute commands to the `/help` listing
in interactive sessions. The `argument-hint` field is shown next to the
command name.

### Interactive slash commands

In a `weft agent` interactive session:

```
/skills              -- list available skills
/use research        -- activate the "research" skill
/use                 -- deactivate the current skill
/status              -- show current agent, model, and active skill
```

### Security

Skills are subject to several security controls:

- **SEC-SKILL-01**: YAML frontmatter nesting depth is limited to 10 levels.
  Deeply nested YAML is rejected.
- **SEC-SKILL-02**: Directory names are validated against path traversal
  (`..`, `/`, `\`).
- **SEC-SKILL-03**: When a skill and an agent both declare `allowed_tools`,
  the effective tool list is their intersection.
- **SEC-SKILL-05**: Workspace skills are **not loaded by default**. Pass
  `--trust-project-skills` to enable loading skills from the project's
  `.clawft/skills/` directory. User-level and built-in skills always load.
- **SEC-SKILL-06**: Prompt injection tokens (`<system>`, `<|im_start|>`,
  `<<SYS>>`, etc.) are stripped from skill instructions automatically.
- **SEC-SKILL-07**: SKILL.md files are limited to 50 KB. Oversized files are
  rejected.

### Autonomous skill creation

Agents can detect repeated prompt patterns and auto-generate skills. When the
same pattern is observed three times (configurable via
`skill_auto_threshold`), the agent:

1. Generates a `SKILL.md` with inferred variables and instructions.
2. Installs it in the user skills directory with a `pending` status.
3. Prompts the user for approval before the skill becomes active.

This feature is **disabled by default**. Enable it in config:

```json
{
  "skills": {
    "auto_create": true,
    "auto_create_threshold": 3
  }
}
```

---

## Agents

An **agent** is a predefined persona that bundles a system prompt, model
selection, tool constraints, and skill activations into a named definition.

### Agent definition format

Agent definitions are YAML or JSON files named `agent.yaml` (or `agent.yml`,
`agent.json`) inside a named directory:

```
agents/
  researcher/
    agent.yaml
  code-reviewer/
    agent.yaml
```

Standalone files are also supported: `agents/researcher.yaml`.

#### YAML example

```yaml
name: researcher
description: Deep research agent with web access
model: anthropic/claude-sonnet-4-20250514
system_prompt: |
  You are a meticulous research assistant. Always cite sources
  and verify claims across multiple references.
skills:
  - research
allowed_tools:
  - WebSearch
  - Read
  - Grep
max_turns: 20
variables:
  output_format: markdown
  citation_style: APA
```

#### JSON example

```json
{
  "name": "code-reviewer",
  "description": "Code review agent",
  "model": "anthropic/claude-sonnet-4-20250514",
  "system_prompt": "You are a senior code reviewer. Focus on correctness, security, and maintainability.",
  "skills": ["coding"],
  "allowed_tools": ["Read", "Grep"],
  "max_turns": 10,
  "variables": {
    "lang": "rust"
  }
}
```

#### Fields

| Field | Required | Description |
|-------|----------|-------------|
| `name` | yes | Unique agent identifier |
| `description` | yes | Human-readable summary |
| `model` | no | LLM model override (e.g. `anthropic/claude-sonnet-4-20250514`) |
| `system_prompt` | no | System prompt prepended to every LLM call |
| `skills` | no | Skills to activate when this agent runs |
| `allowed_tools` | no | Tool allowlist (empty = all tools) |
| `max_turns` | no | Maximum tool-loop turns |
| `variables` | no | Named variables for template expansion |

### Discovery chain

Agents follow the same 3-level priority as skills:

1. **Workspace** -- `.clawft/agents/` in the project root
2. **User** -- `~/.clawft/agents/`
3. **Built-in** -- compiled into the binary

Higher-priority definitions overwrite lower-priority ones with the same name.

### Template variables

Agent system prompts and skill instructions support template substitution:

| Syntax | Replaced with |
|--------|---------------|
| `$ARGUMENTS` | The full argument string passed to the agent |
| `${1}`, `${2}` | Positional arguments (1-based, space-separated) |
| `${NAME}` | Named variable from the agent's `variables` map |

Missing variables are replaced with an empty string (no error is raised).

Example system prompt using templates:

```
You are a ${lang} developer. Analyze the following: $ARGUMENTS
Focus on ${1} first, then ${2}.
```

With `variables: { lang: rust }` and arguments `"performance safety"`, this
renders as:

```
You are a rust developer. Analyze the following: performance safety
Focus on performance first, then safety.
```

### CLI commands

```bash
# List all discovered agents
weft agents list

# Show agent details (model, skills, system prompt preview)
weft agents show researcher

# Select an agent (prints usage instructions)
weft agents use researcher
```

`weft agents list` prints a table with columns: NAME, SOURCE (workspace /
user / builtin), MODEL, and DESCRIPTION.

### Interactive slash command

In a `weft agent` interactive session:

```
/agent researcher    -- switch to the "researcher" agent
/agent               -- show the current agent
/status              -- show agent, model, skill, tool counts
```

### Security

Agent files share the same security controls as skills:

- Directory names are validated against path traversal (SEC-SKILL-02).
- Model strings are validated against shell metacharacters (SEC-SKILL-04).
  Values like `; rm -rf /` or strings with backticks are rejected.
- Agent files are limited to 10 KB (SEC-SKILL-07).
- When an agent and a skill both declare `allowed_tools`, only the
  intersection of the two lists is permitted (SEC-SKILL-03).

### Per-agent workspaces

Each agent can have an isolated workspace at `~/.clawft/agents/<id>/`
containing:

| Path | Purpose |
|------|---------|
| `SOUL.md` | Agent personality and persistent memory |
| `sessions/` | Session store scoped to this agent |
| `skills/` | Skill overrides (take precedence over user/builtin) |
| `config.toml` | Agent-specific configuration |

Configuration merges in three levels: **global** -> **agent** -> **workspace**.
Values at a more specific level override broader ones.

Shared namespaces between agents are configurable. By default, cross-agent
access is read-only. Explicit opt-in is required for write access. Symlink-
based cross-agent references allow one agent to reference another's resources
without duplication.

### Multi-agent routing

When multiple agents are defined, inbound messages are dispatched via a
routing table with first-match-wins semantics.

Each route entry can match on:

- **keywords** -- Words or phrases in the message content.
- **channels** -- Specific channel names (e.g., `"slack"`, `"telegram"`).
- **sender patterns** -- Regex or glob patterns on sender IDs.

A catch-all route handles unmatched messages. If no catch-all is defined and
no route matches, the message is rejected with a warning log (not silently
dropped).

### Inter-agent communication

Agents can exchange messages through the `AgentBus`:

- **InterAgentMessage** -- Typed message with `from_agent`, `to_agent`,
  `task`, and `payload` fields.
- **Per-agent inboxes** -- Each agent has a bounded channel (configurable
  capacity) for incoming inter-agent messages.
- **SwarmCoordinator** -- Provides `dispatch_subtask` (send to one agent)
  and `broadcast_task` (send to all agents) methods.
- **TTL enforcement** -- Messages carry a time-to-live; expired messages are
  dropped on delivery attempt.

---

## Examples

### Creating a custom skill

Create a workspace-level skill for commit message generation:

```bash
mkdir -p .clawft/skills/commit-msg
```

Write `.clawft/skills/commit-msg/SKILL.md`:

```markdown
---
name: commit-msg
description: Generate a conventional commit message from a diff
version: 1.0.0
variables:
  - diff
allowed-tools:
  - Bash
user-invocable: true
argument-hint: Optional scope (e.g. "auth", "api")
---

Analyze the following diff and generate a conventional commit message.
Use the format: type(scope): description

If a scope argument is provided, use it: {{diff}}

Rules:
- Keep the subject line under 72 characters
- Use imperative mood ("add", not "added")
- Include a body only if the change is non-trivial
```

Verify it loads:

```bash
weft skills list
weft skills show commit-msg
```

### Creating a custom agent

Create a user-level security auditor agent:

```bash
mkdir -p ~/.clawft/agents/security-auditor
```

Write `~/.clawft/agents/security-auditor/agent.yaml`:

```yaml
name: security-auditor
description: Security-focused code auditor
model: anthropic/claude-sonnet-4-20250514
system_prompt: |
  You are a security auditor reviewing code for vulnerabilities.
  Focus on: injection attacks, authentication flaws, data exposure,
  and dependency risks. Rate each finding as LOW, MEDIUM, HIGH, or
  CRITICAL. Always suggest a remediation.
skills:
  - research
allowed_tools:
  - Read
  - Grep
  - Bash
max_turns: 15
variables:
  severity_threshold: MEDIUM
```

Verify it loads:

```bash
weft agents list
weft agents show security-auditor
```

### Using skills and agents in interactive mode

```
$ weft agent

> /skills
Available skills (3): commit-msg, research, coding

> /use research
Activated skill: research

> Research the latest changes in the Rust async ecosystem
[agent responds using the research skill's instructions and allowed tools]

> /use
Skill deactivated.

> /agent security-auditor
Switched to agent: security-auditor

> Review src/auth.rs for authentication vulnerabilities
[agent responds using the security-auditor persona and constraints]
```
