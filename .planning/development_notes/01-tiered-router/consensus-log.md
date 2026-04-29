# Consensus Log -- 01-tiered-router Sprint

**Sprint**: Tiered Router & Permission System
**Created**: 2026-02-18

---

## Protocol

This log tracks decisions that required consensus among multiple agents.
The consensus protocol is:

1. **When to invoke**: Any decision where the implementing agent's confidence
   is below **95%**, OR where the decision affects multiple crates, OR where
   security implications exist.

2. **Process**:
   - The implementing agent documents the question and their position
   - At least **2 additional agents** review and state their position
   - Positions are recorded with rationale
   - The team lead (or implementing agent with consensus) records the resolution
   - If no consensus after 3 positions, the team lead makes the final call

3. **Confidence thresholds**:
   - **>= 95%**: Agent proceeds without consensus (log decision in phase notes)
   - **80-94%**: Request 1 additional review (2 agents total)
   - **< 80%**: Request 2 additional reviews (3 agents total)
   - **Security-related**: Always require 2+ reviews regardless of confidence

4. **Resolution status**:
   - `OPEN` -- Awaiting review
   - `RESOLVED` -- Consensus reached, decision recorded
   - `ESCALATED` -- No consensus; team lead decides
   - `DEFERRED` -- Postponed to a later phase

---

## Entry Template

```markdown
### CONS-NNN: [Topic Title]

**Date**: YYYY-MM-DD
**Phase**: [Phase letter]
**Raised by**: [Agent name]
**Initial confidence**: [Percentage]
**Status**: OPEN | RESOLVED | ESCALATED | DEFERRED

#### Question

[Clear statement of the decision to be made]

#### Positions

| Agent | Position | Rationale |
|-------|----------|-----------|
| [name] | [A/B/C or description] | [Why] |
| [name] | [A/B/C or description] | [Why] |
| [name] | [A/B/C or description] | [Why] |

#### Resolution

**Decision**: [What was decided]
**Confidence after**: [Percentage]
**Action items**: [What needs to happen as a result]
```

---

## Entries

### CONS-001: Type Location -- config.rs vs routing.rs

**Date**: 2026-02-18
**Phase**: A
**Raised by**: sparc-implementer
**Initial confidence**: 90%
**Status**: RESOLVED

#### Question

Should the new routing configuration types (`RoutingConfig`, `ModelTierConfig`,
`PermissionsConfig`, `EscalationConfig`, `CostBudgetConfig`, `RateLimitConfig`,
`TierSelectionStrategy`) be added to the existing `crates/clawft-types/src/config.rs`,
or placed in a new `crates/clawft-types/src/routing.rs` module?

Current `config.rs` is ~250 lines. Adding routing types would bring it to ~450.
The project guideline is files under 500 lines.

#### Context

- All existing top-level config structs (`AgentsConfig`, `ChannelsConfig`,
  `ProvidersConfig`, `GatewayConfig`, `ToolsConfig`) live in `config.rs`
- `RoutingConfig` is a peer to these (top-level field on `Config`)
- The delegation types were placed in a separate `delegation.rs` module, but
  delegation is a service concern, not a config-only concern
- `UserPermissions` could arguably be in `auth.rs` since it's used at runtime

#### Positions

| Agent | Position | Rationale |
|-------|----------|-----------|
| sparc-implementer | Extend config.rs | Consistent with existing pattern; 450 lines is within budget |
| remediation-review | New routing.rs | Routing types include runtime types (AuthContext, UserPermissions) beyond pure config; routing.rs matches the Phase A SPARC plan and avoids bloating config.rs |

#### Resolution

**Decision**: All routing types go in `clawft-types/src/routing.rs` (new module).
This includes `RoutingConfig`, `ModelTierConfig`, `PermissionsConfig`,
`PermissionLevelConfig`, `UserPermissions`, `AuthContext`, `TierSelectionStrategy`,
`EscalationConfig`, `CostBudgetConfig`, and `RateLimitConfig`. The `Config` struct
in `config.rs` gains a `pub routing: RoutingConfig` field with `#[serde(default)]`,
importing from the new module. This matches the Phase A SPARC plan
(`A-routing-config-types.md`) and the remediation plan (FIX-01).

