---
title: "Memory & Workspace"
slug: memory-workspace
workstream_id: "06"
release_gate: "0.7.0"
audit_kind: "comprehensive"
status: "complete-for-MVP-with-known-deferrals"
last_updated: 2026-04-28
owner: "agent-08 / agent-08b (legacy); identity work owned by agent-core-v1 stream"
sources_root: "/home/aepod/dev/clawft"
---

# Memory & Workspace

## General Description

This workstream owns everything that lets a clawft agent remember, persist, and run inside a bounded, configurable workspace. It spans four sub-systems:

1. **Memory store** (`MEMORY.md` long-term + `HISTORY.md` session log, plus the optional vector index from H2). Implementation lives in `crates/clawft-core/src/agent/memory.rs` and `crates/clawft-core/src/memory_bootstrap.rs`. All I/O routes through the `Platform::fs()` trait so the same code runs native and WASM.

2. **Sessions** (per-channel/per-chat conversation persistence as JSONL). Implementation in `crates/clawft-core/src/session.rs` plus `crates/clawft-core/src/session_indexer.rs`. Filenames are percent-encoded so colon-separated keys (`telegram:123`) survive round-trip; an underscore-encoding migration path remains for legacy stores.

3. **Identity** (the F1 / F2 deliverables: `<workspace>/.clawft/SOUL.md`, `IDENTITY.md`, `SOUL.journal.md` and the SHA-256 binding-thread descriptor). Implementation in `crates/clawft-core/src/agent/identity.rs`. The promote command lives in `crates/clawft-weave/src/commands/soul_cmd.rs`.

4. **Workspaces & per-workspace policy** (4-step discovery, registry, per-agent isolated `~/.clawft/agents/<id>/` trees, 3-level config merge). Workspace lifecycle in `crates/clawft-core/src/workspace/{mod,agent,config}.rs`; the cwd-relative JSON overlay (Layer 3) added this session lives in `crates/clawft-platform/src/config_loader.rs`.

The workstream pulls together what the SPARC plan called Element 08 (H1 / H2 / H3) plus the post-spike F-phases of agent-core-v1 and the just-shipped `feat(platform): cwd-relative workspace config overlay` (`0452539a`).

## Status & Timeline

| Phase | Scope | Status | Date | Notes |
|-------|-------|--------|------|-------|
| H1 | Per-agent workspace isolation (`~/.clawft/agents/<id>/`, 0700, symlink namespace sharing) | Done | 2026-02-20 | `crates/clawft-core/src/workspace/agent.rs`, ~512 lines + tests |
| H2.1 | HNSW VectorStore (`instant-distance`, 18 tests) | Done | 2026-02-20 | `crates/clawft-core/src/embeddings/hnsw_store.rs` |
| H2.2 | `Embedder` trait + Hash + Api impls + async pipeline | Done | 2026-02-20 | `crates/clawft-core/src/embeddings/mod.rs`, `api_embedder.rs`, `hash_embedder.rs` |
| H2.3 | RVF segment I/O (local JSON fallback after 0.2 audit) | Done | 2026-02-20 | `crates/clawft-core/src/embeddings/rvf_io.rs` (stub still present in `rvf_stub.rs`) |
| H2.4 | `weft memory export/import` | Done | 2026-02-20 | `crates/clawft-cli/src/commands/memory_cmd.rs` |
| H2.5 | POLICY_KERNEL persistence | Done | 2026-02-20 | `crates/clawft-core/src/policy_kernel.rs` |
| H2.6 | WITNESS hash-chain segments | Done | 2026-02-20 | `crates/clawft-core/src/embeddings/witness.rs` |
| H2.7 | Temperature quantization (hot/warm/cold) | Done | 2026-02-20 | `crates/clawft-core/src/embeddings/quantization.rs` |
| H2.8 | WASM micro-HNSW (8KB budget) | Done | 2026-02-20 | `crates/clawft-core/src/embeddings/micro_hnsw.rs` |
| H3 | `DateTime<Utc>` everywhere | Done | 2026-02-20 | Touches `clawft-types/src/{workspace,cron,session}.rs` and downstream |
| F1 | `weaver init` seeds `.clawft/SOUL.md` + `IDENTITY.md` + journal grant | Done | 2026-04 | Per `crates/clawft-core/src/agent/identity.rs` module docs |
| F2 | `weaver soul promote` reads journal, diffs, applies | Done | 2026-04 | `crates/clawft-weave/src/commands/soul_cmd.rs` |
| F3 | WitnessRecord assertions in chat path tests | Done | 2026-04 | `b068b063` and `0fa9d0a3` merge commits |
| Layer 3 | cwd `.clawft/config.json` overlay restored | Done | 2026-04-28 | Commit `0452539a` (this session) |

