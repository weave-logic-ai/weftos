# H2: RVF Phase 3 Vector Memory -- Blockers

> Backfilled 2026-04-28 from the audit
> (`.planning/reviews/0.7.0-release-gate/06-memory-workspace.md`,
> rows WS-O2, WS-O6) and the SPARC plan.

## 2026-02-20 Blocker: rvf-runtime 0.2 binary format unstable

**Item**: H2.3 (RVF segment I/O)
**Severity**: Medium
**Description**: The upstream `rvf-runtime` 0.2 segment format was
flagged by the 0.2 audit as too tightly coupled to its evolving
binary schema. Adopting it directly would force on-disk format
migrations for agent memory on every minor version of the upstream
crate.
**Attempted**: Considered direct adoption; rejected on the audit's
recommendation. Implemented a local JSON fallback (`rvf_io.rs`,
later removed; today the live path is `rvf_stub.rs`).
**Needs**: Re-evaluate when `rvf-runtime` 0.3+ ships with a stable
on-disk format. Until then, the local stub is the path.
**Status**: Resolved (deferred -- not a 0.7.0 blocker).

## 2026-02-20 Blocker: vector index drift versus `MEMORY.md`

**Item**: H2 / `memory_bootstrap`
**Severity**: Medium
**Description**: `memory_bootstrap` builds the vector index once and
is idempotent (skips if the index file exists). Edits to `MEMORY.md`
are *not* reflected in the vector index until the index is manually
deleted. `MemoryStore::search` is substring-only and doesn't see the
index either; the two memory views drift quietly.
**Attempted**: H2.4 export/import gives operators a manual rebuild
path. There is no automatic re-index trigger.
**Needs**: Either (a) check mtime in `bootstrap_memory_index` and
re-index on staleness, or (b) add a `weft memory reindex` CLI.
Filed as WEFT (MW-6).
**Status**: Active (post-0.7.0).
