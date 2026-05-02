---
title: "Core Platform"
slug: core-platform
workstream_id: "01"
status: landed
period_start: 2026-02-16
period_end: null
last_updated: 2026-04-28
versions_landed: ["0.1.0", "0.2.1", "0.3.1", "0.5.0", "0.5.5", "0.6.0", "0.6.19"]
related_plans:
  - docs/plans/agent-core-v1.md
  - docs/plans/chat-agent-v1.md
  - .planning/sparc/00-initial-sprint/1a-types-platform-plugin-api.md
  - .planning/sparc/phase4/03-critical-fixes-cleanup/01-workstream-A-critical-fixes.md
  - .planning/sparc/phase4/03-critical-fixes-cleanup/02-workstream-B-architecture-cleanup.md
  - .planning/sparc/phase4/03-critical-fixes-cleanup/03-workstream-I-type-safety.md
  - .planning/sparc/phase4/03-critical-fixes-cleanup/04-workstream-J-doc-sync.md
related_adrs:
  - adr-010
  - adr-037
  - adr-044
  - adr-001
  - adr-021
sprint_refs:
  - 00-initial-sprint
  - 02-improvements-overview
  - 03-critical-fixes-cleanup
completion_pct: 90
open_task_count: 17
risk: low
---

# Core Platform

## General Description

Workstream 01 covers the foundation crates that every other clawft crate
depends on: `clawft-types` (data model + config schema + error types),
`clawft-platform` (Platform / FileSystem / Environment / HttpClient /
ProcessSpawner traits with native + browser impls), `clawft-rpc` (daemon
protocol + Unix-socket client), `clawft-security` (50+ audit checks for
`weft security scan`), and `clawft-cli` (the `weft` binary that wires
everything together). It also covers the foundation-touching modules of
`clawft-core`: `bootstrap.rs`, `config_merge.rs`, `security/`, and
`workspace/`, which sit between the platform abstraction and the agent
loop proper.

These crates are stable. The Platform trait set has been frozen since
Sprint 11 (universal platform targets), the `clawft-types::config`
schema has shipped through 5+ minor versions, and the workstreams that
hardened them (A — security, B — architecture cleanup, I — type safety,
J — doc sync) all closed in Phase 4. The only foundation-layer activity
in this session was the Layer-3 workspace-config overlay fix (commits
`0452539a` and `ec7bb2bd`), which was a regression repair, not a new
feature.

## Status & Timeline

- **Stream-1A (Foundation port)** landed 2026-02-16: 6 platform files,
  ~987 LoC, 45 unit tests passing — `platform-implementation.md`.
- **Phase 3 Round 4** decoupled `clawft-wasm` from `clawft-platform`
  and `clawft-core` to make the WASM target compile (2026-02-17).
- **Workstream A (Critical fixes)** closed 2026-02-20: 9/9 items —
  SecretString, masked input, SSRF complete, HTTP timeouts, no-default
  build green, A8 (`unsafe set_var` removal in tests).
- **Workstream B (Architecture cleanup)** closed 2026-02-19: 9/9 items —
  unified Usage / LlmMessage / ProviderConfig types, split oversized
  `config.rs` (1382 → 5 modules), shared tool-registry builder.
- **Workstream I (Type safety)** closed 2026-02-19: 8/8 items —
  PolicyMode enum, camelCase normalizer acronym fix (I5), shared
  MockTransport behind `test-utils`.
- **Workstream J (Doc sync)** closed 2026-02-19: 7/7 items.
- **0.6.0 Cognitum-Seed sprint** added kernel sub-config types
  (`LogQuantizedStubConfig`, `SimdDistanceStubConfig` in
  `clawft-types/src/config/kernel.rs:780,810`) — serializable stubs
  whose runtime backends sit in `clawft-kernel`.
- **2026-04-28 (this session)**: workspace-config Layer 3 overlay
  restored (`0452539a` + `ec7bb2bd`); panel-chat policy now reaches
  `PermissionResolver` instead of being silently dropped.

## Released Features

- `clawft-types` (≈9.6 kLoC): `Config`/`AgentsConfig`/`ChannelsConfig`/
  `ProvidersConfig`/`GatewayConfig`/`ToolsConfig`/`DelegationConfig`/
  `RoutingConfig`/`VoiceConfig`/`KernelConfig` schema, `SecretString`
  with redacted Debug, `ClawftError` + `ChannelError`, provider registry,
  cron types, canvas types, agent-bus types, registry trait.
  (`crates/clawft-types/src/lib.rs:32-49`)
