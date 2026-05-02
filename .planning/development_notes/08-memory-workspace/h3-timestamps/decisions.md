# H3: Timestamp Standardization -- Decisions

> Backfilled 2026-04-28 from `crates/clawft-types/src/{workspace,
> session,cron}.rs` and the SPARC plan
> (`.planning/sparc/phase4/08-memory-workspace/01-phase-H1H3-workspace-timestamps.md`).

## 2026-02-20 Decision: `chrono::DateTime<Utc>` everywhere, no raw `i64` ms

**Context**: Pre-H3 the codebase mixed `i64` epoch milliseconds,
`Option<String>` ISO-8601, and `SystemTime` across `clawft-types`,
session log entries, cron schedules, and witness chains. Every
serialization boundary had to know which convention applied; cross-
crate timestamp arithmetic was a footgun.
**Options**:
1. Standardize on `i64` epoch ms (smallest, no timezone story).
2. Standardize on `DateTime<Utc>` from `chrono` (typed, RFC-3339
   serialization, arithmetic methods).
3. Use `time::OffsetDateTime` (smaller dep but worse ecosystem fit
   in the rest of the workspace).
**Decision**: Option 2 -- `DateTime<Utc>` is the canonical type.
**Rationale**: Type-level guarantees prevent the "is this seconds or
ms" class of bugs. `chrono` is already a transitive dep via several
other crates. RFC-3339 is the natural JSON serialization for sessions
and config. Cross-crate arithmetic stays inside the chrono API.
**Consequences**: Touches `clawft-types/src/{workspace,cron,session}.rs`
and downstream. Existing on-disk JSON with epoch-ms fields needed
migration shims. Witness chain timestamps moved to RFC-3339.
Implementation rolled out alongside H1/H2 in commit `a67b9e5c`
(2026-02-20).

## 2026-02-20 Decision: serialize as RFC-3339, not epoch seconds

**Context**: Once `DateTime<Utc>` is the in-memory type, the JSON
shape is a separate question.
**Options**:
1. Numeric epoch seconds/ms.
2. RFC-3339 strings ("2026-02-20T12:34:56Z").
3. Custom format.
**Decision**: RFC-3339.
**Rationale**: Human-readable in `MEMORY.md`, `HISTORY.md`, and
session JSONL. Lossless round-trip with `chrono`. Easy to grep and
diff in operator tooling.
**Consequences**: Slightly larger on-disk bytes than epoch numbers,
but session and memory volumes are not the dominant disk pressure.
Use `serde(with = "chrono::serde::ts_seconds")` only when interop
with an external API requires it.

## 2026-02-20 Decision: timestamps are stamped at the call site, not at serialization

**Context**: Several call sites historically delegated "what time is
it" to the serializer or to a global singleton.
**Options**:
1. Each domain object carries an `Option<DateTime<Utc>>` and the
   serializer fills it.
2. Each domain object requires a `DateTime<Utc>` at construction.
3. A global `Clock` singleton.
**Decision**: Option 2 -- timestamps are required at construction
time, supplied by the caller.
**Rationale**: Tests can inject a fake time without touching globals.
Domain objects that lack a timestamp are no longer representable.
Serializers stay pure.
**Consequences**: Constructors become slightly more verbose; helpers
like `WorkspaceEntry::new(name, root, now())` are common. Test
fixtures use a frozen `Utc.with_ymd_and_hms(...)` value rather than
`Utc::now()`.
