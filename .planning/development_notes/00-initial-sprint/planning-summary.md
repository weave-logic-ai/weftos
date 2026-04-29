# clawft -- Consolidated Planning Summary

> **HISTORICAL — 2026-02-17 snapshot (WEFT-25, archived 2026-04-28).**
> This document captured the original Python → Rust port plan for the
> nanobot-lineage clawft codebase. The project has since been rebranded
> to WeftOS (kernel + governance + ECC) and the 0.6.x → 0.7.0 release
> wave is the operative roadmap. Treat the goals, milestones, and
> success metrics below as historical.
>
> **Current source-of-truth:**
> - `.planning/reviews/0.7.0-release-gate/README.md` (release-gate audit).
> - `docs/plans/0.7.0-release-wave.md` (live release plan).
> - `docs/weftos/VISION.md` and `docs/weftos/architecture.md` for
>   present-tense product/architecture statements.

**Generated**: 2026-02-17
**Source documents**: 01-business-requirements.md, 02-technical-requirements.md, 03-development-guide.md, 04-rvf-integration.md, 05-ruvector-crates.md, phase3-status.md, exit-criteria-review.md

---

## 1. Business Requirements Summary

### What Is clawft

clawft is the Rust rewrite of nanobot, a ~10,000-line Python personal AI assistant framework. nanobot supports 9 chat channels, 14 LLM providers, a tool system, memory/session management, cron scheduling, and MCP integration. clawft produces a single static binary (`weft`) that replaces the Python + pip + Node.js stack (200+ MB footprint) with a < 15 MB native executable.

### Primary Goals

| ID | Goal | Success Metric |
|----|------|----------------|
| G1 | Single static binary for all platforms | `cargo build --release` produces one `weft` executable, no runtime deps |
| G2 | Run on constrained devices | Idle RSS < 10 MB on ARM64, < 5 MB WASM |
| G3 | Sub-second cold start | < 500ms to first message processing |
| G4 | Feature-gated compilation | Binary includes only enabled channels/providers |
| G5 | Config compatibility | Reads existing `~/.nanobot/config.json` without migration |
| G6 | Pluggable channel architecture | All channels implemented as plugins behind a common trait |
| G7 | RVF-powered intelligence | Model routing + vector memory via RVF |

### Secondary Goals

| ID | Goal | Success Metric |
|----|------|----------------|
| G8 | WASM core extraction | Core agent loop compiles to wasm32-wasip2, < 300 KB gzipped |
| G9 | Cross-compilation | CI builds for linux-x86_64, linux-aarch64, macos-arm64, windows-x86_64 |
| G10 | Embeddable library | `clawft-core` usable as a Rust library crate |
| G11 | Drop-in replacement | `weft` CLI has same commands and flags as Python `nanobot` |

### Non-Goals

- GUI or web dashboard
- Multi-tenancy or user management
- Dynamic plugin loading (`.so`/`.dll` at runtime) -- compile-time feature flags only
- WhatsApp channel in initial release
- Chinese platform channels (Feishu, DingTalk, Mochat, QQ)

### Target Users

1. **Self-Hoster** (Primary): Runs on VPS/home server, 1-3 channels, wants zero-dep deployment
2. **IoT/Edge Developer**: Raspberry Pi or similar, minimal binary, low RAM
3. **Developer/Contributor**: Extends with custom skills/tools/channel plugins

### Hard Constraints

1. Standalone within `repos/nanobot/` -- NOT in the barni Cargo workspace
2. Config compatibility with `~/.nanobot/config.json` (Python pydantic model)
3. Session compatibility with `~/.nanobot/sessions/*.jsonl`
4. Workspace compatibility with `~/.nanobot/workspace/` layout
5. No new external services (no databases, Redis, Docker required)
6. MIT license for all dependencies
7. Every channel MUST implement the `Channel` trait

---

## 2. Technical Architecture

### Workspace Structure (9 Crates)

```
repos/nanobot/clawft/
  Cargo.toml                  # Workspace root
  crates/
    clawft-types/             # Zero-dep core types (serde only)
    clawft-platform/          # Platform abstraction (HTTP, FS, Env, Process)
    clawft-core/              # Agent loop, context, memory, skills, bus, pipeline
    clawft-llm/               # Standalone LLM provider library (HTTP transport)
    clawft-tools/             # Built-in tools (feature-gated)
    clawft-channels/          # Channel plugin host + Telegram/Slack/Discord plugins
    clawft-services/          # Cron, heartbeat, MCP client
    clawft-cli/               # Native CLI binary (`weft`)
    clawft-wasm/              # WASM entrypoint (optional)
```

