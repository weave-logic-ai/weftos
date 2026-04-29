# ADR-021: CLI Commands Must Route Through Kernel Daemon

**Date**: 2026-04-03
**Status**: Accepted
**Deciders**: Architecture review, Sprint 14
**Depends-On**: ADR-048 (Kernel Phase Responsibilities) — formerly numbered ADR-020; renumbered 2026-04-28 (WEFT-140) to deduplicate.

## Context

An audit of 113 CLI commands across `weft` (clawft-cli) and `weaver` (clawft-weave) found that 32 commands in `weft` bypass the kernel daemon entirely. They perform direct filesystem I/O, network calls, and state mutations without going through governance gates, capability checks, or ExoChain audit logging.

This creates five classes of security risk:

1. **Governance bypass** — Commands read/write arbitrary files with no capability check.
2. **No audit trail** — State changes (cron jobs, tool allowlists, skill installations) are invisible to ExoChain.
3. **Race conditions** — Both `weft cron add` (direct file write) and `weaver cron add` (daemon RPC) exist, creating data corruption on concurrent access.
4. **Capability escalation** — `weft tools allow exec_*` modifies the denylist with no governance gate.
5. **Uncontrolled network access** — `weft skills publish` makes direct HTTP calls outside the kernel's control.

Meanwhile, `weaver` (clawft-weave) has 51/52 commands correctly routing through the daemon via `DaemonClient` RPC.

## Decision

**All CLI commands that perform operations MUST route through the kernel daemon via RPC.** The CLI binary is a thin client: parse args, send RPC, display response.

### Classification

| Category | Rule | Examples |
|----------|------|----------|
| **Must route through daemon** | Any command that reads project files, writes state, spawns agents, makes network calls, or modifies configuration | assess, security scan, cron add, skills install, tools deny |
| **Exempt: pure display** | Commands that only parse local config and print it, with no side effects | help, completions, config show, status (read-only) |
| **Exempt: bootstrap** | Commands that must work before any daemon exists | onboard (initial `.clawft/` creation), assess init (initial `.weftos/` creation) |
| **Exempt: self-hosting** | Commands that ARE a long-running service, not a client | agent (runs AppContext), gateway (runs channels), mcp-server (runs MCP) |

### Bootstrap Exception

`weft onboard` and `weft assess init` are the only commands allowed to perform direct filesystem operations, because they create the directory structures that the daemon later manages. They MUST:

- Perform minimal operations (create dirs, write config template)
- Print a clear "now run `weaver kernel start`" message
- Not perform any analysis, scanning, or network operations

### Implementation

1. Extract `DaemonClient` from clawft-weave into a shared crate (`clawft-rpc` or added to `clawft-types`)
2. For each bypassing command, add a daemon RPC endpoint and convert the CLI to a thin client
3. If no daemon is running, print a clear error: `"No kernel running. Start with: weaver kernel start"`
4. Migration priority: **assess → security → cron → skills → tools → agents → workspace**

### Offending Commands (32 total)

**Cron** (6): list, add, remove, enable, disable, run
**Assess** (4): run, init†, link, compare
**Security** (1): scan
**Skills** (7): list, show, install, remove, search, publish, remote-install
**Tools** (6): list, show, mcp, search, deny, allow
**Agents** (3): list, show, use
**Workspace** (8): create, list, load, status, delete, config set/get/reset
**Other** (4): onboard†, ui, voice setup/test/talk

† = bootstrap exception

## Consequences

### Positive
- Every operation has a governance gate and ExoChain audit entry
- Capability checks prevent unauthorized file access or network calls
- Single daemon process eliminates race conditions on shared state
- Security posture matches the "agents are OS processes" model — CLI users are just another agent with capabilities

### Negative
- Requires a running daemon for most operations (additional setup step)
- 32 commands need migration work (estimated: 2–3 sprints)
- Slightly higher latency for simple operations (RPC round-trip vs. direct file read)

### Neutral
- `weaver` already demonstrates the correct pattern — this is extending it to `weft`
- The daemon already handles the equivalent operations via its RPC interface
