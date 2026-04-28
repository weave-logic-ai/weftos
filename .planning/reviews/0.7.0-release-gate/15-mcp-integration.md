---
title: "MCP Integration & Extension Surface"
slug: mcp-integration
workstream_id: "15"
audit_kind: comprehensive-depth
release_target: "0.7.0"
last_updated: 2026-04-28
sources_root: /home/aepod/dev/clawft
---

# MCP Integration & Extension Surface

## General Description

This workstream covers every place where clawft talks MCP (Model Context Protocol) or
acts as a remote control surface for an external IDE/editor:

- The **clawft daemon JSON-RPC** served over a Unix domain socket from
  `crates/clawft-weave/src/daemon.rs`, with the wire types and runtime/socket
  resolution living in `crates/clawft-rpc/`.
- The **VSCode/Cursor extension** at `extensions/vscode-weft-panel/`, which proxies
  webview RPC requests to the daemon socket through a hand-maintained
  `ALLOWED_METHODS` allowlist and hosts the egui WASM panel.
- The **`weft mcp-server` CLI subcommand** in `crates/clawft-cli/src/commands/mcp_server.rs`,
  which exposes the local `ToolRegistry` (built-in tools + skill tools + MCP-proxied tools)
  as an MCP server over stdio.
- The **MCP client/transport stack** in `crates/clawft-services/src/mcp/` —
  `client`, `transport`, `composite`, `discovery`, `provider`, `middleware`, `server`,
  `bridge`, `ide`, `types` — plus the `clawft-cli` wrapper at
  `crates/clawft-cli/src/mcp_tools.rs` that bridges discovered MCP tools into the
  agent's `ToolRegistry`.
- The **claude-flow integration surface**: there is no first-class wiring code for
  `npx @claude-flow/cli@latest`; claude-flow is referenced (a) as the canonical
  example in skill `allowed-tools` patterns and tool-name classification helpers,
  (b) in the security allowlist (`clawft` + `claude-flow` are unconditional default
  commands), and (c) as a planned-but-disabled toggle in `DelegationConfig`
  (`claude_flow_enabled: false`). Per-server config lives in
  `clawft_types::config::MCPServerConfig` with an `internal_only` flag the doc
  comment explicitly tags as the path infrastructure servers like claude-flow
  should take.

The architectural anchors are ADR-035 (ServiceApi-layered protocol — MCP is "an
adapter over kernel-native performance") and ADR-036 (hierarchical ToolRegistry —
kernel base + per-agent overlays); ADR-042 (three operating modes) defines the
tray/panel surface that the VSCode extension renders.

## Status & Timeline

