# Follow-up audit — ws01-07, ws10-12 (foundation, channels, voice, agent-core, kg)

Date: 2026-05-01
Scope: 13 items shipped across the parent + M7-A/B/C/D/G sweeps.
Branch verified: `m7-08-sweep` @ `81dd34c6`.
Auditor: audit-D.

## Per-item verification

### WEFT-12 — rpc: replace `version_check` curl shell-out with reqwest
- **Status**: confirmed in code
- **Files**: `crates/clawft-rpc/src/version_check.rs:69-96`; `crates/clawft-rpc/Cargo.toml:23`
- **Acceptance criteria met**:
  - [x] `std::process::Command::new("curl")` replaced with `reqwest::blocking::Client` (3s connect, 5s total timeout, UA header, `Accept: application/vnd.github.v3+json`)
  - [x] 24h cache behaviour preserved (`CACHE_TTL_SECS = 86400`, `read_cache`/`write_cache` unchanged at lines 39-61)
  - [x] Tests cover offline / 404 / parse-failure paths: `fetch_latest_returns_none_on_unreachable_host` (`http://127.0.0.1:1`), `fetch_latest_returns_none_on_404`, `fetch_latest_returns_none_on_unparseable_body`, plus happy-path `fetch_latest_parses_release_json` and `fetch_latest_strips_v_prefix`
  - [x] Crate still spawns a background thread (`std::thread::spawn` at line 129); `reqwest::blocking` does not require a tokio runtime in that thread, so the Windows / minimal-Alpine compile target story is intact
- **Tests**: 7/7 passed (`cargo test -p clawft-rpc --lib version_check`)
- **reqwest feature surface**: per-crate Cargo.toml adds only the `blocking` feature on top of the workspace base (`json` + `default-tls`, `default-features = false`). Cargo tree confirms `reqwest feature "blocking" / "default-tls" / "json"` are the only enabled features. The crate's comment "Other features stay default-off to keep this crate's compile cost minimal" is accurate for the per-crate addition; the workspace-level base set is unavoidably inherited. tokio is already a direct dep of clawft-rpc (for the daemon transport), so reqwest's tokio pull-in is not new surface area.
- **New issue / stub spotted**: none.

### WEFT-88 — workspace: `WorkspaceManager::load` bumps `last_accessed`
- **Status**: confirmed in code
- **Files**: `crates/clawft-core/src/workspace/mod.rs:200-249`
- **Acceptance criteria met**:
  - [x] `WorkspaceManager::load` now updates `last_accessed` to `now()` via `touch_last_accessed_by_name` (line 210 for by-name path; line 221 for by-path-resolves-to-registered path)
  - [x] Atomic write — `touch_last_accessed_by_name` calls `self.save_registry()` after mutating the in-memory entry (line 246; `save_registry` is the existing tempfile-rename atomic writer)
  - [x] Path-only loads that miss the registry remain side-effect free (lines 217-223 guard with `find_by_path` before touching)
  - [x] Tests: `workspace_manager_load_by_name_bumps_last_accessed` (line 480), `workspace_manager_load_by_path_bumps_last_accessed_when_registered` (line 518), and `workspace_manager_load_by_path_does_not_persist_when_unregistered` (line 553+, inferred from listing). All assert ordering after a load sequence with `tokio::time::sleep` between operations.
- **Tests**: 32/32 passed (`cargo test -p clawft-core --lib workspace::`)
- **New issue / stub spotted**: none.

