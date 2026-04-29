# H3: Timestamp Standardization -- Notes

> Backfilled 2026-04-28.

## Implementation map

- `clawft-types/src/workspace.rs` -- `WorkspaceEntry::created_at`,
  `last_accessed`.
- `clawft-types/src/session.rs` -- turn timestamps.
- `clawft-types/src/cron.rs` -- cron schedule + last-run timestamps.
- Downstream: every consumer in `clawft-core`, `clawft-channels`, and
  `clawft-cli` was migrated in the same commit.

## Useful invariants

- `DateTime<Utc>` only -- no `Option<DateTime<Local>>`, no `i64` ms.
- Construction-time stamping. Constructors take a `now: DateTime<Utc>`
  parameter so tests can pass a frozen value.
- Serialization is RFC-3339 by default. Use `serde(with = "...")`
  only for explicit interop overrides (e.g. external APIs).

## Tips

- Frozen test time: `Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()`.
- For `WorkspaceEntry::last_accessed`, see WS-O4 / WEFT-MW-10 -- the
  field exists but `WorkspaceManager::load` doesn't bump it; that
  bug is independent of H3.

## Known follow-ups (see audit + Plane)

- WS-O8 / WEFT-MW-9 -- legacy session JSONL rewrite tooling.
- WS-O4 / WEFT-MW-10 -- `WorkspaceManager::load` should update
  `last_accessed`.