| Component | State | Notes |
|---|---|---|
| `clawft-rpc` protocol crate | shipped | Line-delimited JSON over UDS; runtime-dir walks `.weftos/` ancestors then falls back to `~/.clawft/`. Unix-only client; non-Unix `connect()` returns `None`. |
| Daemon dispatch table (`daemon.rs:dispatch`) | shipped, growing | ~75 method arms; new `agent.chat` (Phase D3 cutover this week) replaced the `handle_agent_chat` spike but kept the `agent-core-chat` feature flag for one-commit revert. |
| VSCode extension RPC proxy | shipped, M1 | `ALLOWED_METHODS` set (24 verbs as of 2026-04-26 spike); 300s `LLM_TIMEOUT_MS` bucket for `llm.prompt` + `agent.chat`; CSP nonce + `wasm-unsafe-eval`; hot-reload watcher on `webview/wasm/`. |
| Webview WASM bundle | shipped, M1 | Built by `extensions/vscode-weft-panel/scripts/build-wasm.sh` (wasm-pack preferred, cargo + wasm-bindgen-cli fallback). Artifacts gitignored. Cache-busted by `Date.now()` on each `renderHtml`. |
| `weft mcp-server` (stdio MCP server) | shipped | Wraps the agent's `ToolRegistry` + `SkillRegistry` in a `CompositeToolProvider`; full middleware pipeline (`SecurityGuard`, `PermissionFilter`, `ResultGuard`, `AuditLog`); honors `MCP_PROTOCOL_VERSION = 2025-06-18`. |
| MCP client (`McpClient` / `McpSession`) | shipped | Stdio + HTTP transports, mock transport for tests, full handshake (`initialize` -> `notifications/initialized`). 30+ unit tests in `mcp/mod.rs`. |
| `McpServerManager` (dynamic add/list/remove + hot-reload) | shipped | Drain-and-swap protocol with 30s drain timeout, 500ms debounce. |
| `IdeToolProvider` (`mcp/ide.rs`) | shipped | 5 IDE tools (`ide_open_file`, `ide_edit`, `ide_diagnostics`, `ide_symbols`, `ide_hover`). Wires through `CompositeToolProvider`; backend bridge to actual IDE not yet enumerated in this audit. |
| Claude-flow first-class wiring | not started | Only present as: example in skill SKILL.md fixtures, MCP tool-name classifier (`classify_source("claude-flow__agent_spawn") -> "mcp:claude-flow"`), default command allowlist entry, `DelegationConfig.claude_flow_enabled: false`. No daemon code instantiates a claude-flow MCP session by default. |
| Windows transport | stub | `clawft-rpc::client` non-Unix module unconditionally returns `None` from `connect()`; a comment promises "Windows named-pipe transport is planned for v0.2." |
| TCP relay for cross-boundary clients | shipped | `[kernel.ipc_tcp]` config wraps the UDS in a byte-copy TCP relay so WSL-host or remote bridges can reach RPC; auth/JSON dispatch stays on the unix path. |

Most recent activity touching this workstream:
- `agent-core-v1` Phase D3 cutover wired `agent.chat` through `clawft-service-agent::AgentService::dispatch` and removed the C2 spike fallback.
- 2026-04-23 unblock: added `substrate.read` / `substrate.subscribe` / `substrate.list` / `control.set_enabled` / `control.list` to the proxy allowlist.
- 2026-04-26 `agent.chat` spike landed, allowlist gained `agent.chat` + matching 300s timeout bucket.

## Released Features

- **Daemon JSON-RPC over UDS** with line-delimited JSON, `Request{method,params,id}` /
  `Response{ok,result,error,id}` types in `clawft-rpc::protocol`. Three-tier
  socket-path resolution: `WEFTOS_RUNTIME_DIR` env override -> ancestor `.weftos/`
  walk -> `~/.clawft/`. Mirrored exactly in the extension's `rpc.ts`.
- **~75-method dispatch table** in `daemon.rs:dispatch()` covering kernel
  (`kernel.{status,ps,services,logs,shutdown,kill-process,restart-service}`),
  cluster (`cluster.{status,nodes,join,leave,health,shards}`), chain
  (`chain.{status,local,checkpoint,verify,export}`), resource tree
  (`resource.{tree,inspect,stats,score,rank}`), substrate
  (`substrate.{read,list,publish,canonical_publish_payload,notify}`), control
  flags (`control.{set_enabled,list}`), agent
  (`agent.{register,spawn,stop,restart,inspect,list,send,chat,chat.cancel}`),
  node (`node.{register,identity}`), assess
  (`assess.{run,status,link,peers,compare,mesh.status,mesh.gossip}`), ECC
  (`ecc.{status,calibrate,search,causal,tick,crossrefs}`), custody/mesh
  (`custody.attest`, `mesh.{revoke,unrevoke,revoked}`), terminal
  (`terminal.{spawn,write,resize,close}`), LLM (`llm.prompt`), cron
  (`cron.{add,list,remove}`), ipc (`ipc.{topics,subscribe,publish,subscribe_stream}`),
  workspace (`workspace.{create,list,load,status,delete,config.set,config.get,config.reset}`).
