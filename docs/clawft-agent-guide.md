# clawft Agent Framework Guide

**Version**: 0.2.0
**Date**: 2026-03-31
**Audience**: Developers building, configuring, and running agents on clawft/WeftOS

---

## 1. What is clawft?

clawft is a Rust agent framework -- the user-space agent runtime that runs on the WeftOS kernel. It implements a complete agent lifecycle: message consumption, context assembly, LLM pipeline routing, tool execution, skill management, memory persistence, and multi-channel communication.

The framework compiles to native binaries, WASM for browsers, and WASI for server-side sandboxing. The primary binary is `weft` (the CLI tool).

**Key facts**:
- 17,795+ lines of agent + pipeline code in `clawft-core`
- 6-stage pluggable pipeline (Classifier -> Router -> Assembler -> Transport -> Scorer -> Learner)
- Complexity-based tiered model routing with cost tracking
- SKILL.md format with three-level priority discovery and auto-generation
- Platform-abstracted (native + WASM/browser + WASI)
- Multi-channel: Telegram, Discord, Slack, Matrix, IRC, Teams, WhatsApp, Signal, Email, Web

---

## 2. Architecture

### The Agent Loop

The `AgentLoop` (in `crates/clawft-core/src/agent/loop_core.rs`, 2,235 lines) is the heart of the system. It implements the consume-process-respond cycle:

```text
Inbound Message (from MessageBus)
  |
  v
Session lookup / creation
  |
  v
ContextBuilder.build_messages()
  |
  v
Pipeline execution (Classifier -> Router -> Assembler -> Transport -> Scorer -> Learner)
  |
  v
Tool execution loop (up to max_tool_iterations)
  |  - Extract tool calls from LLM response
  |  - Execute each tool via ToolRegistry
  |  - Append tool results to context
  |  - Re-invoke LLM if stop_reason == ToolUse
  |
  v
Outbound Message (dispatched to MessageBus)
```

The loop supports:
- **Auto-delegation**: Rule-based routing of complex tasks to delegation tools before LLM processing (e.g., patterns matching "swarm", "orchestrate", "deploy")
- **Tool result size limiting**: Capped at 64 KB per tool result
- **Voice mode**: When enabled, injects a voice-mode system prompt instructing the LLM to respond in natural conversational language suitable for TTS
- **Hallucination detection**: Tracks hallucinated tool calls across iterations

### Crate Structure

| Crate | Role | Key Files |
|-------|------|-----------|
| `clawft-core` | Agent loop, pipeline, skills, memory, context, tools registry, sessions | `agent/`, `pipeline/`, `session/`, `tools/`, `bus/` |
| `clawft-llm` | LLM provider abstraction (OpenAI-compatible) | `provider.rs`, `router.rs`, `failover.rs`, `sse.rs` |
| `clawft-tools` | Built-in tool implementations | `file_tools.rs`, `web_search.rs`, `shell_tool.rs`, `delegate_tool.rs`, ... |
| `clawft-channels` | Messaging channel integrations | `telegram/`, `discord/`, `slack/`, `matrix/`, `web/`, ... |
| `clawft-cli` | CLI interface (`weft` binary) | `commands/`, `interactive/`, `mcp_tools.rs` |
| `clawft-platform` | Platform abstraction (native/WASM filesystem, HTTP) | Trait definitions for cross-platform I/O |
| `clawft-types` | Shared types, configs, error types | `config.rs`, `event.rs`, `skill.rs`, `routing.rs` |
| `clawft-plugin` | Plugin system, cancellation tokens | Plugin lifecycle traits |
| `clawft-security` | Input sanitization, validation | `verification.rs`, security module |
| `clawft-wasm` | Browser/WASI compilation target | WASM bindings |

---

## 3. Getting Started

### Building

Use the project build script for all build operations:

```bash
# Build native CLI (release)
scripts/build.sh native

# Build native CLI (debug, fast iteration)
scripts/build.sh native-debug

# Run workspace tests
scripts/build.sh test

# Build WASM targets
scripts/build.sh wasi
scripts/build.sh browser
```

The `weft` binary is produced by the `clawft-cli` crate.

### Running

```bash
# Start the agent in interactive mode
./target/release/weft

# Start with a specific configuration
./target/release/weft --config path/to/config.toml
```

