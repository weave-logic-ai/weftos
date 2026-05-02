---
title: "Multi-agent Routing & Delegation"
slug: multi-agent-routing
workstream_id: "07"
release: "0.7.0"
audit_kind: comprehensive
last_updated: 2026-04-28
status: in-progress
owners: ["agent-09"]
---

# Multi-agent Routing & Delegation

## General Description

Home of every delegation and multi-agent orchestration subsystem in clawft.
Four overlapping pieces grew out of SPARC Element 09:

- **Auto-delegation short-circuit.** `AutoDelegation` trait
  (`clawft-core/src/agent/loop_core.rs:62-272, 469-854`) lets the agent loop
  bypass the local LLM and invoke `delegate_task` directly when an inbound
  message matches a delegation rule. CLI wires `AutoDelegationRouter` over
  `DelegationEngine` (`crates/clawft-cli/src/commands/agent.rs:474-505`).
- **DelegationEngine + Claude bridge.** `DelegationEngine`
  (`clawft-services/src/delegation/mod.rs`) does regex rule + complexity
  heuristic routing into `DelegationTarget` (`Local`, `Claude`, `Flow`,
  `Auto`). `ClaudeDelegator` (`delegation/claude.rs`, 596 lines) drives the
  Anthropic Messages API tool loop. `DelegateTaskTool`
  (`clawft-tools/src/delegate_tool.rs`) is the LLM-callable tool surface.
- **Agent routing + bus + swarm (L1/L2/L3).** Types in
  `clawft-types/src/{agent_routing.rs,agent_bus.rs}`; runtime in
  `clawft-core/src/{agent_routing.rs,agent_bus.rs}`. `AgentRouter` maps
  `(channel, sender_id, chat_id, phone)` to an agent ID with first-match
  semantics and a catch-all. `AgentBus` and `SwarmCoordinator` give bounded
  per-agent inboxes and fan-out/collect.
- **Dynamic MCP discovery + bridge + planning (M4/M5/L4).**
  `clawft-services/src/mcp/{discovery.rs,bridge.rs}` and
  `clawft-core/src/planning.rs` (`PlanningRouter`, `PlanningStrategy`,
  `TerminationReason`).

The element-09 tracker
(`.planning/sparc/phase4/09-multi-agent-routing/04-element-09-tracker.md`)
declares the workstream "Complete (14/14, 100%)". This audit confirms the
type-level scaffolding is in place and well tested, and flags substantial
unfinished runtime work on FlowDelegator, MCP drain-and-swap, the bridge's
real MCP connection, planning loops, L2 workspace isolation, and integration
of `AgentRouter` / `AgentBus` into the live pipeline.

## Status & Timeline

| Phase | Doc | Tracker | Reality |
|---|---|---|---|
| M-Foundation (W3-5) | `01-phase-MFoundation-flow-delegator.md` | Done | Partial. `FlowDelegator` spec'd in detail but `crates/clawft-services/src/delegation/flow.rs` does **not** exist. `flow_available` was removed: `delegate_tool.rs` calls `engine.decide(task, claude_available)` only; `resolve_availability` now collapses `Flow` -> `Claude` ("Flow delegation removed", `delegation/mod.rs:185`). `claude_enabled` default `true`; `delegate` in CLI defaults. |
| L-Routing (W5-7) + L3 (W7-8) | `02-phase-LRouting-agents-swarming.md` | Done | Types + unit tests done (~25 tests). **L2 per-agent runtime, MCP override, and message-bus integration are not wired.** `MessageBus::consume_inbound` does not call `AgentRouter::route`; routing is type-level only. |
| M-Advanced (W6-8) + L4 (W8-9) | `03-phase-MAdvanced-mcp-planning.md` | Done | Skeletal. `McpServerManager::remove_server` does not implement drain-and-swap (`discovery.rs:138`: "in a full implementation, in-flight calls would be completed"). `McpBridge` is a state machine with no real MCP client connection. `PlanningRouter` has `check_guard_rails` and `explain_termination` but **no `execute()`, no ReAct, no Plan-and-Execute**. |
| Docs (M6) | tracker | Done | `docs/guides/configuration.md` and `docs/guides/tool-calls.md` exist. Bridge docs reference `claude mcp serve` flow that is not yet runtime-connected. |

