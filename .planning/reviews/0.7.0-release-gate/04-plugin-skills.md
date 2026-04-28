---
title: "Plugin & Skills System"
slug: plugin-skills
workstream_id: "04"
status: shipped-with-gaps
audit_scope: comprehensive
release_target: "0.7.0"
last_updated: 2026-04-28
auditor: claude-opus-4-7
---

# Plugin & Skills System -- Workstream 04 Audit

## General Description

Workstream 04 covers the unified plugin architecture for clawft: the
`clawft-plugin` trait crate (six core extension points -- Tool,
ChannelAdapter, PipelineStage, Skill, MemoryBackend, VoiceHandler), the
WASM plugin host (`clawft-wasm`), the skill loader / hot-reloader
(`skills_v2.rs` + `skill_watcher.rs`), the skill autogen pipeline
(`skill_autogen.rs` -> `~/.clawft/skills/<name>/.pending`), the tool
registry (`clawft-core::tools::registry`), the per-agent sandbox policy
(`clawft-plugin::sandbox::SandboxPolicy`), and the family of native
plugin crates (browser, calendar, cargo, ci, containers, git, npm,
oauth2, treesitter) plus the built-in tool implementations in
`clawft-tools`.

The system was scheduled as Phase-4 Element 04 (C1-C7 + C4a) and is
marked **100% complete** in the SPARC tracker. In practice the
foundation traits, sandbox, registry, autogen pipeline, hot-reload, and
MCP exposure are all real and well-tested; the gaps are around
**operator-facing surface** (no `weft skills approve` CLI, no
collision/intersection enforcement on `allowed-tools`, two competing
registry implementations, eight orphaned native plugin crates).

## Status & Timeline

| Phase | Item | Status | Landed |
|-------|------|--------|--------|
| C1    | `clawft-plugin` trait crate (6 traits + manifest) | shipped | 2026-02-19 |
| C2    | WASM plugin host (wasmtime, WIT, host fns, sandbox) | shipped | 2026-02-20 |
| C3    | Skill loader (serde_yaml, discovery, auto-reg) | shipped | 2026-02-20 |
| C4    | Hot-reload watcher + `weft skill install` | shipped | 2026-02-20 |
| C4a   | Autonomous skill creation (pattern detect + .pending) | shipped (opt-in) | 2026-02-20 |
| C5    | Slash-command framework + collision detection | shipped | 2026-02-20 |
| C6    | MCP `tools/list` + `tools/call` exposure | shipped | 2026-02-20 |
| C7    | PluginHost unification (channel migration, SOUL.md inject) | shipped | 2026-02-20 |
| ADR-036 | Hierarchical kernel ToolRegistry (with_parent, signing) | shipped | 2026-04-03 |
| native plugin crates | clawft-plugin-{browser,calendar,cargo,ci,containers,git,npm,oauth2,treesitter} | shipped (orphaned -- only treesitter wired) | 0.6.x |

Sources of truth:
- `.planning/sparc/phase4/04-plugin-skill-system/07-element-04-tracker.md` (says 100%, 45/45 sec tests)
- `.planning/sparc/phase4/04-plugin-skill-system/00-orchestrator.md` (one open exit-criteria: shell-skill approval)
- `docs/adr/adr-036-hierarchical-tool-registry.md` (kernel registry rewrite)

## Released Features

- **Six plugin traits** in `crates/clawft-plugin/src/traits.rs:82` (Tool,
  ChannelAdapter, PipelineStage, Skill, MemoryBackend, VoiceHandler)
  with `KeyValueStore`, `ToolContext`, `ChannelAdapterHost` supports.
- **PluginManifest schema** with PluginCapability, PluginPermissions,
  PluginResourceConfig (`crates/clawft-plugin/src/manifest.rs:13`).
- **Per-agent SandboxPolicy** with canonicalize-prefix path checks,
  symlink-escape rejection, identity hard-deny for `.clawft/SOUL.md`,
  `.clawft/IDENTITY.md`, `.clawft/SOUL.journal.md`
  (`crates/clawft-plugin/src/sandbox.rs:239`,
  `:256`, `:386`). 25+ unit tests covering traversal, symlinks,
  trailing-slash, not-yet-existing targets.