---

## 4. Configuration

Agent configuration is defined in `AgentsConfig` (from `clawft-types`). Key settings:

| Setting | Description | Default |
|---------|-------------|---------|
| `system_prompt` | Base system prompt for the agent | Built-in default |
| `max_tool_iterations` | Maximum tool call rounds per message | 25 |
| `routing.mode` | Router mode: `"static"` or `"tiered"` | `"static"` |
| `routing.tiers` | Model tier definitions for tiered routing | 4 tiers |
| `model` | Default model (e.g., `"anthropic/claude-sonnet-4-20250514"`) | Configurable |
| `voice_mode` | Enable voice-optimized responses | `false` |
| `skill_autogen.enabled` | Enable autonomous skill creation | `false` |
| `skill_autogen.threshold` | Pattern repetitions before suggesting skill | `3` |

### Model Configuration

Models are specified in `provider/model` format:

```
anthropic/claude-sonnet-4-20250514
openai/gpt-4o
openrouter/nousresearch/hermes-3-llama-3.1-405b
```

The `ProviderRouter` in `clawft-llm` routes model names to configured providers.

---

## 5. Skills

### SKILL.md Format

Skills are markdown files with YAML frontmatter defining agent capabilities:

```markdown
---
name: research
description: Deep research on a topic
version: 1.0.0
variables:
  - topic
  - depth
allowed-tools:
  - WebSearch
  - Read
user-invocable: true
---

You are a research assistant. Given a {{topic}}, conduct thorough
research at the specified {{depth}} level...
```

### Discovery Chain

Skills are loaded in priority order (higher priority overwrites lower with the same name):

1. **Workspace skills** -- `.clawft/skills/` in the project root (highest priority)
2. **User skills** -- `~/.clawft/skills/` in the user's home directory
3. **Built-in skills** -- compiled into the binary (lowest priority)

The `SkillsV2Registry` (in `skills_v2.rs`, 1,430 lines) manages this three-level registry with YAML frontmatter parsing, security validation, and hot-reload support.

### Security Validation

Skills undergo security checks before loading:
- File size validation (`MAX_SKILL_MD_SIZE`)
- YAML depth validation (prevents deeply nested YAML bombs)
- Instruction sanitization
- Directory name validation (native only)

### Skill Auto-Generation

When `skill_autogen.enabled = true`, the `SkillAutogen` module (638 lines) detects repeated tool call patterns. After the same sequence appears at least `threshold` times (default: 3), it generates a skill candidate in `~/.clawft/skills/` with "pending" status. The skill requires user approval before activation.

Auto-generated skills have minimal permissions:
- No shell access
- No network access
- Filesystem limited to workspace directory

### Skill Hot-Reload

The `SkillWatcher` (555 lines) monitors the skill directories for file changes and reloads skills at runtime without restarting the agent.

---

## 6. Tools

### Built-in Tools

The `clawft-tools` crate provides these tool implementations:

| Tool | File | Description |
|------|------|-------------|
| File operations | `file_tools.rs` | Read, write, list, search files |
| Web search | `web_search.rs` | Search the web |
| Web fetch | `web_fetch.rs` | Fetch URL content |
| Shell execution | `shell_tool.rs` | Execute shell commands |
| Task delegation | `delegate_tool.rs` | Delegate to sub-agents |
| Memory operations | `memory_tool.rs` | Read/write agent memory |
| Message sending | `message_tool.rs` | Send messages via channels |
| UI rendering | `render_ui.rs` | Render UI components |
| Voice listen | `voice_listen.rs` | Audio transcription input |
| Voice speak | `voice_speak.rs` | Text-to-speech output |
| Audio transcribe | `audio_transcribe.rs` | Audio-to-text |
| Audio synthesize | `audio_synthesize.rs` | Text-to-audio |
| Agent spawning | `spawn_tool.rs` | Spawn sub-agents |
| Security policy | `security_policy.rs` | Query security policies |
| URL safety | `url_safety.rs` | Validate URL safety |

### Tool Registry

Tools are registered in the `ToolRegistry` (in `clawft-core/src/tools/registry.rs`). The registry is compiled and type-safe. Each tool has:
- A name (used in LLM tool definitions)
- Input schema (JSON Schema for validation)
- An async execution function
- Permission requirements