Status overall: **all planned MVP and post-MVP items shipped**, with a small set of explicit deferrals carrying `v1.1` / `agent-core-v1.1` markers in code (see "What's Left" below). The workstream is no longer the gate — but it carries observable debt that should be tracked even if it doesn't block 0.7.0.

## Released Features

What an end user / operator can rely on today:

- **Workspace discovery**: 4-step algorithm (`CLAWFT_WORKSPACE` env var → walk-up from cwd → `~/.clawft/`) in `crates/clawft-core/src/workspace/mod.rs::discover_workspace`. Falls back gracefully on WASM where `dirs::home_dir()` is unavailable.
- **`weaver init` workspace scaffolding**: creates `.clawft/{sessions,memory,skills,agents,hooks}/`, empty `MEMORY.md` + `HISTORY.md`, `config.json`, top-level `CLAWFT.md`. Registers in `~/.clawft/workspaces.json`.
- **Per-agent isolated workspaces**: `~/.clawft/agents/<agent_id>/{sessions,memory,skills,tool_state}` plus `SOUL.md`, `AGENTS.md`, `USER.md`, `config.toml`. Created with 0700 on Unix. Symlink-based cross-agent shared namespaces with directory traversal validation (`crates/clawft-core/src/workspace/agent.rs::link_shared_namespace`).
- **3-level config merge**: defaults → `~/.clawft/config.json` → `<workspace>/.clawft/config.json` (`workspace::config::load_merged_config_from`). camelCase ↔ snake_case normalization is applied before merge so workspace overrides land on the right keys. `null` overlay deletes an entry — verified in `load_merged_config_mcp_servers`.
- **Layer 3 platform overlay** (`config_loader.rs`): in addition to the workspace-aware path through `WorkspaceManager`, the loader at `clawft-platform` now stacks `weave.toml` → `~/.clawft/config.json` → `./.clawft/config.json`, so daemons launched inside a workspace pick up its policies (channel permissions, routing tiers, identity binding) without a workspace flag.
- **`MemoryStore`**: append-only `MEMORY.md`, `HISTORY.md` with double-newline paragraph separation, case-insensitive substring search across both files, sanitization via `crate::security::sanitize_content`. `with_paths` constructor lets callers (and outside-crate integration tests) bypass the home-dir resolution.
- **`memory_bootstrap` (rvf feature)**: indexes existing `MEMORY.md` by `## ` headers (with paragraph fallback) into an `RvfStore`. Idempotent: skips if the index file already exists. 6 unit tests cover empty / missing / pre-existing index / paragraph splitting / search round-trip.
- **`SessionManager`**: write-through cache + JSONL persistence (`{percent_encoded_key}.jsonl`). Migration path reads legacy underscore-encoded files and copies them on first load. `chain_event!` markers fire on `session.create` / `session.destroy`.
- **`session_indexer.rs`**: derives recency / counts (used by routing).
- **Identity loader**: synchronous `IdentityLoader::current()` returns `Some` only when both `<workspace>/.clawft/SOUL.md` and `IDENTITY.md` are present. `FileIdentityProvider` (async, cached, `RwLock`) re-reads on every call but serves cached value if the disk read fails. SHA-256 hash of `soul + "\n" + identity` is the surfaced descriptor; `BINDING_THREAD_EXCERPT` constant is the soft binding check that downgrades the prompt rather than refusing.
- **`weaver soul promote` / `weaver soul status`**: read pending journal entries from substrate, show diff, apply on confirmation, emit a witness record (locally to `.weftos/audit/soul-promote.log` + tracing event).
- **Sandbox identity protections**: `<workspace>/.clawft/SOUL.md`, `IDENTITY.md`, and `SOUL.journal.md` are denied for write even when the path is otherwise allowlisted. See `crates/clawft-plugin/src/sandbox.rs:371` and the test at `:891`.
- **`SkillsLoader`**: dual format (legacy `skill.json + prompt.md` and the newer `SKILL.md` with YAML frontmatter), extra-dir support, primary-wins precedence, RwLock cache. Resolution: `~/.clawft/workspace/skills/` with `~/.nanobot/workspace/skills/` legacy fallback.
- **`SkillRegistry` / autogen / watcher**: see `crates/clawft-core/src/agent/{skills_v2.rs, skill_autogen.rs, skill_watcher.rs}` (1430 + 804 + 555 lines, no live TODO/FIXME markers as of this audit).
- **Workspace registry atomicity**: `WorkspaceRegistry::save` writes via tmp-file rename; `WorkspaceManager::create` is idempotent across registry-only views.