- **WASM host with 5 typed host imports** (`http-request`, `read-file`,
  `write-file`, `get-env`, `log`) plus 3 plugin exports (`init`,
  `execute-tool`, `describe`); fuel metering (default 1B units),
  StoreLimits memory cap (default 16MB), epoch-interrupt wall clock,
  audit logging on every host fn (`crates/clawft-wasm/src/{engine,
  sandbox, fs, http, env, audit}.rs`). 45/45 security tests
  (T01-T32, T37-T45) passing.
- **Skill loader with hot-reload**: `notify`-based file watcher,
  500ms debounce, `tokio::sync::RwLock`-protected
  `SharedSkillRegistry`, atomic swap on rebuild
  (`crates/clawft-core/src/agent/skill_watcher.rs`).
  Skill precedence workspace > user (`~/.clawft/skills`) > builtin.
- **Tool registry**
  (`crates/clawft-core/src/tools/registry.rs:339`): glob-based
  `tool_access`/`tool_denylist`, ToolMetadata-driven permission level &
  custom permission gating, MCP metadata extraction
  (`extract_mcp_metadata`), `schemas_for_tools(&[String])` for per-skill
  filtering, `filtered_tools(allow, deny)` for per-agent overlays,
  `chain_event!(EVENT_KIND_TOOL_REGISTER)` audit on register. 50+ tests.
- **Tool family** (`clawft-tools`): file_tools (read/write/edit/
  list_directory), shell_tool (native-exec gated), spawn_tool,
  memory_tool (read/write), web_search, web_fetch with UrlPolicy SSRF
  guard, voice tools (voice-feature-gated), render_ui (canvas-feature),
  delegate_tool (delegate feature). Wired via `register_all` in
  `crates/clawft-tools/src/lib.rs:65`.
- **Skill autogen pipeline** (`crates/clawft-core/src/agent/
  skill_autogen.rs`): `PatternDetector` with sliding-window pattern
  counting, configurable threshold (default 3) & max_pending (default
  10), `generate_skill_md` writing `user-invocable: false` +
  `autogenerated: true` SKILL.md frontmatter,
  `install_pending_skill` writing `.pending` marker,
  `approve_skill`/`reject_skill` toggling state,
  `improve_skill_instructions` mutating prompt via TrajectoryLearner.
  Disabled by default; enabled via `AutogenConfig{enabled: true}`.
  Wired into agent loop at `crates/clawft-core/src/agent/loop_core.rs:1165`.
- **MCP skill exposure**: `SkillToolProvider` registered in the
  `composite` MCP server
  (`crates/clawft-services/src/mcp/composite.rs:297`,
  `crates/clawft-cli/src/commands/mcp_server.rs:115`); supports
  `tools/list` namespacing and hot-reload propagation.
- **CLI surface**:
  - `weft skills` (`crates/clawft-cli/src/commands/skills_cmd.rs`):
    list, show, install (local), remove, search (ClawHub),
    publish (ClawHub), remote-install, keygen.
  - `weft plugins`
    (`crates/clawft-cli/src/commands/plugins_cmd.rs`): create
    (scaffold), templates, validate.
- **Hierarchical kernel ToolRegistry** (ADR-036):
  `crates/clawft-kernel/src/wasm_runner/registry.rs:18` -- 332-line
  registry with `Arc<ToolRegistry>` parent chain, optional Ed25519
  signature verification, per-agent overlays. CF-3 (per-agent
  duplication) resolved.
- **Native plugin crates published to crates.io** (8 of 9):
  clawft-plugin-{git, cargo, oauth2, browser, calendar, containers,
  treesitter} declared in workspace at `Cargo.toml:14-22` and pinned
  to `0.6.6`.

## What's Left -- Total Depth

### TODOs / FIXMEs in code

A `grep -rn "TODO\|FIXME\|XXX\|HACK\|todo!\|unimplemented"` over
`crates/clawft-plugin*`, `crates/clawft-tools/`, the autogen module,
the registry, and the agent sandbox returned **zero** active markers in
production code. The only matches are:

- **TODO scaffolding inside generated code** (not a TODO in clawft itself):
  the plugin scaffolder template emits `// TODO: implement ...` in the
  scaffold output for downstream plugin authors --
  `crates/clawft-cli/src/commands/plugins_cmd.rs:151` (analyzer
  template), `:190`/`:194` (channel template), `:230`/`:240` (tool
  template), `:269` (generic template).
