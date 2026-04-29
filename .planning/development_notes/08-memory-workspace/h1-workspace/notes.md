# H1: Per-Agent Workspace Isolation -- Notes

> Backfilled 2026-04-28 from `crates/clawft-core/src/workspace/agent.rs`
> (~512 lines + tests), the SPARC plan, and the audit. Use this file
> for ongoing development findings going forward.

## Implementation map

- `crates/clawft-core/src/workspace/mod.rs` -- discovery, registry,
  3-level config merge, `WorkspaceContext`.
- `crates/clawft-core/src/workspace/agent.rs` -- per-agent isolation,
  `ensure_agent_workspace`, `delete_agent_workspace`,
  `list_agent_workspaces`, `link_shared_namespace`.
- `crates/clawft-core/src/workspace/config.rs` -- 3-level merge
  (defaults -> `~/.clawft/config.json` -> `<workspace>/.clawft/config.json`).
- `crates/clawft-platform/src/config_loader.rs` -- Layer 3 platform
  overlay (added `0452539a`, 2026-04-28); stacks `weave.toml` ->
  `~/.clawft/config.json` -> `./.clawft/config.json` for daemons that
  launch inside a workspace.

## Useful invariants

- Every public method on `WorkspaceManager` that derives a path from
  `agent_id` calls `validate_agent_id` first. If you add a new method,
  keep that habit.
- `AGENT_WORKSPACE_SUBDIRS` is the source of truth for directory layout
  (`sessions`, `memory`, `skills`, `tool_state`). Changing it changes
  the documented contract; ripple updates to
  `docs/guides/workspaces.md` and the WEFT-94 contract section.
- Tests use `temp_registry` (`crates/clawft-core/src/workspace/tests.rs`)
  to get a registry under a unique tmp path. Never hit `~/.clawft/`
  from tests.
- `ensure_agent_workspace` is idempotent. Custom content (e.g. an
  edited `SOUL.md`) survives repeated calls.

## Known follow-ups (see audit + Plane)

- WS-T1 / WEFT-MW-2 -- multi-tenant config ceiling.
- WS-O3 / WEFT-MW-1 -- route `MemoryStore` + `SkillsLoader` through
  `WorkspaceContext`.
- WS-O4 / WEFT-MW-10 -- `WorkspaceManager::load` should bump
  `last_accessed`.
- WS-O10 / WEFT-MW-8 -- align `delete` default with FR-W06.
- WS-O7 / WEFT-94 -- document `tool_state/` contract (resolved this
  commit).

## Tips

- When testing per-agent symlinks, prefer `tempfile::tempdir()` over
  hand-rolled `/tmp` paths so cleanup is deterministic.
- `set_dir_permissions_0700` is a no-op on Windows. Don't assert mode
  bits in cross-platform tests.
- `weft workspace status` reads from the registry; if state looks
  stale, check that you're using `WorkspaceManager::with_registry_path`
  pointing at the expected file.