## What's Left — Total Depth

This section is non-filtered: every TODO, FIXME, deferred design item, open question, and orphaned bit of work that touches memory or workspace is captured.

### Live TODO / FIXME comments (grep-confirmed, in-tree)

| ID | Location | Comment | Severity |
|----|----------|---------|----------|
| WS-T1 | `crates/clawft-core/src/bootstrap.rs:633` | `TODO(v1.1): split workspace from global at the loader layer so we can pass them to PermissionResolver::new(global, Some(workspace)) and let enforce_workspace_ceiling clamp workspace permissions against system-wide bounds. Today the workspace overlay is deep-merged into config.routing upstream in config_loader::load_config_raw, so workspace policy reaches the resolver but the security ceiling pattern is bypassed. Fine for single-user kernels; needed for multi-tenant.` | Medium — single-user OK, blocks the multi-tenant security ceiling |
| WS-T2 | `crates/clawft-weave/src/commands/soul_cmd.rs:246` | `TODO(agent-core-v1.1): replace with chain.append RPC once the daemon's public chain surface gains an append handler; the trait shape is already forward-compatible.` | Medium — local audit log is the durable record until then |
| WS-T3 | `crates/clawft-weave/src/commands/soul_cmd.rs:43` (module docs) | "follow-up TODO is filed below" — points at WS-T2 | Same as WS-T2 |

The four `unimplemented!()` stubs in `crates/clawft-platform/src/config_loader.rs:495-510` are not unfinished work — they sit inside a `MockFs` that intentionally panics on methods the workspace-overlay test suite is not supposed to exercise (`write_string`, `append_string`, `list_dir`, `create_dir_all`, `remove_file`). Documented at line 451-455 of the same file.

### Deferred design items (called out in source comments / module docs)