- **String-literal TODO sentinel** used by the tree-sitter complexity
  analyzer to flag agent code, not a project TODO --
  `crates/clawft-wasm/src/lib.rs:512` (`if line.contains("TODO") || line.contains("FIXME") || line.contains("HACK")`).

This is unusually clean for a workstream of this size and reflects
deliberate hygiene during C1-C7. Implication: the gaps below are
**design / wiring** debt, not in-code TODOs.

### Deferred items

1. **Shell-execution skill approval flow not wired**.
   `00-orchestrator.md:198` exit criterion is unchecked: `Shell-execution
   skills require explicit user approval on install (deferred -- part
   of T39 lifecycle tests)`. Today, a skill declaring `allowed-tools:
   - exec_shell` installs without prompting. T39 lifecycle tests are
   not yet in the test suite. Risk: medium -- a malicious skill from
   ClawHub could escalate via shell.
2. **`weft skills approve` / `reject` CLI absent**.
   `skill_autogen::install_pending_skill` writes `.pending` markers
   under `~/.clawft/skills/<name>/`, and `approve_skill` /
   `reject_skill` exist as library functions
   (`crates/clawft-core/src/agent/skill_autogen.rs:335`, `:347`),
   but no CLI subcommand drives them. Searching
   `crates/clawft-cli/src/commands/skills_cmd.rs` for `pending` /
   `approve` returns zero hits. Operators currently have to either
   `rm .pending` by hand or write Rust to drive the lib API. Without
   this surface, the C4a "user prompted for approval" exit criterion
   is technically met by the marker file existing, but is not
   ergonomic.
3. **Pending-skill review TUI** -- nothing renders pending skill
   candidates with their generated `SKILL.md` for human review before
   approval. Inferred from the `.pending` marker contents being just
   the literal string "awaiting user approval"
   (`crates/clawft-core/src/agent/skill_autogen.rs:322`); no diff /
   summary surface exists.
4. **`allowed-tools` intersection (vs union) not enforced for skills**.
   Cross-phase consensus item SEC-SKILL-04 (consensus.md:316) called
   for "allowed_tools intersection semantics". Today, `Tool::metadata`
   level/custom checks happen, and the registry's
   `schemas_for_tools(&allowed)` filters output schemas, but a skill
   declaring an `allowed-tools` list still gets that list **intersected
   with the agent's tool_access only via the registry's
   `check_tool_permission`** at execution time. The skill doesn't
   declare a precedence relationship to `allowed_tools` on the
   per-agent sandbox -- so a project-trusted skill can request a tool
   that the agent's `SandboxPolicy.allowed_tools` would have allowed,
   and the skill metadata adds nothing. There is no "skill-declared
   tools must be a subset of agent-allowed tools at parse time"
   validator.
5. **Eight native plugin crates orphaned** -- not depended on by
   anyone except their own siblings (`clawft-plugin-calendar` -> 
   `clawft-plugin-oauth2`). Confirmed by grepping crate-level
   Cargo.tomls: the only kernel-side wiring is
   `clawft-kernel/Cargo.toml:65` (`treesitter = ["dep:clawft-plugin-
   treesitter"]`). The other 7 (`browser`, `calendar`, `cargo`, `ci`,
   `containers`, `git`, `npm`, `oauth2`) are workspace members
   (`Cargo.toml:14-22`) and published to crates.io at 0.6.6 but **no
   binary in this workspace consumes them**. Either:
   - external consumers exist (downstream Weft / WeftOS users) -- in
     which case the plugins are intentionally library-only and need to
     stay decoupled, OR
   - they were planned for a kernel `--features full` extras bundle
     that was never wired, OR
   - they are now legacy from a pre-ADR-036 architecture.
   Decision needed before 0.7.0: declare each one's intended consumer.
6. **Two ToolRegistry implementations** -- `clawft-core::tools::
   registry::ToolRegistry` (used by the agent loop, async, supports
   MCP tools, glob permissions, ToolMetadata) and
   `clawft-kernel::wasm_runner::registry::ToolRegistry` (ADR-036,
   hierarchical, sync `BuiltinTool` trait, Ed25519 signing). They have
   different Tool traits (async `dyn Tool` vs sync `dyn BuiltinTool`),
   different signature requirements, and overlapping responsibilities
   (both gate-check, both filter). ADR-036 says "All future tool
   registration ... must follow this pattern: register in the
   overlay, not the base" but the agent loop today still talks to the
   `clawft-core` registry, not the kernel one. **No ADR records the
   merger / migration plan**, and no review document acknowledges
   this is dual-tracked.