- `clawft-platform` (≈1.7 kLoC): `Platform` trait + `NativePlatform` +
  `BrowserPlatform`; `HttpClient` trait with reqwest (rustls) + fetch
  impls; async `FileSystem` trait with tokio + in-memory impls;
  `Environment` + `ProcessSpawner` traits; `config_loader` with 3-layer
  load (weave.toml → home JSON → cwd `.clawft/config.json`) and camelCase
  → snake_case key normalization.
  (`crates/clawft-platform/src/lib.rs:75-92`,
  `crates/clawft-platform/src/config_loader.rs:82-164`)
- `clawft-rpc` (≈350 LoC): Unix-socket `DaemonClient`, `Request` /
  `Response` JSON-RPC envelope, runtime-dir/socket-path helpers,
  background version-check with 24h cache.
  (`crates/clawft-rpc/src/client.rs`,
  `crates/clawft-rpc/src/version_check.rs`)
- `clawft-security` (≈940 LoC): `SecurityScanner` engine with 10
  categories of audit checks (prompt injection, exfiltration URLs,
  credential literals, permission escalation, unsafe shell, supply chain,
  DoS, indirect injection, info disclosure, cross-agent access).
  (`crates/clawft-security/src/lib.rs`)
- `clawft-core::security` (897 LoC): boundary validation —
  `validate_session_id`, `truncate_result`, `sanitize_content`,
  YAML-depth check, `validate_directory_name`, `intersect_allowed_tools`,
  `validate_model_string`, `sanitize_skill_instructions` (12 injection
  tokens, 10-pass nested-token loop), `sanitize_llm_input`,
  `sanitize_schema_input`, `validate_file_size`,
  `validate_mcp_tool_name_strict`.
  (`crates/clawft-core/src/security/mod.rs`)
- `clawft-core::config_merge` + `clawft-core::bootstrap`: deep-merge of
  the platform's raw JSON into a typed `Config`; daemon agent-loop
  builder threads loaded `RoutingConfig` through to the
  `PermissionResolver` (this-session fix).
- `clawft-cli` (`weft` binary): 24 command modules covering agent /
  agents / analyze / assess / channels / config / cron / gateway / help /
  kernel / mcp_server / memory / onboard / plugins / security / sessions
  / skills / status / tools / ui / voice / workspace / plugin_registry.

## What's Left — Total Depth

### TODOs / FIXMEs in code

Foundation-crate TODOs (excluding scaffolding-template strings inside
`weft plugins scaffold`, which are not real code-level debt):

- `crates/clawft-core/src/bootstrap.rs:633` — `TODO(v1.1): split
  workspace from global at the loader layer so we can pass them to
  PermissionResolver::new(global, Some(workspace)) and let
  enforce_workspace_ceiling clamp workspace permissions against
  system-wide bounds`. Today the workspace overlay is deep-merged
  upstream, so workspace policy reaches the resolver but the
  security-ceiling pattern is bypassed. Fine for single-user;
  needed for multi-tenant. (Tracked by `ec7bb2bd`.)
- `crates/clawft-types/src/routing.rs:105-108` — sona-backed rerank for
  v2.5 HybridRouter and v3 MicroLoraRouter explicitly deferred.
  Cross-references `docs/research/rvf-context-router.md:118-128`
  (ruvllm-wasm 11-pattern HNSW cap). This is router (stream 03), not
  foundation, but it lives in `clawft-types`.
- `crates/clawft-types/src/config/kernel.rs:780,810` — two
  `*StubConfig` types (LogQuantized, SimdDistance) with comment
  "Requires `ruvector-core` with PR #352 merged". Foundation surface
  area only; runtime in `clawft-kernel`.
- `crates/clawft-platform/src/browser/fs.rs:1-9` — header note: OPFS
  bindings unstable, currently in-memory `HashMap`-backed; "acceptable
  for the current stub/MVP phase". Persistence across reloads is
  outstanding.
- `crates/clawft-platform/src/browser/mod.rs:7` — "OPFS planned for
  future" companion comment.
- `crates/clawft-rpc/src/client.rs:62-64` — non-Unix `DaemonClient` is a
  stub: `connect()` always returns `None`. Comment says "Windows
  named-pipe transport is planned for v0.2"; we are at 0.6.19, so this
  has slipped multiple minor versions.
- `crates/clawft-cli/src/commands/voice.rs:79,91,112` — Talk-Mode handler
  uses a `StubAdapterHost` that logs but does not deliver; voice channel
  prints `"stub (real audio processing deferred)"`. Voice is its own
  workstream; foundation just hosts the `voice` feature flag and CLI
  shim.
