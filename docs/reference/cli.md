# CLI Reference

The `weft` command-line interface provides tools for managing agents, gateways,
channels, scheduled jobs, sessions, memory, and configuration.

## Global Flags

These flags are available on all subcommands.

| Flag | Description |
|------|-------------|
| `--verbose`, `-v` | Enable debug-level logging (default level is `warn`) |
| `--project <ULID>` | Talk to the kernel serving this project (see `weft project list`) |
| `--runtime <DIR>` | Talk to the kernel whose runtime directory is `DIR` (overrides `WEFTOS_RUNTIME_DIR`; `weft` also exports it as `WEFTOS_RUNTIME_DIR` for the rest of the process) |
| `--version` | Show version and exit |
| `--help`, `-h` | Show help text and exit |

Every command that talks to the kernel resolves its endpoint the same way and
verifies the daemon's handshake. The runtime root is the first match of:

1. `--runtime <DIR>`
2. `WEFTOS_RUNTIME_DIR`
3. the project manifest (`project.toml` above the working directory, never
   `$HOME`, or `--project` / `WEFTOS_PROJECT`, then
   `~/.weftos/projects/<ULID>.toml`): its `[serve] runtime_dir`, or, when it
   has none and `[serve] via = "user-daemon"`, the user daemon's root
   `~/.weftos/run`
4. the user default: with no project known, `~/.weftos/run` when its
   `kernel.sock` or `kernel.lock` exists
5. the Phase 0 default: the project's `.weftos/runtime`, otherwise `~/.clawft`

The unreachable-daemon error lists every level tried, in order. When the daemon cannot be reached or is the wrong one, the error names
the socket tried and the exact next command. A daemon that answers but is the
wrong one (another project, another node, an incompatible protocol) is always
an error: commands never fall back to local state changes against it. The user
daemon is not bound to a project, so reaching it for a project is not an error.

The default log level is `warn`. Only warnings and errors are printed unless
`--verbose` is passed (which sets it to `debug`). The `RUST_LOG` environment
variable overrides both defaults if set (e.g. `RUST_LOG=info weft agent`).

---

## weft agent

Start an interactive agent REPL, or send a single message and exit.

### Usage

```
weft agent [FLAGS] [OPTIONS]
```

### Options

| Flag / Option | Description |
|---------------|-------------|
| `--message`, `-m` `<MSG>` | Send a single message to the agent and exit. When omitted, the agent starts in interactive REPL mode. |
| `--model` `<MODEL>` | Override the model specified in config (e.g., `openai/gpt-4o`, `anthropic/claude-sonnet-4-20250514`). |
| `--config`, `-c` `<PATH>` | Path to a config file. Overrides the default config resolution. |
| `--intelligent-routing` | Enable vector-memory routing for context-aware message handling. Requires the `intelligent-routing` feature to be compiled in. |

### Examples

Start an interactive REPL session:

```
weft agent
```

Send a single message and print the response:

```
weft agent -m "Summarize today's open issues"
```

Use a specific model:

```
weft agent --model openai/gpt-4o -m "Draft a status update"
```

Use a custom config file with intelligent routing:

```
weft agent -c ./my-config.toml --intelligent-routing
```

---

## weft gateway

Start the gateway process. This launches all enabled channels, the agent loop,
and outbound message dispatch. The process runs in the foreground until
interrupted.

### Usage

```
weft gateway [FLAGS] [OPTIONS]
```

### Options

| Flag / Option | Description |
|---------------|-------------|
| `--config`, `-c` `<PATH>` | Path to a config file. Overrides the default config resolution. |
| `--intelligent-routing` | Enable vector-memory routing for context-aware message handling. |

### Shutdown

Send `SIGINT` (Ctrl+C) for graceful shutdown. The gateway will drain in-flight
messages before exiting.

### Examples

Start the gateway with default config:

```
weft gateway
```

Start with a custom config and intelligent routing:

```
weft gateway -c /etc/weft/production.toml --intelligent-routing
```

---

## weft status

Display a summary of the current system status, including the gateway, active
channels, and agent health.

### Usage

```
weft status [FLAGS] [OPTIONS]
```

### Options

| Flag / Option | Description |
|---------------|-------------|
| `--detailed` | Show expanded status information for each component. |
| `--config`, `-c` `<PATH>` | Path to a config file. Overrides the default config resolution. |

### Examples

Quick status check:

```
weft status
```

Detailed status with all component info:

```
weft status --detailed
```

---

## weft doctor / weaver doctor

Grouped health checks for the install, the running kernel daemon, the runtime
directory, MCP wiring, config and multi-agent readiness. `weaver doctor` runs
the same engine (`clawft_rpc::doctor`) minus `config` and `agents`, which need
the agent config and stay in `weft doctor`.