7. **Plugin manifest format split**. `clawft-plugin::manifest::
   PluginManifest` is JSON/YAML (`clawft.plugin.json`,
   `crates/clawft-plugin/src/manifest.rs:11`). `weft plugins create`
   emits a different format -- `.weftos-plugin.toml` with
   `[plugin]`/`[compatibility]` sections
   (`crates/clawft-cli/src/commands/plugins_cmd.rs:289`,
   `:401`). The validator (`validate_plugin`,
   `:365`) only checks `.weftos-plugin.toml`, never the actual
   `clawft.plugin.json` schema. A scaffolded plugin will pass
   `weft plugins validate` but won't load via the WASM host's
   manifest reader.
8. **No `clawft.plugin.json` parser test**. The crate ships
   `PluginManifest::deserialize` derived via serde, but
   `crates/clawft-plugin/src/manifest.rs` has no roundtrip /
   schema-version / forward-compat tests at the integration level.
9. **ClawHub remote skill discovery (K4)** -- explicitly deferred per
   `07-element-04-tracker.md:71`. `weft skills search` /
   `remote-install` / `publish` use a local
   `clawft_services::clawhub` client
   (`crates/clawft-cli/src/commands/skills_cmd.rs:465`), but the
   ClawHub server side, signing trust roots, and download
   verification are out of scope for this workstream. Skill signing
   keygen exists (`weft skills keygen`); end-to-end signed-install
   round trip is not tested in this workstream.
10. **VoiceHandler trait remains a placeholder**. C1 decisions.md:20
    documents this as forward-compat for Workstream G;
    `clawft-plugin/src/lib.rs:17` says so explicitly. No native or
    WASM implementation lands in 0.7.0.
11. **Autogen disabled by default with no UX onramp**.
    `AutogenConfig::default().enabled == false`
    (`crates/clawft-core/src/agent/skill_autogen.rs:54`). Enabling is
    only via constructing a config in code; no `weft config` flag, no
    onboarding hint, no `weft skills autogen status` surface.
12. **No `weft skills refresh`**. consensus.md called this out
    (3F-agents review m4): hot-reload covers FS changes but
    in-process discovery state has no manual invalidation surface.
    The skill watcher does this via FS notifications, so most users
    never need it -- but for headless / CI scenarios where the
    watcher is disabled, there's no fallback.
13. **`SkillContext::Fork` execution path stub**. From 3F-agents
    review M2: no SubagentManager, fork variant parses but does not
    spawn. Status of this in 0.7.0 not verified by this audit; review
    indicated "defer to a later phase". Open question whether it
    silently no-ops, errors, or has been implemented since the review.

### Open questions

1. **Are the 8 orphaned native plugin crates still products?** Each
   one ships to crates.io at `0.6.6`; downstream WeftOS / external
   integrators may be the consumers. Decision blocks: should they be
   moved out of the workspace (separate repo), gated behind kernel
   feature flags, or wired into a `weft full` distribution? Without
   that decision, every workspace `cargo check` rebuilds 8 unused
   crates.
2. **Hierarchical tool registry migration**: when does the agent loop
   stop using `clawft-core::tools::registry::ToolRegistry` and start
   using the kernel's hierarchical one? ADR-036 doesn't say.
3. **Skill signing trust root**: `weft skills keygen` exists, but
   where is the trusted-key registry stored, who can add keys, and
   how does a workspace constrain accepted signers? No
   trust-root file is documented under `~/.clawft/`.
4. **Pending-skill review timing**: should the `.pending` marker also
   trigger an interactive prompt at next agent-loop start, or only on
   demand via CLI? Today neither happens automatically.
5. **`skill_autogen` x sandbox**: generated skills emit
   `user-invocable: false` and "minimal permissions" per
   `skill_autogen.rs:691-700`, but the generated SKILL.md does not
   explicitly populate filesystem allowlists. It relies on the
   loading agent's existing sandbox to constrain. Is that sufficient
   when the host agent has broader perms than the autogen design
   target?
6. **WASM resource-limit telemetry**: fuel/memory limits are enforced
   but there's no observability surface (metric / chain event)
   surfacing per-plugin fuel consumption. Plugin authors can't tune
   limits empirically.
