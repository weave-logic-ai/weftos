# H1: Per-Agent Workspace Isolation -- Difficult Tasks

> Backfilled 2026-04-28. Captures the two H1 sub-tasks that the audit
> and the 3G review flagged as the highest-risk areas during
> implementation.

## 2026-02-20 Difficult: symlink-based cross-agent shared namespaces

**Item**: H1 (`link_shared_namespace`)
**Difficulty**: High
**Why**: Symlinks are the right composition primitive but they open a
directory-traversal vector. A "share team-knowledge" call where the
target points (directly or via a symlink chain) at another agent's
private memory has to be refused, not followed.
**Approach**: Canonicalize the target via `Path::canonicalize`, assert
that the canonical path lives under the agents-root, and refuse
otherwise. Validation is on the link target *and* the resolved path
of any intermediate symlinks.
**Findings**: `tokio::fs::canonicalize` would have made this async-
friendly but the rest of `WorkspaceManager` is sync; H1 stays sync.
The traversal-rejection test
(`link_shared_namespace_rejects_traversal`) is the load-bearing test
here -- if it ever flakes, the security posture has slipped.

## 2026-02-20 Difficult: 3-level config merge with key normalization

**Item**: H1 / `workspace::config::load_merged_config_from`
**Difficulty**: High
**Why**: The merge takes raw JSON workspace config and the already-
normalized global config, deep-merges them, and reserializes to
`Config`. If `normalize_keys` doesn't run on the workspace JSON
*before* the merge, camelCase workspace keys
(e.g. `"maxTokens"`) silently fail to override snake_case global
keys (`"max_tokens"`) -- the merge is a no-op on a key that exists.
The 3G review (ISSUE-C2) called this out before implementation.
**Approach**: Normalize the workspace JSON first, deep-merge, then
deserialize. `null` overlay deletes a key. The
`load_merged_config_mcp_servers` test fixes this contract for
the `mcp_servers` HashMap case.
**Findings**: The merge interaction with `mcp_servers` (HashMap, not
Vec) was easy; the unresolved tension is `Vec<T>` config fields
(e.g. `CommandPolicyConfig.allowlist`) where workspace replacement
silently drops global entries (ISSUE-C1). The current behavior is
*replace*, deliberately. A `+key` append-mode escape hatch is a
future enhancement (filed as part of WEFT MW-12 / WEFT-90).