### WEFT-116 — kernel/mesh: flat `mesh_*.rs` layout in K6 plans
- **Status**: confirmed in docs
- **Files**: `.planning/sparc/weftos/0.1/07-phase-K6-mesh-framework.md:37`; `docs/weftos/k5-symposium/04-k6-implementation-plan.md:393`
- **Acceptance criteria met**:
  - [x] Layout standard documented (flat) — preamble at K6 doc:37 reads "Layout standard: flat. All mesh modules live directly under …"
  - [x] `mesh_handshake.rs` (K6.4b) row in the K6 Files-to-Create table (line 61: `crates/clawft-kernel/src/mesh_handshake.rs | K6.4b | Optional ML-KEM-768 hybrid KEM upgrade after Noise XX`)
  - [x] `mesh_adapter.rs` reconciled (line 55, K6.3 row added with description "MeshAdapter — incoming mesh dispatch through local A2ARouter")
  - [x] `04-k6-implementation-plan.md:393` updated with explicit "(flat layout: `crates/clawft-kernel/src/mesh_handshake.rs`)" parenthetical
  - [x] No dangling cross-references: every `mesh_*.rs` filename under `crates/clawft-kernel/src/` (mesh_quic, mesh_noise, mesh_framing, mesh_listener, mesh_discovery, mesh_kad, mesh_mdns, mesh_bootstrap, mesh_ipc, mesh_service, mesh_dedup, mesh_handshake, mesh_chain, mesh_tree, mesh_process, mesh_service_adv) appears flat in the table
- **Notes**: doc-only normalization; no source change required.
- **New issue / stub spotted**: none.

### WEFT-172 — telegram: drop redundant 1s inter-poll sleep
- **Status**: confirmed in code
- **Files**: `crates/clawft-channels/src/telegram/channel.rs:25-38, 282-289`; `docs/guides/channels.md:214-218`
- **Acceptance criteria met**:
  - [x] `DEFAULT_POLL_INTERVAL_SECS = 0` (line 38) — the redundant 1s sleep is removed by default
  - [x] `if self.poll_interval_secs > 0` guard intact (line 282) — the field stays configurable so operators can dial in back-pressure on tight retry loops, but `0` skips the `tokio::select!` sleep block entirely
  - [x] Documented inline (lines 28-37 docblock spells out the long-poll/`tokio::select!` reasoning) and in `docs/guides/channels.md` §3.3 ("no extra client-side sleep is needed between cycles (default `poll_interval_secs = 0`)")
  - [x] No regression in `tokio::select!` cancel-token semantics — the cancel branch on lines 283-284 still wins when the sleep block runs
- **Tests**: 35/35 passed (`cargo test -p clawft-channels --lib telegram`); existing `Telegram*` test suite covers config parse + factory + channel construction. No new latency-regression test added — would have required mocking the Bot API roundtrip — but the change is isolated to a default constant and the existing `factory_uses_default_poll_interval` (or equivalent) tests cover the wiring.
- **New issue / stub spotted**: none.

### WEFT-203 — types: `claude_enabled` default divergence doc
- **Status**: confirmed in code
- **Files**: `crates/clawft-types/src/delegation.rs:18-35, 74-86, 178-192`
- **Acceptance criteria met**:
  - [x] Doc-comment on `DelegationConfig::claude_enabled` (lines 18-33) explicitly spells out the divergence: `Default::default()` → `true` (graceful-degrade); `serde(default)` → `bool::default()` → `false`. Includes the worked example "write nothing → Claude on; write `[delegation]` with other keys but no `claude_enabled` → Claude off."
  - [x] `Default for DelegationConfig` impl preserves the `true` runtime default with the inline comment "Gracefully degrades if no API key" (line 77)
  - [x] Test `delegation_config_from_empty_json` (line 178) carries the comment "WEFT-203: pinning the documented divergence between `Default::default()` (claude_enabled = true) and `serde(default)` on `{}` (claude_enabled = false). See the doc-comment on `DelegationConfig::claude_enabled`."
  - [x] No runtime change — the divergence is now documented rather than reconciled. This was an explicit choice in the close comment ("doc-only").
- **Tests**: 287/287 passed (`cargo test -p clawft-types --lib`)
- **New issue / stub spotted**: none. The doc-comment is genuinely thorough and points to both pinning tests by name.