## Released Features

Verified in source for 0.7.0:

- **`delegate_task` tool, `feature = "delegate"`**, registered when the
  context has a `ClaudeDelegator` + `DelegationEngine`
  (`crates/clawft-cli/src/mcp_tools.rs:275-360`). Returns
  `{"status":"local",...}` or `{"status":"delegated","target":"claude",...}`.
- **`DelegationEngine`** with regex rules, complexity heuristic
  (`COMPLEXITY_KEYWORDS`, length/qmark factors), Claude-unavailable
  fallback, and `delegate_tool_call` / `DelegationResult` for
  kernel-A2A IPC dispatch (`delegation/mod.rs:201-263`).
- **`AutoDelegation` trait + `AutoDelegationRouter`** short-circuiting the
  local LLM (`agent/loop_core.rs:62-272, 469-854`). Wired in
  `commands/agent.rs:118-124` behind `cfg(feature = "delegate")`.
- **`AgentRoute`, `MatchCriteria`, `AgentRoutingConfig`** in
  `clawft-types/src/agent_routing.rs` (200 lines, 7 tests).
  **`AgentRouter`, `RoutingResult`** in
  `clawft-core/src/agent_routing.rs` (279 lines, 10 tests). Stored on
  `AgentContext` (`bootstrap.rs:89, 341-355`) but **not consulted** by
  inbound dispatch.
- **`InterAgentMessage`, `MessagePayload`, `AgentBusError`** in
  `clawft-types/src/agent_bus.rs` (304 lines). **`AgentBus`,
  `SwarmCoordinator`** in `clawft-core/src/agent_bus.rs` (462 lines, 8+
  tests). Bounded inboxes, TTL, `dispatch_subtask`, `broadcast_task`.
- **`PlanningStrategy`, `PlanningConfig`, `PlanningRouter`,
  `TerminationReason`** in `clawft-core/src/planning.rs` (465 lines).
  Guard-rail enforcement and human-readable `explain_termination` only.
- **`McpServerManager`, `McpServerConfig`, `ServerStatus`**
  (`mcp/discovery.rs`, 385 lines): in-memory map + `apply_config_diff`
  returning `(added, removed, changed)` counts.
- **`McpBridge`, `BridgeConfig`, `BridgeStatus`** (`mcp/bridge.rs`, 378
  lines): state-machine wrapper + `mcp:<namespace>:<tool>` helper.
- **`weft mcp-server`** outbound bridge (Phase 3H) with `McpServerShell`
  on stdio; `claude mcp add clawft -- weft mcp-server` documented.
- **Documentation**: `docs/guides/configuration.md` "Delegation &
  Multi-Agent" section and `docs/guides/tool-calls.md` "MCP Bridge Setup".
- **Extended `DelegationError`** variants `Timeout`, `Cancelled`,
  `FallbackExhausted` shipped in `delegation/claude.rs:63-72` even though
  the FlowDelegator that would have produced them did not.

## What's Left -- Total Depth

### TODOs / FIXMEs / inline placeholders

