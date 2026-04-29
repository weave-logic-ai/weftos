# ClawFT Unified Sprint Plan

> Single sprint integrating codebase fixes (from full 9-crate review) with the OpenClaw-parity
> feature roadmap. Items are grouped into workstreams and ordered by dependency within each.

> **Closed 2026-04-28 (WEFT-24)** — historical reference only. This document is the
> original sprint plan for Phase 5 (the "Improvements Sprint"). It is retained for
> context but no longer reflects live tracking. The 0.7.0 release-gate audit at
> `.planning/reviews/0.7.0-release-gate/` is the source-of-truth for what shipped,
> what lingered, and what is deferred per workstream. Outstanding TODOs were lifted
> into Plane work items (`weftos` workspace, cycle `0.7.x` for must-ship-before-0.7,
> `0.8.x`/`0.9.x`/`1.0.x` for deferred). See
> `.planning/development_notes/02-improvements-overview/sprint-tracker.md` for the
> matching closure note on the per-element tracker.

---

## Workstream A: Critical Fixes (Week 1-2)

These bugs and security issues must be resolved before any feature work builds on top.

### A1. Session key round-trip corruption
**File:** `clawft-core/src/session.rs`
**Type:** Bug

`session_path()` replaces `:` with `_`, and `list_sessions()` reverses `_` back to `:`. A key like `"telegram:user_123"` becomes filename `"telegram_user_123.jsonl"` and reloads as `"telegram:user:123"` -- a different key. Any channel or chat ID containing underscores silently corrupts session identity.

**Fix:** Use percent-encoding or a two-character escape sequence instead of 1:1 character substitution.

### A2. Unstable hash function in embeddings
**File:** `clawft-core/src/embeddings/hash_embedder.rs`
**Type:** Bug

Uses `std::collections::hash_map::DefaultHasher`, whose output is explicitly not stable across Rust versions or program runs. Persisted embeddings become silently invalid after a toolchain upgrade, producing incorrect similarity results.

**Fix:** Replace with a stable deterministic hash (`fnv`, `xxhash`, or `ahash` with fixed seed). Include a one-time re-index migration if any embeddings have been persisted.

### A3. Invalid JSON from error formatting
**File:** `clawft-core/src/agent/loop_core.rs`
**Type:** Bug

```rust
format!("{{\"error\": \"{}\"}}", e)
```
If the error message contains a double-quote, the result is malformed JSON sent to the LLM as a tool result.

**Fix:** Use `serde_json::json!({"error": e.to_string()}).to_string()`.

### A4. Plaintext credentials in config structs
**File:** `clawft-types/src/config.rs`
**Type:** Security

`imap_password`, `smtp_password`, `app_secret`, `client_secret`, `claw_token`, and `api_key` are stored as plain `String` fields with no `#[serde(skip_serializing)]` or `Debug` redaction. These appear in serialized JSON, debug output, and audit logs. `clawft-llm` correctly stores only the env var name (`api_key_env`) -- the types crate should follow this pattern.

**Fix:** Store env var names instead of raw secrets. Add custom `Debug` impls that redact sensitive fields. This is prerequisite for the Email and OAuth2 work in Workstream F.

### A5. API key echoed during onboarding
**File:** `clawft-cli/src/commands/onboard.rs`
**Type:** Security

`prompt_provider_config()` reads the API key via `reader.next_line()`, which echoes input to the terminal.

**Fix:** Use `rpassword::read_password()` to suppress terminal echo.

### A6. Incomplete private IP range in SSRF protection
**File:** `clawft-services/src/mcp/middleware.rs`
**Type:** Security

`UrlPolicy` only blocks `172.16.*` but RFC 1918 range `172.16.0.0/12` covers `172.16.*` through `172.31.*`. URLs like `http://172.30.0.1/` bypass the check.

**Fix:** Parse the second octet and check `(16..=31).contains(&n)`.

### A7. No HTTP request timeout on LLM provider client
**File:** `clawft-llm/src/openai_compat.rs`
**Type:** Reliability

`reqwest::Client::new()` is used without a timeout. A provider that never responds blocks the task indefinitely.

**Fix:** Use `reqwest::ClientBuilder` with `.timeout(Duration::from_secs(120))`.

### A8. `unsafe std::env::set_var` in parallel tests
**File:** `clawft-core/src/workspace.rs`
**Type:** Correctness

Tests call `unsafe { std::env::set_var(...) }` under Rust's default parallel test runner. This is UB in Rust 2024 edition.

**Fix:** Use `temp_env` crate or a mutex guard.

### A9. `--no-default-features` does not compile
**File:** `clawft-cli/src/mcp_tools.rs`
**Type:** Bug

`mcp_tools.rs` unconditionally imports `clawft_services` (MCP session, transport, types) even when the `services` feature is disabled. Running `cargo build -p clawft-cli --no-default-features` produces 11 errors (`use of unresolved module or unlinked crate clawft_services`). The `services` feature is effectively required despite being declared optional.