Each check reports PASS, WARN or FAIL, and every non-pass line carries a
one-line `fix:` command. Doctor is read-only unless `--fix` is given.

### Usage

```
weft doctor [COMPONENT...] [OPTIONS]
weaver doctor [COMPONENT...] [OPTIONS]
```

### Options

| Flag / Option | Description |
|---------------|-------------|
| `COMPONENT`, `--component <LIST>` | Limit to `install`, `daemon`, `runtime`, `config`, `mcp`, `agents` (comma-separated or repeated). `weft doctor install` and `weft doctor --component install` are the same. |
| `--json` | Machine-readable output: `summary`, `exit_code`, `findings[]`, and an inventory under `data`. |
| `--strict` | Exit non-zero on WARN as well as FAIL. |
| `--fix` | Remove provably stale `kernel.sock` / `kernel.pid` files in the ACTIVE runtime dir and print exactly what changed. Nothing else is modified. |
| `--all-runtimes` | With `--fix`, also repair every runtime dir doctor can see (`~/.clawft`, `~/.weftos/runtime`, ancestor `.weftos/runtime`). |
| `--multi-agent` | Same as `--component agents` (kept for WEFT-197 users). |
| `--config`, `-c` `<PATH>` | (weft only) config file path. |

Exit code is 1 on any FAIL, or on any WARN with `--strict`; otherwise 0.

### Checks

| Component | What it looks at |
|-----------|------------------|
| `install` | Every copy of `weft`, `weaver`, `weftos` on `PATH` and in `~/.cargo/bin`, `/usr/local/bin`, `/opt/homebrew/bin`, `~/.local/bin`: path, version, sha256, dirty flag, owning channel (Homebrew Cellar, cargo-dist receipt in `~/.config/*/*receipt*.json`, `~/.config/weftos/dev-install.json`, a `-dirty` build, cargo ledger `~/.cargo/.crates2.json`), which copy wins on `PATH`. WARN when the winner is dirty or a shadowed copy is newer, and on duplicates. FAIL when `weft` or `weaver` is missing or does not run. |
| `daemon` | Kernel processes from `ps` (`weaver kernel start`, `weftos boot`): pid, executable (`/proc/<pid>/exe` or `lsof`), version (from `kernel.status` when the socket is tied to the pid, else `<exe> --version`). WARN when the executable is outside any known install location, differs from the `PATH` winner, or the CLI and daemon versions differ. Never starts or signals a process. |
| `runtime` | The resolved runtime dir and how it was resolved; `kernel.sock` connect test (refused means stale), `kernel.pid` liveness, every `node.key` under `~/.clawft`, `~/.weftos/runtime` and project `.weftos/runtime` dirs (paths and permissions only; contents are never read, keys are never deleted). |
| `runtime` (project kernels) | Per-project child kernels under the user daemon (ADR-103), read-only. The run root is `~/.weftos/run`, or `$WEFTOS_RUNTIME_DIR` when set (an isolated run never dials the real home's daemon). Finding ids: `child:<id>:failed` (the restart budget is spent, or the project is revoked; `weaver kernel restart --project <id>` after fixing the cause); `child:<id>:stale-build` (the kernel runs another build than the user daemon, typically after `weaver update`, which keeps children running; `weaver kernel restart --project <id>`); `child:<id>:unsupervised` (a project kernel is in the process table but the supervisor does not list it as `running`, `starting` or `idle-stopping`); `child:<id>:orphan` (a project kernel runs and no user daemon does; `weaver kernel start --profile user` adopts it); `child:<id>:unverified` (`kernel.pid` names a live process that is not a project kernel; never adopted, never signalled); `children` (OK, N children confirmed by the supervisor); `children:unconfirmed` (the user daemon did not answer `project.status`); `legacy:<id>` (a project-rooted daemon runs beside the user daemon; `weaver project migrate-kernel <id> --dry-run`); `legacy:<id>:child-kernel` (a project marked `child-kernel` still has a project-rooted daemon). |
| `mcp` | Each stdio server in the nearest `.mcp.json` resolves on `PATH`. |
| `runtime` (weaver, mesh) | `mesh.*`: the machine mesh service answers on its socket, its `mesh-local` protocol window overlaps this weaver, the machine key matches `~/.weftos/mesh/machine.pub`, the journal verifies, the box key is 0600 and owned by the service account, `~/.weftos/run/node.key` left behind while the service runs (WARN), two listeners on 9489 (FAIL), the user daemon's `mesh.mode` against the service (`mesh.mode`; a project daemon reached from a project directory is only reported), force-revoked users. `user_key` / `user_key_split`: `user.key` and `chain.key` during the migration, WARN when their public keys differ. |
| `config`, `agents` | (weft) config loads; `claude` on `PATH`, auto-delegation rules, agent routes. |

`--fix` is deliberately narrow: a socket is removed only when a connect is
refused and its recorded pid is not running; a pid file only when that pid is
not running. Both conditions are re-checked immediately before each unlink.
If `ps` fails (sandbox), liveness is unknown and `--fix` removes nothing. By
default only the active runtime dir is repaired (the one `weaver kernel`
would use from this directory); `--all-runtimes` widens that. Setting
`WEFTOS_RUNTIME_DIR` makes it the only directory doctor looks at, which is
how to sandbox a doctor run. Live sockets, keys and binaries are never
touched.

Duplicate copies are ranked probed before unprobed, clean before dirty, then
by semver precedence (a prerelease is older than its release; `-dirty`,
git-describe and build suffixes do not count as newer), then by channel
(cargo-dist or Homebrew, then `cargo install`, then `build.sh`, then
unknown). A dirty build is never the copy to keep over a clean one. The
`fix:` line names `rm` only for a copy that is byte-identical to the kept
copy, or that was probed, parses, is strictly older, is not a
prerelease-versus-release mix, involves no dirty build, and is not better
managed than the kept copy. Every other case is advice only, naming both
copies, which one wins on `PATH`, and the fix (reorder `PATH`, or the
channel's update command, or `scripts/build.sh install` when a dev build
shadows a managed copy). A copy that was not probed is never offered for
removal.

Doctor runs `--version` only on files that are native executables (Mach-O,
ELF, PE) named weft, weaver or weftos; a script with that name is reported
as not probed rather than executed. `--json` daemon entries carry only the
binary and subcommand, never the full command line. Copies of a
binary are never removed by doctor; the `fix:` line for a duplicate is a
command for you to run.

### Examples

```
weft doctor                       # everything
weaver doctor install             # just the binary inventory
weaver doctor --component runtime --fix
weft doctor --json --strict       # for CI
```

---

## weaver mesh

The machine mesh service (ADR-103 Phase 3) and its admin verbs. `serve` runs
the service; every other verb is a client of its socket that first verifies
the server (its uid, and the machine key against `service.json` and the pin).
Admin verbs are authorised by the caller's uid (root or `admin_uids` in
`mesh.toml`) and hold no keys. The verbs never write a pin except `trust`; they
compare against `~/.weftos/mesh/machine.pub` when it exists (the user daemon
writes it on first contact).