| Location | Note | Severity |
|---|---|---|
| `mcp/discovery.rs:138-139` | `remove_server` doc: "Marks the server as draining. **In a full implementation, in-flight calls would be completed before disconnection.**" Code only sets `Draining` and removes from map -- no `AtomicU32`, no `InFlightGuard`, no 30s wait, no transport drop. | High |
| `mcp/bridge.rs:148-153` | `initialize()` doc: "Full MCP client connection depends on Element 07/F9a... Until then, this method sets up the configuration and marks the bridge as ready." No subprocess spawn, no MCP handshake, no `tools/list`, no `claude mcp serve`. | High |
| `clawft-core/src/planning.rs` | No `execute()`, no `execute_react`, no `execute_plan_and_execute`. M-Advanced 2.3.5 even leaves `todo!("implementation requires LLM integration")`. | High |
| `delegation/mod.rs:185-187` | `// Flow delegation removed -- treat as Claude fallback.` Whole M1 FlowDelegator track was deleted/skipped without removing the `Flow` variant or `claude_flow_enabled` field. | Medium |
| `mcp/discovery.rs` | No `connect_server`, `disconnect_server`, or `call_tool` on the manager. `ManagedMcpServer.tools` populated only via `mark_connected(name, tools)` from outside. | High |
| `mcp/ide.rs:203` | `IdeToolProvider::stub()` is the only constructor; full IDE provider not implemented. Adjacent (multi-agent tool surface). | Medium |
| `agent/context_router/hybrid.rs:44`, `agent/context_router.rs:206` | `// TODO(agent-core-v1 phase E3+): wire MicroLoraRouter (v3) once...` Micro-lora hybrid context-routing remains. Delegation-adjacent. | Low |
| `clawft-tools/src/delegate_tool.rs:103` | `let claude_available = true; // We have a delegator.` Hardcoded -- no degraded mode if delegator is later non-functional. | Low |
| `.planning/development_notes/09-multi-agent-routing/phase-{M-foundation,L-routing,M-advanced}/{notes,blockers,decisions,difficult-tasks}.md` | All four files in all three phase dirs are placeholders (`_No notes recorded yet._`). The skip rationale for FlowDelegator and bridge no-op is undocumented. | Medium |

### Deferred / orphaned items (not 0.7.0 blockers)

1. **FlowDelegator (M1)** -- entire `delegation/flow.rs` per
   `01-phase-MFoundation-flow-delegator.md` not landed. Tracker says Done;
   file does not exist. Subprocess spawn, `env_clear()` isolation, depth
   threading via `CLAWFT_DELEGATION_DEPTH`, timeout + `child.kill()` --
   none implemented. The `claude_flow_enabled` field still exists, unused.
2. **`flow_available` runtime detection (M2)** -- `which::which("claude")`
   + `AtomicBool` cache on `DelegateTaskTool` not added; the `which` dep
   was not introduced. Detection was simply removed from the engine
   signature.
3. **Recursive delegation depth guard (live path)** --
   `CLAWFT_DELEGATION_DEPTH` not threaded; `MAX_DELEGATION_DEPTH = 3`
   from `bridge.rs` pseudocode not landed. Recursive
   `delegate_task` -> Claude -> MCP-bridge -> `delegate_task` is
   currently unbounded.
4. **L1 routing wired into inbound dispatch** -- `AgentRouter` lives on
   `AgentContext` but `MessageBus::consume_inbound` and
   `AgentLoop::run` never call `router.route(&msg)`. A single agent loop
   still processes every message. Per-user agent routing is not
   observable end-to-end.
5. **L2 per-agent workspace runtime** -- `AgentRuntime` (`agent_id`,
   `workspace_path`, agent-scoped `SessionManager`, `ContextBuilder`,
   `ToolRegistry`, `AgentsConfig`) was specified, not created. Per-agent
   MCP override (Contract 3.2) likewise. Depends on Element 08/H1
   `WorkspaceManager::ensure_agent_workspace`.
6. **Anonymous-agent permission reduction** -- spec'd
   `disable_write_tools()` + disabled delegation;
   `route_anonymous` only routes to catch-all without runtime tightening.
7. **AgentBus + SwarmCoordinator integration** -- types and tests are in;
   no production path constructs an `AgentBus` or runs a
   `worker_message_loop`. `SwarmCoordinator::with_capacity` is unused.
8. **PlanningRouter execute loop** -- `execute_react`,
   `execute_plan_and_execute` unimplemented. `[router.planning]` config
   parses but the planner doesn't run. No-op detection, per-step
   timeout, partial-result return, and D6 `sender_id` cost tracking
   missing.
9. **MCP discovery drain-and-swap actual implementation** -- `inflight:
   AtomicU32`, `InFlightGuard`, 30s wait, transport drop, `call_tool`
   routing through manager: all missing.
10. **MCP discovery transport factory** -- stdio vs HTTP factory,
    `validate_mcp_url` (SSRF), `validate_command_path` (shell
    metachar), `tempfile`-with-`0600` for MCP temp files (M-Advanced
    5.4) not present.