**Fix:** Gate all `clawft_services` imports and the `register_mcp_tools()` function behind `#[cfg(feature = "services")]`. Provide a no-op stub when the feature is off (same pattern already used for `register_delegation()` with the `delegate` feature).

---

## Workstream B: Architecture Cleanup (Week 2-4)

Structural improvements that unblock feature work and reduce maintenance burden. These touch shared types and interfaces that later workstreams depend on.

### B1. Unify `Usage` type across crates
**Files:** `clawft-types/src/provider.rs` (`u32`), `clawft-llm/src/types.rs` (`i32`)
**Type:** Refactor

Token counts are `u32` in one crate and `i32` in the other with no conversion function. Token counts are never negative.

**Fix:** Canonical `Usage` type in `clawft-types` with `u32` fields. `clawft-llm` imports and uses it.

### B2. Unify duplicate `LlmMessage` types
**Files:** `clawft-core/src/agent/context.rs`, `clawft-core/src/pipeline/traits.rs`
**Type:** Refactor

Two separate structs with identical fields. TODO comment acknowledges this.

**Fix:** Single type in `clawft-core/src/pipeline/traits.rs`, re-exported. Remove the duplicate from `context.rs`.

### B3. Split oversized files
**Type:** Refactor

| File | Lines | Target Split |
|------|-------|-------------|
| `clawft-types/src/config.rs` | ~1400 | `config/channels.rs`, `config/providers.rs`, `config/policies.rs`, `config/mod.rs` |
| `clawft-core/src/agent/loop_core.rs` | 1645 | Extract tool execution, streaming, message building |
| `clawft-core/src/pipeline/tiered_router.rs` | 1646 | Extract cost tracker, tier selection, classifier |
| `clawft-core/src/pipeline/transport.rs` | 1282 | Extract request building, response parsing |
| `clawft-core/src/tools/registry.rs` | 1242 | Extract individual tool implementations |
| `clawft-core/src/agent/skills_v2.rs` | 1159 | Extract YAML parsing, caching, registry |
| `clawft-core/src/pipeline/llm_adapter.rs` | 1127 | Extract retry logic, config override |
| `clawft-core/src/pipeline/traits.rs` | 1107 | Extract callback types, pipeline stages |
| `clawft-types/src/routing.rs` | ~950 | Extract permissions, delegation |

### B4. Unify cron storage formats
**Files:** `clawft-cli/src/commands/cron.rs` vs `clawft-services/src/cron_service/`
**Type:** Bug / Refactor

CLI uses `CronStore` (flat JSON); `CronService` uses JSONL event sourcing. Incompatible formats -- jobs created via CLI are invisible to the gateway and vice versa.

**Fix:** Unify on JSONL event sourcing. CLI commands drive the `CronService` API.

### B5. Extract shared tool registry builder
**Files:** `clawft-cli/src/commands/agent.rs`, `gateway.rs`, `mcp_server.rs`
**Type:** Refactor

Identical 6-step tool setup block copy-pasted into three files.

**Fix:** Extract `build_tool_registry(config, platform) -> ToolRegistry` into `commands/mod.rs`.

### B6. Extract shared policy types
**Files:** `clawft-services/src/mcp/middleware.rs`, `clawft-tools/`
**Type:** Refactor

`CommandPolicy` and `UrlPolicy` are defined in both crates. Bug fixes must be manually replicated.

**Fix:** Canonical definitions in `clawft-types`. Both crates import from there.

### B7. Deduplicate `ProviderConfig` naming collision
**Files:** `clawft-llm/src/config.rs`, `clawft-types/src/config.rs`
**Type:** Refactor

Both crates define `ProviderConfig` with different semantics (env var name vs plaintext key). Confusing and error-prone.

**Fix:** Rename `clawft-llm`'s to `LlmProviderConfig` or merge into a single type with the env-var-name pattern.

### B8. Consolidate `build_messages` duplication
**File:** `clawft-core/src/agent/context.rs`
**Type:** Refactor

`build_messages` and `build_messages_for_agent` share ~80% code.

**Fix:** Extract shared base with an `extra_instructions: Option<String>` parameter.

### B9. MCP protocol version constant
**Files:** `clawft-services/src/mcp/server.rs`, `mod.rs`
**Type:** Cleanup

`"2025-06-18"` hardcoded in multiple places.

**Fix:** Single `const MCP_PROTOCOL_VERSION` in `mcp/types.rs`.

---

## Workstream C: Plugin & Skill System (Week 3-8)

New crate for plugin infrastructure. Foundation for all extensibility work.

### C1. Define `clawft-plugin` trait crate
**Type:** Feature -- New Crate