### Usage

```
weaver mesh serve [--config PATH] [--state-dir DIR] [--socket PATH] [--listen ADDR] [--health-listen ADDR|off]
weaver mesh status [--json]
weaver mesh bindings
weaver mesh bind approve <UID> [--user-id ID]
weaver mesh bind revoke <UID> [--reason TEXT]
weaver mesh bind rebind <UID> [--pubkey HEX]
weaver mesh peer revoke <NODE_ID> [--reason TEXT]
weaver mesh peer unrevoke <NODE_ID>
weaver mesh journal verify [--accept-truncate [--seq N] [--floor N]]
weaver mesh trust [--replace]
weaver mesh install-service [--kind launchd|systemd] [--adopt-node-key PATH | --fresh-node-key] [--listen ADDR] [--admin-uid UID]
weaver mesh uninstall-service [--kind launchd|systemd] [--purge-key]
```

Client verbs also take `--socket PATH` (default `$WEFTOS_MESH_SOCKET`, else
`/var/run/weftos/mesh.sock`), `--pin PATH` (default
`~/.weftos/mesh/machine.pub`) and `--json`.

| Verb | What it does |
|------|--------------|
| `serve` | Runs the service in the foreground. Refuses root. Defaults: state `/var/lib/weftos/mesh`, socket `/var/run/weftos/mesh.sock`, listener `127.0.0.1:9489`, health `127.0.0.1:9490`. Tests point `--state-dir`, `--socket` and `--listen 127.0.0.1:0` at tempdirs. |
| `status` | Node id, machine key, protocol window, admission and bind policy, registrations, peers, journal head. |
| `bindings` | Bound, pending and revoked users (admin). |
| `bind approve` | Approves a pending bind; `--user-id` names the key you looked at, and the service refuses if the pending key is another. |
| `bind revoke` | Revokes the uid's binding and every certificate issued to its key. |
| `bind rebind` | Replaces the uid's key (default: the key it last offered in conflict) and revokes the old one. The old daemon must be restarted with the new key. |
| `peer revoke` / `unrevoke` | Revokes a mesh peer by node id (or lifts that) and closes its live connection. Under `admission = enforce` it is refused when it reconnects; under `observe` (the default) the revocation is recorded and the peer can reconnect. |
| `journal verify` | Re-verifies the machine journal's hash chain and signatures; `--accept-truncate` acknowledges a quarantined tail (the newest pending one, or `--seq N` to name it) and lifts read-only mode. `--floor N` raises the serial floor (at most 2^32 above the last serial). After a repeat quarantine, `--json` lists every `pending_quarantines`; one acceptance clears them all, so first review each older `journal.corrupt.*` file and pass `--floor` when a serial in it exceeds the high-water the journal carried. A crash-torn final line with nothing readable in it is accepted by the service itself at start (journalled as `auto: torn_tail`) and needs no action. |
| `trust` | Shows the machine key fingerprint and pins it; `--replace` overwrites a different pin after you verified the new key out of band. |
| `install-service` / `uninstall-service` | Print a reviewed install or uninstall script. They run nothing and refuse `--apply`. When `~/.weftos/run/node.key` exists, install needs `--adopt-node-key PATH` (keep the node id) or `--fresh-node-key` (new id, warned in the script). The listener defaults to `127.0.0.1:9489`; `--listen 0.0.0.0:9489` for LAN peers. Admin verbs need root or a uid from `--admin-uid`. See `docs/guides/weftos-deployment-sops.md`. |