### Crate Dependency Graph

```
clawft-types          (zero deps beyond serde)
    |
clawft-platform       (depends on: types)
    |
clawft-llm            (standalone library: reqwest, serde, async-trait)
    |
clawft-core           (depends on: types, platform, clawft-llm)
    |
    +-- clawft-tools       (depends on: types, platform, core)
    +-- clawft-channels    (depends on: types, platform, core)
    +-- clawft-services    (depends on: types, platform, core)
    |
clawft-cli            (depends on: all above) -> binary: `weft`
clawft-wasm           (depends on: types, serde) -- decoupled from core/platform
```

### Platform Abstraction Traits

Four core traits enable WASM portability:

- **HttpClient**: `post_json()`, `get()`, `post_form()`
- **FileSystem**: `read_to_string()`, `write_string()`, `append_string()`, `exists()`, `list_dir()`, `create_dir_all()`, `remove_file()`, `glob()`
- **Environment**: `var()`, `home_dir()`, `current_dir()`, `now()`, `platform()`
- **ProcessSpawner**: `exec()` (native only)

Native implementations wrap reqwest, std::fs, std::env, tokio::process. WASM implementations are stubs (Phase 4 work).

### 6-Stage Pluggable Pipeline

The agent loop uses a pipeline architecture where each stage is pluggable:

| Stage | Trait | Level 0 (no ruvector) | Level 1+ (ruvector) |
|-------|-------|----------------------|---------------------|
| 1. Classify | `TaskClassifier` | `KeywordClassifier` (regex) | `ruvllm::TaskComplexityAnalyzer` (7-factor) |
| 2. Route | `ModelRouter` | `StaticRouter` (config.json) | `HnswRouter` + `FastGRNN` |
| 3. Context | `ContextAssembler` | `TokenBudgetAssembler` (truncate) | `AttentionAssembler` (Flash, MoE) |
| 4. Transport | `LlmTransport` | `OpenAiCompatTransport` (clawft-llm) | Same |
| 5. Score | `QualityScorer` | `NoopScorer` | `ruvllm::QualityScoringEngine` |
| 6. Learn | `LearningBackend` | `NoopLearner` | `sona::SonaEngine` (micro-LoRA) |

### Channel Plugin Architecture

All channels implement the `Channel` trait (`name()`, `start()`, `send()`, `is_running()`). The plugin host manages lifecycle, message routing, and error recovery. Channels are registered at compile time via feature flags.

| Plugin | Feature Flag | Transport |
|--------|-------------|-----------|
| Telegram | `channel-telegram` | HTTP long-polling + REST |
| Slack | `channel-slack` | Socket Mode WebSocket + REST |
| Discord | `channel-discord` | Gateway WebSocket + REST |

### Tool Trait

All tools implement `Tool` trait: `name()`, `description()`, `parameters()` (JSON Schema), `execute()`.

| Tool | Feature | WASM |
|------|---------|------|
| read_file, write_file, edit_file, list_dir | always | WASIp2 only |
| exec (shell) | `tool-exec` / `native-exec` | No |
| web_search, web_fetch | `tool-web` | Yes (HTTP) |
| message | always | Yes |
| spawn | `tool-spawn` | No |
| cron | `tool-cron` | Partial |
| MCP client | - | Partial |

### LLM Provider Support

**clawft-llm** is a standalone library providing:
- 4 native providers: Anthropic, OpenAI, Bedrock, Gemini
- Config-driven `OpenAiCompatProvider` for any provider (Groq, DeepSeek, Mistral, OpenRouter, Together, Fireworks, Perplexity, xAI, Ollama, Azure)
- Full SSE streaming, tool calling, 4 failover strategies
- Lock-free CircuitBreaker (WASM-safe)
- Cost tracking with real pricing data

### Feature Flag Strategy

```
Default:   channel-telegram + all-tools        (~5 MB binary)
ruvector:  + agentdb + routing + sona + attention  (~8-12 MB binary)
minimal:   channel-telegram only               (~3-4 MB binary)
WASM:      micro-hnsw + temporal-tensor + sona  (< 300 KB)
```

