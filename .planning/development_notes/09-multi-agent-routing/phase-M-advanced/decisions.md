# Architecture Decisions

## D-MA-001 — McpBridge ships as a documented stub (real connection deferred) — 2026-04-28

**Status**: Recorded retroactively (WEFT-202).

**Context**. The M-Advanced phase shipped the bridge type-state
(`crates/clawft-services/src/mcp/bridge.rs`: `McpBridge`, `BridgeConfig`,
`BridgeStatus`, namespace helper) and the discovery skeleton
(`crates/clawft-services/src/mcp/discovery.rs`: `McpServerManager` with
`add/remove/list/get` and `apply_config_diff`). The element-09 tracker
counted this as M4 + M5 Done. The 0.7.0 release-gate audit
(`.planning/reviews/0.7.0-release-gate/07-multi-agent-routing.md`,
TODO table for `mcp/bridge.rs:148-153` and `mcp/discovery.rs:138-139`) found:

1. `McpBridge::initialize` is a state-machine setter only — no subprocess spawn
   of `claude mcp serve`, no MCP `initialize` handshake, no `tools/list` fetch,
   no proxying. `set_inbound_connected()` and `set_outbound_connected()` are
   only called by tests. `call_claude_tool_with_depth` does not exist.
2. `McpServerManager::remove_server` only marks `Draining` and removes the
   entry from the map — no `AtomicU32` in-flight counter, no `InFlightGuard`,
   no 30s wait, no transport drop, no `call_tool` routing through the
   manager. The current behaviour can sever an in-flight tool call mid-stream.
3. `PlanningRouter` (L4) shipped `check_guard_rails` and `explain_termination`
   but no `execute()` loop — `[router.planning]` config parses but the planner
   never runs. A `todo!()` marker remains in M-Advanced 2.3.5.

**Decision**. The bridge and discovery skeletons are recognised as
**documented stubs** for the 0.7.0 cycle. They are kept (not removed) because:

- Other workstreams already depend on the type surface.
- Documentation references the stub as the integration point.
- Real connection work is gated on the recursion-guard
  (D-MF-002 / WEFT-180), which must land first.

The remaining M-Advanced work is split across three Plane items in the 0.7.x
cycle:

- **WEFT-181** — McpBridge real Claude Code connection (spawn, handshake,
  `tools/list`, namespace).
- **WEFT-182** — McpServerManager drain-and-swap on `remove_server`.
- **WEFT-183** — PlanningRouter `execute_react` and `execute_plan_and_execute`
  with no-op detection, per-step timeout, partial-result return, and the D6
  `sender_id` cost-tracking contract.

The element-09 tracker has been corrected so it no longer claims these are
shipped.

**Consequences**.
- **Positive**: The type-state lets adjacent work (CLI `weft mcp ...`, docs)
  compile and run against the stable surface.
- **Negative**: `claude mcp serve` integration documented in
  `docs/guides/tool-calls.md` is aspirational until WEFT-181 lands. The bridge
  stub doc-comment ("Until then, this method sets up the configuration and
  marks the bridge as ready") explicitly flags this.
- **Mitigation**: The "MCP Bridge Setup" doc section was updated in this
  backfill to call out the runtime gap; users wiring up an MCP peer today see
  the stub clearly labelled.

**Cross-references**.
- WEFT-181 — McpBridge real connection.
- WEFT-182 — McpServerManager drain-and-swap.
- WEFT-183 — PlanningRouter execute loops.
- WEFT-202 — this backfill.
- Audit: 07-multi-agent-routing.md TODO table + Deferred items #8, #9, #13.
- Source: `crates/clawft-services/src/mcp/bridge.rs:148-153` (stub note);
  `crates/clawft-services/src/mcp/discovery.rs:138-139` (stub note);
  `crates/clawft-core/src/planning.rs` (M-Advanced 2.3.5 `todo!()`).