The daemon side is `kernel.mesh.service = "auto" | "required" | "off"` in
`~/.weftos/weave.toml`; `weaver kernel status` shows the resulting
`mesh.mode`.

---

## weaver migrate

One-way copies into `~/.weftos` that never modify the source.

```
weaver migrate user-chain [--dry-run] [--from DIR] [--to DIR] [--allow-unsigned]
weaver migrate user-key [--dry-run]
```

| Verb | What it does |
|------|--------------|
| `user-chain` | Copies the legacy chain from `~/.clawft` to `~/.weftos/chain`, verifies hashes, head and signature, and writes `MIGRATED-TO-WEFTOS.txt` beside the original (Phase 1). |
| `user-key` | Copies the seed of `~/.weftos/chain/chain.key` to `~/.weftos/user.key` (0600), proves the two public keys are equal and writes `user.key.MIGRATED_FROM.json`. The user id does not change and `chain.key` is kept. Idempotent (Phase 3). |

---

## weft agents

Manage agent definitions. Agents are discovered from workspace
(`.clawft/agents/`), user (`~/.clawft/agents/`), and builtin sources.

### Subcommands

### weft agents list

List all agents with source annotation.

```
weft agents list
```

Displays a table with columns: NAME, SOURCE, MODEL, DESCRIPTION.

### weft agents show

Show details of a specific agent, including model, skills, system prompt
preview, allowed tools, and variables.

```
weft agents show <NAME>
```

| Argument | Description |
|----------|-------------|
| `<NAME>` | Agent name to inspect. Required. |

### weft agents use

Set the active agent for the next session. Validates the agent exists and
prints instructions for using it.

```
weft agents use <NAME>
```

| Argument | Description |
|----------|-------------|
| `<NAME>` | Agent name to activate. Required. |

### Examples

List all agents:

```
weft agents list
```

Show details for a specific agent:

```
weft agents show researcher
```

Select an agent for use:

```
weft agents use coder
```

---

## weft channels status

Display the status of all configured channels.

### Usage

```
weft channels status [OPTIONS]
```

### Options

| Flag / Option | Description |
|---------------|-------------|
| `--config`, `-c` `<PATH>` | Path to a config file. Overrides the default config resolution. |

### Examples

```
weft channels status
```

```
weft channels status -c ./dev-config.toml
```

---

## weft cron

Manage scheduled cron jobs. Each job runs a prompt on a cron schedule through
the agent.

### Subcommands

### weft cron list

List all configured cron jobs.

```
weft cron list [OPTIONS]
```

| Flag / Option | Description |
|---------------|-------------|
| `--config`, `-c` `<PATH>` | Path to a config file. |

### weft cron add

Add a new cron job.

```
weft cron add --name <NAME> --schedule <CRON_EXPR> --prompt <PROMPT> [OPTIONS]
```

| Flag / Option | Description |
|---------------|-------------|
| `--name` `<NAME>` | Human-readable name for the job. Required. |
| `--schedule` `<CRON_EXPR>` | Cron expression defining the schedule (e.g., `"0 9 * * *"`). Required. |
| `--prompt` `<PROMPT>` | The prompt text to send to the agent on each trigger. Required. |
| `--config`, `-c` `<PATH>` | Path to a config file. |

### weft cron remove

Remove a cron job by ID.

```
weft cron remove <JOB_ID> [OPTIONS]
```

| Argument / Option | Description |
|-------------------|-------------|
| `<JOB_ID>` | The ID of the job to remove. Required. |
| `--config`, `-c` `<PATH>` | Path to a config file. |

### weft cron enable

Enable a previously disabled cron job.

```
weft cron enable <JOB_ID> [OPTIONS]
```

| Argument / Option | Description |
|-------------------|-------------|
| `<JOB_ID>` | The ID of the job to enable. Required. |
| `--config`, `-c` `<PATH>` | Path to a config file. |

### weft cron disable

Disable a cron job without removing it.

```
weft cron disable <JOB_ID> [OPTIONS]
```