### Binary Size Targets

| Configuration | Target (stripped) |
|--------------|-------------------|
| Minimal (Telegram only) | ~3-4 MB |
| Default (all channels + ruvector) | ~8-12 MB |
| WASM core (uncompressed) | < 300 KB |
| WASM core (gzipped) | < 120 KB |

---

## 3. RVF Integration Scope

### What RVF Provides

RVF (RuVector Format) is a universal binary substrate merging vector database, model routing, progressive indexing, and WASM runtime. A single `.rvf` file stores agent memory embeddings, session state, routing policies, and can run queries via a < 8 KB WASM microkernel.

### Five Intelligence Levels

**Level 0 (no ruvector)**: Static registry routing, same as Python nanobot.
- Hardcoded model-to-provider mapping
- Substring memory search
- Token-count context truncation

**Level 1 (ruvllm)**: Complexity-aware routing.
- 7-factor task complexity analysis
- Routes simple tasks to cheaper models, complex tasks to capable ones
- HNSW pattern matching (150x faster than linear scan)

**Level 2 (+ tiny-dancer)**: Neural routing with resilience.
- Sub-ms neural inference for provider selection via FastGRNN
- CircuitBreaker detects provider failures, auto-routes around them
- Uncertainty estimation prevents low-confidence routing

**Level 3 (+ sona)**: Self-learning routing.
- Micro-LoRA (rank-2) instant per-request adaptation
- Base LoRA (rank-8) hourly background consolidation
- EWC++ prevents catastrophic forgetting
- ReasoningBank stores task-outcome trajectories

**Level 4 (+ attention + temporal-tensor)**: Intelligent context management.
- 40+ attention mechanisms (Flash, MoE, InformationBottleneck)
- Auto-tiered quantization: hot=fp16, warm=PQ (16x), cold=binary (32x)
- Memory footprint: ~2 MB for 10K entries with typical distribution

**Level 5 (+ graph + domain-expansion)**: Knowledge fabric.
- Property graph of skills, tools, relationships
- Multi-hop reasoning across knowledge base
- Cross-domain transfer learning

### RVF Segment Types Used

| Segment | Code | Use |
|---------|------|-----|
| VEC | 0x01 | Memory embeddings, session embeddings |
| INDEX | 0x02 | HNSW adjacency for fast search |
| META | 0x07 | Key-value metadata |
| HOT | 0x08 | Frequently-accessed entries |
| SKETCH | 0x09 | Access frequency tracking |
| WITNESS | 0x0A | Audit trail of agent actions |
| POLICY_KERNEL | 0x31 | Model routing policy parameters |
| COST_CURVE | 0x32 | Provider cost/latency/quality curves |

### Integration Points in clawft

1. **clawft-core/memory**: `MemoryStore` uses `RvfVectorStore` for semantic search over MEMORY.md
2. **clawft-core/session**: `SessionManager` indexes session summaries for semantic retrieval
3. **clawft-core/routing**: `IntelligentRouter` uses POLICY_KERNEL + COST_CURVE for learned routing
4. **clawft-core/agent**: Witness log tracks agent actions via WITNESS segments

### rvf-wasm Microkernel

< 8 KB WASM binary with 14 C-ABI exports: `rvf_init`, `rvf_load_query`, `rvf_load_block`, `rvf_distances`, `rvf_topk_merge`, `rvf_topk_read`, etc. Provides vector search in WASM without the full rvf-runtime.

### MCP Server Bridge (Future)

RVF includes `rvf-mcp-server` with 9 tools: create_store, open_store, ingest, query, status, delete, compact, delete_filter, list_stores. Could expose vector store operations as MCP tools to LLMs.

### Development Timeline (from 04-rvf-integration.md, lines 340-351)

| Week | Task | Stream |
|------|------|--------|
| 7 | Add rvf-runtime + rvf-types as workspace deps (feature-gated) | 2B |
| 8 | MemoryStore: integrate RvfVectorStore for semantic search | 2B |
| 8 | IntelligentRouter: basic pattern-matched routing | 2B |
| 9 | SessionManager: index session turns in rvf | 2B |
| 10 | IntelligentRouter: learned routing policies | 2B |
| 11 | WitnessLog: audit trail via rvf WITNESS segments | 2B |
| 11 | First-startup memory indexing from existing MEMORY.md | 2B |
| 13 | WASM: integrate rvf-wasm microkernel for vector ops | 3A |

