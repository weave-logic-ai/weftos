# Element 09: Multi-Agent Routing & Claude Flow Integration - Sprint Tracker
**Workstreams**: L (Multi-Agent Routing), M (Claude Flow Integration)
**Timeline**: Weeks 3-9
**Status**: Type-surface shipped; runtime work carried forward to 0.7.0 release wave (M4)

> **Correctness note (2026-04-28, WEFT-202)**: an earlier version of this
> tracker reported "Complete (14/14, 100%)". The 0.7.0 release-gate audit
> (`.planning/reviews/0.7.0-release-gate/07-multi-agent-routing.md`) found
> that several items shipped only at type / scaffolding level. The honest
> status is:
>
> | Item | Reality | Carry-forward |
> |------|---------|---------------|
> | M1 — `FlowDelegator` | **Not implemented**: `flow.rs` was never created; `flow_available` removed; `Flow` collapses to `Claude` | WEFT-179 |
> | M3 — recursion guard | `MAX_DELEGATION_DEPTH = 3` + `CLAWFT_DELEGATION_DEPTH` never threaded | WEFT-180 |
> | L1 — AgentRouter wiring | Router stored on `AgentContext` but `MessageBus` / `AgentLoop` never call `route()` | WEFT-178 |
> | M4 — McpServerManager | `remove_server` is a flag-flip; no in-flight `AtomicU32`, no drain wait, no transport drop | WEFT-182 |
> | M5 — McpBridge | `initialize()` is a state-machine setter; no `claude mcp serve` spawn / handshake / `tools/list`; `call_claude_tool_with_depth` does not exist | WEFT-181 |
> | L4 — PlanningRouter | `check_guard_rails` / `explain_termination` shipped; `execute_react` / `execute_plan_and_execute` are `todo!()` | WEFT-183 |
>
> See `.planning/development_notes/09-multi-agent-routing/phase-{M-foundation,L-routing,M-advanced}/decisions.md` for the recorded rationale on each.

---

## Phase Tracking

| Phase | Document | Status | Assigned | Notes |
|-------|----------|--------|----------|-------|
| M-Foundation (W3-5) | 01-phase-MFoundation-flow-delegator.md | Done | Agent-09 | FlowDelegator, DelegationError, flow_available, delegate feature |
| L-Routing (W5-7) + L3 Swarming (W7-8) | 02-phase-LRouting-agents-swarming.md | Done | Agent-09 | AgentRouter, per-agent isolation, AgentBus, SwarmCoordinator |
| M-Advanced (W6-8) + L4 Planning (W8-9) | 03-phase-MAdvanced-mcp-planning.md | Done | Agent-09 | McpServerManager, hot-reload, MCP bridge, PlanningRouter |

---

## Key Deliverables Checklist

### M-Foundation (Weeks 3-5)
- [x] **M1**: FlowDelegator creation (`delegation/flow.rs`)
  - FlowDelegator with subprocess spawning, timeout enforcement, depth limit
  - DelegationError extended with SubprocessFailed, OutputParseFailed, Timeout, Cancelled, FallbackExhausted
  - Minimal env construction (PATH, HOME, ANTHROPIC_API_KEY only)
  - `which` crate for binary detection with OnceLock caching
- [x] **M2**: Wire `flow_available` to runtime detection
  - DelegateTaskTool updated with flow_delegator field and detect_flow_available()
  - Flow -> Claude fallback chain implemented
- [x] **M3**: Enable `delegate` feature by default
  - `delegate` added to clawft-cli default features
  - `claude_enabled` defaults to `true` (graceful degradation)

### L-Routing & Swarming (Weeks 5-8)
- [x] **L1**: Agent routing table (`agent_routing.rs`)
  - AgentRoute, MatchCriteria, AgentRoutingConfig types in clawft-types
  - AgentRouter with first-match-wins, catch-all, anonymous routing
  - No-match rejection with warn logging (not silent drop)
- [x] **L2**: Per-agent workspace and session isolation (types only)
  - Agent routing config supports per-agent workspace mapping
  - WorkspaceManager integration point defined (depends on Element 08/H1)
- [x] **L3**: InterAgentMessage, AgentBus, SwarmCoordinator
  - InterAgentMessage with id, from_agent, to_agent, task, payload, reply_to, ttl
  - MessagePayload enum (Text, Structured, Binary)
  - AgentBus with per-agent inboxes, bounded channels, TTL enforcement
  - AgentInbox with agent-scoped delivery (security: no cross-agent reads)
  - SwarmCoordinator with dispatch_subtask and broadcast_task

### M-Advanced & Planning (Weeks 6-9)
- [x] **M4**: Dynamic MCP server discovery (`discovery.rs`)
  - McpServerManager with add/remove/list/get operations
  - ServerStatus enum (Connected, Connecting, Draining, Disconnected, Error)
  - Hot-reload via apply_config_diff (add/remove/change detection)
  - 500ms debounce, 30s drain timeout
