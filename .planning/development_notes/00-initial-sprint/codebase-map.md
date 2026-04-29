# clawft Codebase Map

> **HISTORICAL — 2026-02-17 snapshot (WEFT-25, archived 2026-04-28).**
> This document is a point-in-time snapshot of the original Python →
> Rust port effort, taken when the project was still pinned to the
> nanobot lineage. Counts, crate names, and architectural assumptions
> below are out of date relative to the WeftOS rebrand and the 0.7.0
> release-gate state. Do not treat as living documentation.
>
> **Current source-of-truth:**
> - `.planning/reviews/0.7.0-release-gate/README.md` and the
>   per-workstream audits under that directory.
> - `docs/clawft/architecture.mdx` (live architecture).
> - `docs/weftos/architecture.md` (kernel/governance overview).

**Generated**: 2026-02-17
**Workspace root**: `/home/aepod/dev/clawft/`
**Rust edition**: 2024, MSRV 1.93
**Binary name**: `weft`
**Total lines of Rust**: ~33,222 across all crates
**Total docs**: ~7,072 lines across 16 files

---

## Workspace Structure

```
clawft/
  Cargo.toml              # Workspace manifest
  Cargo.lock
  rust-toolchain.toml     # channel = "1.93", clippy + rustfmt
  Dockerfile              # FROM scratch; static musl binary; ENTRYPOINT weft gateway
  README.md
  CHANGELOG.md
  docs/                   # 16 docs, 7072 lines
  scripts/                # bench/, build/, release/ scripts
  crates/
    clawft-types/
    clawft-platform/
    clawft-core/
    clawft-llm/
    clawft-tools/
    clawft-channels/
    clawft-services/
    clawft-cli/
    clawft-wasm/
```

---

## Crate Details

### 1. clawft-types

**Purpose**: Core shared types -- no heavy deps, no async runtime.

| File | Lines |
|------|-------|
| `src/config.rs` | 1350 |
| `src/provider.rs` | 571 |
| `src/cron.rs` | 339 |
| `src/error.rs` | 194 |
| `src/session.rs` | 198 |
| `src/event.rs` | 161 |
| `src/lib.rs` | 22 |

**Features**: none

**Key deps**: `serde`, `serde_json`, `chrono`, `thiserror`, `uuid`, `dirs`

**Public API**: Config, AgentsConfig, ChannelsConfig, ProvidersConfig, GatewayConfig, ToolsConfig, CommandPolicyConfig, UrlPolicyConfig, MCPServerConfig, ProviderSpec, LlmResponse, InboundMessage, OutboundMessage, Session, CronSchedule, CronJob, CronStore, ClawftError, ContentBlock, StopReason

**Provider registry** (14 entries): custom, openrouter, aihubmix, anthropic, openai, openai_codex, deepseek, gemini, zhipu, dashscope, moonshot, minimax, vllm, groq

---

### 2. clawft-platform

**Purpose**: Platform abstraction traits with native implementations.

| File | Lines |
|------|-------|
| `src/config_loader.rs` | 306 |
| `src/fs.rs` | 234 |
| `src/http.rs` | 215 |
| `src/process.rs` | 179 |
| `src/env.rs` | 107 |
| `src/lib.rs` | 143 |

**Traits**: `Platform`, `FileSystem`, `HttpClient`, `Environment`, `ProcessSpawner`
**Structs**: `NativePlatform`, `NativeFileSystem`, `NativeHttpClient`, `NativeEnvironment`, `NativeProcessSpawner`, `HttpResponse`, `ProcessOutput`
**Fns**: `discover_config_path()`, `load_config_raw()`, `normalize_keys()`, `camel_to_snake()`

---

### 3. clawft-core

**Purpose**: Agent loop, message bus, session management, 6-stage pipeline, tool registry, context building, memory, embeddings.

