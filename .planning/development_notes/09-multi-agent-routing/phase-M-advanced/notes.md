# Development Notes

## 2026-04-28 — Backfill (WEFT-202)

The M-Advanced phase tracker reported M4 (dynamic MCP discovery), M5
(bidirectional MCP bridge), and L4 (ReAct / Plan-and-Execute planning) as
Done. The 0.7.0 release-gate audit found that all three shipped only at
type-level; the runtime work is open. See `decisions.md` D-MA-001 for the
recorded rationale and the WEFT-181 / WEFT-182 / WEFT-183 carry-forwards.

What *did* ship at type-level in this phase:

- `McpServerManager` with `add/remove/list/get` and `apply_config_diff`
  (debounced 500ms, 30s drain timeout — *constants only*, no in-flight
  enforcement yet).
- `ServerStatus` enum (`Connected`, `Connecting`, `Draining`,
  `Disconnected`, `Error`).
- `McpBridge` with `BridgeConfig`, `BridgeStatus`, namespace helper
  (`mcp:<namespace>:<tool-name>`), inbound/outbound flag setters.
- `PlanningStrategy` enum (`React`, `PlanAndExecute`).
- `PlanningConfig` (`max_depth=10`, `max_cost=$1.0`, `step_timeout=60s`).
- `PlanningRouter::check_guard_rails()` + `explain_termination()` returning
  `TerminationReason`.

What is **not yet wired** (tracked elsewhere):

- McpBridge real `claude mcp serve` subprocess spawn, MCP handshake,
  `tools/list` fetch, `notifications/tools/list_changed` propagation
  (WEFT-181).
- `McpServerManager::remove_server` real drain-and-swap with `AtomicU32`
  in-flight counter, `InFlightGuard`, `call_tool` routing through the
  manager, 30s force-drop (WEFT-182).
- `PlanningRouter::execute_react` / `execute_plan_and_execute` with no-op
  detection, per-step timeout, partial-result return on every termination
  path, D6 `sender_id` cost events (WEFT-183).
- `call_claude_tool_with_depth` end-to-end (depends on WEFT-180 +
  WEFT-181).