**Confidence after**: 98%
**Action items**:
- Phase A creates `crates/clawft-types/src/routing.rs`
- Phase A adds `pub mod routing;` to `crates/clawft-types/src/lib.rs`
- Phase A adds `routing: RoutingConfig` field to `Config` in `config.rs`
- Phase A decisions.md updated to reflect this resolution

---

### CONS-002: DashMap vs RwLock<HashMap> for CostTracker and RateLimiter

**Date**: 2026-02-18
**Phase**: D, E
**Raised by**: sparc-implementer
**Initial confidence**: 80%
**Status**: OPEN

#### Question

Should `CostTracker` and `RateLimiter` use `dashmap::DashMap` for concurrent
per-user tracking, or `std::sync::RwLock<HashMap>`?

#### Context

- clawft targets single-user and small-team deployments (1-10 concurrent users)
- `DashMap` provides sharded concurrent access without global locks
- `RwLock<HashMap>` is stdlib-only (no new dependency)
- Binary size impact of `dashmap` is ~20-40 KB
- Current concurrency model uses `tokio::sync::mpsc` channels (not lock-heavy)
- `CostTracker` is read-heavy (budget checks on every request) with occasional
  writes (cost recording after response)
- `RateLimiter` is read-write on every request (check + update window)

#### Positions

| Agent | Position | Rationale |
|-------|----------|-----------|
| sparc-implementer | Leaning RwLock | Stdlib-only; contention negligible at <10 users; simpler |
| | | |
| | | |

#### Resolution

**Decision**: Pending review
**Confidence after**: --
**Action items**: --

---

### CONS-003: Permission Escalation Security Model

**Date**: 2026-02-18
**Phase**: B, C
**Raised by**: sparc-implementer
**Initial confidence**: 85%
**Status**: NEEDS REVIEW

#### Question

Is the escalation model secure? A Level 1 (`user`) can access `premium` tier
models when task complexity exceeds `escalation_threshold` (default 0.6). This
means a user-level sender can trigger requests to `claude-sonnet-4` or `gpt-4o`
by crafting a sufficiently complex prompt.

#### Context