### Embedding Strategy

- **MVP**: LLM-generated embeddings via provider's embedding endpoint (e.g., text-embedding-3-small). ~100ms per embedding, no binary size impact.
- **Fallback**: `HashEmbedding` for development/testing (not semantic).
- **Future**: Local ONNX model (all-MiniLM-L6-v2), adds ~20 MB, feature-gated.

---

## 4. Ruvector Crates

### Tier 1: Directly Applicable (Phase 1-2)

| Crate | Purpose | Binary Impact | WASM |
|-------|---------|--------------|------|
| `ruvllm` (minimal) | Task complexity, model tier, session mgmt | ~2 MB | Via ruvllm-wasm |
| `sona` | Self-learning: two-tier LoRA, EWC++, ReasoningBank | ~100 KB | Excellent |
| `rvf` + `rvf-types` | Binary segment format, WASM microkernel | ~50 KB | no_std |
| `ruvector-core` | AgenticDB: PolicyMemory, SessionState, WitnessLog | ~200-500 KB | memory-only feature |
| `ruvector-attention` | 40+ attention mechanisms (Flash, MoE, etc.) | ~80 KB | Excellent |
| `micro-hnsw-wasm` | Zero-dep 11.8 KB WASM HNSW search | 11.8 KB | Perfect |
| `ruvector-tiny-dancer-core` | Neural routing: FastGRNN, CircuitBreaker | ~500 KB-1 MB | Native only |
| `ruvector-filter` | Metadata filtering for vector queries | ~30 KB | Native only |
| `ruvector-metrics` | Performance metrics collection | ~20 KB | Good |

### Tier 2: Valuable Extensions (Phase 2-3)

| Crate | Purpose | Binary Impact | WASM |
|-------|---------|--------------|------|
| `ruvector-graph` | Property graph + Cypher + RAG + multi-hop reasoning | ~500 KB | graph-wasm variant |
| `ruvector-temporal-tensor` | Tiered quantization (8/7/5/3 bit), zero deps | < 10 KB | Perfect |
| `cognitum-gate-kernel` | no_std coherence gate tile | < 10 KB | Perfect |
| `ruvector-domain-expansion` | Cross-domain transfer learning | < 50 KB | Excellent |
| `ruvector-nervous-system-wasm` | Bio-inspired BTSP, HDC, WTA | < 100 KB | Excellent |
| `rvlite` | WASM vector DB with SQL/Cypher/SPARQL | ~200-500 KB | Excellent (browser) |
| `ruvllm-wasm` | Standalone WASM routing + chat templates | ~50-200 KB | Excellent |

### Tier 3: Future / Distributed (Post-Phase 3)

`ruvector-raft`, `ruvector-cluster`, `ruvector-replication`, `mcp-gate`, `prime-radiant`, `ruvector-gnn`

### WASM Size Budget

| Component | Estimated Size |
|-----------|---------------|
| clawft-core (agent loop, context, tools) | ~100 KB |
| micro-hnsw-wasm | 11.8 KB |
| ruvector-temporal-tensor (ffi) | < 10 KB |
| cognitum-gate-kernel | < 10 KB |
| sona (wasm subset) | ~30 KB |
| rvf-types | ~30 KB |
| reqwest (wasm) or manual HTTP | ~50 KB |
| **Total** | **~242 KB** (under 300 KB budget) |

### Key Design Principle

ruvector provides **routing intelligence** (the navigator). clawft-llm provides **HTTP transport** (the ship). They are separate concerns: ruvector decides *which* model to call; clawft-llm makes the actual HTTP request.

---

## 5. Phase 3 Current State

**Phase**: 3 (Finish) -- CONDITIONAL PASS
**Last Updated**: 2026-02-17
**Rust Toolchain**: Upgrading from 1.85 to 1.93.1

### Stream Status

| Stream | Description | Status | Progress |
|--------|-------------|--------|----------|
| 2I | Security fixes (SEC-1, SEC-2, SEC-3) | COMPLETE | 100% |
| 3A | WASM core (clawft-wasm crate) | CONDITIONAL PASS | ~75% |
| 3B | CI/CD + Polish | COMPLETE | ~95% |
| 3C | Rust toolchain upgrade (1.85 -> 1.93.1) | IN PROGRESS | ~90% |