| Argument / Option | Description |
|-------------------|-------------|
| `<JOB_ID>` | The ID of the job to disable. Required. |
| `--config`, `-c` `<PATH>` | Path to a config file. |

### weft cron run

Manually trigger a cron job immediately, regardless of its schedule.

```
weft cron run <JOB_ID> [OPTIONS]
```

| Argument / Option | Description |
|-------------------|-------------|
| `<JOB_ID>` | The ID of the job to trigger. Required. |
| `--config`, `-c` `<PATH>` | Path to a config file. |

### Examples

List all cron jobs:

```
weft cron list
```

Add a daily summary job at 9 AM UTC:

```
weft cron add --name "daily-summary" --schedule "0 9 * * *" --prompt "Generate a summary of yesterday's activity"
```

Disable a job temporarily:

```
weft cron disable abc123
```

Re-enable a disabled job:

```
weft cron enable abc123
```

Manually run a job for testing:

```
weft cron run abc123
```

Remove a job:

```
weft cron remove abc123
```

---

## weft sessions

Manage agent conversation sessions. Sessions track message history between the
agent and users or channels.

### Subcommands

### weft sessions list

List all sessions in a table showing session key, message count, and last
updated timestamp.

```
weft sessions list [OPTIONS]
```

| Flag / Option | Description |
|---------------|-------------|
| `--config`, `-c` `<PATH>` | Path to a config file. |

### weft sessions inspect

Display the full message history for a session.

```
weft sessions inspect <SESSION_ID> [OPTIONS]
```

| Argument / Option | Description |
|-------------------|-------------|
| `<SESSION_ID>` | The session identifier to inspect. Required. |
| `--config`, `-c` `<PATH>` | Path to a config file. |

### weft sessions delete

Delete a session and its message history.

```
weft sessions delete <SESSION_ID> [OPTIONS]
```

| Argument / Option | Description |
|-------------------|-------------|
| `<SESSION_ID>` | The session identifier to delete. Required. |
| `--config`, `-c` `<PATH>` | Path to a config file. |

### Examples

List all sessions:

```
weft sessions list
```

View messages in a session:

```
weft sessions inspect slack-C04ABCDEF-U01XYZ
```

Delete a session:

```
weft sessions delete slack-C04ABCDEF-U01XYZ
```

---

## weft memory

View and search the agent's persistent memory and history files.

### Subcommands

### weft memory show

Display the contents of `MEMORY.md`.

```
weft memory show [OPTIONS]
```

| Flag / Option | Description |
|---------------|-------------|
| `--config`, `-c` `<PATH>` | Path to a config file. |

### weft memory history

Display the contents of `HISTORY.md`.

```
weft memory history [OPTIONS]
```

| Flag / Option | Description |
|---------------|-------------|
| `--config`, `-c` `<PATH>` | Path to a config file. |

### weft memory search

Search across memory and history for matching entries.

```
weft memory search <QUERY> [OPTIONS]
```

| Argument / Option | Description |
|-------------------|-------------|
| `<QUERY>` | The search term or phrase. Required. |
| `--limit`, `-n` `<N>` | Maximum number of results to return. |
| `--config`, `-c` `<PATH>` | Path to a config file. |

### weft memory export

Export memory data with a WITNESS integrity chain. Produces a JSON file
containing all memory entries and their cryptographic hashes.

```
weft memory export [OPTIONS]
```

| Flag / Option | Description |
|---------------|-------------|
| `--output`, `-o` `<PATH>` | Output file path. Default: `memory-export.json`. |
| `--config`, `-c` `<PATH>` | Path to a config file. |

### weft memory import

Import previously exported memory data. Validates the WITNESS chain before
applying entries.

```
weft memory import <PATH> [OPTIONS]
```

| Argument / Option | Description |
|-------------------|-------------|
| `<PATH>` | Path to the exported memory JSON file. Required. |
| `--config`, `-c` `<PATH>` | Path to a config file. |

### Examples

Show the current memory file:

```
weft memory show
```

Show the history file:

```
weft memory history
```

Search memory for a topic:

```
weft memory search "deployment procedure"
```

Search with a result limit:

```
weft memory search "auth" --limit 5
```

Export memory with WITNESS chain:

```
weft memory export -o backup.json
```

Import memory from a previous export:

```
weft memory import backup.json
```

---

## weft config

Inspect the resolved configuration.

### Subcommands

### weft config show

Print the full resolved configuration as JSON.

```
weft config show [OPTIONS]
```

| Flag / Option | Description |
|---------------|-------------|
| `--config`, `-c` `<PATH>` | Path to a config file. |

### weft config section

Print a specific configuration section as JSON.

```
weft config section <NAME> [OPTIONS]
```

