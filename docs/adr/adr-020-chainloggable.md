# ADR-020: ChainLoggable Trait for Audit Gap Closure

**Date**: 2026-03-28
**Status**: Accepted
**Deciders**: Sprint 11 Symposium Tracks 1, 2, 4, 8 (compound theme)
**Note (2026-04-28, WEFT-140)**: ADR-020 was previously shared with
"Kernel Phase (K-Level) Responsibilities". The kernel-phases ADR has
been renumbered to ADR-048; this ChainLoggable ADR keeps the 020
slot since it is the older Sprint-11 decision (2026-03-28 vs the
kernel-phases ADR's 2026-04-03 date) and is referenced by ADR-019
(Registry Trait) as a sibling Track-1 extraction.

## Context

Tracks 1, 2, and 8 independently identified that significant kernel events are not chain-logged: process restarts (Track 1), governance decisions outside TileZero (Track 1), IPC failures (Track 1), and DEMOCRITUS ticks (Track 2). Track 4 designed the console to chain-log every shell command. The ExoChain is the system's audit backbone, but it has blind spots in the very subsystems (self-healing, governance, IPC) that most need auditing. This is a compound risk: the absence of audit data in these subsystems means post-incident analysis is incomplete.

## Decision

Define a `ChainLoggable` trait that standardizes event logging to the ExoChain. Subsystems that generate auditable events (process restarts, governance decisions, IPC failures, shell commands, DEMOCRITUS ticks) implement this trait. The trait provides a structured event format with metadata, ensuring all events are chain-logged consistently.

Pair with Track 2's chain-audit-completeness tests (T8) to verify that no auditable event goes unlogged.

## Consequences

### Positive
- Closes audit blind spots in self-healing, governance, and IPC subsystems
- Standardized event format makes post-incident analysis consistent
- Chain-audit-completeness tests prevent future audit regressions
- Console chain-logging (Track 4) uses the same trait

### Negative
- 6-hour implementation effort across multiple subsystems
- Additional ExoChain write load from newly logged events
- May increase chain growth rate -- needs monitoring

### Neutral
- This is a v0.2 deliverable (W18 in the work items)
- Depends on the ExoChain being available (feature-gated behind `exochain`)
- Related to ADR-019 (Registry trait) -- both are trait extractions from Track 1
