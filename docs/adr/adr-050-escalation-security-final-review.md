# ADR-050: Escalation Security Model — CONS-003 Final Review

**Date**: 2026-04-28
**Status**: Accepted
**Deciders**: 0.7.0 release-gate review (M2-D)
**Supersedes / closes**: CONS-003 in
`.planning/development_notes/01-tiered-router/consensus-log.md`

## Context

CONS-003 (Permission Escalation Security Model) was raised during the
SPARC tiered-router design phase and parked at "NEEDS REVIEW" pending a
Gate C+G implementation walk. The 0.7.0 release-gate audit
(`.planning/reviews/0.7.0-release-gate/03-pipeline-routing.md`,
Task 03-14) flagged that the review never completed: either it
happened and was never written down, or it never happened.

This ADR is the close-out. It re-walks the two designed mitigations
(FIX-04 and FIX-06) against the shipped code and records the
remaining accepted risks.

## The escalation model in one paragraph

A `Level 1` (`user`) caller may, when the task complexity score
exceeds `escalation_threshold` (default 0.6), be routed to a tier
*above* their `max_tier` — bounded by `max_escalation_tiers`
(default 1). Concretely: a `user` whose `max_tier` is `standard` can
reach `premium` for sufficiently complex prompts, but never `elite`.
`zero_trust` callers have `escalation_allowed: false` and never
escalate. Cost budgets still apply: an exhausted budget knocks the
selection back down to a cheaper tier.

The hazard CONS-003 captured: a malicious caller could craft a prompt
to inflate the complexity score, escalate one tier higher than the
operator intended, and burn budget on a more expensive model.

## Mitigation walk

### FIX-06 — Fallback model permission check

**Designed behaviour**: every fallback selection branch must verify
that the configured `routing.fallback_model` lives in a tier at or
below the caller's `max_tier`. Without the check, a misconfigured
fallback would let any caller — including escalated ones whose
post-escalation budget was exhausted — hit the fallback regardless of
permissions.

**Implementation status (as of 0.7.0)**: ✅ Closed.

The check lives in
`crates/clawft-core/src/pipeline/tiered_router.rs::fallback_chain`
(lines 482–509) and `rate_limited_decision` (lines 525–541). When a
configured fallback model belongs to a tier above the caller's
`max_tier`, both functions return an empty deny decision rather than
the disallowed model. The existing tests
`fallback_model_denied_above_max_tier` and
`rate_limited_fallback_denied_above_max_tier` cover both branches.

WEFT-27 added the missing third branch: `no_tiers_available_decision`
also applies the gate now (test:
`weft27_no_tiers_available_denies_fallback_above_max_tier`). This was
the gap behind the audit's "current code does not appear to gate the
fallback by max_tier" remark — three of three branches are now
covered.

### FIX-04 — Workspace config ceiling enforcement

**Designed behaviour**: a workspace-scoped routing config must not be
able to *expand* permissions beyond the global ceiling. If the global
config sets `user.escalation_allowed = false`, no workspace overlay
can flip it back to `true`. Same for `level`, `max_tier`,
`tool_access`, `rate_limit`, and the budget caps.

**Implementation status (as of 0.7.0)**: ✅ Closed.

`crates/clawft-core/src/pipeline/permissions.rs::PermissionResolver`
holds both global and workspace layer overrides
(`PermissionResolver::new`, lines 195–205). After merging, it calls
`enforce_workspace_ceiling` (line 298) which clamps every security-
relevant field down to the global ceiling. A static-validation entry
point `validate_workspace_ceiling` (line 326) returns a `Vec<String>`
of violations for early rejection.

The existing tests `test_workspace_ceiling_level_clamped`,
`test_workspace_ceiling_tool_access_filtered`, and
`test_workspace_ceiling_budget_clamped` cover the runtime clamp;
`test_validate_workspace_ceiling_detects_violations` covers the
static check.

## Decision

CONS-003 is **resolved** as of this ADR. FIX-04 and FIX-06 are
shipped and tested; the remaining risk surface — complexity-score
gaming via crafted prompts — is the original accepted risk and is
documented below.

## Accepted residual risks

1. **Classifier-gaming.** The complexity classifier
   (`pipeline/classifier.rs`) is keyword-based at Level 0; a caller
   can pad a request with technical terminology to push the score
   above `escalation_threshold`. Mitigations:
   - Cost budgets cap the blast radius — at most one tier of
     escalation per request, and the daily/monthly budget cap stops
     repeated abuse.
   - Operators can set `escalation_allowed: false` for any
     permission level when the threat model warrants it.
   - The chain audit log records every routing decision, so
     after-the-fact governance review can detect repeated escalations
     by the same principal.
2. **Operator misconfiguration.** A loose `routing.fallback_model:
   anthropic/claude-opus-4` that admins forget to bound by
   `model_denylist` is now caught by the FIX-06 gates, but only at
   request time. There is no static validator that rejects a
   fallback model whose tier exceeds every user's `max_tier`. Tracked
   as a 0.8.x improvement (NOT release-blocking).
3. **WEFT-31 (model_override) audit logging.** With WEFT-31 shipped,
   any caller whose `permissions.model_override` is true and who
   supplies an explicit `request.model` lifts every gate including
   tier filtering. This is now audited via a `routing.audit`
   `tracing::warn!` plus a `model_override_bypass` chain event, so
   bypasses leave a paper trail. Operators must still treat
   `model_override: true` as a privileged grant.

## Consequences

- Closes the longest-standing security review item in the tiered-
  router consensus log.
- The three FIX-06 gates make `routing.fallback_model` safe to leave
  set in shared configs even when individual users have restrictive
  `max_tier` clamps.
- Any future change to the fallback selection branches MUST preserve
  the tier-check on every code path. Reviewers should reject
  routing-pipeline PRs that introduce a new fallback path without a
  matching gate.

## References

- `.planning/development_notes/01-tiered-router/consensus-log.md`
  (CONS-003 row, line 165)
- `.planning/reviews/0.7.0-release-gate/03-pipeline-routing.md`
  (Task 03-14)
- `crates/clawft-core/src/pipeline/tiered_router.rs::fallback_chain`
- `crates/clawft-core/src/pipeline/tiered_router.rs::rate_limited_decision`
- `crates/clawft-core/src/pipeline/tiered_router.rs::no_tiers_available_decision`
- `crates/clawft-core/src/pipeline/permissions.rs::PermissionResolver`
- ADR-045 (Tiered Router with Permission-Based Model Selection)