### Codebase Statistics

| Metric | Value |
|--------|-------|
| Rust source files | ~121 |
| Crates in workspace | 9 |
| CI workflow files | 4 (all valid YAML) |
| Build/bench/release scripts | 13 (all pass syntax check) |
| Unit + integration tests | 1,058 (0 failures, 8 ignored) |
| Clippy warnings | 0 |

### Test Count Progression

| Milestone | Tests | Delta |
|-----------|-------|-------|
| Phase 2 complete | 892 | - |
| Phase 3 Round 1 | 960 | +68 |
| Phase 3 Round 2 | 1,029 | +69 |
| Phase 3 Round 3 | 1,048 | +19 |
| Phase 3 Round 4 | 1,058 | +10 |

### Performance Benchmarks (Measured)

| Metric | Python (nanobot) | Rust (clawft) | Improvement |
|--------|-----------------|---------------|-------------|
| Startup time | ~2-5 sec | 3.5 ms | ~229x faster |
| Binary size | 200+ MB installed | 4.6 MB | ~43x smaller |
| Memory (RSS) | 50-80 MB | < 10 MB (est.) | ~5-8x less |

### What Is Done

- All 9 crates compile (`cargo check --workspace` passes)
- 1,058 tests pass, 0 clippy warnings
- Security hardening: CommandPolicy (allowlist), UrlPolicy (SSRF protection), wired into all relevant tools
- WASM crate builds for wasip1 (41 tests, dlmalloc allocator, decoupled from core/platform)
- CI/CD: 4 GitHub Actions workflows (ci, release, wasm-build, benchmarks)
- Dockerfile (FROM scratch), cross-compile scripts, benchmark scripts, release packaging
- CHANGELOG, deployment docs (3), security docs, architecture docs, CLI reference
- CLI integration tests (29), security integration tests (33)
- Benchmark regression detection scripts

### What Is Remaining (Phase 3)

- `cargo clippy --workspace -- -D warnings` on 1.93.1 (verification running)
- `cargo test --workspace` on 1.93.1 (verification running)
- `cargo check -p clawft-wasm --target wasm32-wasip2` (now available after Rust upgrade)
- `docs/benchmarks/results.md` (data exists, standalone file missing)

---

## 6. Outstanding Requirements -- Not Yet Implemented

### From Business Requirements (01)

| Requirement | Status | Priority | Notes |
|-------------|--------|----------|-------|
| `weft gateway` processes Telegram messages | NOT VERIFIED end-to-end | P0 | Plugin code exists; needs real API test |
| `weft agent -m "hello"` CLI works | NOT VERIFIED | P0 | Agent command exists; needs LLM integration test |
| Web search and fetch tools work | IMPLEMENTED | P1 | Code exists with security policies |
| MCP integration | IMPLEMENTED (stub) | P2 | MCP client code exists in clawft-services |
| Binary size < 10 MB (stripped) | NOT MEASURED | P0 | 4.6 MB measured pre-strip; likely passes |
| RSS idle < 15 MB | NOT MEASURED in production | P0 | Estimated to pass |
| RVF-powered model routing | NOT IMPLEMENTED | P1 | No ruvector crate deps yet |
| RVF vector memory search | NOT IMPLEMENTED | P1 | HashEmbedder exists as placeholder |
| WASM core < 300 KB gzipped | PARTIAL | P2 | rlib is 142 KB; cdylib not configured |
| Runs in WAMR and Wasmtime | NOT TESTED | P2 | Deferred to Phase 4 |

### From Technical Requirements (02)

| Requirement | Status | Notes |
|-------------|--------|-------|
| `clawft-llm` as standalone library | EXISTS in workspace | Currently a workspace member, not external repo |
| Anthropic, Bedrock, Gemini native providers | OPENAI-COMPAT ONLY | Only `openai_compat.rs` provider exists; Anthropic/Bedrock/Gemini have no dedicated modules |
| SSE streaming | IMPLEMENTED | In clawft-llm |
| JSON repair | NOT FOUND | Mentioned in plan, not seen in codebase |
| Email channel | NOT STARTED | P2, future phase |
| litellm-rs sidecar (optional) | NOT STARTED | Assessed as optional (~1.5 days LOE) |
| Codex OAuth provider | NOT STARTED | Planned for Phase 2 Stream 2C |

