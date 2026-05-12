# WeftOS — Claude Code plugin

This directory makes the WeftOS repo installable as a [Claude Code plugin](https://docs.claude.com/claude-code/plugins). Once installed, Claude Code can drive the WeftOS kernel, knowledge graph, ECC substrate, and mesh layer through the `weft mcp-server` MCP server plus a set of slash commands.

## Prerequisites

- `weft` and `weaver` binaries on `PATH` (`cargo install --path crates/clawft-weave` from the repo root, or grab a release tarball)
- Claude Code with plugin support enabled

## Install (local, from a git checkout)

```bash
# point Claude Code at this repo as a marketplace
/plugin marketplace add /absolute/path/to/weftos

# then install the plugin from that marketplace
/plugin install weftos@weftos
```

Restart Claude Code afterward so the MCP server is picked up.

## Install (once published to a marketplace)

```bash
/plugin install weftos@<marketplace-name>
```

## What ships

| Surface              | Where                          | What                                                                                               |
| -------------------- | ------------------------------ | -------------------------------------------------------------------------------------------------- |
| MCP server           | `.mcp.json` → `weftos`         | `weft mcp-server` — full weft CLI surface as MCP tools (agent, memory, channels, skills, ...)      |
| Slash commands       | `commands/`                    | `/weftos-status`, `/weftos-kernel`, `/weftos-graphify`, `/weftos-ecc`, `/weftos-mesh`, `/weftos-install` |
| Agents               | `agents/`                      | `clawft`, `weftos-kernel`, `weftos-mesh`, `weftos-ecc`, top-level `weftos`                          |
| Skills               | `skills/`                      | `agent-dispatch`, `claude-flow`, `discord`, `linkedin`, `prompt-log`, `ruv-researcher`, `skill-vetting`, `social-auth`, `twitter` |

## Architecture

```
┌─────────────────────────┐
│   Claude Code (host)    │
│                         │
│  ┌───────────────────┐  │
│  │ plugin: weftos    │  │
│  │  - slash commands │  │
│  │  - agents/skills  │  │
│  └────────┬──────────┘  │
│           │ stdio       │
│  ┌────────▼──────────┐  │
│  │  weft mcp-server  │  │ <-- weft binary, MCP over stdio
│  └────────┬──────────┘  │
└───────────┼─────────────┘
            │ JSON-RPC over UDS
            │  (.weftos/runtime/kernel.sock)
            │
   ┌────────▼──────────┐
   │   weaver kernel   │ <-- long-running daemon
   │  (mesh, chain,    │
   │   graphify, ecc)  │
   └───────────────────┘
```

Two processes: the plugin host (`weft mcp-server`, short-lived per Claude Code session) and the kernel daemon (`weaver kernel boot`, long-running). The plugin host talks to the daemon over the Unix Domain Socket at `.weftos/runtime/kernel.sock` (resolution rules mirror `clawft_rpc::protocol::socket_path()`).

## Notes on the existing `.mcp.json`

This repo's `.mcp.json` already declared a `claude-flow` MCP server (with `autoStart: false`), presumably from a prior `/ruflo-setup` run. This plugin adds a `weftos` entry alongside it rather than replacing — if `claude-flow` here is unwanted cruft, delete that entry separately.