- **Optional TCP relay** (`[kernel.ipc_tcp]`) so non-Unix or networked clients can
  speak the same JSON wire format.
- **VSCode extension dev-panel** (`vscode-weft-panel`) with: webview panel, RPC
  proxy, allowlist gate, per-method timeout buckets (`llm.prompt` and `agent.chat`
  get 300 s, others fall through to the default 3 s), wasm hot-reload watcher,
  CSP-locked HTML render with cache-busted module imports, watchdog splash that
  surfaces module-import / wasm-init failures into the DOM.
- **WASM build script** (`scripts/build-wasm.sh`) with wasm-pack preferred and
  cargo + wasm-bindgen-cli fallback.
- **`weft mcp-server` stdio bridge**: lets external MCP clients (Claude Desktop,
  Cursor, etc.) call clawft's tools, including built-in tools, dynamically
  registered MCP-proxied tools, and SKILL.md-derived tools.
- **MCP client + session** with negotiated protocol version `2025-06-18` and
  full `initialize` handshake; supports HTTP and stdio transports plus a mock
  transport for tests (~14 unit tests on `McpClient`, ~6 on `McpSession`).
- **`McpServerManager`** with drain-and-swap hot-reload semantics (30s drain
  timeout, 500ms debounce, status enum: `Connected | Connecting | Draining |
  Disconnected | Error`).
- **`CompositeToolProvider`** that fans `tools/list` / `tools/call` across
  built-in, skill, and IDE providers. Middleware pipeline:
  `SecurityGuard -> PermissionFilter -> ResultGuard -> AuditLog`.
- **`IdeToolProvider`** with 5 declared tools (`ide_open_file`, `ide_edit`,
  `ide_diagnostics`, `ide_symbols`, `ide_hover`) — schema layer is published.

## What's Left — Total Depth

### TODOs / FIXMEs in source

The MCP and extension code paths are unusually clean for inline TODO markers.
Direct `grep TODO|FIXME|XXX|HACK` across `crates/clawft-rpc`, `crates/clawft-weave`,
`crates/clawft-services/src/mcp/`, `crates/clawft-cli/src/commands/mcp_server.rs`,
`crates/clawft-cli/src/mcp_tools.rs`, and `extensions/vscode-weft-panel/src/`
returns essentially nothing — a single `TODO(agent-core-v1.1)` in
`clawft-weave/src/commands/soul_cmd.rs:246` ("replace with `chain.append` RPC")
plus a comment that points at it. The debt lives in deferred items, not
inline markers.

### Deferred items (carried forward in code comments / handoff notes)

- **`weft mcp-server` middleware tightening.** `PermissionFilter::new(None)` is
  passed at server boot — meaning no allowlist is applied; every registered tool
  is exposed to the connecting MCP client. The security boundary today is
  `SecurityGuard` (command + URL policy) + `ResultGuard` (output truncation) +
  audit logging, but per-tool gating per session is not enforced.
- **VSCode extension `ALLOWED_METHODS` is hand-maintained.** Every new daemon
  verb that the WASM panel needs requires (a) a code edit in `extension.ts`
  and (b) re-running `npm run compile`. There is no automated extraction from
  `daemon.rs:dispatch()`, no shared canonical list. The `0.7.0` cutover added
  five entries in two commits; the 2026-04-23 incident postmortem in
  `.planning/explorer/PROJECT-PLAN.md:14-26` was caused by exactly this gap
  (chip icons grey because `substrate.read` was absent from the allowlist).
- **WASM bundle rebuild is manual.** `scripts/build-wasm.sh` is not invoked by
  any CI gate, and `scripts/build.sh` does not have a `webview-wasm` target.
  Memory `feedback_rebuild_webview_wasm.md` (referenced upstream) is the
  source of "rebuild after every gui-egui change" guidance; in practice
  developers run it under `cargo watch` per the comment block in `extension.ts:202`.
