# H1: Per-Agent Workspace Isolation -- Blockers

> Backfilled 2026-04-28 from the audit
> (`.planning/reviews/0.7.0-release-gate/06-memory-workspace.md`,
> rows WS-T1, WS-O3, WS-O7, WS-O10) and the source TODO at
> `crates/clawft-core/src/bootstrap.rs:633`. No real-time blockers
> were captured; the entries below summarize the structural issues
> that surfaced during H1 implementation and were deferred rather
> than resolved.

## 2026-02-20 Blocker: workspace/global config not split at the loader layer

**Item**: H1 (intersects with Element 09 multi-tenant)
**Severity**: Medium
**Description**: `PermissionResolver::new(config)` takes a single merged
`Config`. The plan's "global vs workspace" ceiling pattern
(`enforce_workspace_ceiling(global, workspace)`) is bypassed because the
merge happens upstream in `config_loader::load_config_raw` before the
resolver ever sees the two layers. Single-user kernels are fine; any
multi-tenant deployment leaks workspace permissions past the global
ceiling.
**Attempted**: H1 ships with the merged config. The TODO at
`bootstrap.rs:633` (WS-T1) documents the structural fix. No multi-tenant
deployment has needed it yet.
**Needs**: Split workspace and global at the loader layer, pass both
into `PermissionResolver::new(global, Some(workspace))`, wire
`enforce_workspace_ceiling`. Filed as WEFT (MW-2) for 0.8.x.
**Status**: Active (not blocking 0.7.0; blocking multi-tenant).

## 2026-02-20 Blocker: `MemoryStore` and `SkillsLoader` use legacy home paths

**Item**: H1 (intersects WS-O3 / WS-Q11 / WS-Q12)
**Severity**: Medium
**Description**: `MemoryStore::new` resolves `~/.clawft/workspace/memory/`
and `SkillsLoader::new` resolves `~/.clawft/workspace/skills/`. Both
were written before workspace-aware bootstrap. In a workspace-loaded
daemon there are now *two* memory roots: the workspace-scoped
`<root>/.clawft/memory/` and the home-scoped legacy path. No central
code path enforces the workspace-aware constructors.
**Attempted**: `MemoryStore::with_paths` exists as the explicit override.
Bootstrap could plumb `WorkspaceContext.memory_path` through, but H1
shipped without that wiring. The audit (WS-O3) and 3G review
(Cross-Phase Conflicts §1) both flagged it.
**Needs**: Route `MemoryStore` and `SkillsLoader` construction through
the loaded `WorkspaceContext` so they hit `<workspace>/.clawft/{memory,
skills}` instead of `~/.clawft/workspace/...`. Filed as WEFT (MW-1).
**Status**: Active.

## 2026-02-20 Blocker: `WorkspaceManager::delete` defaults disagree with FR-W06

**Item**: H1 (spec/code drift)
**Severity**: Low
**Description**: `WorkspaceManager::delete` removes the registry entry
but does not delete files on disk ("the caller's responsibility").
FR-W06 says `--keep-data` is the *opt-in* -- i.e. the default should
remove `.clawft/` + `CLAWFT.md`. The current behavior is the opposite.
**Attempted**: The CLI command did not surface this until the audit.
**Needs**: Align `WorkspaceManager::delete` with FR-W06: default to
removing `.clawft/` + `CLAWFT.md`, opt out via `--keep-data`. Filed as
WEFT (MW-8).
**Status**: Active.

## 2026-04-28 Blocker (informational): `tool_state/` has no live consumer

**Item**: H1 / Contract 3.1 (Element 04)
**Severity**: Low (documentation)
**Description**: Per-agent `tool_state/` subdirectory is created but no
in-tree plugin host implements `KeyValueStore` against it. Plugins ship
with `MockKvStore` test fixtures only. Operators see an empty directory
and have to read SPARC plans to learn the contract.
**Attempted**: Documentation in source comments
(`crates/clawft-plugin/src/traits.rs::KeyValueStore`) plus the SPARC
plan. No user-facing doc until WEFT-94.
**Needs**: User-facing contract doc in `docs/guides/workspaces.md` so
operators understand why the directory exists. Resolved as part of
WEFT-94.
**Status**: Resolved (see WEFT-94, this commit).