- Escalation is bounded: `max_escalation_tiers: 1` means at most one tier above
  `max_tier` (user's `standard` -> `premium`, but never `elite`)
- Escalation respects cost budgets (if budget is exhausted, falls back to lower tier)
- The complexity classifier is the gatekeeper; at Level 0 it uses keyword matching
  which could be gamed
- Design doc explicitly calls this out as a feature, not a bug
- Escalation can be disabled per-level (`escalation_allowed: false`)
- Zero-trust users have `escalation_allowed: false` by default

#### Positions

| Agent | Position | Rationale |
|-------|----------|-----------|
| sparc-implementer | Accept with mitigations | Budget caps limit damage; classifier gaming is low-reward |
| | | |
| | | |

#### Resolution

**Decision**: Needs further security review. Two related remediation fixes strengthen
the escalation security model but do not fully close this item:

- **FIX-06** (Fallback model permission check): Phase C's `fallback_chain()` and
  `rate_limited_decision()` must verify fallback models belong to a tier at or below
  the user's `max_tier`. Without this, escalation fallback could bypass tier restrictions.

- **FIX-04** (Workspace config ceiling enforcement): Phase H must enforce that workspace
  configs cannot grant escalation privileges above the global config ceiling. Phase B's
  `PermissionResolver` must apply ceiling enforcement after merge.

These two fixes mitigate the primary risks but the classifier-gaming vector (crafting
prompts to inflate complexity scores) remains an accepted risk per the design doc.
This item should be reviewed during Phase C implementation to confirm both FIX-04 and
FIX-06 are addressed in the code.

**Confidence after**: 88% (pending implementation review)
**Action items**:
- Phase C: Implement fallback model permission check per FIX-06
- Phase H: Implement workspace ceiling enforcement per FIX-04
- Phase B: Accept both global and workspace configs in PermissionResolver per FIX-04
- Security review during Phase C gate (Gate C+G)

---

### CONS-004: AuthContext Location (config.rs vs auth.rs)

**Date**: 2026-02-18
**Phase**: B
**Raised by**: sparc-implementer
**Initial confidence**: 85%
**Status**: RESOLVED

#### Question

Should `AuthContext` be defined in `clawft-types/src/config.rs` alongside
`UserPermissions`, or in a new `clawft-types/src/auth.rs` module?

#### Context

- `UserPermissions` is a config type (defined in config JSON, deserialized)
- `AuthContext` is a runtime type (constructed per-request, carries resolved permissions)
- `AuthContext` contains `UserPermissions` (has-a relationship)
- The delegation module set precedent: `DelegationConfig` (config type) in its
  own `delegation.rs`, `DelegationEngine` (runtime) in `clawft-services`
- `AuthContext` is simple (~4 fields), not worth a whole module just for itself
- Other runtime types that carry config-adjacent data: `Session` is in `session.rs`

#### Positions

| Agent | Position | Rationale |
|-------|----------|-----------|
| sparc-implementer | Leaning auth.rs | Clean separation of config vs runtime types |
| remediation-review | routing.rs | AuthContext is tightly coupled to routing permissions; co-locating with UserPermissions avoids a single-struct module |

#### Resolution

**Decision**: `AuthContext` is defined in `clawft-types/src/routing.rs`, alongside
`UserPermissions` and all other routing types. This is the canonical location
established by FIX-01 in the remediation plan. The rationale:

1. `AuthContext` contains a `UserPermissions` field -- co-location avoids cross-module
   imports within `clawft-types`.
2. A dedicated `auth.rs` module for a single 4-field struct adds unnecessary file
   proliferation.
3. The `routing.rs` module already serves as the boundary between config types and
   routing runtime types, so `AuthContext` fits naturally.
4. `AuthContext::default()` returns zero_trust permissions (not admin), matching the
   "private by default" design principle.

All phases import `AuthContext` from `clawft_types::routing`, NOT from
`clawft_types::auth` (which does not exist).

**Confidence after**: 98%
**Action items**:
- Phase A: Define `AuthContext` in `routing.rs` with `Default` impl returning zero_trust
- Phase A: Add `AuthContext::cli_default()` constructor for admin permissions
- Phase C: Import from `clawft_types::routing`, not `clawft_types::auth`
- Phase F: Import from `clawft_types::routing`, do NOT create `auth.rs`

---

### CONS-005: RoutingDecision Extension Strategy

**Date**: 2026-02-18
**Phase**: C
**Raised by**: sparc-implementer
**Initial confidence**: 92%
**Status**: RESOLVED

#### Question

The `RoutingDecision` struct needs new fields (`tier`, `cost_estimate_usd`,
`escalated`, `budget_constrained`). Should these be added as `Option<T>` fields
directly on the struct, or wrapped in a separate `RoutingMetadata` struct?

#### Context

- `RoutingDecision` currently has 3 fields: `provider`, `model`, `reason`
- `StaticRouter` would set new fields to `None`/`false` (zero overhead)
- Adding 4 optional fields keeps the struct flat and easy to pattern-match
- A separate `RoutingMetadata` adds indirection for minimal benefit
- The design doc shows the fields directly on `RoutingDecision`

#### Positions

| Agent | Position | Rationale |
|-------|----------|-----------|
| sparc-implementer | Flat Option fields | Matches design doc; simple; StaticRouter sets None |
| remediation-review | Flat Option fields + Default impl | Agree with flat approach; adding Default impl ensures backward compat at all construction sites |

#### Resolution

**Decision**: Extend `RoutingDecision` with flat optional fields directly on the
struct, plus a `Default` impl for the new fields. The new fields are:

- `tier: Option<String>` -- name of the selected tier (None for StaticRouter)
- `cost_estimate_usd: Option<f64>` -- estimated cost for this request
- `escalated: bool` -- whether the request was escalated above max_tier (default: false)
- `budget_constrained: bool` -- whether budget enforcement forced a cheaper tier (default: false)

The `Default` impl sets `tier: None`, `cost_estimate_usd: None`, `escalated: false`,
`budget_constrained: false`. Existing construction sites (StaticRouter, tests) use
`..Default::default()` to fill the new fields automatically, avoiding breakage.

This matches the design doc layout and the FIX-10 remediation plan. Phase C owns
this change and must document the breaking change clearly. All existing
`RoutingDecision` construction sites must be updated with the default fill pattern.

**Confidence after**: 98%
**Action items**:
- Phase C: Add the 4 fields to `RoutingDecision` with `#[serde(default)]`
- Phase C: Add `Default` impl (or derive) for `RoutingDecision`
- Phase C: Update all existing construction sites with `..Default::default()`
- Phase C: Document the change as a breaking change in the plan notes

---

### CONS-006: Config Validation Boundary

**Date**: 2026-02-18
**Phase**: H
**Raised by**: sparc-implementer
**Initial confidence**: 88%
**Status**: OPEN

#### Question

Where should config validation live? `clawft-types` (at deserialization) or
`clawft-core` (at router construction)?

#### Context

- `clawft-types` is zero-dep beyond serde; adding validation logic increases
  its surface area
- Some validations are simple (range checks): `complexity_range[0] <= 1.0`
- Some validations are cross-referential (business rules): "tier name in
  permissions.max_tier must exist in tiers list"
- Rust's type system catches some issues at compile time (enums, required fields)
- serde `#[serde(default)]` handles missing fields gracefully

#### Positions

| Agent | Position | Rationale |
|-------|----------|-----------|
| sparc-implementer | Split: basic in types, business in core | Types validates data integrity; core validates semantic correctness |
| | | |
| | | |

#### Resolution

**Decision**: Pending review
**Confidence after**: --
**Action items**: --

---

### CONS-007: Permission Resolution Priority Ordering

**Date**: 2026-02-18
**Phase**: B, F, C
**Raised by**: gap-analysis (GAP-T14)
**Initial confidence**: 100%
**Status**: RESOLVED

#### Question

The design doc (Section 3.2) defines per-channel overrides (priority 5) as taking
precedence over per-user overrides (priority 4). Phase B and Phase F implemented
the inverse ordering (per-user as highest priority). Which ordering is correct?

#### Positions

| Agent | Position | Rationale |
|-------|----------|-----------|
| gap-analysis | Identified contradiction | GAP-T14: Design doc says channel > user; Phase B says user > channel |
| design-doc-author | Per-channel > per-user | Design doc Section 3.2 is authoritative. Channel restrictions must be enforceable even for named users. |
| remediation-worker | Per-channel > per-user | Adopt design doc ordering. Security patterns like "limit all users in #general to free tier" require channel to override user. |

#### Resolution

**Decision**: Per-channel overrides (priority 5) take precedence over per-user overrides
(priority 4), per design doc Section 3.2. Channel restrictions are enforceable even for
named users. This enables security patterns like "limit all users in #general to free tier."
If an admin needs to bypass a channel restriction, the channel config itself should be
modified rather than relying on per-user overrides to override channel rules.

**Confidence after**: 100%
**Action items**:
- Phase B (primary): Swap merge steps 5 and 6 so per-user is applied before per-channel.
  Update Section 1.3 priority listing, resolve() pseudocode, conflicting overrides text,
  and test descriptions.
- Phase F (reference update): Update Section 2.3 merge ordering reference and Section 4.4
  edge case about admin user on channel with rate_limit.
- Phase C (comment check): Verified no ordering-dependent comments exist. Phase C reads
  pre-resolved permissions and is not affected.
- Consensus log: This entry (CONS-007) documents the decision.

**Affected Plans**: B (primary), F (reference update), C (comment update)

---

## Summary Statistics

| Status | Count |
|--------|-------|
| OPEN | 2 |
| RESOLVED | 4 |
| NEEDS REVIEW | 1 |
| ESCALATED | 0 |
| DEFERRED | 0 |
| **Total** | **7** |

### Resolution tracking (updated 2026-02-18 per FIX-11 remediation)

| Entry | Status | Resolution Reference |
|-------|--------|---------------------|
| CONS-001 | RESOLVED | Types in `routing.rs` per FIX-01, Phase A SPARC plan |
| CONS-002 | OPEN | DashMap vs RwLock -- pending Phase D/E implementation |
| CONS-003 | RESOLVED | Escalation security closed by ADR-050 (2026-04-28); FIX-04 + FIX-06 walked + the third no-tiers-available branch added by WEFT-27 |
| CONS-004 | RESOLVED | AuthContext in `routing.rs` per FIX-01 |
| CONS-005 | RESOLVED | Flat Option fields + Default impl per FIX-10 |
| CONS-006 | OPEN | Config validation boundary -- pending Phase H implementation |
| CONS-007 | RESOLVED | Per-channel > per-user priority ordering per design doc Section 3.2 (GAP-T14) |