- **`weft-gui-egui` is not in `scripts/build.sh native`.** Per
  `docs/handoff.md:258`, building the native eframe binary requires
  `cargo build -p clawft-gui-egui --features native --bin weft-gui-egui`
  directly. The handoff explicitly defers promoting it to a first-class
  artifact ("user is staying with the Cursor panel for the chat demo").
- **No daemon-side allowlist on RPC.** Anything that connects to the UDS can
  call any method. The VSCode extension's `ALLOWED_METHODS` is a *webview*
  containment fence — it stops a malicious webview script from reaching
  arbitrary RPC, but the daemon itself accepts all callers. With the optional
  `[kernel.ipc_tcp]` relay this becomes a network-exposed surface.
- **Windows transport: stub only.** `crates/clawft-rpc/src/client.rs:62`
  promises Windows named-pipe transport "for v0.2." Today every non-Unix
  `DaemonClient::connect()` returns `None`; every client method bails with
  "daemon not available on this platform."
- **`agent-core-chat` feature flag** survives in `daemon.rs:3591-3621` so the
  D3 cutover can be reverted with one commit + flag flip. Removal of the
  flag is a future cleanup once D3 has burned in.
- **`SkillToolProvider` dispatcher** in `mcp_server.rs:115-124` returns the
  raw `instructions` text on tool call. Skills are exposed as MCP tools whose
  output is "the SKILL.md prompt body to inject into the calling agent's
  context." This is the agreed contract but documented nowhere outside that
  closure — first-time MCP clients hitting these tools see surprising shapes.
- **`internal_only: true` is the default for `MCPServerConfig`.** Per the
  field doc comment, infrastructure servers (claude-flow, claude-code) "should
  be internal" — meaning their tools are never exposed via `tools/list`. There
  is no test fixture or sample config showing the expected non-default
  configuration; the correct shape is implicit.
- **Pre-spike `handle_agent_chat`** was removed in the D3 cutover; the comment
  in `daemon.rs:3592` notes the C2 spike fallback is gone but the protocol
  type `AgentChatParams`/`AgentChatResult` is still owned by `clawft-weave`,
  not by `clawft-service-agent`. A future commit needs to relocate or alias
  the wire types so `clawft-weave` does not have to import service crates
  just for serde.

### Open questions

- **Should `ALLOWED_METHODS` be generated?** Today the extension allowlist is
  hand-curated and drifts from the daemon dispatch table on a multi-day lag
  (2026-04-23 incident proved this). Options: (a) emit a JSON manifest from
  the daemon at boot and have the extension fetch it, (b) generate a static
  `allowed_methods.ts` from a shared schema crate, (c) move enforcement into
  the daemon (per-method capability tags + caller identity). No decision in
  the planning docs.
- **Webview versus daemon allowlist semantics.** The extension comment block
  at `extension.ts:24-93` carefully documents that the daemon's "own
  capability check is the real gate" and the proxy allowlist "just keeps the
  webview from reaching arbitrary RPC surface." The daemon does not
  currently implement a per-method capability check (`grep -n allow.*method`
  on `daemon.rs` returns nothing useful). Either the comment is aspirational
  or the gating happens at a deeper layer the audit did not reach.
- **`substrate.publish` deliberately omitted from `ALLOWED_METHODS`.** Comment
  in `extension.ts:55` says "the webview is a viewer, not a writer." But the
  `agent.chat` handler can call tools that mutate substrate via the daemon
  process. The audit cannot tell from this workstream whether the spike's
  tool surface (`read_file`, `list_directory`) is exhaustive of the
  spike-time gap, or whether D3 introduced more.
- **Claude-flow MCP wiring at the project level.** Repo-name and CLAUDE.md
  call this "Claude Flow V3," but no Rust crate spawns a claude-flow MCP
  session by default. Is it expected that users add it via
  `weft mcp add claude-flow npx -y @claude-flow/cli@latest` per
  `mcp/discovery.rs:11`? If yes, the install path is a CLI command without
  a discovery shortcut. If no, what is the integration shape?