| File | Lines |
|------|-------|
| `src/pipeline/llm_adapter.rs` | 845 |
| `src/agent/loop_core.rs` | 812 |
| `src/pipeline/traits.rs` | 775 |
| `src/session.rs` | 730 |
| `src/pipeline/transport.rs` | 652 |
| `src/agent/context.rs` | 616 |
| `src/bootstrap.rs` | 584 |
| `src/intelligent_router.rs` | 550 (feature-gated) |
| `src/tools/registry.rs` | 536 |
| `src/agent/skills.rs` | 518 |
| `src/pipeline/classifier.rs` | 419 |
| `src/agent/memory.rs` | 454 |
| `src/vector_store.rs` | 436 (feature-gated) |
| `src/session_indexer.rs` | 434 (feature-gated) |
| `src/pipeline/assembler.rs` | 323 |
| `src/bus.rs` | 332 |
| `src/pipeline/router.rs` | 232 |
| `src/security.rs` | 289 |
| `src/embeddings/hash_embedder.rs` | 241 (feature-gated) |
| `src/embeddings/mod.rs` | 81 (feature-gated) |
| `src/pipeline/scorer.rs` | 145 |
| `src/pipeline/learner.rs` | 138 |

**Features**: `full` (default), `vector-memory` (enables embeddings, vector_store, intelligent_router, session_indexer)

**Pipeline stages**: KeywordClassifier -> StaticRouter -> TokenBudgetAssembler -> OpenAiCompatTransport -> NoopScorer -> NoopLearner

**External tests**: `phase1_integration.rs`, `phase2_integration.rs`, `security_tests.rs`

---

### 4. clawft-llm

**Purpose**: LLM provider abstraction -- OpenAI-compatible HTTP provider and router.

| File | Lines |
|------|-------|
| `src/openai_compat.rs` | 342 |
| `src/router.rs` | 330 |
| `src/types.rs` | 322 |
| `src/config.rs` | 214 |
| `src/error.rs` | 121 |
| `src/provider.rs` | 44 |
| `src/lib.rs` | 43 |

**Traits**: `Provider`
**Structs**: `OpenAiCompatProvider`, `ProviderRouter`, `ProviderConfig`, `ChatMessage`, `ChatRequest`, `ChatResponse`

---

### 5. clawft-tools

**Purpose**: Tool implementations -- file I/O, shell, memory, web, spawning. Security-gated.

| File | Lines |
|------|-------|
| `src/file_tools.rs` | 782 |
| `src/url_safety.rs` | 496 |
| `src/shell_tool.rs` | 415 |
| `src/security_policy.rs` | 405 |
| `src/memory_tool.rs` | 435 |
| `src/spawn_tool.rs` | 293 |
| `src/message_tool.rs` | 190 |
| `src/web_search.rs` | 216 |
| `src/web_fetch.rs` | 179 |
| `src/lib.rs` | 106 |

**Features**: `native-exec` (default) -- gates shell_tool, spawn_tool
**Registered tools**: read_file, write_file, edit_file, list_directory, memory_read, memory_write, web_search, web_fetch, exec_shell, spawn

---

### 6. clawft-channels

**Purpose**: Channel plugin system -- Telegram, Slack, Discord.

| Channel | Key Files | Transport |
|---------|-----------|-----------|
| Telegram | channel.rs, client.rs, types.rs | HTTP long-polling + REST |
| Slack | channel.rs, api.rs, events.rs, signature.rs | Socket Mode WebSocket + REST |
| Discord | channel.rs, api.rs, events.rs | Gateway WebSocket + REST |

**Traits**: `Channel`, `ChannelHost`, `ChannelFactory`
**Key struct**: `PluginHost` manages `Vec<Arc<dyn Channel>>`

---

### 7. clawft-services

**Purpose**: Background services -- cron, heartbeat, MCP client.

| Service | Key Files |
|---------|-----------|
| Cron | `cron_service/mod.rs`, `scheduler.rs`, `storage.rs` |
| Heartbeat | `heartbeat/mod.rs` |
| MCP | `mcp/mod.rs`, `transport.rs`, `types.rs` |