| Argument / Option | Description |
|-------------------|-------------|
| `<NAME>` | The section name to display. One of: `agents`, `gateway`, `channels`, `tools`. Required. |
| `--config`, `-c` `<PATH>` | Path to a config file. |

### Examples

Show the full resolved config:

```
weft config show
```

Show only the channels section:

```
weft config section channels
```

Show agent configuration from a custom file:

```
weft config section agents -c ./staging.toml
```

---

## weft help

Show help for a specific topic or a general overview of all subcommands.

### Usage

```
weft help [TOPIC]
```

### Arguments

| Argument | Description |
|----------|-------------|
| `[TOPIC]` | Topic to display help for. One of: `skills`, `agents`, `tools`, `commands`, `config`. When omitted, prints a general overview. |

### Examples

Show general help:

```
weft help
```

Show help for skills:

```
weft help skills
```

Show help for configuration:

```
weft help config
```

---

## weft mcp-server

Start an MCP tool server over stdio. Exposes all registered tools (builtin
and MCP-proxied) as an MCP server, reading JSON-RPC requests from stdin and
writing responses to stdout. This allows MCP clients (Claude Desktop, Cursor,
etc.) to use clawft tools natively.

### Usage

```
weft mcp-server [OPTIONS]
```

### Options

| Flag / Option | Description |
|---------------|-------------|
| `--config`, `-c` `<PATH>` | Path to a config file. Overrides the default config resolution. |

### Examples

Start the MCP server with default config:

```
weft mcp-server
```

Start with a custom config:

```
weft mcp-server -c /etc/weft/production.toml
```

---

## weft mcp

Manage dynamic MCP server connections. These commands add, remove, and list
external MCP servers that clawft proxies as tools.

### Subcommands

### weft mcp add

Register a new MCP server. The command after `--` is used to spawn the server
process.

```
weft mcp add <NAME> -- <COMMAND...>
```

| Argument | Description |
|----------|-------------|
| `<NAME>` | Unique name for the MCP server. Required. |
| `<COMMAND...>` | Shell command to start the server (e.g., `npx -y @modelcontextprotocol/server-github`). Required. |

### weft mcp remove

Remove a previously registered MCP server.

```
weft mcp remove <NAME>
```

| Argument | Description |
|----------|-------------|
| `<NAME>` | Name of the MCP server to remove. Required. |

### weft mcp list

List all configured MCP servers with their status.

```
weft mcp list
```

### Examples

Add a GitHub MCP server:

```
weft mcp add github -- npx -y @modelcontextprotocol/server-github
```

List configured servers:

```
weft mcp list
```

Remove a server:

```
weft mcp remove github
```

---

## weft security

Security auditing tools.

### weft security scan

Run 57 security audit checks against the current workspace and configuration.
Reports findings with severity levels (INFO, WARN, ERROR, CRITICAL).

```
weft security scan [OPTIONS]
```

| Flag / Option | Description |
|---------------|-------------|
| `--fix` | Automatically fix issues where possible. |
| `--config`, `-c` `<PATH>` | Path to a config file. |

### Examples

Run a security scan:

```
weft security scan
```

Run a scan and auto-fix issues:

```
weft security scan --fix
```

---

## weft onboard

Set up initial clawft configuration and workspace. Creates the `~/.clawft/`
directory structure, generates a config template, and optionally prompts for
API key configuration.

### Usage

```
weft onboard [OPTIONS]
```

### Options

| Flag / Option | Description |
|---------------|-------------|
| `--yes`, `-y` | Skip interactive prompts and use defaults. |
| `--dir` `<PATH>` | Override the config directory (default: `~/.clawft`). |

### Examples

Run the interactive onboarding wizard:

```
weft onboard
```

Non-interactive setup with defaults:

```
weft onboard --yes
```

Use a custom config directory:

```
weft onboard --dir /tmp/my-clawft
```

---

## weft skills

Manage skills. Skills are discovered from workspace (`.clawft/skills/`),
user (`~/.clawft/skills/`), and builtin sources.

### Subcommands

### weft skills list

List all skills with source annotation.

```
weft skills list
```

Displays a table with columns: NAME, SOURCE, FORMAT, DESCRIPTION.

### weft skills show

Show details of a specific skill, including description, version, format,
variables, allowed tools, metadata, and an instructions preview.

```
weft skills show <NAME>
```

| Argument | Description |
|----------|-------------|
| `<NAME>` | Skill name to inspect. Required. |

### weft skills install

Install a skill from a local path to the user skills directory
(`~/.clawft/skills/`).

```
weft skills install <PATH>
```

| Argument | Description |
|----------|-------------|
| `<PATH>` | Path to a skill directory (containing `SKILL.md` or `skill.json`). Required. |

### weft skills remove

Remove a user-installed skill from `~/.clawft/skills/`. Built-in and
workspace skills cannot be removed via this command.

```
weft skills remove <NAME>
```