### Tool Execution Loop

When the LLM returns `stop_reason == ToolUse`, the agent loop:
1. Extracts tool calls from the response
2. Validates each tool call against the registry and permissions
3. Executes tools (result capped at 64 KB)
4. Appends tool results to the context
5. Re-invokes the LLM with updated context
6. Repeats up to `max_tool_iterations`

---

## 7. Memory

### File-Based Memory

clawft uses two markdown files for persistent memory, following the nanobot pattern:

- **MEMORY.md** -- Long-term facts (append-only, periodically consolidated). Contains knowledge the agent should remember across sessions. **Today the agent loop injects this entire file** as `# Relevant Memory:` (`ContextBuilder`). That dump is the fail-open store, not the target architecture — see RMM (`docs/research/rmm-reflective-memory-management.md`, ws06-memory): retrieve Top-K topic nodes, citation-rerank Top-M, merge-or-insert on session end.
- **HISTORY.md** -- Session summaries (grep-searchable log). Contains a chronological record of past sessions.

The `MemoryStore` (461 lines) manages these files through the platform filesystem abstraction.

### Directory Resolution

Memory files are resolved via fallback chain:
1. `~/.clawft/workspace/memory/` (preferred)
2. `~/.nanobot/workspace/memory/` (legacy fallback)

If neither exists, the `.clawft` path is created on first write.

### Platform Abstraction

All memory I/O goes through the `Platform` filesystem trait, making the module WASM-compatible and testable with mock filesystems.

---

## 8. Pipeline

The pipeline is a 6-stage pluggable processing chain defined in `crates/clawft-core/src/pipeline/`:

### Stage 1: Classifier (`classifier.rs`)

Classifies incoming messages by task type using keyword pattern matching. Categories: CodeGeneration, CodeReview, Research, Creative, Analysis, Math, Chat (default). The first matching keyword group wins. This is a Level 0 implementation -- no ML, no embeddings, just case-insensitive substring matching.

### Stage 2: Router (`router.rs`, `tiered_router.rs`)

Selects the LLM model based on task classification, user permissions, and cost budgets. Two modes:

- **Static**: Always uses the configured default model
- **Tiered** (1,650 lines): Complexity-based routing across model tiers with cost tracking and permission awareness

The tiered router maps complexity scores to model tiers:

| Tier | Complexity Range | Example Models | Use Case |
|------|-----------------|----------------|----------|
| Free | 0.0 - 0.15 | Local models | Trivial queries |
| Standard | 0.15 - 0.40 | Haiku-class | Simple tasks |
| Premium | 0.40 - 0.70 | Sonnet-class | Moderate complexity |
| Elite | 0.70 - 1.0 | Opus-class | Complex reasoning |

### Stage 3: Assembler (`assembler.rs`)

Assembles the final `ChatRequest` from the context messages, selected model, tool definitions, and configuration. Prepares the request for transport.

### Stage 4: Transport (`transport.rs`)

Sends the assembled request to the selected LLM provider via `clawft-llm`. Handles streaming, retries, and failover.

### Stage 5: Scorer (`scorer.rs`) -- GEPA FitnessScorer (v0.2)

Evaluates response quality after the LLM returns. The `FitnessScorer` (replacing the earlier `NoopScorer`) scores responses on 4 weighted dimensions: relevance (0.35), coherence (0.25), completeness (0.25), and conciseness (0.15). Weights are configurable. Scores feed into the learner for prompt evolution.

### Stage 6: Learner (`learner.rs`) -- GEPA TrajectoryLearner (v0.2)

Records trajectories (request + response + score) for adaptive learning. The `TrajectoryLearner` (replacing the earlier `NoopLearner`) implements GEPA (Genetic Evolution of Prompt Architectures, ADR-017): trajectory collection in a ring buffer, periodic pattern extraction, and 4 prompt mutation strategies (rephrase, add examples, remove ineffective, emphasize). Mutations are proposed as skill candidates requiring approval before activation.

### Supporting Modules

| Module | Lines | Purpose |
|--------|-------|---------|
| `cost_tracker.rs` | 954 | Per-tier budget enforcement, daily/monthly limits |
| `rate_limiter.rs` | 632 | Rate limiting per sender |
| `permissions.rs` | 757 | Permission resolver for tools and models |
| `traits.rs` | -- | Trait definitions for all pipeline stages |

