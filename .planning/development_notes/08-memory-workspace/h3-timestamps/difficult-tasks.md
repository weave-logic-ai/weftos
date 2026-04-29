# H3: Timestamp Standardization -- Difficult Tasks

> Backfilled 2026-04-28. H3 was mostly mechanical; one subtask
> required care.

## 2026-02-20 Difficult: cross-crate ripple of the `i64` -> `DateTime<Utc>` change

**Item**: H3 / clawft-types -> all downstream
**Difficulty**: High
**Why**: `clawft-types` sits at the bottom of the dep graph. Changing
the signature of `WorkspaceEntry::created_at` from `i64` to
`DateTime<Utc>` cascades through every consumer (CLI, daemon,
channels, kernel). The compiler caught the obvious cases; the
non-obvious cases lived in serde-derived JSON shapes that round-tripped
silently through `Value`.
**Approach**: Land the `clawft-types` change as a single commit
(`a67b9e5c`), fix every compile error mechanically, then run the full
serialization test matrix. Where on-disk JSON had to keep accepting
the legacy shape, add an `untagged` deserialize enum that normalizes
on read.
**Findings**: The matrix caught two cases (session turn timestamps,
witness chain timestamps) where `serde_json::Value`-typed code paths
silently passed `Number` payloads through. Those paths now use the
typed wrapper. Migration shims live behind a once-only deserialize
adapter; new writes are always RFC-3339.