### WEFT-237 — voice: `publish_wav` role doc per ADR-053
- **Status**: confirmed in code
- **Files**: `crates/clawft-service-whisper/examples/publish_wav.rs:1-37`
- **Acceptance criteria met**:
  - [x] Decision documented as **keep** (lines 4-16 carry the `# Status (WEFT-237)` block declaring it a "long-lived dev/operator harness for the canonical substrate-side STT path (see ADR-053)")
  - [x] Role and use case are explicit: "lowest-friction reproducer when the live deployment is misbehaving", "live documentation of the `substrate/_derived/transcript/<source-node-id>/mic` shape", "easiest entry point for new contributors"
  - [x] "Do not delete" warning makes the keep-vs-delete decision binding (line 14)
  - [x] CI build status: example is buildable via `cargo run -p clawft-service-whisper --example publish_wav` (Cargo.toml `[[example]]` already declares it). Not exercised by the workspace test, but the docblock claims it as manual triage tooling, not CI.
- **Tests**: not directly tested; docblock-only change.
- **New issue / stub spotted**: none.

### WEFT-241 — voice: `transcript_log` doc per ADR-053
- **Status**: confirmed in code
- **Files**: `crates/clawft-plugin/src/voice/transcript_log.rs:40-58, 64-70`
- **Acceptance criteria met**:
  - [x] Type-level docblock includes `# Join key contract (WEFT-241)` section that names the join: `session_id == <source-node-id>`. Cites the substrate path `substrate/_derived/transcript/<source-node-id>/mic` and the helper `clawft_service_whisper::derive_source_node_from_path` that produces the canonical key.
  - [x] Constructor docblock on `TranscriptLogger::new` (lines 64-70) reinforces the contract: "`session_id` should be the substrate `<source-node-id>` that produced the transcripts being recorded"
  - [x] Manual correlation step is the one chosen (no automatic emit at session-start) — explicitly states "The sensor node publishing the PCM owns this identifier; the agent / consumer reads it off the transcript path. Any other choice (UUID per session, agent name, etc.) breaks the join."
  - [x] References ADR-053 by name (line 56) for the canonical-path provenance
- **Tests**: 114/114 passed (`cargo test -p clawft-plugin --lib`); transcript_log tests cover the JSONL writer mechanics, not the join key directly (which is a contract on the caller).
- **New issue / stub spotted**: none. The contract is sufficiently specific that a future automatic-emit pass (deferred per the original issue) has a target shape to land into.

### WEFT-340 — agent-core: tests for `agent.chat` dispatch (audit-only close)
- **Status**: confirmed in code (audit-only)
- **Files**: `crates/clawft-weave/tests/agent_chat_dispatch.rs:136-225`
- **Acceptance criteria met**:
  - [x] Integration test in `crates/clawft-weave/tests/` constructs a daemon without `DAEMON_AGENT` and asserts the error response shape — file exists, three `#[tokio::test]`s pin three branches:
    - `agent_chat_returns_error_when_service_not_wired` (line 137) — full payload but no LLM, asserts `err.contains("agent service not wired")`
    - `agent_chat_returns_error_when_params_invalid` (line 167) — missing `messages`, asserts the typed `invalid params` arm
    - `agent_chat_cancel_clean_error_when_service_not_wired` (line 203) — D3 cancel arm doesn't panic when service is unwired
  - [x] Test runs in `cargo test -p clawft-weave` — all 3 pass in 28.56s (whole-crate compile dominated)
  - [x] Test daemon mirror of `tests/control_rpc.rs::spawn_test_daemon` (lines 78-110) is documented in the file header (lines 9-13) — pattern is consistent
- **Tests**: 3/3 passed (`cargo test -p clawft-weave --test agent_chat_dispatch`)
- **New issue / stub spotted**: none. The audit-only close is honest — coverage was already in the codebase, the Plane item retroactively documented it.

