# Architecture Decisions

## D-LR-001 — AgentRouter integration with MessageBus deferred — 2026-04-28

**Status**: Recorded retroactively (WEFT-202).

**Context**. The L-Routing phase shipped the routing types and the router
itself (`AgentRoute`, `MatchCriteria`, `AgentRoutingConfig` in `clawft-types`;
`AgentRouter` in `clawft-core/src/agent_routing.rs`) plus the AgentBus and
SwarmCoordinator types. The element-09 tracker reported all three as Done and
counted the phase as 100% complete. The 0.7.0 release-gate audit
(`.planning/reviews/0.7.0-release-gate/07-multi-agent-routing.md` L-Routing
row + Deferred item #4) found that:

1. `AgentRouter` is constructed and stored on `AgentContext`
   (`crates/clawft-core/src/bootstrap.rs:89, 341-355`), but
2. neither `MessageBus::consume_inbound` nor
   `crates/clawft-core/src/agent/loop_core.rs::AgentLoop::run` ever calls
   `router.route(&msg)`; a single agent loop still processes every message and
   per-user agent routing is not observable end-to-end.

The L1 deliverable is feature-flagged on but inert.

**Decision**. Wiring `AgentRouter` into `MessageBus`/`AgentLoop` is tracked
under **WEFT-178** (AgentRouter dispatch into MessageBus) in the 0.7.0 release
wave; the work is gated on a still-open design question about
"multiple AgentLoops per agent" vs "one loop, per-message agent runtime"
(captured as an open question in the audit). The element-09 tracker has been
corrected so it no longer claims this is shipped.

**Consequences**.
- **Negative**: AgentBus / SwarmCoordinator type-level work shipped earlier
  than the routing wiring that gives those types runtime meaning. Until WEFT-178
  lands, multi-agent routing is type-only.
- **Mitigation**: Catch-all routing keeps the single-agent path working, so the
  release-gate is unaffected for the single-tenant default deployment.

**Cross-references**.
- WEFT-178 — AgentRouter dispatch into MessageBus.
- WEFT-202 — this backfill.
- Audit: 07-multi-agent-routing.md L-Routing row + Deferred item #4.
- Original spec: `02-phase-LRouting-agents-swarming.md` (L1).