- **`McpServerManager` integration with the daemon.** `McpServerManager` is
  defined in `clawft-services/src/mcp/discovery.rs` and exposes add/remove
  and a planned drain-and-swap protocol. The daemon dispatch does not appear
  to host `mcp.add` / `mcp.list` / `mcp.remove` verbs — the doc comment says
  "CLI commands `weft mcp add/list/remove`" but the verbs route to a CLI
  helper, not RPC. Are MCP servers managed per-process or through the daemon?
- **WASM panel auth.** The webview connects to the same UDS the local user
  owns; there is no token, capability, or per-panel identity on the proxy
  layer. Multi-user kernels (per ADR-042 modes) would need to add this.
- **`IdeToolProvider` backend.** The provider declares 5 tools with detailed
  schemas, but the audit did not locate a backend that actually opens files
  / applies edits / fetches diagnostics from a connected IDE. The dispatcher
  closure's actual side effects need to be re-checked before claiming the
  tools are functional.
- **MCP protocol version negotiation.** Hard-coded to `2025-06-18` in
  `mcp/mod.rs:26` and `mcp/server.rs:18`. Server-side `initialize` advertises
  this verbatim; client side uses it as a fallback when the server omits
  `protocolVersion` in the response. There is no version-mismatch path —
  if a server returns a different version, we accept it silently.

### Orphaned work

- **`weft-gui-egui` native bin.** Compiled and exists, but not wired into
  `scripts/build.sh native`, not packaged for release. The Cursor panel is
  the user's daily driver per `handoff.md:263-266`.
- **`McpServerManager` hot-reload protocol.** The drain-and-swap design is
  documented in `discovery.rs` but the audit did not find a callsite that
  invokes the full reload flow against a live config-file watcher. The
  watcher integration is implied by the doc but the wire-up is not visible
  from this workstream.
- **MCP HTTP transport.** `mcp/transport.rs` exposes the trait and the audit
  did not exhaustively verify the HTTP transport implementation; if the
  HTTP path is tested only against `MockTransport`, real HTTP MCP servers
  may surface bugs first contact.
- **`agent-core-chat` feature flag.** Slated for removal once D3 burns in,
  but no scheduled removal commit. Will accumulate cruft if forgotten.
- **`DelegationConfig.claude_flow_enabled: false`** + `claude_enabled: true`.
  The "claude_flow" path is half-wired: config field + serde default exist,
  but flipping it on does not appear to engage any concrete dispatcher in
  this workstream. Either remove the field until wired, or land the wiring.

## Task List

