# ADR-022: All State-Changing Operations Must Log to ExoChain

**Date**: 2026-04-03
**Status**: Accepted
**Deciders**: Architecture review, Sprint 14
**Depends-On**: ADR-048 (Kernel Phase Responsibilities, formerly ADR-020 — renumbered 2026-04-28 / WEFT-140), ADR-021 (CLI Kernel Compliance)

## Context

ExoChain is the tamper-evident audit trail at the heart of WeftOS. However, when commands bypass the kernel (ADR-021), their operations are invisible to the chain. This defeats the purpose of having cryptographic provenance — you cannot prove what happened if the action was never logged.

## Decision

Every operation that changes system state MUST produce an ExoChain event. This includes:

| Operation Category | Event Type | Minimum Fields |
|-------------------|------------|----------------|
| Agent spawned/stopped | `agent.lifecycle` | agent_id, action, parent_pid, capabilities |
| File read for analysis | `assess.scan` | file_path, hash, scope |
| Assessment completed | `assess.report` | scope, file_count, finding_count, coherence_score |
| Cron job added/removed | `cron.mutation` | job_id, action, schedule |
| Skill installed/removed | `skill.mutation` | skill_name, action, source, signature |
| Tool allow/deny changed | `tool.policy` | pattern, action, previous_state |
| Config modified | `config.mutation` | key, previous_value_hash, new_value_hash |
| Cross-project link | `coordination.link` | peer_name, peer_location, direction |
| Network call made | `network.egress` | url, method, response_status |
| Governance gate decision | `governance.decision` | gate_id, action, allowed, reason |

### Chain of Custody

For assessment specifically, the chain of custody is:

```
TRIGGER (who/what initiated) → SCOPE (what was scanned) → SCAN (each file read) →
ANALYZE (findings generated) → REPORT (summary produced) → COMMIT (chain entry)
```

Every step is logged. An auditor can verify: this assessment was triggered by this user, scanned these specific files, produced these findings, and the chain hashes are intact.

## Consequences

### Positive
- Complete audit trail for compliance and forensics
- Tamper-evident: cannot retroactively hide an operation
- Cross-project coordination can verify peer integrity via chain comparison
- "Prove every decision" marketing claim is actually backed by implementation

### Negative
- Chain grows with every operation (storage cost)
- Slight performance overhead per operation (hash computation)
- Requires all paths to go through kernel (enforced by ADR-021)

### Neutral
- ExoChain already supports all event types via `ChainEvent`
- SHAKE-256 hash computation is sub-microsecond