- `crates/clawft-cli/src/commands/skills_cmd.rs:793-803` — keygen writes
  `"(derived on first sign)"` as the public-key file when the `signing`
  feature is not compiled in. Real Ed25519 derivation only happens at
  publish-time.
- `crates/clawft-cli/src/commands/memory_cmd.rs:30,56` — placeholder
  fallback when the resolved memory file is missing.
- Plugin-scaffolding template emits literal `TODO:` strings in
  generated plugin source (`crates/clawft-cli/src/commands/plugins_cmd.rs:150,190,195,230,240,269`).
  These are output, not debt.
- `crates/clawft-cli/src/commands/assess_cmd.rs:859-861` — `weft assess`
  greps for the literal strings `TODO`/`FIXME` in user code; matches in
  this report came from there.

### Deferred items (from plans / handoff / ADRs)

- **ADR-044 wasip2 migration**: ADR specifies `wasip1` is the *current*
  target with migration to `wasip2` planned for Sprint 12. The
  `.cargo/config.toml` `wasm` alias already points at `wasm32-wasip2`
  (using the `release-wasm` profile) and `rust-toolchain.toml` pins
  channel `1.93` (which supports wasip2). Inconsistency: ADR says
  primary is wasip1; cargo alias says wasip2. Sprint 12 (W49) work
  appears partially done — confirm before 0.7.0.
  (`docs/adr/adr-044-wasm-wasip1-target.md:31-37`,
  `.cargo/config.toml`)
- **ADR-010 cancel-correctness audit**: ADR called for a v0.3 audit of
  `select!` branches in mesh networking code. We are at 0.6.19; no
  evidence of a completed audit in this stream's notes. Mesh code is
  outside this workstream, but the ADR governs the foundation runtime
  choice.
