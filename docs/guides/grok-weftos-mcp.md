# Grok Build ↔ WeftOS MCP bridge

**Decision records:** [ADR-075](../adr/adr-075-grok-weftos-mcp-client-bridge.md) (attach), [ADR-076](../adr/adr-076-mcp-tool-surface-capability-catalog.md) (tool surface + profiles)  
**Catalog:** [mcp-capability-catalog.md](../weftos/mcp-capability-catalog.md)  
**Related:** [MCP integration](./mcp-integration.md), [tool calls / Claude bridge](./tool-calls.md), [ADR-073 Agent Workspace](../adr/adr-073-agent-workspace-cnvs-principles.md), [ADR-074 xAI voice](../adr/adr-074-interim-xai-grok-voice.md)

This guide is the operator write-up for attaching **Grok Build** (`grok`) to a **WeftOS / clawft** instance so Grok can use WeftOS as a tool surface (and later as a control plane for agents and windows).

---

## 1. Mental model

| Role | Component |
|------|-----------|
| **MCP client** | Grok Build (TUI / headless / ACP) |
| **MCP server** | `weft mcp-server` (stdio JSON-RPC; optional `--listen` HTTP/SSE) |
| **Governance** | Middleware on the server (security, permissions, audit) |
| **Intent bus (later)** | `WindowIntent` + agent runtime — same path as voice/GUI (ADR-073) |

```
Grok Build
   │  tools/list, tools/call
   ▼
weft mcp-server  ──► ToolRegistry / skills / (future) WindowIntent
```

**Not the same as ADR-074:** Grok *Voice* (speech-to-speech) is Talk-Mode. This guide is Grok *CLI* as a coding/ops agent.

**Not the same as Ruflo MCP:** Project Ruflo/claude-flow MCP is swarm orchestration. WeftOS MCP is the **OS/agent tool surface**. Both can be enabled in Grok at once.

---

## 2. Prerequisites

1. `weft` on `PATH` (e.g. `cargo install --path crates/clawft-cli` or workspace build → `~/.cargo/bin/weft`).
2. Grok Build installed and authenticated (`grok --version`).
3. Optional: WeftOS config (`~/.clawft/config.json` or project config) for workspace roots and MCP-inbound servers.

Verify serve path:

```bash
weft mcp-server --help
# Leave it running only when spawned by Grok; normally Grok starts it.
```

---

## 3. Attach Grok (Level 1 — ready today)

### Option A — CLI

```bash
grok mcp add weftos -- weft mcp-server
# explicit profile (default is already product-safe):
# grok mcp add weftos -- weft mcp-server --profile default
# pin config:
# grok mcp add weftos -- weft mcp-server --config /path/to/config.json

grok mcp list
grok mcp doctor weftos
```

### Option B — config.toml

User (`~/.grok/config.toml`) or **project** (`.grok/config.toml` when the folder is trusted):

```toml
[mcp_servers.weftos]
command = "weft"
args = ["mcp-server"]
enabled = true
startup_timeout_sec = 60
tool_timeout_sec = 600
```

With explicit config file:

```toml
[mcp_servers.weftos]
command = "weft"
args = ["mcp-server", "--config", "/path/to/config.json"]
enabled = true
```

Restart Grok or reload MCPs, then:

```bash
grok mcp doctor weftos
```

Tools appear namespaced as `weftos__<tool_name>` (server name + tool).

### Claude parity (same server)

```bash
claude mcp add clawft -- weft mcp-server
# or weftos as the server name — either is fine
```

---

## 4. Levels of integration (roadmap)

| Level | What you get | Status |
|-------|----------------|--------|
| **L1 Tool client** | Call WeftOS tools from Grok | **Now** (stdio; catalog still evolving) |
| **L2 Control plane** | Profiles + weave façade tools | Phased (ADR-076 C1–C3, ADR-075 G1–G2) |
| **L3 Remote instance** | Grok laptop → remote WeftOS HTTP MCP | **Now** (`--listen` + bearer; WEFT-696/697) |

### Profiles (ADR-076 / WEFT-699–700) — live defaults

