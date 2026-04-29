# Architecture Decisions

## D-MF-001 — FlowDelegator never landed in element-09 (deferred to M4) — 2026-04-28

**Status**: Recorded retroactively (WEFT-202).

**Context**. The M-Foundation phase doc
(`.planning/sparc/phase4/09-multi-agent-routing/01-phase-MFoundation-flow-delegator.md`)
specifies a full `FlowDelegator` (`crates/clawft-services/src/delegation/flow.rs`)
with subprocess spawn, `env_clear()` minimal env construction, recursion-depth
threading via `CLAWFT_DELEGATION_DEPTH`, and `which::which("claude")` runtime
detection cached on an `AtomicBool`. The element-09 tracker
(`04-element-09-tracker.md`) reported M1/M2/M3 as "Done", but the 0.7.0
release-gate audit
(`.planning/reviews/0.7.0-release-gate/07-multi-agent-routing.md`) found that
`flow.rs` was never created. `crates/clawft-services/src/delegation/mod.rs:185`
explicitly notes "Flow delegation removed -- treat as Claude fallback.";
`flow_available` was removed from the engine; `resolve_availability` collapses
`Flow` → `Claude`. The `Flow` `DelegationTarget` variant and
`claude_flow_enabled` config field are still in the type-system but inert —
users with `claude_flow_enabled = true` silently re-route to Claude.

**Decision**. Element-09 ships without `FlowDelegator`. The decision on whether
to implement it per the M-Foundation spec or formally retire it (preferred
outcome per the audit) is deferred to M4 of the 0.7.0 release wave and tracked
under **WEFT-179** (FlowDelegator implementation). The element-09 tracker has
been corrected (see element-09-tracker.md correction note 2026-04-28) so it no
longer claims `FlowDelegator` is shipped.

**Consequences**.
- **Positive**: element-09's other deliverables (AgentRouter scaffolding,
  AgentBus types, McpServerManager skeleton, McpBridge type-state, PlanningRouter
  guard rails) shipped on schedule. Honest tracker is now possible.
- **Negative**: `claude_flow_enabled = true` in user configs is a silent no-op
  until WEFT-179 lands. The dead `Flow` variant and config field remain in the
  workspace as governance debt.
- **Mitigation**: WEFT-179 is in the 0.7.x cycle; the audit row also flags
  `claude_flow_enabled` for either real wiring or removal as part of the same
  decision.

**Cross-references**.
- WEFT-179 — FlowDelegator implementation (M4).
- WEFT-202 — this backfill (governance / docs cleanup).
- Audit: `.planning/reviews/0.7.0-release-gate/07-multi-agent-routing.md`
  M-Foundation status row + Deferred items #1, #2.
- Source-of-truth code: `crates/clawft-services/src/delegation/mod.rs:185-187`
  ("Flow delegation removed -- treat as Claude fallback.").
- Original spec (kept for when WEFT-179 lands):
  `01-phase-MFoundation-flow-delegator.md`.

## D-MF-002 — Recursive-delegation depth guard not yet enforced — 2026-04-28

**Status**: Recorded retroactively (WEFT-202).

**Context**. The M-Foundation phase doc and the McpBridge pseudocode both
specify a hard cap of `MAX_DELEGATION_DEPTH = 3` carried via
`CLAWFT_DELEGATION_DEPTH` (subprocess paths) and tool-call metadata (in-process
paths). Audit confirms the cap was never threaded through
`crates/clawft-services/src/delegation/claude.rs` and the bridge constant was
never landed.

**Decision**. The guard is tracked under **WEFT-180** (recursive-delegation
guard) in the 0.7.0 release wave, and gates the McpBridge real-connection work
(`call_claude_tool_with_depth`). It is **not** considered shipped by element-09
despite the tracker's prior "Done" framing.

**Consequences**.
- Until WEFT-180 lands, accidental `delegate_task → Claude → MCP-bridge →
  delegate_task` recursion is unbounded. Cost and latency blow up under
  accidental recursion.
- McpBridge real-connection work (WEFT-181 / D-MA-001 below) is sequenced after
  WEFT-180.

**Cross-references**.
- WEFT-180 — recursive-delegation guard.
- WEFT-181 — McpBridge real Claude Code connection.
- Audit: 07-multi-agent-routing.md Deferred item #3.