### From RVF Integration (04)

All RVF/ruvector integration is outstanding:

| Component | Status |
|-----------|--------|
| rvf-runtime as workspace dep | NOT ADDED |
| RvfVectorStore for MemoryStore | NOT INTEGRATED |
| IntelligentRouter with POLICY_KERNEL | NOT INTEGRATED |
| Session index in rvf | NOT INTEGRATED |
| Learned routing via COST_CURVE | NOT INTEGRATED |
| Witness log via WITNESS segments | NOT INTEGRATED |
| First-startup memory indexing | NOT INTEGRATED |
| rvf-wasm microkernel in clawft-wasm | NOT INTEGRATED |
| MCP server bridge for rvf | NOT STARTED |

### From Ruvector Crates (05)

No ruvector crate has been integrated. The following are planned:

**Phase 2 (Tier 1)**:
- ruvector-core (AgenticDB), rvf + rvf-types, sona, ruvllm, ruvector-attention, ruvector-tiny-dancer-core, ruvector-temporal-tensor, rvf-crypto

**Phase 3 (WASM)**:
- micro-hnsw-wasm, ruvector-temporal-tensor (ffi), cognitum-gate-kernel, sona (wasm subset)

### From Phase 3 Deferrals

| Item | Priority | Notes |
|------|----------|-------|
| Real WasiHttpClient (WASI preview2) | HIGH | Stubs exist; blocked on WASI maturity |
| Real WasiFileSystem (WASI preview2) | HIGH | Stubs exist; blocked on WASI maturity |
| cdylib crate type for standalone WASM module | MEDIUM | Currently rlib only |
| micro-hnsw-wasm vector search in WASM | MEDIUM | Crate structure supports it |
| wasmtime/WAMR runtime validation | MEDIUM | After real impls exist |
| wasm32-wasip2 upgrade | MEDIUM | Now available with Rust 1.93.1 |
| Multi-arch Docker images (buildx) | LOW | |
| macOS code signing | LOW | |
| Message throughput benchmark clarity | LOW | 418 inv/s vs 1000 msg/s target |

---

## 7. Key Architecture Decisions

### AD-1: Standalone Workspace

clawft lives in `repos/nanobot/clawft/`, separate from the barni Cargo workspace at `barni/src/`. This avoids coupling and allows independent versioning.

### AD-2: clawft-llm as Standalone Library

The LLM provider layer was extracted from barni-providers into a standalone library. It provides only HTTP transport; intelligence wrapping lives in clawft-core. Total internalization from barni: ~415 lines of CircuitBreaker + UUID newtypes converted to generic `Option<String>` metadata.

### AD-3: Pipeline Traits in clawft-core, Not clawft-llm

The 6-stage pipeline architecture lives in clawft-core. clawft-llm provides only `LlmTransport` implementations. This separation keeps the standalone library focused on HTTP while clawft-core owns intelligence orchestration.

### AD-4: Feature-Gated Ruvector

All ruvector intelligence is opt-in via feature flags. Without `ruvector`, clawft falls back to static routing (same as Python nanobot). This keeps the default binary small (~5 MB).

### AD-5: Plugin Architecture for Channels

Every channel is a plugin implementing the `Channel` trait. No channel logic in core. Compile-time registration via feature flags, not runtime dynamic loading.

### AD-6: Config Compatibility

clawft reads `~/.clawft/config.json` with automatic fallback to `~/.nanobot/config.json`. Workspace directory follows the same pattern. serde annotations use `#[serde(default)]` to match Python's defaults.

### AD-7: Dual-Target (Native + WASM)

Platform abstraction traits enable the same agent loop to run native (via reqwest/tokio) or WASM (via WASI). The WASM build uses standalone reimplementation pattern (purpose-built minimal core, not feature-flagged subset).

### AD-8: WASM Allocator

Changed from `talc` to `dlmalloc` for WASM target. The `dlmalloc` crate is well-tested for wasm32 targets.

### AD-9: WASM Target

Primary: `wasm32-wasip2` (requires Rust 1.87+, now available with 1.93.1 upgrade). Fallback: `wasm32-wasip1` (current working target). Runtime targets: WAMR (IoT), Wasmtime (edge/cloud).