11. **Hot-reload file watcher** -- `notify`-crate watcher on
    `clawft.toml` `[tools.mcp_servers]` with 500ms debounce not wired;
    `apply_config_diff` exists but no caller invokes it on file change.
12. **`weft mcp add/list/remove` CLI** -- `commands/mcp_cmd.rs` not
    added (existing `mcp_server.rs` is the outbound shell, not the
    discovery CLI).
13. **`McpBridge` real connection (M5)** -- `set_inbound_connected()` /
    `set_outbound_connected()` are setters called by tests only. No
    spawn of `claude mcp serve`, no initialize handshake, no proxying.
    `call_claude_tool_with_depth` does not exist.
14. **MCP server-mode access control (3H CRIT-01)** -- `weft mcp-server`
    `tools/call` dispatches directly to `tool_registry.execute` with no
    `allowed_tools`, `CommandPolicy`, or `UrlPolicy`. Unresolved.
15. **Tool-execution helper extraction (3H CRIT-02)** --
    `agent/loop_core.rs::run_tool_loop` truncation/error/logging not
    shared with `ClaudeDelegator`'s loop. `MAX_TOOL_RESULT_BYTES`
    truncation is not applied to results sent back to Anthropic.
16. **Auto-delegation classification accuracy (3H MIN-02)** --
    regex+keyword classifier is fragile. No follow-up bead.
17. **`notifications/tools/list_changed` from external MCP servers** --
    advertise `tools.listChanged: false`; not handled either way.
18. **MCP session lifecycle (3H MAJ-03)** -- no keepalive/ping, no
    reconnection, no `is_alive()` on `StdioTransport`, no graceful
    `notifications/cancelled`. Long gateway sessions cannot recover
    from a crashed child server.
19. **`JsonRpcRequest.id` typed `u64` (3H MIN-01)** -- string-id MCP
    clients cannot interop with `weft mcp-server`. No follow-up.
20. **Per-agent MCP-server config override (Contract 3.2)** -- not
    implemented; needs L2 runtime first.
21. **Cost-tracking events from L4 planning** --
    `track_step_cost(sender_id, step, model)` per M-Advanced 2.3.7 not
    implemented.
22. **`weft delegate` debug subcommand** -- not in the SPARC plan but a
    common ask (manually exercise the engine to see which target a
    free-text task would route to).
23. **mesh / hierarchical / adaptive coordinator agents** -- referenced
    in `CLAUDE.md` but only exist as claude-flow swarm prompts.
    `SwarmCoordinator` is a flat fan-out/collect; no topology axis.

### Open questions

- Should `Flow` `DelegationTarget` and `claude_flow_enabled` be retired
  (since Flow now silently re-routes to Claude), or kept for a future
  MCP-only "flow" path? Migration story for users with
  `claude_flow_enabled = true`?
- Where does `AgentRouter` plug into the dispatch graph? Multiple
  `AgentLoop` instances per agent, or one loop with per-message
  agent-scoped runtimes? SPARC implies the former; bootstrap supports
  neither.
- Should auto-delegation depth thread via env (`CLAWFT_DELEGATION_DEPTH`)
  per the M1 spec, or via tool-call metadata / a session field? Env-var
  approach is hostile to WASM.
- Does L4 planning live inside the agent loop, alongside it, or as a
  fourth `DelegationTarget::Plan`? Code commits to none of the three.
- Is `SwarmCoordinator` agent-loop-aware? Without L2 runtimes there is
  nowhere to host a `worker_message_loop`.
- Does the bidirectional MCP bridge unify clawft + Claude Code tools
  into one `ToolRegistry` with prefixes, or keep them logically
  separate? Namespace helper exists; registry merge does not.
- Should auto-delegation be on by default once `delegate` is in default
  features? Current path needs `delegate` feature AND `claude_enabled`
  AND `tools.has("delegate_task")`; discoverability is poor and there
  is no `weft doctor` check.

### Dead-code risk

- `claude_flow_enabled` on `DelegationConfig` is read nowhere except
  tests.
- `DelegationResult::to_ipc_message` (kernel A2A IPC dispatch) at
  `delegation/mod.rs:254-262` has no in-tree caller.