### WEFT-383 — graphify: clean up dead `clawft-llm` optional dep flag
- **Status**: confirmed in code
- **Files**: `crates/clawft-graphify/Cargo.toml:67-73`; `crates/clawft-graphify/src/semantic_extract.rs:5`
- **Acceptance criteria met**:
  - [x] `clawft-llm` removed from `[dependencies]` — Cargo.toml lines 68-73 carry the explanatory comment ("`clawft-llm` is intentionally NOT a dep here … The `semantic-extract` and `vision-extract` features take an `FnOnce(String) -> Future` callback instead of binding the provider, which keeps extraction logic testable with fake LLM responses … The previously-declared optional `clawft-llm` dep was dead — no feature gated it on, no source `use`d it. Removed in WEFT-383 (ws12 cleanup).")
  - [x] Decision documented in Cargo.toml comment (citation above)
  - [x] No source `use`d the dep — confirmed by `grep -n "clawft_llm\|clawft-llm" crates/clawft-graphify/src/`: only the corrected docblock at `semantic_extract.rs:5` (the docblock now describes the callback shape rather than the missing trait)
- **Tests**: not run (no test surface change); the workspace `cargo check` would have flagged any remaining `use clawft_llm::…` line.
- **New issue / stub spotted**: none.

### WEFT-498 — types: relocate `AgentChat*` wire types to clawft-types
- **Status**: confirmed in code (load-bearing relocation, not a re-export drift)
- **Files**: `crates/clawft-types/src/agent_chat.rs:1-185`; `crates/clawft-service-agent/src/protocol.rs:1-17`; `crates/clawft-weave/src/protocol.rs:580-594, 1178-1199`; `crates/clawft-weave/src/daemon.rs:107, 3849, 5080`
- **Acceptance criteria met**:
  - [x] `AgentChatMessage / Params / ToolCall / Result` + `default_conv_id` moved to `clawft-types/src/agent_chat.rs` — confirmed single source of truth via `grep -rn "pub struct AgentChatParams\|…Result\|…Message\|…ToolCall" crates/`: only `crates/clawft-types/src/agent_chat.rs` defines them. The duplicates in `clawft-weave::protocol` and `clawft-service-agent::protocol` are gone.
  - [x] Re-exports preserve the pre-WEFT-498 import paths: `crates/clawft-weave/src/protocol.rs:592-594` and `crates/clawft-service-agent/src/protocol.rs:14-16` are pure `pub use clawft_types::agent_chat::{…}` lines.
  - [x] Daemon dispatch (`clawft-weave/src/daemon.rs:3849`) deserializes directly into the relocated `AgentChatParams`; no `From` bridge survives.
  - [x] **Compile-time `_assert_same` test present** at `crates/clawft-weave/src/protocol.rs:1178-1199` (test name `agent_chat_wire_and_service_types_are_identical`). The test instantiates an `AgentChatParams` and a `clawft_service_agent::AgentChatParams` and passes them through `fn _assert_same<T>(_: T, _: T)` — if a future contributor reintroduces a duplicate, this assertion stops compiling. **This is exactly the future-drift guard the audit instructions called out.**
  - [x] No behaviour change; tests still pass (9/9 in `cargo test -p clawft-weave --lib protocol`, 17/17 in `cargo test -p clawft-service-agent --lib`, 287/287 in `cargo test -p clawft-types --lib`, 3/3 integration tests).
  - [x] Audit-row close referenced — close comment cites commit `bd58db14`.
- **Tests**: see counts above. All targeted suites green.
- **New issue / stub spotted**: none. The `default_conv_id` doc note (lines 70-75) is well-scoped — it explicitly flags the function as a Phase-A-only ephemeral generator; future work should swap callers to ULID-style stable ids without changing the wire shape.

### WEFT-72 — plugin-skills: `SkillContext::Fork` audit-only close
- **Status**: confirmed (audit-only — type does not exist)
- **Verification**: `grep -rni "SkillContext\|skill_context\|SubagentManager" crates/` returns **zero hits** across the workspace. The 3F-agents review M2 footgun ("fork variant silently does nothing") is not present in 0.7.x because the dispatch scaffold was never landed.
- **Acceptance criteria met**:
  - [x] Audit current code path for `SkillContext::Fork` — done; absent.
  - [x] If silently no-ops: change to explicit error + close on a later milestone — N/A, no code path exists.
  - [x] If still deferred: explicit `tracing::warn!` on use + Plane item documenting the timeline — N/A; close comment promises a fresh Plane item if/when subagent forking is reintroduced (Workstream H or 1.0.x).