### AD-10: litellm-rs Rejected as Direct Dependency

Assessed as too heavy (pulls actix-web, tokio-full), not WASM-compatible, and too young (7 months, 17 stars). Same API patterns adopted as traits instead. Could be plugged in as optional `SidecarTransport` backend in the future.

### AD-11: Security Hardening (Phase 3)

- CommandPolicy: Allowlist mode for shell command execution
- UrlPolicy: SSRF protection for web_fetch and web_search tools
- Both wired into tool implementations and CLI entry points

### AD-12: Rust Toolchain

MSRV: Rust 1.85+ (edition 2024). Currently upgrading to 1.93.1, which unblocks wasm32-wasip2 support.

---

## 8. Existing Codebase Module Inventory

### clawft-types (6 modules)
- `config.rs` -- Config, AgentsConfig, ChannelsConfig, ProvidersConfig, GatewayConfig, ToolsConfig, CommandPolicyConfig, UrlPolicyConfig, PipelineConfig
- `provider.rs` -- ProviderSpec, ProviderEntry, static PROVIDERS registry, LlmResponse, ToolCallRequest
- `event.rs` -- InboundMessage, OutboundMessage
- `session.rs` -- Session, SessionTurn
- `cron.rs` -- CronJob, CronSchedule
- `error.rs` -- ClawftError, ConfigError, ProviderError, ToolError, ChannelError

### clawft-platform (6 modules)
- `lib.rs` -- Platform trait bundle, trait re-exports
- `http.rs` -- HttpClient trait + NativeHttpClient (reqwest)
- `fs.rs` -- FileSystem trait + NativeFileSystem (std::fs)
- `env.rs` -- Environment trait + NativeEnvironment (std::env, dirs, chrono)
- `process.rs` -- ProcessSpawner trait + NativeProcessSpawner (tokio::process)
- `config_loader.rs` -- Config file discovery and loading logic

### clawft-core (19 modules)
- `lib.rs` -- Module declarations and re-exports
- `bus.rs` -- MessageBus (tokio::sync::mpsc channels)
- `bootstrap.rs` -- Application bootstrap and initialization
- `session.rs` -- SessionManager (JSONL read/write)
- `session_indexer.rs` -- Session semantic indexing (placeholder for rvf)
- `vector_store.rs` -- VectorStore abstraction (placeholder for rvf)
- `intelligent_router.rs` -- IntelligentRouter (placeholder for ruvector)
- `security.rs` -- Security policy enforcement layer
- `agent/mod.rs` -- Agent module declarations
- `agent/loop_core.rs` -- AgentLoop: LLM call -> tool exec -> repeat
- `agent/context.rs` -- ContextBuilder (system prompt assembly)
- `agent/memory.rs` -- MemoryStore (MEMORY.md + HISTORY.md)
- `agent/skills.rs` -- SkillsLoader (progressive skill loading)
- `embeddings/mod.rs` -- Embedding trait and module declarations
- `embeddings/hash_embedder.rs` -- HashEmbedding fallback (not semantic)
- `pipeline/mod.rs` -- PipelineRegistry
- `pipeline/traits.rs` -- TaskClassifier, ModelRouter, ContextAssembler, LlmTransport, QualityScorer, LearningBackend
- `pipeline/classifier.rs` -- KeywordClassifier
- `pipeline/router.rs` -- StaticRouter
- `pipeline/assembler.rs` -- TokenBudgetAssembler
- `pipeline/transport.rs` -- OpenAiCompatTransport (wraps clawft-llm)
- `pipeline/scorer.rs` -- NoopScorer
- `pipeline/learner.rs` -- NoopLearner
- `pipeline/llm_adapter.rs` -- Adapter between pipeline and clawft-llm
- `tools/mod.rs` -- Tool trait definition
- `tools/registry.rs` -- ToolRegistry (dynamic dispatch)

### clawft-llm (7 modules)
- `lib.rs` -- Module declarations and re-exports
- `provider.rs` -- Provider trait + ProviderRegistry
- `openai_compat.rs` -- OpenAI-compatible HTTP provider (generic for any base_url)
- `router.rs` -- Prefix-based model routing + model aliasing
- `config.rs` -- Provider configuration types
- `types.rs` -- CompletionRequest, CompletionResponse, StreamChunk, ToolCall
- `error.rs` -- LlmError types