7. **Plugin sandbox `effective_sandbox_type` on macOS**: silently
   downgrades `OsSandbox`/`Combined` to `Wasm` and emits
   `tracing::warn!`
   (`crates/clawft-plugin/src/sandbox.rs:299`). This is correct
   but downstream operators on macOS may not know they're running in
   a weaker sandbox -- no error, no opt-in confirmation.

### Orphaned work

- **`clawft-plugin-{browser, calendar, cargo, ci, containers, git, npm, oauth2}`**
  -- workspace members (`Cargo.toml:14-22`), each with full
  Tool-trait impls (e.g. `crates/clawft-plugin-git/src/lib.rs:510` --
  `all_git_tools(...) -> Vec<Box<dyn Tool>>`), no consumer in the
  workspace.
- **`clawft-plugin-treesitter`** -- the *only* plugin crate with a
  workspace consumer (`clawft-kernel` via `treesitter` feature),
  proving the pattern works when wired.
- **`PluginManifest` JSON schema** -- defined in `manifest.rs` but
  never produced by the scaffold tooling (which writes
  `.weftos-plugin.toml` instead) and never validated by the CLI
  validator.
- **`weftos-plugin.toml` format** -- defined only in the scaffolder
  templates (`plugins_cmd.rs:289-303`), no Rust struct, no parser,
  no reader. Pure scaffold artifact.
- **`improve_skill_instructions` (TrajectoryLearner integration)** --
  fully implemented at `skill_autogen.rs:376` with tests, but the
  agent-loop wiring at `loop_core.rs:1190` constructs an
  `AutogenConfig` with no learner reference. Function is dead from
  the loop's perspective.
- **`generate_skill_md_with_learning`** (`:412`) -- same: tested,
  not called from the loop.
- **C1 trait `VoiceHandler`** -- placeholder, no impl, no plan to
  remove or implement in 0.7.0.

## Task List

| # | Task | Priority | Owner | Effort | Notes |
|---|------|----------|-------|--------|-------|
| 1 | Wire `weft skills approve <name>` / `weft skills reject <name>` CLI to `skill_autogen::approve_skill` / `reject_skill` | P1 | skills/CLI | S | Closes the autogen approval UX gap; library API already exists |
| 2 | Add `weft skills pending` to list `.pending`-marked skills with their generated SKILL.md | P1 | skills/CLI | S | Sibling of #1 |
| 3 | Decide fate of 8 orphaned `clawft-plugin-*` crates: feature-gate-bundle, separate-repo, or document downstream consumers | P0 | architecture | M | Blocks workspace cleanup; was raised in commit 8c08ce0a (handoff TODO) |
| 4 | Add ADR documenting `clawft-core` vs `clawft-kernel` ToolRegistry split + migration plan | P0 | architecture | M | ADR-036 left this implicit |
| 5 | Implement shell-execution skill approval prompt at install time | P1 | skills/security | M | Outstanding 00-orchestrator exit criterion (line 198) |
| 6 | Reconcile plugin-manifest formats: pick `clawft.plugin.json` or `.weftos-plugin.toml`, update scaffolder + validator + WASM host loader | P1 | plugin-system | M | Currently scaffolded plugins are unloadable |
| 7 | Add per-skill `allowed-tools` intersection validator at skill-load time, fail loudly when skill requests a tool the agent denies | P1 | security | S | Closes SEC-SKILL-04 from consensus.md |
| 8 | Wire `improve_skill_instructions` / `generate_skill_md_with_learning` into agent-loop autogen path | P2 | autogen | S | Otherwise the trajectory-learning code is dead |
| 9 | Add `weft skills autogen {enable, disable, status}` CLI to surface AutogenConfig | P2 | CLI | S | Currently config-file-only |
| 10 | Add WASM per-plugin fuel/memory observability (chain event + `weft plugins inspect`) | P2 | observability | M | |
| 11 | Document skill signing trust root location & rotation under `docs/skills/` | P1 | docs | S | `weft skills keygen` exists, ops story does not |
| 12 | Add macOS-sandbox-downgrade warning to startup banner (not just trace log) | P2 | security/UX | XS | |
| 13 | Add `clawft.plugin.json` schema roundtrip test in `clawft-plugin` | P2 | tests | S | |
| 14 | Verify (or close) `SkillContext::Fork` status post-3F review M2 | P2 | skills | S | |
| 15 | Land `T39` plugin-lifecycle tests referenced in 00-orchestrator.md | P2 | tests | M | Closes the only remaining checked-out box on the C2 security exit list |