Define unified plugin traits: `Tool`, `ChannelAdapter`, `PipelineStage`, `Skill`, `MemoryBackend`, `VoiceHandler`. All new capabilities (email, browser, voice, dev tools) implement these traits rather than modifying core.

**Output:** New crate `clawft-plugin` with trait definitions, manifest schema (JSON/YAML), and `SKILL.md` compatibility types.

### C2. WASM plugin host
**Type:** Feature -- `clawft-wasm` + `clawft-plugin`
**Deps:** C1

Implement WASM plugin host using `wasmtime` + `wit` component model for typed interfaces. Plugins ship as `.wasm` + manifest + optional `SKILL.md`.

**Includes:**
- Complete `WasiFileSystem` (currently all stubs returning `Unsupported`)
- Wire `init()` and `process_message()` in `clawft-wasm`
- Implement `WasiEnvironment` against the actual `Platform::Environment` trait (currently standalone struct with matching signatures but no trait impl)
- WASM HTTP client implementation
- Size budget enforcement: <300KB uncompressed, <120KB gzipped

### C3. Skill Loader (OpenClaw-compatible)
**Type:** Feature -- `clawft-core/src/agent/skills_v2.rs`
**Deps:** C1

Parse `SKILL.md` (YAML frontmatter -> tool description + execution hints), auto-register as WASM or native wrapper. Support `ClawHub` discovery (HTTP index + git clone).

**Prerequisite fix:** Replace the hand-rolled YAML parser in `skills_v2.rs` (item B3/30) with `serde_yaml::from_str` before building on top. Current parser doesn't handle nested structures, multi-line values, or quoted strings.

### C4. Dynamic skill loading & hot-reload
**Type:** Feature
**Deps:** C2, C3

Runtime loading with sandbox isolation. `weft skill install github.com/openclaw/skills/coding-agent` works. Agent can `weft skill create "new skill for X"` and compile to WASM.