---

## 9. Tiered Routing

The tiered router implements complexity-based model selection:

1. **Classify**: Keyword classifier determines task type
2. **Score complexity**: Based on task type, message length, tool requirements
3. **Check permissions**: User's `max_tier` permission limits model selection
4. **Check budget**: Cost tracker enforces daily/monthly spending limits
5. **Select tier**: Complexity score maps to a tier; fallback to lower tier if budget exceeded
6. **Route**: Return `RoutingDecision` with model name, tier, and estimated cost

Cost tracking is per-sender with configurable daily and monthly limits. The router records both estimated (pre-call) and actual (post-call) costs.

---

## 10. Channels

The `clawft-channels` crate provides messaging integrations:

| Channel | Directory | Protocol |
|---------|-----------|----------|
| Telegram | `telegram/` | Bot API |
| Discord | `discord/` | Gateway WebSocket |
| Slack | `slack/` | Events API |
| Matrix | `matrix/` | Client-Server API |
| IRC | `irc/` | IRC protocol |
| Microsoft Teams | `teams/` | Bot Framework |
| WhatsApp | `whatsapp/` | Cloud API |
| Signal | `signal/` | Signal protocol |
| Email | `email/` | SMTP/IMAP |
| Google Chat | `google_chat/` | Chat API |
| Web | `web/` | HTTP/WebSocket |

All channels implement the shared `ChannelTrait` (in `traits.rs`). The `PluginHost` (in `plugin_host.rs`) manages channel lifecycle. Channels convert platform-specific messages into the unified `InboundMessage`/`OutboundMessage` types consumed by the agent loop.

---

## 11. LLM Providers

The `clawft-llm` crate provides a unified interface for LLM APIs:

### Provider Architecture

- `Provider` trait defines the chat completion interface
- `OpenAiCompatProvider` implements it for any OpenAI-compatible API (Claude, OpenAI, Hermes via OpenRouter, local models via vLLM/llama.cpp)
- `LocalProvider` (v0.2) -- dedicated provider for local inference servers with key-optional auth, streaming, and model listing. Factory methods: `LocalProvider::ollama()`, `::vllm()`, `::llamacpp()`, `::lmstudio()`
- `ProviderRouter` routes `"provider/model"` strings to provider instances
- `FailoverChain` handles provider failover (retry -> fallback)

### Configuration

Providers are configured via `LlmProviderConfig`:

```rust
let config = LlmProviderConfig {
    name: "anthropic".into(),
    base_url: "https://api.anthropic.com/v1".into(),
    api_key_env: "ANTHROPIC_API_KEY".into(),
    // ...
};
```

### Streaming

The `sse.rs` module provides Server-Sent Events parsing for streaming responses. The `browser_transport.rs` module handles browser-specific transport for WASM targets.

---

## 12. Context Compression (v0.2)

The `ContextBuilder` now supports sliding-window context compression via `crates/clawft-core/src/agent/context.rs`. When enabled, older messages beyond the sliding window are automatically summarized using first-sentence extraction, keeping the context within token budgets.

### Configuration

```rust
let config = ContextCompressionConfig {
    max_context_tokens: 8192,  // Default token budget
    window_size: 20,           // Keep last N messages verbatim
    summary_strategy: SummaryStrategy::FirstSentence,
};

let builder = ContextBuilder::new()
    .with_compression(config);
```

### How It Works

1. Messages within the sliding window (most recent N) are kept verbatim.
2. Messages outside the window are summarized: the first sentence of each message is extracted as a condensed representation.
3. The token count is estimated and messages are dropped from the middle if the total still exceeds `max_context_tokens`.
4. The system prompt and most recent messages are always preserved.

Context compression is opt-in. Without `with_compression()`, the builder behaves identically to v0.1 (no summarization, middle-drop only).

---

## 13. Running Inside WeftOS

When clawft agents run on the WeftOS kernel, they gain additional capabilities:

### PID Tracking
Every agent process is registered in the kernel's ProcessTable with a unique PID. The supervisor manages agent lifecycle (spawn, stop, restart).