**MCP**: `McpClient`, `StdioTransport`, `HttpTransport`, `MockTransport`, JSON-RPC protocol

---

### 8. clawft-cli

**Purpose**: The `weft` binary. CLI commands wrapping the full stack.

**Subcommands**: agent, gateway, status, channels, cron, sessions, memory, config, completions

**Markdown dispatch**: telegram.rs, slack.rs, discord.rs per-channel rendering

**Features**: `channels` (default), `services` (default), `vector-memory`

**External tests**: `cli_integration.rs` (542 lines)

---

### 9. clawft-wasm

**Purpose**: WASM entrypoint (wasm32-wasip1). Minimal deps, stubs for HTTP/FS.

| File | Lines |
|------|-------|
| `src/fs.rs` | 165 |
| `src/env.rs` | 155 |
| `src/platform.rs` | 146 |
| `src/http.rs` | 134 |
| `src/lib.rs` | 119 |
| `src/allocator.rs` | 8 |

**Deps**: clawft-types, serde, serde_json, dlmalloc (wasm32 only)
**Status**: Pipeline not wired. Stubs only. Phase 3D will add real implementations.

---

## Crate Dependency Graph

```
clawft-wasm     --> clawft-types
clawft-platform --> clawft-types
clawft-llm      --> (no clawft-* deps)
clawft-core     --> clawft-types, clawft-platform, clawft-llm
clawft-tools    --> clawft-types, clawft-platform, clawft-core
clawft-channels --> clawft-types, clawft-platform
clawft-services --> clawft-types
clawft-cli      --> all above (channels + services optional)
```

---

## Feature Flags Summary

| Crate | Feature | Effect |
|-------|---------|--------|
| clawft-core | `full` (default) | no-op |
| clawft-core | `vector-memory` | embeddings, vector_store, intelligent_router, session_indexer |
| clawft-tools | `native-exec` (default) | shell_tool, spawn_tool |
| clawft-cli | `channels` (default) | pulls clawft-channels |
| clawft-cli | `services` (default) | pulls clawft-services |
| clawft-cli | `vector-memory` | propagates to clawft-core |

---

## Test Inventory

| Crate | Inline tests | External test files |
|-------|-------------|---------------------|
| clawft-types | ~65 | none |
| clawft-platform | ~76 | none |
| clawft-core | ~239 | 3 files (666L) |
| clawft-llm | ~58 | none |
| clawft-tools | ~136 | 1 file (655L) |
| clawft-channels | ~118 | 3 files (1091L) |
| clawft-services | ~28 | none |
| clawft-cli | ~223 | 1 file (542L) |
| clawft-wasm | ~41 | none |
| **Total** | **~984** | **8 files, ~2754L** |

---

## Scripts

```
scripts/
  bench/    run-all.sh, startup-time.sh, throughput.sh, memory-usage.sh,
            wasm-size.sh, regression-check.sh, save-results.sh, baseline.json
  build/    cross-compile.sh, docker-build.sh, size-check.sh
  release/  generate-changelog.sh, package-all.sh, package.sh
```

---

## Key Architectural Notes

1. **6-stage pipeline**: All stages trait-backed and swappable via PipelineRegistry
2. **Tool registry**: `Arc<dyn Tool>` with dynamic dispatch, 10-11 tools
3. **Skills**: Filesystem-based (`skill.json` + `prompt.md`), lazy loaded by SkillsLoader
4. **WASM**: Minimal deps, pipeline not wired, stubs for HTTP/FS
5. **Security**: SSRF (UrlPolicy), command policy (CommandPolicy), session ID validation, workspace containment
6. **Config**: snake_case/camelCase tolerant via serde aliases, ~/.clawft/ with ~/.nanobot/ fallback
7. **Channels**: PluginHost manages Arc<dyn Channel>, compile-time feature flags
8. **vector-memory**: Full semantic layer behind feature flag (HashEmbedder, VectorStore, IntelligentRouter, SessionIndexer)
