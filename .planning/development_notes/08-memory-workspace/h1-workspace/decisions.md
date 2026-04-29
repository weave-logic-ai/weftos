# H1: Per-Agent Workspace Isolation -- Decisions

> Backfilled 2026-04-28 from source comments, the SPARC plan
> (`.planning/sparc/phase4/08-memory-workspace/01-phase-H1H3-workspace-timestamps.md`),
> the audit (`.planning/reviews/0.7.0-release-gate/06-memory-workspace.md`),
> and git history (`a67b9e5c`, `0a4108c2`). The original H1 work shipped
> without running notes; the file existed as an empty template until WEFT-89.

## 2026-02-20 Decision: 0700 permissions on every per-agent directory

**Context**: Per-agent workspaces under `~/.clawft/agents/<agent_id>/` carry
SOUL.md, AGENTS.md, USER.md, and live session/memory state. They sit on
shared developer laptops and shared CI runners. Accidental world-readability
would leak agent personas and session transcripts.
**Options**:
1. Default `0755` and document tightening as an operator concern.
2. Apply `0700` to the agent root and every subdirectory at create time.
3. Apply `0700` only to the agent root, rely on inheritance.
**Decision**: Option 2 -- explicit `0700` on the agent root and on every
subdirectory in `AGENT_WORKSPACE_SUBDIRS` at `ensure_agent_workspace` time.
**Rationale**: Inheritance is unreliable across `tmpfs`, network mounts,
and umask-altered shells. Explicit per-directory permission set is cheap
and reproducible. Tests assert mode `0o700` post-create.
**Consequences**: Cross-user sharing on the same host now requires an
explicit symlink with traversal validation (see `link_shared_namespace`).
Windows is a no-op via `#[cfg(unix)]`; Windows ACL hardening is a future
follow-up. Implementation: `crates/clawft-core/src/workspace/agent.rs`,
`set_dir_permissions_0700`.

## 2026-02-20 Decision: symlink-only cross-agent shared namespaces

**Context**: Agents in a swarm sometimes need to share a memory namespace
(`shared/team-knowledge/`) without copying state. Naive options open the
door to directory-traversal attacks where one agent points a "shared"
namespace at another agent's private memory.
**Options**:
1. Hard-copy the namespace into each agent's tree (no sharing semantics).
2. Symlink the agent's namespace dir at a fixed shared root, validated.
3. Bind-mount via OS facilities (Linux-only; complicates CI).
**Decision**: Option 2 -- symlink-based shared namespaces with explicit
canonicalization + traversal validation in `link_shared_namespace`. The
target must canonicalize under the agents root; targets that escape are
refused.
**Rationale**: Symlinks compose with the existing 0700 trees, work the
same on macOS and Linux, and make the sharing relationship visible in
`ls -la`. Bind-mounts would force a privileged setup step.
**Consequences**: Read/write semantics follow filesystem permissions on
the *target*, not the symlink. Cross-host sharing is out of scope and
remains a future "substrate-backed" path. Implementation: see
`crates/clawft-core/src/workspace/agent.rs::link_shared_namespace` and
its test `link_shared_namespace_rejects_traversal`.

## 2026-02-20 Decision: agent_id charset validation

**Context**: `agent_id` is concatenated into a filesystem path
(`~/.clawft/agents/<agent_id>/`). User-supplied IDs from CLI flags, API
calls, and config files reach this path resolver. Arbitrary IDs are a
path-injection surface (`..`, `/`, `\0`, leading `.`).
**Options**:
1. Reject only path separators.
2. Allowlist `[a-z0-9_-]` and a 1..64 length cap.
3. Hash the ID (lose human readability; complicates debugging).
**Decision**: Option 2 -- allowlist `[a-z0-9_-]`, 1..64 chars, no leading
dot. Validation lives in `validate_agent_id`; every public method on
`WorkspaceManager` that derives a path from the ID calls it first.
**Rationale**: Keeps directories readable for operators, makes the rule
explicit, and matches Unix usernames. `..` and `/` are excluded by
construction.
**Consequences**: Migration of existing IDs that don't fit the rule is
the caller's responsibility; the validator returns a typed error so CLI
can surface a fix-up message. Implementation: `validate_agent_id` in
`crates/clawft-core/src/workspace/agent.rs`.

## 2026-02-20 Decision: idempotent `ensure_agent_workspace`

**Context**: Multiple call sites bootstrap the same agent (chat loop,
`weft agent run`, the multi-agent router). A non-idempotent create would
either error on the second call or silently clobber an in-flight
`SOUL.md`.
**Options**:
1. Hard-create, error on existing.
2. Idempotent create that only writes templates if the file is missing.
3. Replace-on-create with content-hash skip.
**Decision**: Option 2 -- `ensure_agent_workspace` is fully idempotent.
Templates land only on first create; existing custom content is
preserved.
**Rationale**: The L2 multi-agent router (Element 09) calls this on every
agent dispatch. Operators edit `SOUL.md` by hand. Either of the other
behaviors loses work or fights with the operator.
**Consequences**: Test `ensure_agent_workspace_is_idempotent` enforces
this contract. Templates that change shape later cannot retroactively
update existing workspaces; that's acceptable for personality files.

## 2026-02-20 Decision: per-agent `tool_state/` subdirectory created eagerly

**Context**: Element 04 Contract 3.1 specifies that the host backs the
`KeyValueStore` plugin trait at
`~/.clawft/agents/<agent_id>/tool_state/<plugin_name>/`. No live host
implementation existed at H1 time.
**Options**:
1. Don't create the directory; the (future) host creates it on first
   write.
2. Create it eagerly so the layout is visible to operators and the
   sandbox path-grant rules can lock it down.
3. Make it lazy + tracked by a config flag.
**Decision**: Option 2 -- eager create as part of `AGENT_WORKSPACE_SUBDIRS`.
**Rationale**: The directory is part of the documented contract, so
making it visible at create time is more honest than hiding it. Sandbox
rules can pre-grant it. Operators see the layout once and don't have to
guess.
**Consequences**: Empty `tool_state/` looks like dead weight in tools
that grep for orphan dirs (this is exactly what WS-O7 / WEFT-94 / MW-16
flagged). The current resolution: keep the directory, document the
contract in `docs/guides/workspaces.md`. Revisit when the host-backed
KV implementation lands.

## 2026-04-28 Decision: backfill empty post-mortem trail

**Context**: Per WS-O1 / WEFT-89 / MW-11, the `decisions.md`,
`blockers.md`, `difficult-tasks.md`, and `notes.md` files for H1, H2,
and H3 shipped as empty templates. The actual decisions only lived in
source comments and the SPARC plan, which makes future audits blind.
**Options**:
1. Replace each empty file with an `ARCHIVED.md` pointer.
2. Backfill from source comments + SPARC plan + audit.
3. Leave empty.
**Decision**: Option 2 -- backfill. Pointers would still leave the
template files empty.
**Rationale**: Future readers (incl. agent swarms) need to see the
historical reasoning even if it wasn't recorded in real time. The
backfill is explicitly dated and sourced.
**Consequences**: All four files in each phase dir have at least a
one-paragraph entry referencing the canonical source. The audit-doc
remains the source-of-truth for the original survey; these notes are
the running record going forward.
