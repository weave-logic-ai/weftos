# Development Notes

## 2026-04-28 — Backfill (WEFT-202)

The M-Foundation tracker line ("FlowDelegator, DelegationError, flow_available,
delegate feature — Done") in `04-element-09-tracker.md` was misleading:
`flow.rs` was never created, and `flow_available` was *removed* (not wired) so
that all `Flow` requests collapse onto the `Claude` path.
See `decisions.md` D-MF-001 for the recorded rationale and forward link to
WEFT-179.

The recursion-guard work (depth threading via `CLAWFT_DELEGATION_DEPTH`,
`MAX_DELEGATION_DEPTH = 3`) was never landed either; tracked separately under
WEFT-180 (see `decisions.md` D-MF-002). Together with WEFT-179 and the
McpBridge real-connection work (M-Advanced D-MA-001 / WEFT-181), these three
items form the unfinished M-Foundation slice.

What *did* ship in this phase:

- `DelegationError` extended with `SubprocessFailed`, `OutputParseFailed`,
  `Timeout`, `Cancelled`, `FallbackExhausted` variants in `delegation/claude.rs`.
- `delegate` feature added to `clawft-cli` default features.
- `claude_enabled = true` default for graceful degradation.
- `which` workspace dep wired into `clawft-services` for future
  `flow_available` use.