| Profile | Contents |
|---------|----------|
| **`default`** | `control` ∪ `workspace` — **product default** when flag omitted |
| `control` | status, agents, windows, **`skill_list` + `skill_get`** façades |
| `workspace` | sandboxed FS, shell, file memory, web (policy), **`process_spawn`** |
| `media` | voice / audio / render_ui — **opt-in** (session-bound) |
| `full` | entire registry + per-skill tool expansion — **explicit only** |

```bash
weft mcp-server                         # same as --profile default
weft mcp-server --profile default
weft mcp-server --profile full          # skill expansion; still no peer re-export
weft mcp-server --profile full --reexport-mcp  # re-export inbound MCP peers (dangerous)
weft mcp-server --profile control,media # compose
# weft mcp-server --attach              # live kernel façade (WEFT-701)
```

**Public wire (WEFT-700):**

- Client-visible tool names are **flat product names** — no `builtin__` (or `skill__`) prefix. Grok still namespaces by server key as `weftos__read_file`, never `weftos__builtin__read_file`.
- OS subprocess tool is **`process_spawn`** (not bare `spawn`). Agent lifecycle uses `agent_*` when present — different names.
- Default/control skills: **`skill_list` + `skill_get` only** — not one MCP tool per skill. Per-skill expansion only on `full`.
- Proxied external MCP re-export requires **`--profile full` and `--reexport-mcp`**. `full` alone does not re-export peers.

**Important:** empty `tools.allowed_tools` is **not** a full dump. Profile filters first; a non-empty allowlist is an *additional* filter. Media tools (`voice_*`, `audio_*`, `render_ui`) are excluded from `default`.

### Level 2 tools (control plane — WEFT-694 / WEFT-701)

Grok conductor basics over MCP (no freeform WM required):

| MCP tool | Mode | Backend |
|----------|------|---------|
| `status` | `--attach` | weave `kernel.status` (version / mode / reachability) |
| `agent_list` | `--attach` | weave `agent.list` (long-running sessions; aligns substrate inventory WEFT-685) |
| `agent_spawn` | `--attach` | weave `agent.spawn` — default policy is runtime-visible when UI exists; CLI-only path: same tool without panes |
| `agent_stop` | `--attach` | weave `agent.stop` (governed cancel) |

```bash
# Live instance (daemon must be running)
weaver kernel start
weft mcp-server --attach --profile control
# tools/list → status, agent_list, agent_spawn, agent_stop (+ skill façades if allowed)

# Grok project config
# args = ["mcp-server", "--attach", "--profile", "control"]
```

Standalone `weft mcp-server` (no `--attach`) remains offline/dev coding tools only — control tools are **not** silent empties; use attach for live agents.

Prefer public names: `status`, `agent_list`, `agent_spawn`, `agent_stop`, `window_*`, `read_file`, `process_spawn`, `skill_list`, `skill_get`, …  
Full rows: [capability catalog](../weftos/mcp-capability-catalog.md).

### Level 3 (remote) — HTTP/SSE listen (WEFT-696 / WEFT-697)

On the **WeftOS host**:

```bash
# Generate a strong secret (do not commit it)
export WEFT_MCP_TOKEN="$(openssl rand -hex 32)"

# Loopback (default-safe): auth required
weft mcp-server --listen 127.0.0.1:8742 --token-env WEFT_MCP_TOKEN

# LAN / public interface: must opt in; auth still required
weft mcp-server --listen 0.0.0.0:8742 --token-env WEFT_MCP_TOKEN --dangerously-bind-public

# Enterprise: force reduced scopes (no agent_spawn; restricted tool globs)
weft mcp-server --listen 127.0.0.1:8742 --token-env WEFT_MCP_TOKEN --enterprise

# Optional: mint a token document (static bearer = put same value in WEFT_MCP_TOKEN)
weft mcp-server --issue-token --client-label grok --enterprise
```

**Auth rules (refuse open public bind):**

| Bind | Auth | Flag |
|------|------|------|
| `127.0.0.1` / `::1` | Bearer required by default | `--allow-unauthenticated` only for trusted loopback dev |
| `0.0.0.0` / non-loopback | Bearer **always** required | also needs `--dangerously-bind-public` |
| Open public (no token) | **Refused** | even with dangerous flag |

Endpoints:

| Method | Path | Notes |
|--------|------|--------|
| `POST` | `/mcp` | JSON-RPC (initialize, tools/list, tools/call, ping) |
| `GET` | `/mcp` or `/mcp/sse` | SSE keepalive stream (auth required) |
| `GET` | `/health` | Liveness (no auth) |

**Session capability (WEFT-697):** bearer tokens map to scopes — tool globs, workspace roots, `agent_spawn` rights. Audit log lines include `client=` when known (`X-MCP-Client: grok` or `clientInfo.name` on initialize). Secrets never appear in tool results.

**TLS:** terminate TLS at a reverse proxy (Caddy / nginx / cloud LB). The process speaks plain HTTP on the listen port by design.

```nginx
# Example reverse-proxy snippet (TLS at edge → loopback MCP)
location /mcp {
    proxy_pass http://127.0.0.1:8742;
    proxy_http_version 1.1;
    proxy_set_header Authorization $http_authorization;
    proxy_set_header X-MCP-Client $http_x_mcp_client;
}
```

**Grok remote stanza** (`~/.grok/config.toml` or project `.grok/config.toml`):

```toml
[mcp_servers.weftos-remote]
url = "https://your-host/mcp"
enabled = true
# Prefer env expansion / local overrides — do not commit real tokens
headers = { Authorization = "Bearer ${WEFT_MCP_TOKEN}", "X-MCP-Client" = "grok" }
```

Checklist (manual):

1. `export WEFT_MCP_TOKEN=…` on server; start `weft mcp-server --listen 127.0.0.1:8742 --token-env WEFT_MCP_TOKEN`
2. `curl -s http://127.0.0.1:8742/health` → `{"ok":true,…}`
3. `curl -s -X POST http://127.0.0.1:8742/mcp -H "Authorization: Bearer $WEFT_MCP_TOKEN" -H "Content-Type: application/json" -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"grok","version":"1"}}}'` → serverInfo
4. Without `Authorization` → HTTP 401
5. `weft mcp-server --listen 0.0.0.0:8742 --token-env WEFT_MCP_TOKEN` (no dangerous flag) → refuse at startup
6. Point Grok `url` + `headers` at the TLS edge; `grok mcp doctor weftos-remote`

Local stdio remains the default for single-machine attach (`grok mcp add weftos -- weft mcp-server`).

---

## 5. Bidirectional use (optional)

| Direction | Config |
|-----------|--------|
| **Grok → WeftOS** | This guide (`weft mcp-server` in Grok) — **primary** |
| **WeftOS → external MCP** | `weft mcp add` / `tools.mcp_servers` — WeftOS agent calls other servers |

Running both at once can create **delegation loops**. Prefer one primary driver per session. See recursive-delegation notes in [tool-calls.md](./tool-calls.md).

### WeftOS on a Ruflo agent team (ADR-320)

WeftOS joins a Ruflo team in two ways. Neither needs Rust changes.

**As an exec host (a spawnable teammate).** `.claude-flow/team-hosts.json` (tracked) declares the `weft` command host:

```json
{ "hosts": { "weft": { "kind": "exec", "command": "weft",
  "args": ["agent", "-m", "{prompt}"], "promptVia": "arg",
  "passEnv": ["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "OPENROUTER_API_KEY"],
  "isolation": "none" } } }
```

A lead then registers and runs a WeftOS teammate:

```bash
ruflo team create --params '{"name":"demo","host":"weft"}'
ruflo team plan   --params '{"team":"demo","steps":["helper"]}'
ruflo team spawn  --params '{"team":"demo","agent":"helper","role":"researcher","prompt":"..."}'
ruflo team run --team demo --agent helper --dry-run   # show the argv first
ruflo team run --team demo --agent helper             # one weft agent -m turn
```

The runner starts `weft agent -m` with no shell, captures stdout as the result, sends it to the next agent (or `lead`), and advances the plan. A non-zero exit or a timeout marks the step `failed` without advancing it. `passEnv` names the only secret-named variables the child keeps; the runner strips every other key, token and password variable. Keep `passEnv` in step with the providers in `.clawft/config.json`, and remember that `./.env` can shadow config values (see the `.env` gotcha in the build notes). `--trust-project-skills` is left out on purpose (SEC-SKILL-05); add it to `args` only when team children need workspace skills.