- `AgentBus::with_capacity` defined but no caller picks a non-default.
- All twelve `phase-*/{notes,blockers,decisions,difficult-tasks}.md`
  files are empty placeholders -- delete or backfill rationale.
- `delegation_config_from_empty_json` test
  (`clawft-types/src/delegation.rs:163-173`) asserts
  `claude_enabled == false` for `{}` because `serde(default)` uses the
  field default (`bool::default()` = false), not
  `Default::default()` (true). The two tests are technically consistent
  but the divergence is a documented footgun for users.

## Task List

Every item is non-blocking for 0.7.0 unless flagged.

- [ ] **CRIT-01 (3H)**: `allowed_tools` config + `CommandPolicy` /
      `UrlPolicy` enforcement on `weft mcp-server` `tools/call`.
- [ ] **CRIT-02 (3H)**: Extract a shared `execute_tool_with_guards` from
      `run_tool_loop`; reuse from `ClaudeDelegator::delegate`. Apply
      `MAX_TOOL_RESULT_BYTES` truncation in the delegation path.
- [ ] Decide fate of `Flow` target / `claude_flow_enabled`: land
      FlowDelegator or remove the dead config; document the choice.
- [ ] If keeping FlowDelegator: implement
      `clawft-services/src/delegation/flow.rs` per the M1 spec.
- [ ] Wire `AgentRouter` into inbound dispatch in `bus.rs` /
      `loop_core.rs`. Either dispatch to per-agent runtimes or carry
      routed `agent_id` through the pipeline.
- [ ] Land `AgentRuntime` (L2): per-agent `SessionManager`,
      `ContextBuilder`, `ToolRegistry`, `AgentsConfig`. Lazy
      `ensure_agent_workspace` on first message.
- [ ] Implement anonymous-agent permission tightening (no write tools,
      no delegation, no MCP write).
- [ ] Per-agent MCP config override (Contract 3.2): merge global +
      per-agent; drop disabled servers from per-agent registry.
- [ ] Spawn worker agent loops over `AgentBus` inboxes; integrate
      `SwarmCoordinator` with at least one demo coordinator workflow.
- [ ] Implement `PlanningRouter::execute_react` and
      `execute_plan_and_execute` (no-op detection,
      `execute_step_with_timeout`, partial-result on every termination
      path).
- [ ] Wire `[router.planning]` config to either the agent loop or a
      new `DelegationTarget::Plan` variant; pick one.
- [ ] Emit cost-tracking events per planning step keyed on `sender_id`
      (D6 contract).
- [ ] Replace `McpServerManager::remove_server` no-op with real
      drain-and-swap (`AtomicU32`, `InFlightGuard`, 30s loop, transport
      drop). Route `call_tool` through the manager.
- [ ] Implement transport factory (`stdio` vs `http`) with
      `validate_mcp_url`, `validate_command_path`, `tempfile`-with-`0600`.
- [ ] Wire `notify` watcher on `[tools.mcp_servers]` with 500ms
      debounce; call `apply_config_diff` on change.
- [ ] Add `weft mcp add/list/remove` (`commands/mcp_cmd.rs`).
- [ ] Real `McpBridge` connection: spawn `claude mcp serve`, do
      handshake, fetch `tools/list`, register under `mcp:claude-code:*`
      namespace, propagate `notifications/tools/list_changed`. Add
      depth-tracking `call_claude_tool_with_depth` and a
      `MAX_DELEGATION_DEPTH = 3` check.
- [ ] `is_alive()` on `StdioTransport`; reconnect-with-backoff in
      `McpSession`; ping/keepalive for long sessions; graceful
      `notifications/cancelled` on shutdown (3H MAJ-03).
- [ ] `JsonRpcRequest.id` / `JsonRpcResponse.id` -> `serde_json::Value`
      (3H MIN-01).
- [ ] Track init state in `weft mcp-server`; reject pre-handshake
      methods with -32002 (3H MIN-03).
- [ ] Update `TestTransport` in `clawft-cli/src/mcp_tools.rs` if
      `McpTransport` gains new required methods (3H MAJ-04).
