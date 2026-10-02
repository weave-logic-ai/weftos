# Agents

This guide covers agent identity, workspace roots, and how the daemon
resolves persona files for Concierge / `agent.chat`.

For skills catalogs and custom agent definitions, see
[skills-and-agents.md](./skills-and-agents.md). For the full config schema,
see [configuration.md](./configuration.md) and
[reference/config.md](../reference/config.md).

---

## Identity files

Each initialized workspace holds the Concierge persona under `.clawft/`:

| File | Role |
|------|------|
| `.clawft/SOUL.md` | Persona, ethical constraints, binding-thread values |
| `.clawft/IDENTITY.md` | Operational identity, skills, tone |

`weaver init` materializes both. The daemon’s
[`FileIdentityProvider`](../../crates/clawft-core/src/agent/identity.rs)
loads them via [`Platform::fs`]. On native builds a notify watcher
(WEFT-329) invalidates the cache when `SOUL.md` / `IDENTITY.md` change;
the next turn re-reads. Without the watcher (WASM / notify failure)
every call re-reads (small files). Missing files make `agent.chat` fail
with `identity load failed` — run `weaver init` to seed.

---

## `agents.workspace_root` (WEFT-83)

By default the daemon treats **process CWD** as the workspace: identity,
file tools, and related paths resolve relative to wherever the daemon was
started. That is awkward for systemd units (fixed WorkingDirectory) and for
serving a project tree that is not the launch CWD.

Set an explicit root under the `agents` section:

```json
{
  "agents": {
    "workspace_root": "/home/user/my-project",
    "defaults": {
      "model": "openrouter/meta-llama/llama-3.3-70b-instruct"
    }
  }
}
```

CamelCase alias: `workspaceRoot`.

| Value | Behaviour |
|-------|-----------|
| unset / `null` | Use `std::env::current_dir()` (back-compat) |
| absolute path | Identity + daemon workspace use that path |
| `~/…` | Expanded to the home directory on native builds |

### What it affects

- **Identity** — `IdentityLoader` / `FileIdentityProvider` load
  `<workspace_root>/.clawft/{SOUL.md,IDENTITY.md}`
- **Agent tools** — file-tool workspace root passed at daemon wiring
- **Agent loop defaults** — `agents.defaults.workspace` is stamped from the
  resolved root when the daemon builds the loop

### What it does **not** change

- **`agents.defaults.workspace`** when set by the operator for CLI /
  non-daemon paths (nanobot-style working directory string)
- **Skills catalog discovery** (still walks `.clawft/skills/` and user dirs)
- **Multi-workspace RPC switching** — a single daemon process still binds
  one root at boot. Per-RPC workspace selection is a later 0.8.x story;
  until then, run one daemon per workspace or restart with a new config.

### Example: systemd with a fixed project root

```ini
# /etc/systemd/system/weaver.service
[Unit]
Description=WeftOS kernel (weaver)
After=network-online.target

[Service]
Type=simple
User=weaver
WorkingDirectory=/var/lib/weaver
ExecStart=/usr/local/bin/weaver kernel start --foreground
Restart=on-failure
RestartSec=3

[Install]
WantedBy=multi-user.target
```

```toml
# /var/lib/weaver/weave.toml  (read from the unit's WorkingDirectory)
[agents]
workspace_root = "/srv/projects/acme"
```

`kernel start` backgrounds by default, so a unit must pass `--foreground`
(there is no `daemon` subcommand). The same settings can live in
`~/.clawft/config.json` as the JSON shown above. To run the per-user daemon
under a service manager instead, `weaver service unit --kind launchd|systemd` prints a
user unit for `weaver kernel start --foreground --profile user` and
the commands to install it; it installs nothing itself.

The daemon may start from `/var/lib/weaver` while identity and tools use
`/srv/projects/acme`.

### Example: two workspaces (two configs / processes)

```json
// project-a/.clawft/config.json
{ "agents": { "workspace_root": "/workspaces/project-a" } }

// project-b/.clawft/config.json
{ "agents": { "workspace_root": "/workspaces/project-b" } }
```

Each process loads its own SOUL/IDENTITY. Unit tests under
`clawft-core::agent::identity` cover this with two temp trees and distinct
hashes.

---

## Related config

| Key | Purpose |
|-----|---------|
| `agents.workspace_root` | Daemon identity + workspace path (this page) |
| `agents.defaults.workspace` | Default working directory string for file ops |
| `agents.defaults.model` | Default LLM model id |
| `agents.cost_budget` | Per-conversation spend circuit-breaker |
| `agents.cow_memory` | Per-turn COW memory checkpoints |
| (planned, WEFT-732/733) | RMM citation-rerank + topic merge — `docs/research/rmm-reflective-memory-management.md` |