## Sources

- `crates/clawft-plugin/src/lib.rs`
- `crates/clawft-plugin/src/traits.rs:82` (Tool trait)
- `crates/clawft-plugin/src/manifest.rs:11` (PluginManifest)
- `crates/clawft-plugin/src/sandbox.rs:239`, `:256`, `:386` (path checks, identity hard-deny)
- `crates/clawft-core/src/tools/registry.rs:339` (ToolRegistry)
- `crates/clawft-core/src/tools/registry.rs:160` (check_tool_permission)
- `crates/clawft-core/src/agent/skill_autogen.rs` (whole file, 804 LoC)
- `crates/clawft-core/src/agent/skill_autogen.rs:306-356` (install/approve/reject)
- `crates/clawft-core/src/agent/skill_autogen.rs:376-430` (trajectory-driven mutation)
- `crates/clawft-core/src/agent/skill_watcher.rs` (notify watcher, 555 LoC)
- `crates/clawft-core/src/agent/loop_core.rs:1165-1207` (autogen wiring)
- `crates/clawft-tools/src/lib.rs:65` (register_all)
- `crates/clawft-cli/src/commands/skills_cmd.rs` (skills CLI; `pending`/`approve` absent)
- `crates/clawft-cli/src/commands/plugins_cmd.rs:289`,`:365` (manifest split + validator)
- `crates/clawft-cli/src/commands/mcp_server.rs:115` (SkillToolProvider wiring)
- `crates/clawft-services/src/mcp/composite.rs:297-371` (MCP skill tests)
- `crates/clawft-kernel/src/wasm_runner/registry.rs:18-200` (ADR-036 hierarchical registry)
- `crates/clawft-kernel/Cargo.toml:65` (only treesitter wired)
- `crates/clawft-wasm/src/{engine,sandbox,fs,http,env,audit}.rs` (WASM host)
- `Cargo.toml:14-22` (workspace member listing for plugin crates)
- `.planning/sparc/phase4/04-plugin-skill-system/00-orchestrator.md` (phase plan; line 198 open shell-skill approval)
- `.planning/sparc/phase4/04-plugin-skill-system/07-element-04-tracker.md` (per-phase status table)
- `.planning/development_notes/04-plugin-skill-system/{c1-plugin-traits,c2-wasm-host,security}/notes.md` (impl notes; 30/45 -> 45/45 sec tests)
- `.planning/development_notes/04-plugin-skill-system/c1-plugin-traits/decisions.md` (separate crate, async_trait, MessagePayload enum)
- `.planning/reviews/3f-agents-review.md` (3F skills/agents review; C1, C2, M1-M4, m1-m6)
- `.planning/reviews/3f-rvf-review.md` (orthogonal: RVF/ruvector; only weak overlap with workstream 04 via skill autogen)
- `.planning/reviews/consensus.md:108-120` (skill discovery alignment, SEC-SKILL-01..07)
- `.planning/reviews/consensus.md:316,356,397` (skills install / autogen / pending follow-ups)
- `docs/adr/adr-036-hierarchical-tool-registry.md` (kernel registry, all 66 lines)
- `docs/skills/clawft/{AGENTS,IDENTITY,SOUL,TOOLS,USER}.md` (in-tree clawft skill content)
- `.clawft/skills/{claude-code,claude-flow}/SKILL.md` (real installed skills, no `.pending` markers)
- recent git log: `8c08ce0a docs(handoff): add worktree + branch cleanup item` (recent docs hygiene; cf. orphan-crate decision)

<!-- TRIAGED-STAMP:BEGIN -->
## Triaged into Plane — 2026-04-28

All open items in this audit have been filed as Plane work items in the WeftOS workspace under the `ws04-plugin-skills` label.

- **Range**: WEFT-59 … WEFT-78 (20 items)
- **Per cycle**: 0.7.x: 14, 0.8.x: 6
- **Triage spec**: `.planning/reviews/0.7.0-release-gate/triage/`
- **WEFT-N → name map**: `.planning/reviews/0.7.0-release-gate/triage/weft-mapping.json`

Per the project rule (CLAUDE.md → "Plane is the authoritative work tracker"): future updates to these items happen in Plane, not in this audit doc. This doc remains the source-of-truth for the original survey.
<!-- TRIAGED-STAMP:END -->