| ID | Location | Item | Disposition |
|----|----------|------|-------------|
| WS-D1 | `crates/clawft-core/src/agent/identity.rs:32-38` | "**No SOUL.journal write path** — F1 seeds the empty journal file and stamps the soul_journal derived-write grant; F2's weaver soul promote reads it, diffs, and applies on confirmation. The journal is not consulted on every-turn loads." | Deliberate — agent-side write happens via the `soul_journal` substrate topic, not direct fs writes. Any consumer that needs a per-turn read will need to design that path (likely via the same substrate, not a file watcher). |
| WS-D2 | `crates/clawft-core/src/agent/identity.rs:36-39` | "**No hot-reload watcher** — the cached FileIdentityProvider re-reads on every call (small files; cheap). A notify-driven watcher arrives when measurement says it earns its keep." | Open — premature optimization gate; revisit if profile shows identity load on the per-turn hot path. |
| WS-D3 | `crates/clawft-core/src/agent/identity.rs:55-58` | "Hard refusal is a v1.1 follow-up." (binding-thread mismatch currently downgrades to an annotation + warn log, never refuses) | Open — security posture decision: which workspace edits should be allowed to break the binding thread? |
| WS-D4 | `crates/clawft-core/src/agent/identity.rs:79-82` | "future substrate-backed provider can introduce new variants without touching callers" — `Identity::source` is `&'static str` placeholder | Open — substrate-backed identity is on the post-F2 roadmap; the trait is forward-compatible. |
| WS-D5 | `crates/clawft-core/src/agent/identity.rs:86-89` | "Today only signals the 'files missing' case; in future a substrate-backed loader will need to distinguish IO from deserialization errors. Variants stay shaped for forward compatibility." | Open — error variants need to be added when substrate path lands. |
| WS-D6 | `crates/clawft-core/src/agent/identity.rs:175-179` | "The workspace is the daemon CWD by default (plan §15.4 — soon `agent.workspace_root` config key)." | Open — `agent.workspace_root` is not yet a config key. Today the daemon CWD is implicit. Adding it would let the daemon serve multiple workspaces without restart. |

### Open architectural questions (collected from planning + reviews)

| ID | Question | Source |
|----|----------|--------|
| WS-Q1 | Should `--no-hooks` propagate via `WorkspaceContext.hooks_enabled: bool` or via a `WorkspaceManager::new` parameter? Reviewer (3g-review) recommended the latter as cleaner. Current code path has not been re-verified post-3g. | `.planning/reviews/3g-review.md` ISSUE-M3 |
| WS-Q2 | Array merge semantics in workspace config: 07-workspaces.md says "workspace replaces global" but 02-technical-requirements.md says "concat". Resolved in 3G in favor of replacement, but no `+key` append-mode escape hatch was added. | `.planning/reviews/3g-review.md` ISSUE-C1 |
| WS-Q3 | Hierarchical CLAWFT.md walk: should it stop at `.git/` boundary? Currently the plan documents "depth limit 10" but a `.git/`-bounded walk (Claude Code style) is the recommended fix. Reviewer-flagged information-leak risk. | `.planning/reviews/3g-review.md` ISSUE-M2 |
| WS-Q4 | Import regex `r"@([\w./-]+)"` was flagged as too permissive (false positives on email addresses, `@mention`, code-block contents). Tightened pattern not yet applied. | `.planning/reviews/3g-review.md` ISSUE-M1 |
| WS-Q5 | `WorkspaceContext.name` field — present? The reviewer flagged this as missing; if still missing, every consumer is doing `root.file_name()` on demand. | `.planning/reviews/3g-review.md` ISSUE-m2 |
| WS-Q6 | `weft workspace init` (in-place) alias — does it exist? Reviewer recommended adding it for `git init` parity. | `.planning/reviews/3g-review.md` Missing Requirements §2 |
| WS-Q7 | `.gitignore` template content for `--git` — what's actually written? Reviewer recommended: `.clawft/config.json`, `.clawft/sessions/`, `.clawft/memory/`. | `.planning/reviews/3g-review.md` Missing Requirements §3 |
| WS-Q8 | Env-var overlay (`$CLAWFT_*`, priority 5 in the tech-requirements spec) is noted as GAP-27 but is not part of 3G. Should env-var overlay be Layer 4 of `load_config_raw`, applied after the cwd workspace overlay? | `.planning/reviews/3g-review.md` Missing Requirements §1 |
| WS-Q9 | `mcp/` subdirectory (Phase 3H) was reserved but not created by 3G's `WorkspaceManager::create`. Is per-workspace MCP-server config now expected via deep-merge of `.clawft/config.json` instead? | `.planning/reviews/3g-review.md` Cross-Phase Conflicts §3 |
| WS-Q10 | The workspace overlay added in `0452539a` deep-merges into the same JSON tree as the home config. The follow-up `ec7bb2bd` ("thread loaded RoutingConfig to daemon agent loop") suggests an upstream caller was independently throwing away the loaded config. Is there a regression test ensuring routing config from `./.clawft/config.json` reaches `AgentLoop` end-to-end? `tests/overlay_probe.rs` is `#[ignore]`. | `0452539a` commit body |
| WS-Q11 | `MemoryStore::new` resolves `~/.clawft/workspace/memory/` (legacy `~/.nanobot/...`). When called from a workspace-aware context the in-workspace `<root>/.clawft/memory/` is the expected path, not the home directory. Are all `MemoryStore` constructions plumbed through `WorkspaceContext`, or is the agent loop still using home-dir memory? | `crates/clawft-core/src/agent/memory.rs:52-79` |
| WS-Q12 | `SkillsLoader::new` resolves `~/.clawft/workspace/skills/` exclusively (no workspace skills dir param). 3F defines a 4-level skill discovery chain; `add_extra_dir` is the closest hook. Are workspace skills wired in via `add_extra_dir(ctx.skills_dir)` in the bootstrap path? | `crates/clawft-core/src/agent/skills.rs:186-211`; `.planning/reviews/3g-review.md` Cross-Phase Conflicts §1 |