### Governance
All agent actions pass through the dual-layer governance gate:
- **CapabilityGate**: RBAC-based permission check
- **TileZeroGate** (optional): Three-branch constitutional governance with effect vectors

### ExoChain Provenance
Agent actions are chain-logged via the ExoChain, providing cryptographic audit trails (Ed25519 + ML-DSA-65 signatures). Every tool execution, message send, and state change is recorded with tamper-evident hashing.

### ECC Cognitive Substrate
The Ephemeral Causal Cognition (ECC) system provides:
- Causal DAG tracking of agent decisions
- HNSW semantic search across knowledge
- DEMOCRITUS tick loop for periodic cognitive processing
- Spectral analysis of causal graph structure
- BVH spatial-temporal index (ADR-056 / WEFT-720) for geometric overlap

#### ECC CLI (operator)

```bash
# Similarity (HNSW)
weaver ecc search "authentication refactor" -k 10
weaver ecc vector-config

# Causal walk
weaver ecc causal --node 42 --direction forward -D 3

# Spatial BVH (ADR-056 Phase E)
weaver ecc spatial status
weaver ecc spatial insert --tag wm_object --aabb 0,0,0,1,1,1
weaver ecc spatial query point --at 0.5,0.5,0.5
weaver ecc spatial query aabb --region -1,-1,-1,2,2,2 --filter-tags wm_object
weaver ecc spatial branch derive --parent 0 --name experiment
weaver ecc spatial diff --a 0 --b 1 --region -10,-10,-10,10,10,10
weaver ecc spatial events
weaver ecc spatial replay
```

**OQ4 (membership filters):** query APIs stay filter-free; pass
`--filter-tags` (comma-separated names or numeric tags) to apply a
post-query `MembershipFilter` wrapper. Full tenancy models stay out of
the `SpatialBackend` trait until product tenancy is defined.

### WASM Sandboxing
Tools can be executed in Wasmtime WASM sandboxes with deterministic, capability-constrained execution. The sandbox provides isolation and resource limits.

### Mesh Networking
Multi-node agent coordination via encrypted P2P mesh networking with Noise protocol handshake, peer discovery (mDNS, DHT, seed nodes), and cluster state synchronization.

### Self-Healing
The kernel's OS patterns provide supervisor restart strategies and dead-letter queues for failed agent processes.

---

## 14. Comparison with Hermes

Hermes (NousResearch, 18.5K stars, Python, MIT) is the closest parallel to clawft. Both implement the same agent pattern (loop, skills, memory, tools, channels).

### Where clawft is ahead

| Feature | clawft | Hermes |
|---------|--------|--------|
| Pipeline | 6-stage pluggable (Classify -> Route -> Assemble -> Transport -> Score -> Learn) | Simple LLM call |
| Tiered routing | Complexity-based, cost-tracked, permission-aware (1,650 lines) | Keyword heuristic |
| Skill hot-reload | File system watcher (555 lines) | Not available |
| Cost tracking | Per-tier budget enforcement (954 lines) | Not available |
| Rate limiting | Per-sender (632 lines) | Not available |
| Permissions | Full permission resolver (757 lines) | Command allowlist |
| Platform targets | Native + WASM/browser + WASI | Python only |
| Performance | Compiled Rust | Python runtime |
| Kernel integration | PID tracking, governance, ExoChain, ECC, WASM sandbox, mesh | None |

### Where Hermes is ahead

| Feature | Hermes | clawft Status |
|---------|--------|---------------|
| Context compression | Structured summarization | **Implemented (v0.2)**: sliding window + first-sentence summarization |
| GEPA prompt evolution | Genetic evolution of skill prompts | **Implemented (v0.2)**: TrajectoryLearner + FitnessScorer + 4 mutation strategies |
| User modeling | Honcho dialectic understanding | Not implemented (gap) |
| Session search | SQLite FTS5 + LLM summary | Not implemented (ECC could serve) |
| RL trajectories | ShareGPT JSONL + Atropos | Not implemented |

### Key Differentiators

clawft's advantages are in the kernel integration layer: ExoChain cryptographic provenance, three-branch constitutional governance, ECC cognitive substrate, WASM sandboxing, and mesh networking. These capabilities have no Hermes equivalent and represent the enterprise moat. Hermes models are supported as an `clawft-llm` provider for air-gapped deployments (ADR-018).