- **Workspace-version drift**: workspace package is at `0.6.19` but
  every inter-crate `path` dependency in `Cargo.toml:182-200` pins
  `version = "0.6.6"`. Cargo accepts this (path overrides version when
  publishing isn't involved), but it will bite the next crates.io
  publish. Foundation crates (`clawft-types`, `clawft-platform`,
  `clawft-rpc`, `clawft-security`) are all affected.
- **ADR-001 lockstep-semver**: with the version-drift above, the
  lockstep promise is currently broken at the dep-pin level even though
  every crate inherits `version.workspace = true`.
- **Identity / IdentityLoader split**: `agent-core-v1` D1 added the
  identity-aware system prompt and SHA-256 hash on `Identity`, but
  `IdentityLoader` lives in `clawft-core::agent` rather than the
  `clawft-types` schema layer. No follow-up TODO, but worth noting that
  identity types straddle the foundation/agent boundary.
- **`chain.append` RPC**: `weaver soul promote` falls back to writing a
  local audit log because there's no daemon `chain.append` method yet.
  TODO marker is `TODO(agent-core-v1.1)` per `docs/handoff.md:60`.
  Adjacent to `clawft-rpc` (this stream) but the impl lives in
  `clawft-weave`.
- **Defer UX**: D2's `Defer { reason }` reaches the LLM as a
  tool-result; real interactive defer (panel prompt + resume) is v1.1.
  Touches `clawft-types::routing` enums but the UX code is panel/UI.
- **Per-user `agent_id`s**: chat is single-tenant
  (`concierge-bot` registered once at boot). Multi-tenant requires
  splitting workspace from global at the loader (the `bootstrap.rs:633`
  TODO above). Same root cause.

### Open questions and known limitations

1. The `clawft-cli` crate has `publish = false` and `clawft-security`
   has `publish = false`. Intentional for the binary; for `clawft-security`
   it means downstream consumers cannot use the audit checks via
   crates.io. Confirm before publish.
2. `clawft-platform` browser feature exposes an in-memory `HashMap`
   filesystem with no persistence. Anyone using `BrowserPlatform` for a
   real PWA will lose state on reload. Tracked as "OPFS planned" but
   no ADR.
3. `clawft-platform/src/config_loader.rs` `discover_config_path` uses
   sync `Path::exists()` for Layer 2 (home-dir JSON). The
   workspace-overlay layer (Layer 3) is properly async via
   `fs.exists().await`. Asymmetry is documented in the file but not in
   any ADR; Layer 2 cannot be mocked, hence the `#[ignore]`d
   `tests/overlay_probe.rs`.
4. `clawft-rpc` Unix-only without a path to Windows. The "v0.2" promise
   in `client.rs:63` has slipped 4+ minor versions.
5. `clawft-types::config` accepts both camelCase and snake_case via
   `#[serde(alias)]` and silently ignores unknown fields. Forward-compat
   is good; typo-resistance is bad. No `deny_unknown_fields` lint mode.
6. `clawft-core::security::validate_mcp_tool_name` has a lenient mode
   that always returns `Ok(())` for any input that looks "local-ish";
   the strict variant is the one called at registration. The lenient
   helper is a code-smell — either delete or document why both exist.
7. `clawft-rpc::version_check` uses `std::process::Command::new("curl")`
   for the GitHub API check (`version_check.rs:64-83`). This shells out
   from a foundation crate and assumes `curl` exists on every host. A
   `reqwest`-based implementation would be more portable, especially on
   Windows (where the rest of `clawft-rpc` is already a stub).
8. `clawft-types/src/canvas.rs` uses `panic!` macros inside test-only
   match arms (`canvas.rs:259,287,318,…`). These are `#[cfg(test)]` and
   harmless, but a couple of `panic!`s also live in
   `provider.rs:775` and `agent_bus.rs:255,270,287` test code — same
   shape, same containment.

### Orphaned work (planning docs that didn't ship)

- `.planning/development_notes/00-initial-sprint/codebase-map.md` and
  `planning-summary.md`: snapshot of the original Python → Rust port.
  Historical artefact; pinned to nanobot lineage. Not orphaned per se,
  but stale relative to the WeftOS rebrand.
- `.planning/improvements.md` and the `02-improvements-overview/sprint-tracker.md`
  Phase-5 tracker still list MVP / Full-Vision checkboxes from
  Week 8/12. Foundation-relevant items (A1-A9, B1-B9, I1-I8, J1-J7) are
  all checked, but the doc itself was never closed out and may give the
  illusion of work-in-progress to a future reader.
- `.planning/development_notes/00-initial-sprint/phase3/exit-criteria-review.md`,
  `phase3-status.md`, `round3-summary.md`, `round4-summary.md`: useful
  history (WASM target switch, dep decoupling) but no living tracking.

## Task List

- [ ] Reconcile workspace inter-crate dep pins (`Cargo.toml:182-200` at
      `0.6.6`) with `workspace.package.version = "0.6.19"`. Required
      before next crates.io publish. — source: `Cargo.toml:50,182-200`
- [ ] Decide and document wasip1 vs wasip2 primary target. ADR-044
      says wasip1; `.cargo/config.toml` `wasm` alias targets wasip2.
      Either complete the Sprint 12 migration or back out the alias.
      — source: `docs/adr/adr-044-wasm-wasip1-target.md:31-37`,
      `.cargo/config.toml`
- [ ] Split workspace and global config at the loader so
      `PermissionResolver::new(global, Some(workspace))` can apply the
      multi-tenant security ceiling. — source:
      `crates/clawft-core/src/bootstrap.rs:633`
- [ ] Implement Windows daemon transport (named pipes) for
      `clawft-rpc::DaemonClient`. — source:
      `crates/clawft-rpc/src/client.rs:54-80`
- [ ] Replace `version_check.rs` shell-out to `curl` with a `reqwest`
      call so the foundation crate works on Windows / minimal images.
      — source: `crates/clawft-rpc/src/version_check.rs:63-83`
- [ ] Implement OPFS-backed `BrowserFileSystem` so PWA users keep state
      across reloads. — source: `crates/clawft-platform/src/browser/fs.rs:1-9`
- [ ] Wire `LogQuantizedStubConfig` and `SimdDistanceStubConfig`
      runtime in `clawft-kernel` (waiting on ruvector-core PR #352).
      Foundation surface is ready. — source:
      `crates/clawft-types/src/config/kernel.rs:770-824`
- [ ] Either delete or rationalize the lenient
      `validate_mcp_tool_name` next to the strict variant. — source:
      `crates/clawft-core/src/security/mod.rs:448-483`
- [ ] Add a `chain.append` RPC method to `clawft-rpc` so
      `weaver soul promote` can stop falling back to a local audit log.
      — source: `docs/handoff.md:60`,
      `crates/clawft-cli/src/commands/skills_cmd.rs` (publish path)
- [ ] Run the ADR-010 v0.3 cancel-correctness audit on `select!`
      branches in mesh code (foundation runtime decision; mesh is
      stream 04). — source: `docs/adr/adr-010-keep-tokio.md:14-15`
- [ ] Decide whether `clawft-security` should be `publish = true` so
      downstream users can run the audit checks. — source:
      `crates/clawft-security/Cargo.toml:3`
- [ ] Decide whether `Config` should grow a `deny_unknown_fields` lint
      mode for typo detection (off-by-default). — source:
      `crates/clawft-types/src/config/mod.rs:1-5`
- [ ] Land OPFS-or-equivalent persistence for `BrowserEnvironment`
      (currently in-memory only). — source:
      `crates/clawft-platform/src/browser/env.rs`
- [ ] Document the Layer-2 vs Layer-3 sync/async asymmetry in
      `config_loader` either in an ADR or in
      `docs/guides/configuration.md`. — source:
      `crates/clawft-platform/src/config_loader.rs:30-65`
- [ ] Remove `TODO(E1)` and `TODO(C5)` markers (Discord ResumePayload,
      interactive slash commands) once their sub-streams ship. — source:
      `.planning/development_notes/03-critical-fixes-cleanup/workstream-I-type-safety/notes.md`
- [ ] Replace `(derived on first sign)` placeholder pubkey output with
      a real Ed25519 derivation, regardless of `signing` feature.
      — source: `crates/clawft-cli/src/commands/skills_cmd.rs:793-803`
- [ ] Close out `.planning/improvements.md` Phase-5 sprint-tracker now
      that all foundation-relevant items are done. — source:
      `.planning/development_notes/02-improvements-overview/sprint-tracker.md`

## Sources

- `crates/clawft-types/src/lib.rs` (lines 1-52)
- `crates/clawft-types/src/config/mod.rs` (lines 1-80)
- `crates/clawft-types/src/config/kernel.rs` (lines 770-824)
- `crates/clawft-types/src/routing.rs` (lines 90-140)
- `crates/clawft-platform/src/lib.rs` (lines 1-174)
- `crates/clawft-platform/src/config_loader.rs` (lines 1-165, 450-515)
- `crates/clawft-platform/src/browser/fs.rs` (lines 1-9)
- `crates/clawft-platform/src/browser/mod.rs`
- `crates/clawft-rpc/src/lib.rs`
- `crates/clawft-rpc/src/client.rs` (lines 1-103)
- `crates/clawft-rpc/src/version_check.rs`
- `crates/clawft-security/src/lib.rs`
- `crates/clawft-core/src/bootstrap.rs` (lines 620-660)
- `crates/clawft-core/src/security/mod.rs` (lines 1-484)
- `crates/clawft-cli/src/commands/voice.rs` (lines 70-115)
- `crates/clawft-cli/src/commands/skills_cmd.rs` (lines 780-806)
- `crates/clawft-cli/src/commands/plugins_cmd.rs` (lines 140-275)
- `Cargo.toml` (lines 1-50, 180-205)
- `.cargo/config.toml`
- `rust-toolchain.toml`
- `docs/handoff.md` (lines 1-110)
- `docs/plans/agent-core-v1.md` (lines 30-46, 80-100, 165-172)
- `docs/adr/adr-010-keep-tokio.md`
- `docs/adr/adr-037-rust-edition-2024-msrv.md`
- `docs/adr/adr-044-wasm-wasip1-target.md`
- `.planning/sparc/00-initial-sprint/1a-types-platform-plugin-api.md`
- `.planning/development_notes/00-initial-sprint/phase1/stream-1a/platform-implementation.md`
- `.planning/development_notes/00-initial-sprint/phase1/stream-1a/types-implementation.md`
- `.planning/development_notes/00-initial-sprint/phase3/round4-summary.md`
- `.planning/development_notes/03-critical-fixes-cleanup/workstream-A-security/notes.md`
- `.planning/development_notes/03-critical-fixes-cleanup/workstream-B-architecture/notes.md`
- `.planning/development_notes/03-critical-fixes-cleanup/workstream-I-type-safety/notes.md`
- `.planning/development_notes/03-critical-fixes-cleanup/workstream-J-doc-sync/notes.md`
- `.planning/development_notes/02-improvements-overview/sprint-tracker.md`
- Recent commits: `0452539a` (workspace overlay), `ec7bb2bd` (resolver
  threading), `8b05d868` (null-content), `cb947080` (weaver --update),
  `b068b063` (soul promote)

<!-- TRIAGED-STAMP:BEGIN -->
## Triaged into Plane — 2026-04-28

All open items in this audit have been filed as Plane work items in the WeftOS workspace under the `ws01-core` label.

- **Range**: WEFT-9 … WEFT-26 (18 items)
- **Per cycle**: 0.7.x: 8, 0.8.x: 9, 0.9.x: 1
- **Triage spec**: `.planning/reviews/0.7.0-release-gate/triage/`
- **WEFT-N → name map**: `.planning/reviews/0.7.0-release-gate/triage/weft-mapping.json`

Per the project rule (CLAUDE.md → "Plane is the authoritative work tracker"): future updates to these items happen in Plane, not in this audit doc. This doc remains the source-of-truth for the original survey.
<!-- TRIAGED-STAMP:END -->