| # | Item | Type | Effort | Owner hint |
|---|---|---|---|---|
| 1 | Decide and implement: shared canonical method list between daemon and extension allowlist (codegen vs runtime fetch vs daemon-side capability check) | open question -> design | M | clawft-weave + extensions |
| 2 | Add daemon-side per-method capability gating, then make `ALLOWED_METHODS` redundant or advisory | architecture | L | governance + daemon |
| 3 | Promote `webview-wasm` rebuild into `scripts/build.sh` (e.g. `scripts/build.sh webview` calling `extensions/vscode-weft-panel/scripts/build-wasm.sh`) | dev-experience | S | scripts |
| 4 | Wire `cargo watch` recipe + CI smoke that compiles `extension.ts` and rebuilds the wasm bundle on every PR touching `clawft-gui-egui` or `extension.ts` | CI | S | scripts |
| 5 | Document the `weft mcp add` install path for `@claude-flow/cli@latest` and put a sample in `weave.toml` (or wherever the canonical config lives) under `tools.mcp_servers` | docs | S | docs/handoff |
| 6 | Resolve "is claude-flow first-party or user-installed" question; either land the integration code or strip the half-wired `claude_flow_enabled` config field | decision -> code | M | core/types |
| 7 | Replace `PermissionFilter::new(None)` in `mcp_server.rs:140` with a real per-tool allowlist sourced from config | security | S | mcp_server |
| 8 | Implement Windows named-pipe transport in `clawft-rpc::client::imp` (currently stubbed) — or explicitly drop Windows from the v0.7.0 support matrix | platform | L | rpc |
| 9 | Audit `IdeToolProvider` dispatcher to confirm actual side-effects vs. tool schema; add integration tests covering at least `ide_open_file` and `ide_diagnostics` | testing | M | mcp/ide |
| 10 | Add a version-mismatch path to `McpSession::connect` so a foreign `protocolVersion` is logged or rejected, not silently accepted | robustness | S | mcp/mod |
| 11 | Move `agent-core-chat` flag removal onto a deletion roadmap; once D3 has soaked for one minor version, remove the spike pathway | cleanup | S | clawft-weave |
| 12 | Document the `SkillToolProvider` "tools/call returns SKILL.md instructions" contract in user-facing MCP docs (today only inferable from the closure body) | docs | S | docs/skills |
| 13 | Add a CI smoke that connects to a stdio MCP server (`weft mcp-server`) and round-trips `tools/list` + a `tools/call` against a fixed builtin (e.g. echo) | CI | S | mcp_server |
| 14 | Audit `McpServerManager` -> live-reload integration; either land the file-watcher wire-up or remove the hot-reload affordances from the public API | feature gap | M | mcp/discovery |
| 15 | Treat `[kernel.ipc_tcp]` relay as a security audit point: explicit auth + bind-address default of `127.0.0.1` only; document it in the security review | security | S | clawft-weave |
| 16 | Add an end-to-end smoke for the VSCode extension (build wasm, compile ts, install in a headless VSCode test host, open `WeftOS: Open Panel`, assert chip icons go green) | testing | L | extensions |
| 17 | Promote `weft-gui-egui` native bin to a `scripts/build.sh native --gui` flag; ship the `.deb`/`.dmg` artifact alongside `weft`/`weaver` | release | M | scripts/release |
| 18 | Decide whether wire types (`AgentChatParams`/`AgentChatResult`) belong in `clawft-types` so `clawft-weave` does not have to import service crates for serde | refactor | S | types |

## Sources

Inspected files (absolute paths):