### clawft-tools (10 modules)
- `lib.rs` -- Tool registration and feature-gated re-exports
- `file_tools.rs` -- read_file, write_file, edit_file, list_dir
- `shell_tool.rs` -- exec (shell command execution, `native-exec` feature)
- `web_search.rs` -- web_search (Brave Search API)
- `web_fetch.rs` -- web_fetch (HTTP fetch with readability)
- `message_tool.rs` -- message (inter-agent messaging via bus)
- `spawn_tool.rs` -- spawn (subagent creation)
- `memory_tool.rs` -- memory read/write tool
- `security_policy.rs` -- CommandPolicy, security enforcement
- `url_safety.rs` -- UrlPolicy, SSRF protection

### clawft-channels (14 modules across 4 groups)
- `lib.rs` -- Feature-gated channel re-exports, `available_channels()`
- `traits.rs` -- Channel, ChannelHost, ChannelFactory traits
- `host.rs` -- Plugin host (lifecycle, registry, message routing)
- **telegram/** -- `mod.rs`, `channel.rs`, `client.rs`, `types.rs`, `tests.rs`
- **slack/** -- `mod.rs`, `channel.rs`, `api.rs`, `events.rs`, `signature.rs`, `factory.rs`, `tests.rs`
- **discord/** -- `mod.rs`, `channel.rs`, `api.rs`, `events.rs`, `factory.rs`, `tests.rs`

### clawft-services (8 modules across 3 services)
- `lib.rs` -- Module declarations
- `error.rs` -- Service error types
- **cron_service/** -- `mod.rs`, `scheduler.rs`, `storage.rs`
- **heartbeat/** -- `mod.rs`
- **mcp/** -- `mod.rs`, `transport.rs`, `types.rs`

### clawft-cli (17 modules)
- `main.rs` -- CLI entry point, clap command definitions
- `completions.rs` -- Shell completion generation
- `mcp_tools.rs` -- MCP tool definitions for CLI
- **commands/** -- `mod.rs`, `agent.rs`, `gateway.rs`, `status.rs`, `channels.rs`, `cron.rs`, `config_cmd.rs`, `memory_cmd.rs`, `sessions.rs`
- **markdown/** -- `mod.rs`, `dispatch.rs`, `telegram.rs`, `slack.rs`, `discord.rs`

### clawft-wasm (6 modules)
- `lib.rs` -- WASM entrypoint with init/process/capabilities exports
- `allocator.rs` -- dlmalloc allocator for wasm32
- `http.rs` -- WasiHttpClient stub
- `fs.rs` -- WasiFileSystem stub
- `env.rs` -- WasiEnvironment (in-memory HashMap, functional)
- `platform.rs` -- WasmPlatform bundle struct

### Total: ~87 Rust source modules across 9 crates

---

## 9. Development Phases and Timeline

### Phase 1: Warp (Foundation) -- COMPLETE
- Workspace setup, types, platform traits, channel plugin API
- Core engine: MessageBus, SessionManager, MemoryStore, AgentLoop
- clawft-llm extraction, tools, Telegram plugin, CLI

### Phase 2: Weft (Channels + Services) -- COMPLETE
- Slack and Discord channel plugins
- Cron, heartbeat, MCP services
- Security hardening (Phase 2I)
- Test count reached 892

### Phase 3: Finish (WASM + CI/CD + Polish) -- IN PROGRESS (~85%)
- WASM crate infrastructure (wasip1, stubs, feature flags, tests)
- CI/CD: 4 workflows, Dockerfile, cross-compile, benchmarks, release
- Rust toolchain upgrade to 1.93.1
- Remaining: verification on new toolchain, wasip2 check, benchmark results doc

### Phase 4 (Planned): WASM Completion + RVF Integration
- Real WASI HTTP/FS implementations
- micro-hnsw-wasm integration
- Runtime validation (wasmtime, WAMR)
- Begin ruvector crate integration (Tier 1)
- wasip2 upgrade

### Phase 5+ (Future): Full Intelligence
- Complete ruvector Tier 1 integration
- Tier 2 extensions (graph, temporal tensor, domain expansion)
- Email channel, Codex OAuth, litellm-rs sidecar
- Multi-instance deployment (raft, cluster)
