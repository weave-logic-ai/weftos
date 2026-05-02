# H3: Timestamp Standardization -- Blockers

> Backfilled 2026-04-28. H3 shipped without unresolved blockers.

## 2026-02-20 Blocker: legacy on-disk session files used inconsistent timestamp shapes

**Item**: H3 / `clawft-types::session`
**Severity**: Low
**Description**: Pre-H3 session JSONL files mixed `i64` epoch ms with
`Option<String>` ISO-8601 across different turn types. A single load
path had to handle both shapes.
**Attempted**: Added a serde `untagged` enum during deserialization
that accepts either shape and normalizes to `DateTime<Utc>` on the
in-memory side. New writes are RFC-3339 only.
**Needs**: A `weft session gc` command to rewrite legacy files into
the canonical shape, or document manual cleanup. Today's behavior:
load works, but the on-disk file is not rewritten until a turn is
appended. Filed as part of the WS-O8 / WEFT-MW-9 thread (overlaps
with the percent-encoded migration).
**Status**: Resolved (load path handles both; rewrite is a future
follow-up, not blocking 0.7.0).