**As an MCP participant.** Register Ruflo's team tools with the WeftOS MCP client so a WeftOS agent can call `team_inbox` and `team_send` itself:

```bash
weft mcp add ruflo \
  --env CLAUDE_FLOW_MCP_TOOLS=team \
  --env CLAUDE_FLOW_CWD="$(pwd)" \
  --internal-only=false \
  -- node <ruflo-checkout>/v3/@claude-flow/cli/bin/cli.js mcp start
```

`CLAUDE_FLOW_MCP_TOOLS=team` keeps the ToolRegistry to the nine `team_*` tools instead of the full Ruflo catalog. `CLAUDE_FLOW_CWD` pins the team root, so a child started elsewhere still reads this repo's mailboxes. Use a local Ruflo checkout until a published release ships `ruflo team` (the same caveat as `.grok/config.toml`).

**Grok CLI bus.** `scripts/grok-team-bus.mjs` keeps its flags but is now a shim over `ruflo team <verb> --params`, and `scripts/grok-subagent-stop-hook.mjs` calls `ruflo team hook-stop --host grok`. Both find the Ruflo CLI through `RUFLO_CLI` or a one-line, gitignored `.claude-flow/ruflo-cli-path`:

```bash
echo "$HOME/dev/ruflo/v3/@claude-flow/cli/bin/cli.js" > .claude-flow/ruflo-cli-path
node --test scripts/grok-team-bus.interop.test.mjs   # SKIPs when the CLI cannot be resolved
```

If the CLI cannot be resolved, the shim exits 2 instead of writing team state in its own format.

---

## 6. Security notes

- All MCP calls through `weft mcp-server` pass the middleware pipeline (validation, permission, result guard, audit).  
- Workspace file tools stay sandboxed to configured workspace roots.  
- Shell tools still hit denylist / policy.  
- Remote serve (`--listen`) **requires** auth by default; open public bind is refused.  
- Prefer `--token-env` / `WEFT_MCP_TOKEN` over `--token` (avoids argv leakage).  
- Do not commit API keys in `.grok/config.toml`; use env expansion / local overrides.

---

## 7. Relationship to Agent Workspace and voice

```
Grok MCP ──┐
Keys/GUI ──┼──► WindowIntent / agent runtime ──► Agent Workspace panes
Voice ─────┘     (ADR-073 / ADR-074)
```

Product bar (CNVS-like conductor): Grok or voice can spawn visible agents and drive layout **only** through shared intents — not a Grok-only UI fork.

---

## 8. Troubleshooting

| Symptom | Check |
|---------|--------|
| `grok mcp doctor weftos` fails spawn | `which weft`; build CLI; increase `startup_timeout_sec` |
| Zero tools | Server crashed on init — run `weft mcp-server` manually and watch stderr |
| Tools huge / noisy | Use `--profile default` (product default); avoid `--profile full --reexport-mcp` unless debugging |
| Permission denied on tools | WeftOS config policies / workspace root |
| Ruflo tools present, WeftOS not | Separate MCP entries — both can coexist |

---

## 9. Plane work (tracking)

| WEFT | Phase | Cycle |
|------|-------|-------|
| **WEFT-692** | G0 docs + config + doctor | 0.8.x |
| **WEFT-693** | G1 curated serve profile *(dup intent with WEFT-699)* | 0.8.x |
| **WEFT-694** | G2 status / agents MCP tools | 0.8.x |
| **WEFT-695** | G3 MCP → WindowIntent | 0.9.x |
| **WEFT-696** | G4 HTTP/SSE listen + auth | 0.8.x (shipped) |
| **WEFT-697** | G5 session capability tokens | 0.8.x (shipped) |
| **WEFT-698…703** | ADR-076 C0–C5 catalog / profiles / attach / CI | 0.8–0.9 |

Related workspace/voice: WEFT-685…691 (ADR-073/074).

---

## 10. See also

- [ADR-075](../adr/adr-075-grok-weftos-mcp-client-bridge.md)  
- [ADR-070](../adr/adr-070-mcp-registry-ownership.md) — config vs daemon registry  
- [mcp-integration.md](./mcp-integration.md) — WeftOS as client + `internal_only`  
- [docs/grok/README.md](../grok/README.md) — Ruflo-on-Grok host setup  
