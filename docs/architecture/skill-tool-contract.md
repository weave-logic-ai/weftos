# Skill ↔ MCP tool contract

This document specifies what `SkillToolProvider::call_tool` returns
and why. It is the wire-level companion to the rustdoc on
`crates/clawft-services/src/mcp/provider.rs`. Tracks WEFT-490.

## Summary

When an MCP client invokes `tools/call` against a tool provided by
the `SkillToolProvider`, the result content is the **SKILL.md prompt
body** — not the output of running the skill. The remote LLM is
expected to treat the returned text as a refresher prompt and
continue reasoning with that prompt now in its context.

This is intentional. Skills are not executables; they are LLM prompts
with a small metadata header. Returning the prompt body lets a remote
MCP client ask the WeftOS daemon "what does the `code-review` skill
say?" and get back the canonical text without needing local access to
the skill registry.

## What lives where

| Layer | Path | Responsibility |
|-------|------|----------------|
| SKILL.md (on disk) | `~/.clawft/skills/<name>/SKILL.md` or `.clawft/skills/<name>/SKILL.md` | Human-authored skill. YAML frontmatter + Markdown body. |
| `SkillDefinition` (in memory) | `clawft_types::skill::SkillDefinition` | Parsed metadata (name, description, variables, instructions). `instructions` is the prompt body. |
| `ToolDefinition` (over MCP) | `clawft_services::mcp::ToolDefinition` | The MCP-shaped surface: name, description, inputSchema. Generated from `SkillDefinition` by `skill_to_tool_definition`. |
| `SkillToolProvider` (provider) | `clawft_services::mcp::provider::SkillToolProvider` | Implements the `ToolProvider` trait. `list_tools()` returns the `ToolDefinition` list; `call_tool(name, args)` returns the prompt body wrapped as `CallToolResult::text(...)`. |

## SKILL.md → ToolDefinition mapping

```text
SKILL.md                                    ToolDefinition
─────────────────────────────────────────   ──────────────────────────
---                                         ToolDefinition {
name: code-review                              name: "code-review",
description: Review changed code for...        description: "Review changed code...",
variables:                                     input_schema: {
  - target_branch                                "type": "object",
  - diff_path                                    "properties": {
---                                                "target_branch": {"type": "string", ...},
                                                   "diff_path":     {"type": "string", ...}
# Code Review                                    },
                                                 "required": ["target_branch", "diff_path"]
You are reviewing a pull request...            }
                                            }
1. Read the diff at {{diff_path}}.          (instructions: "# Code Review\n\nYou are
2. ...                                       reviewing a pull request...\n\n1. Read the
                                             diff at {{diff_path}}.\n2. ...")
```

The `inputSchema` is auto-generated from the `variables` list. If the
skill has no variables, the schema is permissive (`{type: "object",
properties: {args: {type: "string"}}}`).

## Wire-level example

```jsonc
// MCP request from a remote client
{
  "jsonrpc": "2.0",
  "id": 42,
  "method": "tools/call",
  "params": {
    "name": "code-review",
    "arguments": {
      "target_branch": "main",
      "diff_path": "/tmp/pr-123.diff"
    }
  }
}

// MCP response from `weft mcp-server`
{
  "jsonrpc": "2.0",
  "id": 42,
  "result": {
    "content": [
      {
        "type": "text",
        "text": "# Code Review\n\nYou are reviewing a pull request...\n\n1. Read the diff at /tmp/pr-123.diff.\n2. ..."
      }
    ],
    "isError": false
  }
}
```

The remote LLM receives the prompt body as a text content block and
proceeds to act on it. It does not see the WeftOS tool registry —
follow-up tool calls happen on the *remote* side (or against tools
the remote client has separately configured). This is the same
posture as Claude Code's "skill" surface: the skill text is delivered
into the conversation, and execution unfolds in the agent loop, not
inside `call_tool`.

## Why not run the skill?

The alternative would be: invoke the skill against the daemon's own
LLM, capture the resulting messages and tool calls, and stream them
back over MCP. This was rejected for three reasons:

1. **Composition**: the caller already has its own LLM. Returning
   the prompt body lets the caller use its own reasoning loop, its
   own model preference, and its own tool ecosystem. Running the
   skill server-side would force the caller's request through the
   daemon's model, which is rarely what the caller wants.

2. **Cost transparency**: the caller pays for tokens it consumes.
   Hiding an LLM round-trip inside `tools/call` would charge the
   wrong account and obscure latency.

3. **Determinism**: returning the prompt body is a pure read. The
   caller can cache, version, diff, or audit the skill text without
   any side effects. Running the skill would be a side-effecting
   operation that's much harder to reason about in CI.

## Variable substitution

The dispatcher closure (provided to `SkillToolProvider::new`) is
responsible for interpolating `{{var}}` placeholders in the prompt
body using the `arguments` JSON object. The default dispatcher in
`crates/clawft-cli/src/commands/mcp_server.rs` looks up the skill by
name and returns its `instructions` field verbatim — variable
substitution is left to the caller (the LLM is generally fine
substituting placeholders in its own context). A future revision may
do server-side substitution if the variable set is mandatory.

## Failure modes

| Condition | Result |
|-----------|--------|
| Tool name not in the registered list | `Err(ToolError::NotFound(name))` — the MCP client receives a JSON-RPC error. |
| Dispatcher returns `Err(msg)` | `Ok(CallToolResult::error(msg))` — in-band, `isError: true`, the LLM can reason about the failure. |
| Tool list lock poisoned | Panic. The provider holds an `Arc<RwLock>` shared with the hot-reload watcher; a panic here means the watcher already crashed and the daemon should restart. |
| Skill registry empty (no skills found at startup) | The provider is not registered at all (`mcp_server.rs` skips it). `tools/list` simply does not include any skill tools. |

## See also

- `crates/clawft-services/src/mcp/provider.rs` — `SkillToolProvider`
  source and rustdoc.
- `crates/clawft-cli/src/commands/mcp_server.rs` — the
  `SkillToolProvider` instantiation that ships with `weft mcp-server`.
- `docs/guides/mcp-integration.md` §4 (Skill-Based Tool Discovery)
  for the broader skill-as-discovery model.