### Orphaned / undocumented work (in-tree but no planning trail)

| ID | Item | Notes |
|----|------|-------|
| WS-O1 | `.planning/development_notes/08-memory-workspace/h1-workspace/{decisions,blockers,difficult-tasks,notes}.md` are all empty templates — no decisions or blockers were ever logged for H1. Same for h2-vector-memory and h3-timestamps. | Empty trail makes post-mortem hard. Decisions for H1 (0700 perms, symlink-only sharing, namespace validation) only live in the source comments and the SPARC plan, not in the running notebook. |
| WS-O2 | `crates/clawft-core/src/embeddings/rvf_stub.rs` and `crates/clawft-core/src/embeddings/rvf_io.rs` both ship. The plan says rvf_stub gets *replaced* by rvf_io; if both are alive, callers need to know which to use. | `find` says both files exist. Worth a follow-up to either remove `rvf_stub.rs` or document that `rvf_stub` is the brute-force fallback for the no-`rvf`-feature build. |
| WS-O3 | `MemoryStore` and `SkillsLoader` use the legacy `~/.clawft/workspace/memory` and `~/.clawft/workspace/skills` paths (not the workspace-aware `<workspace>/.clawft/memory`). In a workspace-loaded daemon there are now two memory roots — the workspace-scoped one created by `WorkspaceManager::create` and the home-scoped one used by `MemoryStore::new`. | This is technically a known gap (the loader was added before workspace-scoping went in), and there is no documented migration plan. The workaround is to construct `MemoryStore::with_paths(workspace.memory_path, ...)` explicitly, but no central code path enforces it. |
| WS-O4 | `WorkspaceEntry.last_accessed` and `created_at` are populated on `create` but the only update path in-tree is in `create`. `load` doesn't touch `last_accessed`. The 3G plan implied `load` would update it. | Real impact: `weft workspace list --by-recency` would show creation order, not last-use order. |
| WS-O5 | `chain_event!` for `session.create` fires from `get_or_create` only when neither cache nor disk has the session. There is no `session.update` event for appended turns, only on the file-not-existing branch of `append_turn`. | Audit-trail completeness: a long-running session looks like one create event followed by a destroy. The hot loop activity is invisible to the chain. |
| WS-O6 | `MemoryStore::search` is purely substring + case-insensitive. `memory_bootstrap` builds a vector index in parallel. There is no glue that calls `memory_bootstrap` from the same place that creates `MemoryStore`, so the two memory views can drift (vector index built once at bootstrap, MEMORY.md edited later). | Open: who runs the rebuild? `memory_bootstrap` is idempotent and skips if the index exists, which means edits to MEMORY.md are *not* reflected in vector search until the index is manually deleted. |
| WS-O7 | Per-agent workspace `tool_state/` subdirectory is created but no consumer in `clawft-core` writes there. Plugins are presumably the consumers (Contract 3.1). | Open: confirm the plugin sandbox grants per-agent `tool_state/` paths and no other directory. |
| WS-O8 | `SessionManager` migration path (`old_filename` with `_` instead of `:`) reads the old file and writes the new one, but never removes the old file ("keep old for safety"). After several sessions this leaves orphaned files in `sessions/`. | Open: ship a `weft session gc` or document manual cleanup. |
| WS-O9 | `IdentityLoader::current` uses `std::fs::read_to_string` directly instead of going through `Platform::fs()`. This is the only sync, platform-bypassing read in the agent identity path, which means it cannot be exercised in WASM the same way the rest of the agent loop is. | Discrepancy with the platform abstraction principle. Documented behavior, not a bug, but worth noting. |
| WS-O10 | `WorkspaceManager::delete` removes the registry entry but explicitly does *not* delete files on disk ("that is the caller's responsibility"). FR-W06 says `--keep-data` is the *opt-in* — i.e. default should remove `.clawft/` + `CLAWFT.md`. Today's behavior is the opposite of the spec. | Spec/code drift. |
| WS-O11 | The `tests/overlay_probe.rs` integration test for the Layer 3 overlay (`0452539a`) is `#[ignore]` because it touches the real filesystem. The overlay's only non-ignored coverage is via mocked `FileSystem` and the `home_dir` returns `None` path. | End-to-end coverage of "workspace's `.clawft/config.json` actually reaches the running daemon" is currently absent from CI. |