- **Tests**: N/A — no code change; no surface to test.
- **New issue / stub spotted**: none. If a contributor adds `SkillContext` back without filing the explicit-error / `tracing::warn!` issue, that would be a regression — but there is no automated gate for "code that doesn't exist yet". This is acceptable for an audit-only close.

### WEFT-77 — plugin: `VoiceHandler` forward-compat banner doc
- **Status**: confirmed in code
- **Files**: `crates/clawft-plugin/src/traits.rs:298-325`; `crates/clawft-plugin/src/lib.rs:17, 84`
- **Acceptance criteria met**:
  - [x] Decision recorded — option (b), keep `pub` for forward-compat with a banner doc comment. Explicitly stated at `traits.rs:317-320` ("Decision (release-gate WEFT-77): keep `pub` for forward-compat with a banner doc comment. Do not `#[doc(hidden)]` — external integrators reading the public surface should see this trait *and* be told plainly that it is not load-bearing yet.")
  - [x] Banner block at the top of the trait docstring (`> **Status (0.7.x):** Reserved API surface only — no production implementations are shipped, no plugin loader path exercises this trait, and no end-to-end audio pipeline is wired through it. Treat any concrete impl you build against it as experimental.`) — lines 300-303
  - [x] Module-level trait table at `lib.rs:17` echoes the same banner: "Voice/audio processing — forward-compat placeholder, no impl in 0.7.x (Workstream G)"
  - [x] No public API surface drift in 0.7.0 — `VoiceHandler` is still re-exported at `lib.rs:84`, signatures unchanged
- **Tests**: 114/114 passed (`cargo test -p clawft-plugin --lib`). Existing `MockVoiceHandler` exercises the trait shape (lines 639+); `assert_send_sync::<dyn VoiceHandler>()` (line 407) pins the auto-trait bounds.
- **New issue / stub spotted**: none. The banner is unambiguous and visible to `cargo doc` consumers.