- [ ] Resolve `CLAUDE.md` references to `mesh-/hierarchical-/adaptive-
      coordinator`: implement a topology axis on `SwarmCoordinator` or
      document them as claude-flow-prompt-only constructs.
- [ ] Backfill `phase-*/decisions.md` (FlowDelegator skip, router
      non-integration, bridge stub rationale).
- [ ] Reconcile `delegation_config_from_empty_json` vs
      `delegation_config_defaults` for `claude_enabled`, or document
      the serde-default vs `Default::default` divergence.
- [ ] `weft doctor`: claude binary on PATH? auto-delegation enabled?
      at least one agent route configured?
- [ ] Decide if `claude-flow` MCP server is added by default to
      `[tools.mcp_servers]` (skill manifest at
      `agent/skills.rs:702-728` references `claude-flow__swarm_*` /
      `claude-flow__agent_*` prefixes, but dynamic discovery is M4).

## Sources

- `crates/clawft-types/src/agent_routing.rs`,
  `crates/clawft-types/src/agent_bus.rs`,
  `crates/clawft-types/src/delegation.rs`
- `crates/clawft-core/src/agent_routing.rs`,
  `crates/clawft-core/src/agent_bus.rs`,
  `crates/clawft-core/src/planning.rs`
- `crates/clawft-core/src/agent/loop_core.rs:62-272, 469-854, 2606-2625`
- `crates/clawft-core/src/bootstrap.rs:81, 89, 185, 210-211, 337-355`
- `crates/clawft-services/src/delegation/{mod.rs,claude.rs,schema.rs}`
- `crates/clawft-services/src/mcp/{discovery.rs,bridge.rs,server.rs,
  transport.rs,ide.rs}`
- `crates/clawft-tools/src/delegate_tool.rs`
- `crates/clawft-cli/src/mcp_tools.rs:275-360`
- `crates/clawft-cli/src/commands/agent.rs:36, 115-124, 472-505,
  574-650`
- `crates/clawft-cli/src/commands/tools_cmd.rs:210-213, 528-535`
- `.planning/sparc/phase4/09-multi-agent-routing/00-orchestrator.md`
- `.planning/sparc/phase4/09-multi-agent-routing/01-phase-MFoundation-flow-delegator.md`
- `.planning/sparc/phase4/09-multi-agent-routing/02-phase-LRouting-agents-swarming.md`
- `.planning/sparc/phase4/09-multi-agent-routing/03-phase-MAdvanced-mcp-planning.md`
- `.planning/sparc/phase4/09-multi-agent-routing/04-element-09-tracker.md`
- `.planning/development_notes/09-multi-agent-routing/{phase-M-foundation,
  phase-L-routing,phase-M-advanced}/{notes,blockers,decisions,
  difficult-tasks}.md` (all empty)
- `.planning/reviews/3h-review.md` (CRIT-01, CRIT-02, MAJ-01..MAJ-04,
  MIN-01..MIN-05, MR-01..MR-06)
- `.planning/reviews/3i-review.md`, `.planning/reviews/consensus.md`
- `docs/guides/{configuration.md,tool-calls.md,testing-mcp-delegation.md}`
- `CLAUDE.md` (mesh/hierarchical/adaptive coordinator references)
- `crates/clawft-core/src/agent/skills.rs:702-728` (`claude-flow` skill
  manifest)

<!-- TRIAGED-STAMP:BEGIN -->
## Triaged into Plane — 2026-04-28

All open items in this audit have been filed as Plane work items in the WeftOS workspace under the `ws07-multi-agent` label.

- **Range**: WEFT-178 … WEFT-204 (27 items)
- **Per cycle**: 0.7.x: 11, 0.8.x: 13, 0.9.x: 3
- **Triage spec**: `.planning/reviews/0.7.0-release-gate/triage/`
- **WEFT-N → name map**: `.planning/reviews/0.7.0-release-gate/triage/weft-mapping.json`

Per the project rule (CLAUDE.md → "Plane is the authoritative work tracker"): future updates to these items happen in Plane, not in this audit doc. This doc remains the source-of-truth for the original survey.
<!-- TRIAGED-STAMP:END -->