### Open questions for ship-gate decisioning (not blocking 0.7.0, but log-worthy)

- Should WS-T1 (loader-layer global/workspace split + `enforce_workspace_ceiling`) be promoted from `v1.1` to a 0.8.0 commitment? Single-user-only is a sharp cliff.
- Should WS-T2 (substitute audit log → `chain.append` RPC) ship inside the next `agent-core` minor or wait for the daemon's chain surface to land?
- Are WS-Q11 / WS-Q12 / WS-O3 (workspace-vs-home memory and skills paths) one bug or three? They share a root cause: `MemoryStore::new` and `SkillsLoader::new` were written before workspace-aware bootstrap and never migrated.

## Task List

Sized for follow-up sprints. Numbered for cross-reference; not ordered by priority.

1. **MW-1** — Resolve WS-O3 / WS-Q11 / WS-Q12: route `MemoryStore` and `SkillsLoader` construction through the loaded `WorkspaceContext` so they hit `<workspace>/.clawft/{memory,skills}` instead of `~/.clawft/workspace/...`. Add a regression test that loads a workspace, writes to MEMORY.md, and verifies the *workspace* file changed (not the home dir).
2. **MW-2** — Land WS-T1: split workspace and global config at the loader layer, pass both into `PermissionResolver::new(global, Some(workspace))`, wire `enforce_workspace_ceiling`. Required for any multi-tenant deployment.
3. **MW-3** — Land WS-T2: implement the daemon's `chain.append` RPC and have `weaver soul promote` route through it; keep the local `.weftos/audit/soul-promote.log` as a redundant durable record.
4. **MW-4** — Convert `tests/overlay_probe.rs` from `#[ignore]` to a hermetic test that creates a temp workspace, drops a `.clawft/config.json` with a sentinel value, runs the loader, and asserts the sentinel reaches the parsed `Config`.
5. **MW-5** — Resolve WS-D6: add `agent.workspace_root` config key, default to daemon CWD, and use it in `IdentityLoader::new`. Document the implication for daemons that should serve multiple workspaces.
6. **MW-6** — WS-O6: rebuild `memory.rvf.json` when `MEMORY.md` changes. Either (a) check mtime in `bootstrap_memory_index` and re-index on staleness, or (b) add a `weft memory reindex` command. Today edits to MEMORY.md silently fail to land in vector search.
7. **MW-7** — WS-O5: emit `chain_event!` for `session.append` (or `session.update`) on every appended turn, with a sample-rate cap if the volume is too high. Required for chain-based audit completeness.
8. **MW-8** — WS-O10: align `WorkspaceManager::delete` with FR-W06 — default to removing `.clawft/` + `CLAWFT.md`, opt out via `--keep-data`. Spec/code drift fix.
9. **MW-9** — WS-O8: ship `weft session gc` (or have the migration path delete the old underscore-encoded file once the percent-encoded copy is verified).
10. **MW-10** — WS-O4: have `WorkspaceManager::load` update `last_accessed`. Required for `weft workspace list --by-recency`.
11. **MW-11** — Backfill `.planning/development_notes/08-memory-workspace/{h1,h2-vector-memory,h3-timestamps}/{decisions,blockers}.md`. Empty post-mortem trail makes the *next* refactor blind.
12. **MW-12** — WS-Q1, WS-Q2, WS-Q3, WS-Q4, WS-Q5, WS-Q6, WS-Q7, WS-Q8, WS-Q9: re-walk `.planning/reviews/3g-review.md` against current code, mark each ISSUE as either fixed (with commit), still-open (with task ID), or won't-do (with rationale). Today the review is a frozen 2026-02-17 snapshot with no closure markers.
13. **MW-13** — WS-D2: decide whether `FileIdentityProvider` needs a `notify`-based watcher. If yes, ship it; if no, delete the deferred-doc and close the question.
14. **MW-14** — WS-D3: decide the binding-thread-mismatch policy for v1.1 (refuse vs. annotate). Currently we annotate; reviewer-flagged as a security posture decision.
15. **MW-15** — WS-O2: pick a fate for `embeddings/rvf_stub.rs` vs. `embeddings/rvf_io.rs`. Either remove the stub or document that it's the no-feature-flag fallback.
16. **MW-16** — WS-O7: write or document the consumer of per-agent `tool_state/`. If no plugin uses it, drop the subdirectory from `AGENT_WORKSPACE_SUBDIRS`.
17. **MW-17** — WS-O9: route `IdentityLoader::current` through `Platform::fs()` so it works on WASM the same way the rest of the agent identity flow does.