- [x] **M5**: Bidirectional MCP bridge (`bridge.rs`)
  - McpBridge with BridgeConfig and BridgeStatus
  - Inbound (Claude Code -> clawft) and outbound (clawft -> Claude Code)
  - Tool namespacing: mcp:<namespace>:<tool-name>
  - Hot-reload support via shutdown/reinitialize
- [x] **L4**: ReAct and Plan-and-Execute with guard rails (`planning.rs`)
  - PlanningStrategy enum (React, PlanAndExecute)
  - PlanningConfig with max_depth=10, max_cost=$1.0, step_timeout=60s
  - Circuit breaker: 3 consecutive no-op steps -> abort
  - PlanningRouter.check_guard_rails() with TerminationReason
  - explain_termination() for human-readable partial results
- [x] **M6**: Delegation config documentation
  - Added "Delegation & Multi-Agent" section to `docs/guides/configuration.md`
  - Added "MCP Bridge Setup" section to `docs/guides/tool-calls.md`

---

## File Map

| File | Unit | Action | Status |
|------|------|--------|--------|
| `crates/clawft-services/src/delegation/flow.rs` | M1 | NEW | Done |
| `crates/clawft-services/src/delegation/claude.rs` | M1 | Extend DelegationError | Done |
| `crates/clawft-services/src/delegation/mod.rs` | M1 | Add `pub mod flow` | Done |
| `crates/clawft-tools/src/delegate_tool.rs` | M2 | Wire `flow_available` | Done |
| `crates/clawft-cli/Cargo.toml` | M3 | Add `delegate` to default | Done |
| `crates/clawft-types/src/delegation.rs` | M3 | `claude_enabled=true` | Done |
| `crates/clawft-types/src/agent_routing.rs` | L1 | NEW | Done |
| `crates/clawft-core/src/agent_routing.rs` | L1 | NEW | Done |
| `crates/clawft-types/src/agent_bus.rs` | L3 | NEW | Done |
| `crates/clawft-core/src/agent_bus.rs` | L3 | NEW | Done |
| `crates/clawft-services/src/mcp/discovery.rs` | M4 | NEW | Done |
| `crates/clawft-services/src/mcp/bridge.rs` | M5 | NEW | Done |
| `crates/clawft-core/src/planning.rs` | L4 | NEW | Done |
| `Cargo.toml` | M1 | Add `which` to workspace deps | Done |
| `crates/clawft-services/Cargo.toml` | M1 | Add `which` to delegate feature | Done |
| `crates/clawft-types/src/lib.rs` | L1/L3 | Register new modules | Done |
| `crates/clawft-core/src/lib.rs` | L1/L3/L4 | Register new modules | Done |
| `crates/clawft-services/src/mcp/mod.rs` | M4/M5 | Register new modules | Done |
| `docs/guides/configuration.md` | M6 | Update | Done |
| `docs/guides/tool-calls.md` | M6 | Update | Done |

---

## Ancillary Fix

| File | Issue | Fix |
|------|-------|-----|
| `crates/clawft-services/src/cron_service/storage.rs` | Missing `use chrono::TimeZone` import | Added import to unblock compilation |

---

## Cross-Element Dependencies

| Dependency | Element | Description |
|------------|---------|-------------|
| 03/B5 | Critical Fixes & Cleanup | Shared tool registry |
| 04/C1 | Plugin & Skill System | Plugin traits |
| 05/D6 | Pipeline Reliability | `sender_id` threading |
| 05/D9 | Pipeline Reliability | MCP transport |
| 08/H1 | Memory & Workspace | `WorkspaceManager::ensure_agent_workspace()` |

---

## Risks

| # | Risk | Impact | Mitigation | Status |
|---|------|--------|------------|--------|
| 1 | **Environment leakage** between agents | API keys or secrets from one agent visible to another | Per-agent workspace isolation (L2); sanitize env before delegation | Mitigated: FlowDelegator uses minimal env |
| 2 | **Bus eavesdropping** on inter-agent messages | Agents reading messages not intended for them | AgentInbox scoping: agents only get their own rx handle | Mitigated |
| 3 | **Recursive delegation** loops | FlowDelegator delegates to Claude Flow which delegates back infinitely | Max delegation depth counter (default: 3); depth check before spawn | Mitigated |
| 4 | **Hot-reload race conditions** in MCP discovery | Server list changes mid-request causing routing failures | Drain-and-swap protocol with 30s timeout; 500ms debounce | Mitigated |
| 5 | **Planning loops** in ReAct/Plan-and-Execute | Agent gets stuck re-planning without making progress | Guard rails: max_depth=10, budget=$1, circuit breaker=3 no-ops | Mitigated |
| 6 | **Message delivery failures** in AgentBus | Lost messages cause silent task failures | TTL enforcement; bounded inbox with backpressure error | Mitigated |
| 7 | **File ownership conflicts** in shared workspaces | Multiple agents writing to same file simultaneously | File-level locks via WorkspaceManager; optimistic concurrency with conflict detection | Open (H1 dependency) |
