# ADR-051: Fate of the Eight Orphaned `clawft-plugin-*` Crates

**Date**: 2026-04-28
**Status**: Accepted
**Deciders**: 0.7.0 release-gate review (M4-E, WEFT-61)
**Closes**: 04-plugin-skills task #3 ("Decide fate of 8 orphaned
`clawft-plugin-*` crates"); handoff TODO from commit `8c08ce0a`.
**Related**: ADR-036 (Hierarchical kernel ToolRegistry), ADR-052
(ToolRegistry split documentation).

## Context

The 0.7.0 release-gate audit
(`.planning/reviews/0.7.0-release-gate/04-plugin-skills.md`, lines
190-205, 266-272) flagged nine `clawft-plugin-*` crates in the
workspace, eight of which are **orphaned**: they are workspace members
and compile on every `cargo check`, but no binary, library, or test in
the workspace consumes them. The ninth, `clawft-plugin-treesitter`, is
wired into `clawft-kernel` via the optional `treesitter` feature and
used by `clawft-kernel::assessment::extract_symbols`
(`crates/clawft-kernel/src/assessment/mod.rs:356-370`).

The eight orphaned crates and their as-found state:

| Crate                          | LOC   | Last touched               | Workspace consumer | Sibling consumer                    |
|--------------------------------|-------|----------------------------|--------------------|-------------------------------------|
| `clawft-plugin-browser`        |   827 | Sprint 11 (`8f29daa1`)     | none               | none                                |
| `clawft-plugin-calendar`       |   803 | Sprint 11 (`8f29daa1`)     | none               | depends on `clawft-plugin-oauth2`   |
| `clawft-plugin-cargo`          |   679 | Sprint 11 (`8f29daa1`)     | none               | none                                |
| `clawft-plugin-ci`             |   394 | `21561575` (clippy sweep)  | none               | none                                |
| `clawft-plugin-containers`     | 1,241 | Sprint 11 (`8f29daa1`)     | none               | none                                |
| `clawft-plugin-git`            | 1,212 | `cd779585` (musl fix)      | none               | none                                |
| `clawft-plugin-npm`            |   309 | `89ad7d57` (Sprint 15)     | none               | none                                |
| `clawft-plugin-oauth2`         | 1,317 | Sprint 11 (`8f29daa1`)     | none               | consumed only by `*-calendar`       |

All eight carry `publish = false` in their `Cargo.toml` (audit prose
about them being "shipped at 0.6.6" was inaccurate — only treesitter
has `publish = true`). They are not on crates.io. There are no public
references to them outside this repository.

The cost of leaving them as workspace members:

1. Every `cargo check`, `cargo clippy`, and `scripts/build.sh check`
   compiles ~7 KLOC of dead code, including transitive cost of
   `chromiumoxide`, `git2` (vendored OpenSSL), `oauth2`, `which`, and
   `tree-sitter-{rust,typescript,python,javascript}` grammars (the
   tree-sitter cost is unavoidable because we keep the surviving
   crate, but we still pay it twice if all four language features are
   enabled and the dead `clawft-plugin-treesitter` mirror lived in
   another crate — that is not the case here, just noting).
2. Dependency-update PRs (cargo-audit, dependabot) churn nine crates
   instead of one.
3. The "what is shipping in 0.7" surface is muddied for downstream
   integrators.

The cost of deleting them outright:

1. Each crate represents 300-1,300 lines of working, tested, recent
   domain code (git operations, Docker/Podman orchestration, OAuth2
   token handling, browser automation via CDP, calendar / Google
   Calendar API). Deleting throws that work away.