- `/home/aepod/dev/clawft/crates/clawft-rpc/src/lib.rs`
- `/home/aepod/dev/clawft/crates/clawft-rpc/src/protocol.rs`
- `/home/aepod/dev/clawft/crates/clawft-rpc/src/client.rs`
- `/home/aepod/dev/clawft/crates/clawft-rpc/src/version_check.rs`
- `/home/aepod/dev/clawft/crates/clawft-weave/src/daemon.rs` (4814 lines; dispatch table at L2949–4805)
- `/home/aepod/dev/clawft/crates/clawft-services/src/mcp/mod.rs`
- `/home/aepod/dev/clawft/crates/clawft-services/src/mcp/server.rs`
- `/home/aepod/dev/clawft/crates/clawft-services/src/mcp/middleware.rs`
- `/home/aepod/dev/clawft/crates/clawft-services/src/mcp/discovery.rs`
- `/home/aepod/dev/clawft/crates/clawft-services/src/mcp/ide.rs`
- `/home/aepod/dev/clawft/crates/clawft-services/src/mcp/composite.rs`
- `/home/aepod/dev/clawft/crates/clawft-services/src/mcp/transport.rs`
- `/home/aepod/dev/clawft/crates/clawft-services/src/mcp/provider.rs`
- `/home/aepod/dev/clawft/crates/clawft-services/src/mcp/client.rs`
- `/home/aepod/dev/clawft/crates/clawft-services/src/mcp/bridge.rs`
- `/home/aepod/dev/clawft/crates/clawft-services/src/mcp/types.rs`
- `/home/aepod/dev/clawft/crates/clawft-cli/src/commands/mcp_server.rs`
- `/home/aepod/dev/clawft/crates/clawft-cli/src/mcp_tools.rs`
- `/home/aepod/dev/clawft/crates/clawft-cli/src/commands/tools_cmd.rs`
- `/home/aepod/dev/clawft/crates/clawft-core/src/agent/skills.rs` (claude-flow as sample skill)
- `/home/aepod/dev/clawft/crates/clawft-types/src/config/mod.rs` (`MCPServerConfig`, L466–499)
- `/home/aepod/dev/clawft/crates/clawft-types/src/delegation.rs` (`claude_flow_enabled`)
- `/home/aepod/dev/clawft/crates/clawft-types/src/security.rs` (default allowlist `claude-flow`)
- `/home/aepod/dev/clawft/extensions/vscode-weft-panel/src/extension.ts` (allowlist L39–93)
- `/home/aepod/dev/clawft/extensions/vscode-weft-panel/src/rpc.ts`
- `/home/aepod/dev/clawft/extensions/vscode-weft-panel/SMOKE.md`
- `/home/aepod/dev/clawft/extensions/vscode-weft-panel/scripts/build-wasm.sh`
- `/home/aepod/dev/clawft/extensions/vscode-weft-panel/package.json`
- `/home/aepod/dev/clawft/.planning/explorer/PROJECT-PLAN.md` (allowlist incident, Phase 0/1)
- `/home/aepod/dev/clawft/docs/handoff.md` (recent session context, build commands, deferred items)
- `/home/aepod/dev/clawft/docs/adr/adr-035-serviceapi-layered-protocol.md`
- `/home/aepod/dev/clawft/docs/adr/adr-036-hierarchical-tool-registry.md`
- `/home/aepod/dev/clawft/docs/adr/adr-042-three-operating-modes.md`
- `/home/aepod/dev/clawft/docs/plans/chat-agent-v1.md` (referenced; agent.chat plan)
- `/home/aepod/dev/clawft/.planning/symposiums/compositional-ui/adrs/adr-018-ide-bridge-protocol.md` (referenced, not exhaustively read)

Method-name evidence: `grep '^\s*"[a-z][a-z_]*\.[a-z_.-]\+"\s*=>'` against
`crates/clawft-weave/src/daemon.rs` returned ~75 dispatch arms between L2950 and
L4801; the audit listed every namespace prefix above.

Allowlist evidence: `extensions/vscode-weft-panel/src/extension.ts:39-93` is the
canonical `ALLOWED_METHODS` set; comment block per-entry documents the
2026-04-23 incident chronology and the M1.5.x additions.

Memory files referenced in the original task brief
(`claude-flow-integration.md`, `feedback_extension_rpc_allowlist.md`,
`feedback_rebuild_webview_wasm.md`) were not present at any path the audit
reached under `/home/aepod/dev/clawft/`; the corresponding context lives in
`docs/handoff.md`, `.planning/explorer/PROJECT-PLAN.md`, the per-line comment
blocks in `extension.ts`, and the build-wasm script header.

<!-- TRIAGED-STAMP:BEGIN -->
## Triaged into Plane — 2026-04-28

All open items in this audit have been filed as Plane work items in the WeftOS workspace under the `ws15-mcp` label.

- **Range**: WEFT-478 … WEFT-501 (24 items)
- **Per cycle**: 0.7.x: 15, 0.8.x: 9
- **Triage spec**: `.planning/reviews/0.7.0-release-gate/triage/`
- **WEFT-N → name map**: `.planning/reviews/0.7.0-release-gate/triage/weft-mapping.json`

Per the project rule (CLAUDE.md → "Plane is the authoritative work tracker"): future updates to these items happen in Plane, not in this audit doc. This doc remains the source-of-truth for the original survey.
<!-- TRIAGED-STAMP:END -->