## Sources

- `/home/aepod/dev/clawft/crates/clawft-core/src/agent/memory.rs` (468 lines)
- `/home/aepod/dev/clawft/crates/clawft-core/src/agent/skills.rs` (862 lines)
- `/home/aepod/dev/clawft/crates/clawft-core/src/agent/identity.rs` (378 lines)
- `/home/aepod/dev/clawft/crates/clawft-core/src/agent/system_prompt.rs` (299 lines)
- `/home/aepod/dev/clawft/crates/clawft-core/src/agent/skills_v2.rs` (1430 lines)
- `/home/aepod/dev/clawft/crates/clawft-core/src/agent/skill_autogen.rs` (804 lines)
- `/home/aepod/dev/clawft/crates/clawft-core/src/agent/skill_watcher.rs` (555 lines)
- `/home/aepod/dev/clawft/crates/clawft-core/src/session.rs` (848 lines)
- `/home/aepod/dev/clawft/crates/clawft-core/src/session_indexer.rs` (445 lines)
- `/home/aepod/dev/clawft/crates/clawft-core/src/workspace/mod.rs` (472 lines)
- `/home/aepod/dev/clawft/crates/clawft-core/src/workspace/agent.rs` (511 lines)
- `/home/aepod/dev/clawft/crates/clawft-core/src/workspace/config.rs` (276 lines)
- `/home/aepod/dev/clawft/crates/clawft-core/src/memory_bootstrap.rs` (428 lines)
- `/home/aepod/dev/clawft/crates/clawft-core/src/vector_store.rs` (440 lines)
- `/home/aepod/dev/clawft/crates/clawft-core/src/clawft_md.rs`
- `/home/aepod/dev/clawft/crates/clawft-core/src/bootstrap.rs` (lines 615-665, plus the WS-T1 TODO)
- `/home/aepod/dev/clawft/crates/clawft-platform/src/config_loader.rs` (594 lines, Layer 3 overlay at lines 124-157)
- `/home/aepod/dev/clawft/crates/clawft-types/src/config/mod.rs` (`workspace_path` / `workspace_path_with_home` at lines 121-151)
- `/home/aepod/dev/clawft/crates/clawft-types/src/workspace.rs`
- `/home/aepod/dev/clawft/crates/clawft-types/src/session.rs`
- `/home/aepod/dev/clawft/crates/clawft-plugin/src/sandbox.rs` (`SOUL.md` / `IDENTITY.md` / `SOUL.journal.md` deny rules at line 371 onward)
- `/home/aepod/dev/clawft/crates/clawft-weave/src/commands/soul_cmd.rs` (`soul promote/status` and the WS-T2 TODO)
- `/home/aepod/dev/clawft/.planning/sparc/phase4/08-memory-workspace/00-orchestrator.md`
- `/home/aepod/dev/clawft/.planning/sparc/phase4/08-memory-workspace/01-phase-H1H3-workspace-timestamps.md`
- `/home/aepod/dev/clawft/.planning/sparc/phase4/08-memory-workspace/02-phase-H2-hnsw-embedder.md`
- `/home/aepod/dev/clawft/.planning/sparc/phase4/08-memory-workspace/03-phase-H2-advanced-rvf-witness-quantization.md`
- `/home/aepod/dev/clawft/.planning/sparc/phase4/08-memory-workspace/04-element-08-tracker.md`
- `/home/aepod/dev/clawft/.planning/development_notes/08-memory-workspace/README.md`
- `/home/aepod/dev/clawft/.planning/development_notes/08-memory-workspace/h1-workspace/{decisions,blockers,difficult-tasks,notes}.md` (all empty)
- `/home/aepod/dev/clawft/.planning/development_notes/08-memory-workspace/h2-vector-memory/{decisions,blockers,difficult-tasks,notes}.md` (all empty)
- `/home/aepod/dev/clawft/.planning/development_notes/08-memory-workspace/h3-timestamps/{decisions,blockers,difficult-tasks,notes}.md` (all empty)
- `/home/aepod/dev/clawft/.planning/reviews/3g-review.md` (Phase 3G workspace review, 2026-02-17, APPROVE_WITH_CHANGES)
- Git commits: `0452539a feat(platform): cwd-relative workspace config overlay`, `ec7bb2bd fix(core,weave): thread loaded RoutingConfig to daemon agent loop`, `cb947080 feat(weaver): add --update flag to init`, `b068b063 feat(weaver): soul promote command`, `187642c9 feat(weaver): init seeds .clawft/SOUL.md + IDENTITY.md + journal grant`, `6111b4a1 feat(service-agent): WitnessRecord assertions in chat path tests`

<!-- TRIAGED-STAMP:BEGIN -->
## Triaged into Plane — 2026-04-28

All open items in this audit have been filed as Plane work items in the WeftOS workspace under the `ws06-memory` label.

- **Range**: WEFT-79 … WEFT-97 (19 items)
- **Per cycle**: 0.7.x: 9, 0.8.x: 8, 0.9.x: 2
- **Triage spec**: `.planning/reviews/0.7.0-release-gate/triage/`
- **WEFT-N → name map**: `.planning/reviews/0.7.0-release-gate/triage/weft-mapping.json`

Per the project rule (CLAUDE.md → "Plane is the authoritative work tracker"): future updates to these items happen in Plane, not in this audit doc. This doc remains the source-of-truth for the original survey.
<!-- TRIAGED-STAMP:END -->