2. The crates were written for a kernel `--features full` extras
   bundle that has not yet been wired (see audit lines 202-204:
   *"they were planned for a kernel `--features full` extras bundle
   that was never wired"*). That wiring may still be done in 0.8.x.
3. Some functionality (notably `clawft-plugin-git` and
   `clawft-plugin-cargo`) may be revived as kernel-builtin tools via
   the ADR-036 hierarchical `ToolRegistry` once the
   `BuiltinTool`-trait migration is decided (see ADR-052).

## Decision

**Archive, do not delete.** The eight orphaned crates move to
`crates/archive/clawft-plugin-*` via `git mv` and are removed from the
workspace `members` list. The `crates/archive/` path is added to the
top-level `[workspace] exclude` list so cargo ignores it entirely — no
build cost, no dependency resolution, no clippy noise. Git history is
preserved through the move; revival is a matter of `git mv` back and
re-adding the workspace member entry.

`clawft-plugin-treesitter` stays in place at `crates/clawft-plugin-
treesitter/` and remains a workspace member. It is the only plugin
crate with a real consumer, and the kernel feature gate
(`treesitter = ["dep:clawft-plugin-treesitter"]`) keeps it off-path
when not requested.

### Per-crate disposition

| Crate                          | Disposition | Rationale                                                                                                  |
|--------------------------------|-------------|------------------------------------------------------------------------------------------------------------|
| `clawft-plugin-browser`        | archive     | CDP automation; no consumer. Browser-side automation belongs in `clawft-wasm` browser path (ADR-044).      |
| `clawft-plugin-calendar`       | archive     | Calendar / Google Calendar; no consumer. Out-of-scope for 0.7.0 release.                                   |
| `clawft-plugin-cargo`          | archive     | Cargo wrappers; no consumer. May be revived as kernel `BuiltinTool` per ADR-052 migration.                 |
| `clawft-plugin-ci`             | archive     | YAML parsing for CI configs; no consumer. Niche; revive only on demand.                                    |
| `clawft-plugin-containers`     | archive     | Docker/Podman orchestration; no consumer. Out-of-scope for 0.7.0.                                          |
| `clawft-plugin-git`            | archive     | git2 wrappers; no consumer. Strong revival candidate as a kernel builtin tool in 0.8.x.                    |
| `clawft-plugin-npm`            | archive     | npm/yarn lockfile parsing; no consumer.                                                                    |
| `clawft-plugin-oauth2`         | archive     | Generic OAuth2 helper. Only `*-calendar` consumed it; both archive together.                               |
| `clawft-plugin-treesitter`     | **keep**    | Live consumer in `clawft-kernel::assessment`. `publish = true`. No change.                                 |

### Workspace-publishing surface

Net effect on the published surface: **none**. All eight archived
crates already had `publish = false`, so they were never reaching
crates.io. The only published plugin crate (`*-treesitter`) is
unchanged.

## Consequences

### Positive

- `cargo check` workspace becomes ~7 KLOC + transitive deps lighter.
  Wall-clock improvement varies; cold cargo-audit and dependabot runs
  speed up correspondingly.
- The 0.7.0 "what ships" surface is unambiguous: `clawft-plugin` (the
  SDK), `clawft-plugin-treesitter` (the live consumer), and that's it.
- Domain code is preserved verbatim under `crates/archive/` for future
  revival without git-spelunking.
- Closes the handoff TODO from commit `8c08ce0a` and the audit's
  Open Question 1.

### Negative / accepted risks

- Anyone with a working out-of-tree consumer of the archived crates
  (no evidence such consumer exists in the public repo set) will see
  a path-dependency break on `git pull`. Mitigation: the archived
  paths are deterministic (`crates/archive/clawft-plugin-X/`) and the
  crates retain their own `Cargo.toml` so an external consumer can
  point at the archived path until they migrate.
- Future re-add work: if 0.8.x revives any of these, the operation is
  `git mv crates/archive/clawft-plugin-X crates/clawft-plugin-X` and
  re-adding the member + workspace.dependencies entries. Trivial, but
  not zero.
- Dead-code rot: archived crates are not built, so dependency
  upgrades and rustc-edition bumps will let them bit-rot. This is
  intentional — they are frozen at this revision until someone makes
  an active decision to revive them. A `crates/archive/README.md` is
  not added (per the project's "no proactive doc files" rule); the
  rationale lives in this ADR.

### Reversal

To revive a crate, e.g. `clawft-plugin-git`:

```bash
git mv crates/archive/clawft-plugin-git crates/clawft-plugin-git
# Then in Cargo.toml:
#  - re-add "crates/clawft-plugin-git" to [workspace] members
#  - re-add the [workspace.dependencies] entry
#  - bump version to current workspace.package.version (0.6.x -> 0.7.x)
#  - flip publish = true if it should ship
```

## Migration plan

No migration is required. The decision is purely a workspace-layout
change:

1. `git mv crates/clawft-plugin-{browser,calendar,cargo,ci,containers,
   git,npm,oauth2} crates/archive/`
2. Strip those eight names from `Cargo.toml`'s `[workspace] members`.
3. Strip the seven entries that existed in `[workspace.dependencies]`
   (`*-npm` and `*-ci` were members but had no workspace.dependencies
   entries, so only seven removals there).
4. Add `crates/archive` to `[workspace] exclude`.
5. Verify `scripts/build.sh check` exits 0.

This ADR is the entirety of the action.

## Sources

- `.planning/reviews/0.7.0-release-gate/04-plugin-skills.md` (audit
  lines 190-205, 266-272, 301-310, task #3 in the task list)
- Commit `8c08ce0a` (handoff TODO that this closes)
- `Cargo.toml` (workspace members + dependencies, pre-archival)
- `crates/clawft-kernel/Cargo.toml:65` (`treesitter` feature wiring)
- `crates/clawft-kernel/src/assessment/mod.rs:356-370` (only live
  consumer of any plugin crate)
- ADR-036 (kernel hierarchical ToolRegistry — the eventual home for
  some of this code, per ADR-052 migration plan)
- ADR-052 (ToolRegistry split — explains why some archived crates
  may resurface as `BuiltinTool` impls)