### WEFT-78 — plugin-skills: `.weftos-plugin.toml` scaffold (audit-only close)
- **Status**: confirmed in code (audit-only — already shipped under WEFT-64)
- **Files**: `crates/clawft-plugin/src/manifest.rs:421-…`; `crates/clawft-cli/src/commands/plugins_cmd.rs:436-466`
- **Acceptance criteria met**:
  - [x] Subsumed by task 6 (WEFT-64): canonical = `clawft.plugin.json`; legacy = `.weftos-plugin.toml` read-only with deprecation warning. Confirmed at `manifest.rs:421` (`from_legacy_toml` emits `tracing::warn!("loading deprecated .weftos-plugin.toml manifest format; please migrate to clawft.plugin.json")` at lines 422-425) and `manifest.rs:603` (the hand-rolled `parse_legacy_toml` scanner that avoids dragging the `toml` crate into clawft-plugin's dep graph).
  - [x] Validator path consolidated at `plugins_cmd.rs:436-466`:
    - lines 441-453: prefer `clawft.plugin.json`; warn if both files are present
    - lines 454-464: accept legacy TOML standalone, push deprecation warning into the validation report, route through `PluginManifest::from_legacy_toml`
    - line 466: hard error when no manifest is found
  - [x] Tests: `manifest.rs:1063+` carries `legacy_toml_basic_parse`, `legacy_toml_channel_type_maps_capability`, `legacy_toml_analyzer_type_maps_pipeline_stage`, … (the close comment lists at least three; full suite passes 114/114)
- **Tests**: 114/114 passed (`cargo test -p clawft-plugin --lib`)
- **New issue / stub spotted**: none.

## Cross-cutting findings

### Stubs / TODOs spotted
- `crates/clawft-cli/src/commands/plugins_cmd.rs:153,193,198,233,243,272` — `// TODO: implement …` comments. **These are inside template strings emitted by the plugin scaffolder (`weft plugins new`)** that get written verbatim into newly generated user plugins. They are NOT stubs in the codebase; they are content the scaffolder produces for the user to fill in. The surrounding code (`plugin_template_*` functions) wraps them in `r#"…"#` raw strings. No action required.
- No `todo!()`, `unimplemented!()`, `// stub`, or load-bearing `FIXME:` markers in any of the touched files for the 13 WEFT items.

### Foundation crate health
- **clawft-rpc reqwest feature surface**: per-crate Cargo.toml adds `blocking` only; workspace base contributes `json + default-tls` (default-features off). `cargo tree -p clawft-rpc -e features` confirms exactly three reqwest feature edges: `blocking`, `default-tls`, `json`. The crate already pulls tokio for its UDS daemon transport, so reqwest's tokio runtime pull-in is not new surface. **Confirmed: blocking only on top of the inherited workspace base. ✓**
- **clawft-types `agent_chat` single source of truth**: confirmed via `grep -rn "pub struct AgentChat(Params|Result|Message|ToolCall)" crates/` — only `crates/clawft-types/src/agent_chat.rs` carries the struct definitions. Both `clawft-weave::protocol` and `clawft-service-agent::protocol` are pure `pub use` re-exports. The compile-time `_assert_same` test at `clawft-weave/src/protocol.rs:1178-1199` will catch any future re-introduction of a duplicate as a compilation failure. **Confirmed. ✓**
- **clawft-channels telegram poll**: `DEFAULT_POLL_INTERVAL_SECS = 0` (line 38) and `if self.poll_interval_secs > 0` guard (line 282) both intact. The `tokio::select!` cancel-token branch wraps the sleep block. **Confirmed. ✓**

### Tests
- clawft-rpc: 7 passed (version_check::tests; `cargo test -p clawft-rpc --lib version_check`)
- clawft-core: 32 passed (workspace::; 1247 filtered out — the rest of the lib's tests are not in scope for this audit)
- clawft-channels: 35 passed (telegram filter; 161 filtered out)
- clawft-types: 287 passed (full lib)
- clawft-service-agent: 17 passed (full lib)
- clawft-weave: 9 passed (`--lib protocol`); 3 passed (`--test agent_chat_dispatch`)
- clawft-plugin: 114 passed (full lib)
- **Total: 504 passing tests across the audit surface; zero failures, zero ignored, zero hangs.** No daemon-client pre-existing failures encountered (no daemon running during audit).

### Recommendations / new issues
- **None warranting a new Plane item.** All 13 items shipped exactly as their close comments described, and the documented contracts/divergences (WEFT-203 default-divergence, WEFT-241 join key, WEFT-77 voice forward-compat, WEFT-78 legacy-TOML deprecation) are precise enough to survive future refactors.
- Minor observation (NOT a new issue): the Cargo.toml comment at `crates/clawft-rpc/Cargo.toml:21-23` says "Other features stay default-off to keep this crate's compile cost minimal" — this is true only for the per-crate addition; the workspace base inherits `json + default-tls`. The framing is accurate but a future reader scanning only the per-crate file might think reqwest has *only* `blocking`. Not material; not worth a new ticket.
- WEFT-72 audit-only close: there is no automated gate that fails CI if a contributor reintroduces `SkillContext` without the documented `tracing::warn!` / explicit-error guard. This is an acceptable risk for 0.7.0 (the absence is genuine; the ticket discharged itself), but if Workstream H lands, the explicit-error contract should be re-instated as part of the new Plane item the close comment promises.

## Summary
- Items confirmed shipped: **13/13**
- Items with concerns / partial: **0**
- New issues filed: **0**

audit-D (foundation): 13/13 confirmed, 0 concerns, 0 new issues filed.