**Includes (OpenClaw parity):**
- **Skill precedence layering:** workspace > managed/local > bundled (matching OpenClaw's resolution order). Skills in `~/.clawft/skills` (managed) are visible to all agents; workspace skills override.
- **Hot-reload watcher:** File-system watcher (`notify` crate) on skill directories. Changes take effect mid-session without restart.
- **Plugin-shipped skills:** Plugins declare skill directories in their manifest (`clawft.plugin.json`). Plugin skills load when the plugin is enabled and participate in normal precedence.

### C4a. Autonomous skill creation
**Type:** Feature
**Deps:** C4

The agent can decide on its own to create new skills when it encounters a repeated task it doesn't have a skill for. The agent loop detects "I've done this pattern N times" and triggers skill generation: writes `SKILL.md` + implementation, compiles to WASM if native, and installs into managed skills directory. This is OpenClaw's "self-improving" capability.

### C5. Wire interactive slash-command framework
**File:** `clawft-cli/src/interactive/`
**Type:** Feature / Cleanup
**Deps:** C3

The `builtins` and `registry` modules are dead code -- `agent.rs` implements commands inline with `match`. Wire agent commands through the registry to support dynamic skill-contributed commands.

### C6. Extend MCP server for loaded skills
**Type:** Feature
**Deps:** C3

Auto-expose loaded skills/tools through the MCP server for VS Code/Copilot/Claude Desktop integration.

### C7. Update PluginHost to unify channels + tools
**File:** `clawft-channels/src/host.rs`
**Type:** Refactor
**Deps:** C1

Unify under plugin trait system. Add `SOUL.md`/`AGENTS.md` personality injection into Learner/Assembler pipeline stages.

**Includes:** Make `start_all`/`stop_all` concurrent (currently sequential loops).

---

## Workstream D: Pipeline & LLM Reliability (Week 2-5)

Improvements to the agent loop, LLM transport, and routing pipeline.

### D1. Parallel tool execution
**File:** `clawft-core/src/agent/loop_core.rs`
**Type:** Performance

When the LLM returns multiple tool calls, they execute sequentially in a `for` loop.

**Fix:** Use `futures::future::join_all` for concurrent execution.

### D2. Streaming failover correctness
**File:** `clawft-llm/src/failover.rs`
**Type:** Bug

Mid-stream provider failure sends partial data from the first provider followed by full data from the next, concatenated on the same channel.

**Fix:** Implement "reset stream" that discards partial output before failover. At minimum, document the limitation.

### D3. Structured error variants for retry
**File:** `clawft-llm/src/retry.rs`
**Type:** Refactor

`is_retryable()` uses fragile string prefix matching (`"HTTP 500"`, etc.).

**Fix:** Add `ServerError { status: u16 }` variant to `ProviderError`.

### D4. Configurable retry policy
**File:** `clawft-core/src/pipeline/llm_adapter.rs`
**Type:** Feature

Retry count (3), backoff delay, and eligible status codes are hardcoded.

**Fix:** Make configurable via `ClawftLlmConfig`.

### D5. Record actual latency
**Files:** `clawft-core/src/pipeline/traits.rs`, `src/agent/loop_core.rs`
**Type:** Feature

`ResponseOutcome.latency_ms` is hardcoded to `0` everywhere.

**Fix:** Record wall-clock latency around provider calls. Required for routing feedback and observability.

### D6. Thread `sender_id` for cost recording
**File:** `clawft-core/src/pipeline/tiered_router.rs`
**Type:** Feature

`update()` cannot record costs -- `sender_id` not available on `RoutingDecision`. `CostTracker` infrastructure is built but integration is a no-op.

**Fix:** Thread `sender_id` through the pipeline.

### D7. Change `StreamCallback` to `FnMut`
**File:** `clawft-core/src/pipeline/traits.rs`
**Type:** Fix

`Fn` prevents token accumulators or progress trackers from working as callbacks.

**Fix:** Change to `FnMut`.

### D8. Bounded message bus channels
**File:** `clawft-core/src/bus.rs`
**Type:** Reliability

Uses unbounded channels. No backpressure; fast producer with slow consumer grows memory without limit.

**Fix:** Switch to `bounded_channel` with configurable buffer size.

### D9. MCP transport concurrency
**File:** `clawft-services/src/mcp/transport.rs`
**Type:** Performance

`StdioTransport` serializes concurrent calls completely. `HttpTransport` creates a new `reqwest::Client` per instance.

**Fix:** Implement request-ID multiplexer for stdio. Accept `Arc<reqwest::Client>` for HTTP. Redirect child stderr to log stream.

### D10. Cache skill/agent bootstrap files
**File:** `clawft-core/src/agent/context.rs`
**Type:** Performance

`build_system_prompt` reads files from disk on every LLM call.

**Fix:** Cache content with mtime checking.

### D11. Async file I/O in skills loader
**File:** `clawft-core/src/agent/skills_v2.rs`
**Type:** Performance

`std::fs::read_dir` and `std::fs::read_to_string` block the Tokio executor.

**Fix:** Replace with `tokio::fs` equivalents.

---

## Workstream E: Channel Enhancements (Week 4-8)

Improvements to existing channels and new channel plugins.

### E1. Discord Resume (OP 6)
**File:** `clawft-channels/src/discord/channel.rs`
**Type:** Feature

On reconnect, always re-identifies instead of resuming. `session_id` and `resume_url` are stored but unused. `ResumePayload` is dead code.

**Fix:** Implement Gateway Resume when `session_id` is available.

### E2. Email channel plugin
**Type:** Feature -- New Plugin
**Deps:** C1, A4

IMAP + SMTP via `lettre` + `imap` crates. Gmail OAuth2 via `oauth2` crate. Full read/reply/attach, proactive inbox triage via cron. Implemented as a `clawft-plugin` `ChannelAdapter`.

### E3. WhatsApp channel
**Type:** Feature -- New Plugin
**Deps:** C1

Via official WhatsApp Cloud API wrapper. Implemented as plugin.

### E4. Signal / iMessage bridge
**Type:** Feature -- New Plugin
**Deps:** C1

Via `signal-cli` subprocess or macOS bridge. Implemented as plugin.

### E5. Matrix / IRC channels
**Type:** Feature -- New Plugin
**Deps:** C1

Generic Matrix and IRC channel adapters.

### E5a. Google Chat channel
**Type:** Feature -- New Plugin
**Deps:** C1

Google Chat via Google Workspace API. OAuth2 flow (reuse F6). Supports DMs and Spaces.

### E5b. Microsoft Teams channel
**Type:** Feature -- New Plugin
**Deps:** C1

Microsoft Teams via Bot Framework / Graph API. Azure AD auth. Supports channels and 1:1 chats.

### E6. Enhanced heartbeat / proactive check-in
**File:** `clawft-services/src/heartbeat/`
**Type:** Feature
**Deps:** B4

Enhance existing CronService with "check-in" mode for proactive agent behavior (inbox triage, status summaries).

---

## Workstream F: Software Dev & App Tooling (Week 5-10)

Developer tools and application integrations, all implemented as plugins.

### F1. Git tool plugin
**Type:** Feature -- New Plugin
**Deps:** C1

Via `git2` crate: clone, commit, branch, PR, diff, blame. Integrated as MCP-exposed tool.

### F2. Cargo/build integration
**Type:** Feature -- New Plugin
**Deps:** C1

Build, test, clippy, publish. Integrated as skill with tool calls.

### F3. Code analysis via tree-sitter
**Type:** Feature -- New Plugin
**Deps:** C1

AST-level code parsing and analysis. LSP client for IDE-like code intelligence.

### F4. Browser CDP automation
**Type:** Feature -- New Plugin
**Deps:** C1

Using `chromiumoxide` (async Rust CDP client). Headless/full control: screenshot, form fill, scraping. Sandboxed via separate process.

### F5. Calendar integration
**Type:** Feature -- New Plugin
**Deps:** C1

Google Calendar / Outlook / iCal via APIs. OAuth2 flow.

### F6. Generic REST + OAuth2 helper
**Type:** Feature -- New Plugin
**Deps:** C1

Reusable OAuth2 flow for all API integrations. Used by email, calendar, and future integrations.

### F7. Docker/Podman orchestration tool
**Type:** Feature -- New Plugin
**Deps:** C1

Container lifecycle management from agent context.

### F8. MCP deep IDE integration
**Type:** Feature
**Deps:** C6

Expose agent as VS Code extension backend. Agent edits code live in IDE through MCP.

### F9. MCP client for external servers
**Type:** Feature
**Deps:** D9

**OpenClaw parity:** Connect to 1000+ community MCP servers (Google Drive, Slack, databases, enterprise systems) as a client. Agent discovers available MCP servers, lists their tools, and invokes them through the standard MCP protocol. Config via `mcp_servers` section in `clawft.toml` or `weft mcp add <server-uri>`.

**Includes:** Auto-discovery of local MCP servers, connection pooling, tool schema caching, health checks.

---

## ~~Workstream G: Voice~~ -- OUT OF SCOPE

*Deferred to post-sprint. Full plan in `voice_development.md`.*

**Forward-compatibility requirements (in scope):**
- C1 must define the `VoiceHandler` trait alongside other plugin traits -- placeholder only, no implementation
- Plugin manifest schema must reserve a `voice` capability type
- `ChannelAdapter` trait must support binary (audio) payloads, not just text (needed for voice and media messages generally)
- Feature flag `voice` must be wired in Cargo.toml as empty/no-op so it can be populated later without breaking the feature matrix

---

## Workstream H: Memory & Workspace (Week 4-8)

### H1. Markdown workspace with per-agent isolation
**Type:** Feature
**Deps:** C1

`~/.clawft/workspace` with `SKILL.md`, `SOUL.md`, `USER.md`, conversation logs. Coexists with JSONL/vector memory. Auto-summarization of long conversations.

**Per-agent isolation (OpenClaw parity):** Each agent gets its own workspace under `~/.clawft/agents/<agentId>/` with dedicated `SOUL.md`, `AGENTS.md`, `USER.md`, and session store. Agent-specific personality and memory are fully isolated unless cross-agent access is explicitly enabled.

### H2. Complete RVF Phase 3 (vector memory)
**Type:** Feature
**Deps:** A2

RVF Phase 3 roadmap (`docs/guides/rvf.md`) is 1/9 complete -- only the crate dependency integration is done. All functional items are stubs. After fixing the unstable hash (A2), complete the following:

1. **HNSW-backed VectorStore** -- Replace brute-force cosine scan in `vector_store.rs` and `progressive.rs` (both are stubs) with HNSW graph from `rvf-index` when available, or `instant-distance` / `hnsw` crate as interim.
2. **Production embedder** -- Replace `HashEmbedder` with LLM embedding API (OpenAI/local ONNX). `api_embedder.rs` exists but needs wiring.
3. **RVF file I/O** -- Implement real RVF segment read/write for memory persistence. Currently `ProgressiveSearch` saves as plain JSON.
4. **`weft memory export` / `weft memory import`** -- CLI commands for portable data transfer. Currently only `show`, `history`, `search` exist.
5. **POLICY_KERNEL storage** -- Persist `IntelligentRouter` routing policies across restarts.
6. **WITNESS segments** -- Tamper-evident audit trail of agent actions.
7. **Temperature-based quantization** -- Hot/warm/cold tiers to reduce storage for infrequently accessed vectors.
8. **WASM compatibility** -- `micro-hnsw-wasm` for browser and edge deployments.

### H3. Standardize timestamp representations
**Files:** Various across `clawft-types`
**Type:** Refactor

| Type | Current |
|------|---------|
| `InboundMessage.timestamp` | `DateTime<Utc>` |
| `CronJob.created_at_ms` | `i64` (ms) |
| `WorkspaceEntry.last_accessed` | `Option<String>` |

**Fix:** Standardize on `DateTime<Utc>` throughout.

---

## Workstream I: Type Safety & Cleanup (Week 2-6)

Smaller fixes that improve correctness and maintainability.

### I1. `DelegationTarget` serde consistency
**File:** `clawft-types/src/routing.rs`

Serializes as PascalCase (`"Local"`, `"Claude"`) while all other enums use `snake_case`.

**Fix:** Add `#[serde(rename_all = "snake_case")]`.

### I2. String-typed policy modes to enums
**File:** `clawft-types/src/config.rs`

`CommandPolicyConfig::mode` and `RateLimitConfig::strategy` are `String` fields accepting specific values.

**Fix:** Define proper enums.

### I3. `ChatMessage::content` serialization
**File:** `clawft-llm/src/types.rs`

`None` content serializes as `"content": null` which some providers reject.

**Fix:** Add `skip_serializing_if = "Option::is_none"`.

### I4. Job ID collision fix
**File:** `clawft-cli/src/commands/cron.rs`

`generate_job_id()` uses seconds + PID. Same-second collisions.

**Fix:** Use `uuid::Uuid::new_v4()` (already in workspace deps).

### I5. `camelCase` normalizer acronym handling
**File:** `clawft-platform/src/config_loader.rs`

`"HTMLParser"` becomes `"h_t_m_l_parser"`.

**Fix:** Add consecutive-uppercase handling.

### I6. Dead code removal
- `evict_if_needed` in `clawft-core/src/pipeline/rate_limiter.rs` (`#[allow(dead_code)]`)
- `ResumePayload` in `clawft-channels/src/discord/events.rs` (dead until E1)
- Interactive slash-command framework in `clawft-cli/src/interactive/` (dead until C5)
- `--trust-project-skills` and `--intelligent-routing` CLI flags (no-ops)

**Fix:** Remove dead code, or add `// TODO(workstream)` with clear references to the feature work that will use it.

### I7. Fix always-true test assertion
**File:** `clawft-core/src/pipeline/transport.rs`

```rust
assert!(result.is_err() || result.is_ok());
```

**Fix:** Assert the expected specific outcome.

### I8. Share `MockTransport` across crates
**File:** `clawft-services/src/mcp/transport.rs`

`#[cfg(test)]` prevents downstream crates from reusing it.

**Fix:** Expose behind a `test-utils` feature flag.

---

## Workstream J: Documentation & Docs Sync (Week 3-5)

### J1. Fix provider counts
**Files:** `docs/architecture/overview.md`, `docs/guides/providers.md`, `docs/getting-started/quickstart.md`, `docs/reference/config.md`, `clawft-types/src/lib.rs`

Docs say 7-8 providers; actual is 9 (gemini, xai missing). `lib.rs` says 14; `PROVIDERS` has 15.

### J2. Fix assembler truncation description
**File:** `docs/architecture/overview.md`

Says "no truncation at Level 0" but `TokenBudgetAssembler` actively truncates with first+last preservation.

### J3. Fix token budget source reference
**File:** `docs/guides/routing.md`

Says budget comes from `agents.defaults.max_tokens`; code now sources from `max_context_tokens` across routing tiers.

### J4. Document identity bootstrap behavior
**Files:** `docs/guides/skills-and-agents.md` or `docs/guides/configuration.md`

`SOUL.md` and `IDENTITY.md` override default agent identity preamble when placed in workspace root or `.clawft/`. Not documented anywhere.

### J5. Document rate-limit retry behavior
**File:** `docs/guides/providers.md`

3-retry with 500ms minimum wait in `ClawftLlmAdapter` -- undocumented.

### J6. Document CLI log level change
**File:** `docs/reference/cli.md`

Default non-verbose level changed from `info` to `warn`.

### J7. Plugin system documentation
**Deps:** C1-C6

Full docs for the plugin/skill system: architecture, creating plugins, SKILL.md format, ClawHub registry, WASM compilation guide.

---

## Workstream K: Deployment & Community (Week 8-12)

*K1 (Web Dashboard + Live Canvas) and K6 (Native Shells) are OUT OF SCOPE -- see `ui_development.md`.*

**Forward-compatibility requirements (in scope):**
- Agent loop and bus must not assume text-only I/O (support structured/binary payloads for future canvas rendering)
- MCP server tool schemas should be stable enough that a future dashboard can introspect them without breaking changes
- Config and session APIs should be read-accessible without going through the agent loop (future dashboard will need direct access)

### K2. Docker images
**Type:** DevOps

Multi-arch Docker images. One-click VPS scripts. Voice deps added later when Workstream G is in scope.

### K3. Enhanced sandbox with per-agent isolation
**Type:** Security
**Deps:** C2

WASM + seccomp/landlock. Per-skill permission system. Audit logs.

**Per-agent sandboxing (OpenClaw parity):** Each agent gets its own sandbox with independent tool restrictions. Agent A can have shell access while Agent B is restricted to read-only file ops. Configured in agent workspace config (`~/.clawft/agents/<id>/config.toml`).

### K3a. Security plugin system (SecureClaw-equivalent)
**Type:** Feature / Security
**Deps:** C1, K3

A dedicated security plugin with:
- **Audit checks** (50+): Scan skills for prompt injection vectors, data exfiltration patterns, unsafe shell commands, credential leaks.
- **Hardening modules:** Auto-apply seccomp/landlock profiles, restrict network access per-skill, enforce allowlisted domains.
- **Background monitors:** Watch for anomalous tool usage, excessive API calls, unexpected file access patterns.
- **CLI integration:** `weft security scan`, `weft security audit`, `weft security harden`.

### K4. ClawHub skill registry with vector search
**Type:** Feature
**Deps:** C3, C4, H2

CLI for publishing/installing skills. Community examples repo. Skill templates.

**OpenClaw parity additions:**
- **Vector search for skill discovery:** Embed skill descriptions and match user queries semantically instead of keyword search. Uses the vector store from H2.
- **Agent auto-search:** When the agent can't find a matching local skill, it queries ClawHub automatically and offers to install.
- **Star/comment system:** Users can rate and review skills. Moderation hooks for quality control.
- **Versioning:** Skills are versioned with semver. `weft skill update` checks for newer versions.

### K5. Benchmarks vs OpenClaw
**Type:** Testing

Feature parity test suite. Performance comparison (binary size, cold start, memory, throughput).

---

## Workstream L: Multi-Agent Routing & Orchestration (Week 5-9)

Multi-agent features that match OpenClaw's isolation and routing model.

### L1. Agent routing table
**Type:** Feature
**Deps:** B5, C1

Route inbound channels/accounts/peers to isolated agents. Configuration maps channel-specific identifiers (WhatsApp number, Telegram user ID, Slack workspace) to agent IDs. Different people sharing one Gateway get fully isolated AI "brains".

```toml
[[agent_routes]]
channel = "telegram"
match = { user_id = "12345" }
agent = "work-agent"

[[agent_routes]]
channel = "whatsapp"
match = { phone = "+1..." }
agent = "personal-agent"
```

### L2. Per-agent workspace and session isolation
**Type:** Feature
**Deps:** L1, H1

Each routed agent gets: dedicated `agentDir` under `~/.clawft/agents/<agentId>/`, own session store, own skill overrides, own `SOUL.md`/`AGENTS.md`/`USER.md`. No cross-talk unless explicitly enabled via shared memory namespace.

### L3. Multi-agent swarming
**Type:** Feature
**Deps:** L1, L2

Leverage existing `.swarm/` directory and agent spawning. Agents can delegate subtasks to other agents, share results through the message bus. Coordinator agent pattern: one "lead" agent decomposes tasks and dispatches to worker agents.

### L4. Planning strategies in Router
**Type:** Feature
**Deps:** D6

ReAct (Reason+Act) and Plan-and-Execute strategies in the pipeline Router. The router can decide to decompose a complex request into a multi-step plan before executing, similar to OpenClaw's agentic reasoning patterns.

---

## Workstream M: Claude Flow / Claude Code Integration (Week 3-7)

The delegation system has well-designed engine logic (rule matching, complexity heuristics, fallback chains) but **zero functional integration** with Claude Flow or Claude Code. The `DelegationTarget::Flow` path is completely dead code.

### M1. Implement `FlowDelegator`
**File:** `clawft-services/src/delegation/flow.rs` (new)
**Type:** Feature
**Deps:** D9

The only delegator is `ClaudeDelegator`. No `FlowDelegator` exists anywhere in the codebase. Implement a `FlowDelegator` that:
- Spawns `claude` (Claude Code CLI) as a subprocess via `tokio::process::Command`
- Passes tasks via `claude --print` (non-interactive mode) or `claude --json`
- Streams results back through the agent loop
- Supports tool-use: Claude Code can call back into clawft's tool registry via MCP
- Falls back to `ClaudeDelegator` (direct Anthropic API) if `claude` binary is not available

**Alternative transport:** If `npx @claude-flow/cli` is preferred for multi-agent orchestration, implement as a second transport option alongside the direct CLI.

### M2. Wire `flow_available` to runtime detection
**File:** `clawft-tools/src/delegate_tool.rs:105`
**Type:** Bug fix

Currently hardcoded:
```rust
let flow_available = false; // Flow not wired yet.
```

Replace with runtime detection:
- Check if `claude` binary is on `$PATH` (via `which`/`command -v`)
- Check if `DelegationConfig.claude_flow_enabled` is `true`
- Optionally probe the Flow endpoint with a health check
- Cache the result (don't re-probe on every delegation decision)

### M3. Enable `delegate` feature by default
**Files:** `clawft-cli/Cargo.toml`, `clawft-services/Cargo.toml`, `clawft-tools/Cargo.toml`
**Type:** Config fix

The `delegate` feature is optional and off by default. Without it, `register_delegation()` compiles to a no-op stub (`mcp_tools.rs:275-281`). Both `claude_enabled` and `claude_flow_enabled` default to `false` in `DelegationConfig`.

**Fix:**
- Add `delegate` to the `default` feature list in `clawft-cli/Cargo.toml`
- Set `claude_enabled` default to `true` (gracefully degrades if no API key)
- Document the feature flags and config toggles in `docs/guides/configuration.md`

### M4. Dynamic MCP server discovery
**Files:** `clawft-cli/src/mcp_tools.rs`, `clawft-cli/src/commands/` (new subcommand)
**Type:** Feature
**Deps:** F9

`register_mcp_tools()` connects once at startup. If an MCP server is added later, or if a connection fails, there's no recovery path.

**Fix:**
- Add `weft mcp add <name> <command|url>` CLI command to register servers at runtime
- Add `weft mcp list` and `weft mcp remove` for management
- Implement reconnection with exponential backoff for failed MCP connections
- Support health-check pings to detect stale connections
- Hot-reload: watch `clawft.toml` for `mcp_servers` changes (reuse `notify` from C4)

### M5. Claude Code as MCP client transport
**Type:** Feature
**Deps:** M1, M4

Enable clawft to expose its tools to Claude Code *and* consume Claude Code's tools:

**Bidirectional MCP bridge:**
- **Outbound (clawft → Claude Code):** clawft registers itself as an MCP server that Claude Code can connect to. Already partially working via `clawft-services/src/mcp/server.rs`, but needs testing and documentation for the `claude mcp add` workflow.
- **Inbound (Claude Code → clawft):** clawft connects to Claude Code's MCP server as a client, gaining access to Claude Code's tool ecosystem. Uses the MCP client from F9.

**Config example:**
```toml
[tools.mcp_servers.claude-code]
command = "claude"
args = ["mcp", "serve"]
```

### M6. Delegation config in `clawft.toml` documentation
**Files:** `docs/guides/configuration.md`, `docs/guides/tool-calls.md`
**Type:** Documentation
**Deps:** M1, M2, M3

The delegation system is undocumented for end users. Add:
- How to enable delegation (`claude_enabled`, `claude_flow_enabled`)
- How to write routing rules (regex patterns → targets)
- How to configure excluded tools
- How to set up Claude Code integration (PATH, API key, MCP bridge)
- Troubleshooting: common failures and how to diagnose

---

## Cross-Cutting Concerns

These apply across multiple workstreams:

1. **Keep core tiny** -- Heavy deps (`wasmtime`, `chromiumoxide`, `git2`) go in optional plugins behind feature flags. Target: <10 MB base binary, sub-100ms cold start. Voice and UI deps are out of scope but the plugin/feature-flag system must accommodate them cleanly when they arrive.

2. **Offline capability** -- All local-first where possible. Cloud is always a fallback, never required.

3. **No core forks** -- After Workstream C, all new capabilities are plugins. No more modifying `clawft-core` for features.

4. **Recommended new dependencies** (minimal, Rust-native):
   - `wasmtime` + `wit-bindgen` (plugins)
   - `lettre`, `imap` (email)
   - `chromiumoxide` (browser)
   - `git2` (git ops)
   - `oauth2` (auth flows)
   - `tree-sitter` (code analysis)
   - `notify` (file-system watcher for skill hot-reload)
   - *Deferred:* `sherpa-rs`, `rustpotter`, `cpal` (voice -- see `voice_development.md`)
   - *Deferred:* `tauri` / `wry` (native shells -- see `ui_development.md`)

---

## Timeline Summary

| Weeks | Focus |
|-------|-------|
| 1-2 | **A**: Critical fixes, **I**: Type safety quick wins |
| 2-4 | **B**: Architecture cleanup, **D** (early): Pipeline fixes |
| 3-5 | **J**: Documentation sync, **C** (start): Plugin trait crate, **M** (start): Claude Flow integration (M1-M3) |
| 4-8 | **C**: Plugin system (incl. skill precedence, hot-reload, autonomous creation), **E**: Channels (incl. Google Chat, Teams), **H**: Memory & per-agent workspaces, **M** (complete): Dynamic MCP + bidirectional bridge (M4-M6) |
| 5-9 | **L**: Multi-agent routing & orchestration, **D** (complete): Pipeline reliability, **F**: Dev tools & apps (incl. MCP client) |
| 8-12 | **K**: Deployment (Docker), security plugin, ClawHub, benchmarks |

**Out of scope (separate tracks):** Voice (`voice_development.md`), UI/dashboard/native shells (`ui_development.md`). Forward-compat hooks are built into the plugin system (C1) and channel adapter traits so these can be added without breaking changes.

**MVP milestone (Week 8):** Plugin system working with skill precedence + hot-reload, email channel, multi-agent routing, 3 ported OpenClaw skills, MCP client for external servers, **Claude Flow integration functional (FlowDelegator + dynamic MCP + delegate feature enabled by default)**, all critical/high fixes resolved.

**Full vision (Week 12):** Browser automation, dev tool suite, ClawHub with vector search, per-agent sandboxing, security plugin, Docker images. All forward-compat hooks for voice and UI in place.

**Post-sprint tracks:** Voice (see `voice_development.md`), UI/dashboard/native shells (see `ui_development.md`), mobile, autonomous skill creation, planning strategies (ReAct/Plan-and-Execute).