| Argument | Description |
|----------|-------------|
| `<NAME>` | Skill name to remove. Required. |

### Examples

List all skills:

```
weft skills list
```

Show details for a specific skill:

```
weft skills show research
```

Install a skill from a local directory:

```
weft skills install ./my-skills/summarize
```

Remove a user-installed skill:

```
weft skills remove summarize
```

---

## weft project

Project identity (ADR-103). A project is a directory tree with a ULID in
`.weftos/project.toml`, registered in the user-level index
`~/.weftos/projects/<id>.toml`. The index directory can be overridden with
`WEFTOS_MANIFESTS_DIR`. See the [Projects section of the workspaces
guide](../guides/workspaces.md#projects).

### weft project init

```
weft project init [--name <NAME>] [--fork [--force]]
```

Give the current project root (the nearest ancestor with a `project.toml`, else
the current directory) an identity and register it. Idempotent. Adopts an id
already seeded from `workspaces.json`. Refuses `$HOME` itself. Adds
`.weftos/chain/`, `.weftos/project.key`, `.weftos/project.cert.json` and
`.weftos/state/` to an existing `.gitignore` (`.weftos/overlay.toml` is meant
to be committed).
Prints the id, the root, the identity file and the manifest path.

| Option | Description |
|--------|-------------|
| `--name` | Project name (default: directory name) |
| `--fork` | The tree is a copy of another project: mint a new id, record the old one as `parent` |
| `--force` | With `--fork`: also re-identify the registered home of the id (its manifest is archived) |

A copy of a registered tree on the same machine is refused with a
`RootConflict` error; `--fork` is the remedy.

### weft project list

```
weft project list [--json]
```

Read the index directly (no daemon). Entries whose root is gone show `missing`.

### weft project show

```
weft project show [<ID|NAME|.>] [--here] [--json]
```

Show one project (default: the one containing the current directory), plus the
live daemon handshake when one answers, or why it was not verified. A name
matching several projects is an error listing their ids.

### weft project seed

```
weft project seed
```

Import `~/.clawft/workspaces.json` into the index and print what was created,
adopted, missing, unchanged or skipped. Idempotent.

---

## weaver kernel: project kernels

Per-project child kernels supervised by the user daemon (ADR-103 Phase 2,
amendment A7). A project runs as a child only after it opts in with
`weaver project migrate-kernel`, which sets `[serve] via = "child-kernel"`
in its manifest. These commands talk to the user daemon; none of them starts
it. When it is not running they print the command that does
(`weaver kernel start --profile user`).

```
weaver kernel start --project <ID|NAME>
weaver kernel stop --project <ID|NAME>
weaver kernel stop --all-children
weaver kernel stop --profile user [--keep-children]
weaver kernel restart --project <ID|NAME>
weaver kernel status --profile user
```

| Command | What it does |
|---------|--------------|
| `start --project` | Start the project's kernel, or find the one already running (`project.start`). It is ready when its handshake names the project. |
| `stop --project` | Graceful stop: `kernel.shutdown` (final anchor first), then `SIGTERM`, then `SIGKILL`. A stopped child is not restarted. |
| `stop --all-children` | Stop every project kernel and leave the user daemon running. |
| `stop --profile user` | Stop the user daemon. Its children are stopped cleanly first unless you pass `--keep-children`, which leaves them for the next daemon to adopt. |
| `restart --project` | Stop the project's kernel, clear a `failed` state and start it again. |
| `status --profile user` | On the user daemon, also lists the children and any unverifiable leftovers. Without `--profile user`, `status` addresses the daemon the current directory resolves to, which may be a project kernel. |

A crashed child is restarted after a 1 s backoff that doubles up to 30 s.
After `restart_max` crashes inside `restart_window_secs` (manifest `[serve]`,
defaults 5 and 60 s) the child is marked `failed` and is not started again,
not even on demand, until `restart --project`. A clean exit (an idle stop or
an explicit stop) is never restarted. A `child-kernel` project with no
activity for `idle_stop_secs` (default 1800, `0` disables it) is stopped
gracefully. `project.revoke` is terminal for the project's id: the project
never starts again and no key is ever certified for that id again, so it
cannot re-enrol (`project_revoked`). The marker is `<run_root>/<id>/revoked`
(`~/.weftos/run/<id>/revoked` unless `WEFTOS_RUNTIME_DIR` names the run root);
nothing removes it, and deleting it does not re-enable the id. To run the tree
again, give it a new identity with `weft project init --fork --force` and
migrate that id. If the revoke RPC answers `revoke_marker_unwritten` or
`identity_change_incomplete`, the revocation is in force and the child was
stopped, but a file could not be written or removed; fix the cause (the
message names the file).

Inside a project, a plain `weaver kernel start` is refused while a user
daemon runs on this account. `--legacy-project-daemon` overrides that for one
more release.

## weaver project migrate-kernel

```
weaver project migrate-kernel <ID|NAME> [--dry-run | --revert]
```

Opt one project into a supervised child kernel. The command refuses while the
project's own daemon runs, and that daemon is never signalled. It also
refuses symlinked sources. It copies `<root>/.weftos/runtime/{workloads.json,
apps.json}` to `<root>/.weftos/state/` (copies only; the originals are kept),
sets `[serve] via = "child-kernel"` and prints the rollback line.
`--dry-run` prints the plan and changes nothing. `--revert` sets the project
back to the user daemon and leaves the copies in place.

---

## weft workspace

Manage workspaces. Workspaces provide isolated directories for sessions,
memory, skills, agents, hooks, and configuration.

### Subcommands

### weft workspace create

Create a new workspace.

```
weft workspace create <NAME> [OPTIONS]
```

| Argument / Option | Description |
|-------------------|-------------|
| `<NAME>` | Workspace name. Required. |
| `--dir` `<PATH>` | Parent directory for the workspace. Defaults to the current directory. |

### weft workspace list

List all registered workspaces.

```
weft workspace list [OPTIONS]
```

| Flag / Option | Description |
|---------------|-------------|
| `--all` | Show all entries including those with missing directories. |

### weft workspace show

Show details of a specific agent workspace, including its SOUL.md, session
count, skill overrides, and configuration.

```
weft workspace show <AGENT_ID>
```

| Argument | Description |
|----------|-------------|
| `<AGENT_ID>` | Agent identifier whose workspace to inspect. Required. |

### weft workspace load

Load (activate) a workspace by name or filesystem path.

```
weft workspace load <NAME_OR_PATH>
```

| Argument | Description |
|----------|-------------|
| `<NAME_OR_PATH>` | Workspace name or filesystem path. Required. |

### weft workspace status

Show status of the current workspace, including session count, config
presence, and scoped resource paths.

```
weft workspace status
```

### weft workspace delete

Delete a workspace from the registry. By default also removes `.clawft/` and
`CLAWFT.md` at the workspace root (FR-W06). Use `--keep-data` for a
registry-only delete. The project root and other user files are not removed.

```
weft workspace delete <NAME> [OPTIONS]
```

| Argument / Option | Description |
|-------------------|-------------|
| `<NAME>` | Workspace name to delete. Required. |
| `--yes`, `-y` | Skip confirmation prompt. |
| `--keep-data` | Keep workspace files on disk (registry-only delete). |

### weft workspace config

Manage workspace configuration using dot-separated keys.

### weft workspace config set

Set a configuration key.

```
weft workspace config set <KEY> <VALUE>
```

| Argument | Description |
|----------|-------------|
| `<KEY>` | Dot-separated config key (e.g., `agents.defaults.model`). Required. |
| `<VALUE>` | Value to set. Parsed as JSON primitive when possible (number, boolean, null), otherwise stored as string. Required. |

### weft workspace config get

Get the value of a configuration key.

```
weft workspace config get <KEY>
```

| Argument | Description |
|----------|-------------|
| `<KEY>` | Dot-separated config key. Required. |

### weft workspace config reset

Reset workspace configuration to empty.

```
weft workspace config reset
```

### Examples

Create a new workspace:

```
weft workspace create my-project
```

Create a workspace in a specific directory:

```
weft workspace create my-project --dir /home/user/projects
```

List all workspaces:

```
weft workspace list
```

Show an agent workspace:

```
weft workspace show my-agent-id
```

Load a workspace:

```
weft workspace load my-project
```

Show current workspace status:

```
weft workspace status
```

Set a config value:

```
weft workspace config set agents.defaults.model openai/gpt-4o
```

Get a config value:

```
weft workspace config get agents.defaults.model
```

Reset workspace config:

```
weft workspace config reset
```

Delete a workspace (with confirmation; removes `.clawft/` + `CLAWFT.md`):

```
weft workspace delete my-project
```

Delete a workspace without confirmation:

```
weft workspace delete my-project -y
```

Registry-only delete (keep files on disk):

```
weft workspace delete my-project --keep-data -y
```

---

## weft completions

Generate shell completion scripts for `weft`. Completions are printed to
stdout and can be evaluated or saved to a file.

### Usage

```
weft completions <SHELL>
```

### Arguments

| Argument | Description |
|----------|-------------|
| `<SHELL>` | Target shell. One of: `bash`, `zsh`, `fish`, `powershell`. Required. |

### Examples

Enable completions in the current bash session:

```
eval "$(weft completions bash)"
```

Persist completions for zsh (add to `.zshrc`):

```
weft completions zsh > ~/.zfunc/_weft
```

Generate fish completions:

```
weft completions fish > ~/.config/fish/completions/weft.fish
```

Generate PowerShell completions:

```
weft completions powershell > weft.ps1
```
