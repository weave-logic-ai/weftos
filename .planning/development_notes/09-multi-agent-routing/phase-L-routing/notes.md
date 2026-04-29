# Development Notes

## 2026-04-28 — Backfill (WEFT-202)

The L-Routing phase tracker reported all three rows (L1 routing, L2 isolation,
L3 swarming) as Done, but the 0.7.0 release-gate audit found that L1's wiring
into `MessageBus::consume_inbound` / `AgentLoop::run` was never landed. The
router is constructed and stored on `AgentContext` but never consulted. See
`decisions.md` D-LR-001 and the WEFT-178 carry-forward.

What *did* ship at type-level in this phase:

- `AgentRoute`, `MatchCriteria`, `AgentRoutingConfig` types in `clawft-types`.
- `AgentRouter` with first-match-wins, catch-all, anonymous routing.
- `InterAgentMessage`, `MessagePayload` (Text / Structured / Binary).
- `AgentBus` with per-agent inboxes, bounded channels, TTL enforcement.
- `AgentInbox` agent-scoped delivery (security: no cross-agent reads).
- `SwarmCoordinator` with `dispatch_subtask` and `broadcast_task`.

What is **not yet wired** (tracked elsewhere):

- L1 wiring into `MessageBus::consume_inbound` / `AgentLoop::run`
  (WEFT-178).
- L2 per-agent runtime (`SessionManager`, `ContextBuilder`, `ToolRegistry`,
  `AgentsConfig`) — depends on H1 from element-08 (memory & workspace).
