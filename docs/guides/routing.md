# Routing and Pipeline Guide

This guide covers the 6-stage processing pipeline, how requests are classified
and routed to LLM providers, how context assembly and quality scoring work,
and the planned upgrades at each intelligence level.

---

## 1. Overview

Every incoming message passes through a **6-stage pipeline** before a response
is produced. Each stage is defined as a trait object held in an `Arc<dyn T>`,
making stages independently replaceable at compile time (via feature flags) or
at runtime (via `set_pipeline()`).

A `PipelineRegistry` maps `TaskType` variants to specialized `Pipeline`
instances. When no specialized pipeline is registered for a task type, the
registry falls back to the default pipeline. The registry's `complete()` method
orchestrates the full flow and feeds the resulting `Trajectory` back into the
`LearningBackend`.

The current Level 0 implementation ships with simple, zero-dependency defaults
for every stage. Higher-level implementations are activated by feature flags
(`vector-memory`, and planned flags for `ruvllm`, `tiny-dancer`, `sona`,
`attention`, `temporal-tensor`, `graph`, and `domain-expansion`).

---

## 2. Pipeline Architecture

```mermaid
flowchart LR
    subgraph "6-Stage Pipeline"
        C[Classifier] --> R[Router] --> A[Assembler] --> T[Transport] --> S[Scorer] --> L[Learner]
    end

    IN["ChatRequest"] --> C
    L --> OUT["LlmResponse"]

    C -- "TaskProfile\n(type, complexity)" --> R
    R -- "RoutingDecision\n(provider, model)" --> A
    A -- "AssembledContext\n(messages, token_estimate)" --> T
    T -- "LlmResponse" --> S
    S -- "QualityScore\n(overall, relevance, coherence)" --> L
    L -- "Trajectory" --> L
```

The `Pipeline` struct bundles all six stages:

```rust
pub struct Pipeline {
    pub classifier: Arc<dyn TaskClassifier>,
    pub router:     Arc<dyn ModelRouter>,
    pub assembler:  Arc<dyn ContextAssembler>,
    pub transport:  Arc<dyn LlmTransport>,
    pub scorer:     Arc<dyn QualityScorer>,
    pub learner:    Arc<dyn LearningBackend>,
}
```

And `PipelineRegistry` maps task types to pipelines:

```rust
pub struct PipelineRegistry {
    pipelines: HashMap<TaskType, Pipeline>,
    default: Pipeline,
}
```

Calling `registry.complete(request)` runs all six stages in sequence and
returns the `LlmResponse`.

---

## 3. Stage-by-Stage Breakdown

### Stage 1: Classifier (`TaskClassifier`)

The classifier examines the incoming `ChatRequest` and produces a `TaskProfile`
containing a detected `TaskType` and an estimated complexity score.

**Trait:**

```rust
pub trait TaskClassifier: Send + Sync {
    fn classify(&self, request: &ChatRequest) -> TaskProfile;
}
```

**Supporting types:**

```rust
pub struct TaskProfile {
    pub task_type:  TaskType,
    pub complexity: f32,       // 0.0..1.0
    pub keywords:   Vec<String>,
}

pub enum TaskType {
    Chat, CodeGeneration, CodeReview, Research,
    Creative, Analysis, ToolUse, Unknown,
}
```

**Current implementation: `KeywordClassifier`**

The Level 0 classifier scans the last user message (ignoring system messages)
for keyword patterns using case-insensitive substring matching. Patterns are
checked in priority order -- the first matching group determines the task type:

| Priority | Keywords | Task Type |
|----------|----------|-----------|
| 1 | code, function, implement, debug, fix, program, script, compile, refactor, class, struct, module | `CodeGeneration` |
| 2 | review, check, audit, lint, inspect | `CodeReview` |
| 3 | search, find, research, look up, lookup, discover | `Research` |
| 4 | write, story, poem, creative, compose, draft, narrative | `Creative` |
| 5 | analyze, explain, summarize, compare, evaluate, assess | `Analysis` |
| 6 | use tool, run tool, execute, call function | `ToolUse` |

If no keywords match, the task defaults to `Chat`.

**Complexity heuristic:** `matched_keywords.len() / word_count`, clamped to
the range [0.1, 0.9]. More keyword hits relative to message length produce a
higher complexity estimate.

**Planned upgrades:**

- Level 1 (`ruvllm`): 7-factor complexity scoring with HNSW-accelerated
  embedding comparison.
- Level 2 (`tiny-dancer`): Sub-millisecond neural classification.

---

### Stage 2: Router (`ModelRouter`)

The router selects a provider and model for the classified task.

**Trait:**

```rust
#[async_trait]
pub trait ModelRouter: Send + Sync {
    async fn route(&self, request: &ChatRequest, profile: &TaskProfile) -> RoutingDecision;
    fn update(&self, decision: &RoutingDecision, outcome: &ResponseOutcome);
}
```

**Supporting types:**

```rust
pub struct RoutingDecision {
    pub provider: String,   // e.g. "openai", "anthropic"
    pub model:    String,   // e.g. "gpt-4o", "claude-opus-4-5"
    pub reason:   String,
}

pub struct ResponseOutcome {
    pub success:    bool,
    pub quality:    QualityScore,
    pub latency_ms: u64,
}
```

**Current implementation: `StaticRouter`**

The Level 0 router parses the `"provider/model"` string from
`config.agents.defaults.model` (e.g. `"anthropic/claude-opus-4-5"`) and always
returns that pair. If no slash is present, the provider defaults to `"openai"`.
The `update()` method is a no-op.

**Upgrade: `TieredRouter` (Level 1)** -- see Section 5a below.

**Planned upgrade: `IntelligentRouter` (Level 2)** -- see Section 5.

---

### Stage 3: Assembler (`ContextAssembler`)

The assembler takes the full conversation history and trims it to fit within a
token budget, producing an `AssembledContext` ready for transport.

**Trait:**

```rust
#[async_trait]
pub trait ContextAssembler: Send + Sync {
    async fn assemble(&self, request: &ChatRequest, profile: &TaskProfile) -> AssembledContext;
}
```

**Supporting types:**

```rust
pub struct AssembledContext {
    pub messages:       Vec<LlmMessage>,
    pub token_estimate: usize,
    pub truncated:      bool,
}
```

**Current implementation: `TokenBudgetAssembler`**

Token estimation uses a `content.len() / 4 + 4` heuristic (4 tokens of
overhead per message for role and structure).

Truncation strategy:

1. Always keep the first message (typically the system prompt).
2. Walk backwards from the most recent message, adding messages until the
   budget is exhausted.
3. Middle messages are dropped.
4. A single message that exceeds the entire budget is included anyway.

The `profile` parameter is ignored at Level 0.

**Planned upgrades:**

- Level 1: Task-aware budget allocation (more tokens for code, fewer for chat).
- Level 2: Semantic compression of older messages before truncation.

---

### Stage 4: Transport (`LlmTransport`)

The transport sends the assembled request to an LLM provider and returns
the response.

**Trait:**

```rust
#[async_trait]
pub trait LlmTransport: Send + Sync {
    async fn complete(&self, request: &TransportRequest) -> Result<LlmResponse>;
}
```

**Supporting types:**

```rust
pub struct TransportRequest {
    pub provider:    String,
    pub model:       String,
    pub messages:    Vec<LlmMessage>,
    pub tools:       Vec<serde_json::Value>,
    pub max_tokens:  Option<i32>,
    pub temperature: Option<f64>,
}
```

**Current implementation: `OpenAiCompatTransport`**

Operates in two modes:

- **Stub mode** (`OpenAiCompatTransport::new()`): Returns a
  `ClawftError::Provider` error on every call. Used during development or when
  no provider is configured.
- **Provider mode** (`OpenAiCompatTransport::with_provider(provider)`): Wraps
  an `Arc<dyn LlmProvider>` and makes real HTTP calls.

The `enable_live_llm()` method on `AppContext` replaces the stub transport
with a live `ClawftLlmAdapter` backed by a `clawft_llm::Provider`. The adapter
converts between the pipeline's `TransportRequest` format and `clawft-llm`'s
typed `ChatRequest`/`ChatResponse`, routing through
`OpenAiCompatProvider` to the configured API endpoint.

The response parser handles both text completions and tool calls, extracting
content blocks, stop reason, and usage statistics from the OpenAI-format JSON.

---

### Stage 5: Scorer (`QualityScorer`)

The scorer assesses the quality of the response.

**Trait:**

```rust
pub trait QualityScorer: Send + Sync {
    fn score(&self, request: &ChatRequest, response: &LlmResponse) -> QualityScore;
}
```

**Supporting types:**

```rust
pub struct QualityScore {
    pub overall:   f32,    // 0.0..1.0
    pub relevance: f32,    // 0.0..1.0
    pub coherence: f32,    // 0.0..1.0
}
```

**Current implementation: `NoopScorer`**

Returns 1.0 for all three dimensions on every call. Input and response content
are completely ignored.

**Planned upgrades:**

- Level 1 (`ruvllm`): 5-dimension scoring -- accuracy, relevance, completeness,
  coherence, and safety.
- Level 2 (`tiny-dancer`): Neural quality model with confidence estimation.

---

### Stage 6: Learner (`LearningBackend`)

The learner records complete interaction trajectories for future improvement.

**Trait:**

```rust
pub trait LearningBackend: Send + Sync {
    fn record(&self, trajectory: &Trajectory);
    fn adapt(&self, signal: &LearningSignal);
}
```

**Supporting types:**

```rust
pub struct Trajectory {
    pub request:  ChatRequest,
    pub routing:  RoutingDecision,
    pub response: LlmResponse,
    pub quality:  QualityScore,
}

pub struct LearningSignal {
    pub feedback_type: String,   // e.g. "thumbs_up", "thumbs_down", "correction"
    pub value:         f32,      // -1.0..1.0
}
```

**Current implementation: `NoopLearner`**

Both `record()` and `adapt()` silently discard all data.

**Planned upgrades (`sona`):**

- Micro-LoRA: Per-request gradient-free adaptation.
- Base-LoRA: Hourly aggregated parameter updates.
- EWC++: Weekly elastic weight consolidation to prevent catastrophic forgetting.
- ReasoningBank: Persistent store of successful reasoning traces for retrieval.
- **RMM citation loop (ws06-memory):** generator citations on retrieved
  *memory* nodes as `+1/−1` into SONA `observe`, retriever frozen. Canonical
  note `docs/research/rmm-reflective-memory-management.md`. This is not the
  skill-routing WEFT-46 path; it generalizes it.

---

## 4. Message Flow

```mermaid
sequenceDiagram
    participant Channel as Channel Plugin
    participant Bus as MessageBus
    participant Agent as AgentLoop
    participant Session as SessionManager
    participant Context as ContextBuilder
    participant Registry as PipelineRegistry
    participant Tools as ToolRegistry

    Channel->>Bus: publish_inbound(InboundMessage)
    Bus->>Agent: consume_inbound()
    Agent->>Session: get_or_create("channel:chat_id")
    Session-->>Agent: Session
    Agent->>Context: build_messages(session, skills)
    Context-->>Agent: Vec<LlmMessage>
    Agent->>Registry: complete(ChatRequest)

    Note over Registry: 1. Classifier: ChatRequest -> TaskProfile
    Note over Registry: 2. Router: TaskProfile -> RoutingDecision
    Note over Registry: 3. Assembler: ChatRequest + TaskProfile -> AssembledContext
    Note over Registry: 4. Transport: TransportRequest -> LlmResponse
    Note over Registry: 5. Scorer: ChatRequest + LlmResponse -> QualityScore
    Note over Registry: 6. Learner: record(Trajectory)

    Registry-->>Agent: LlmResponse

    alt Response contains ToolUse blocks
        loop up to max_tool_iterations
            Agent->>Tools: execute(tool_name, args)
            Tools-->>Agent: Result<Value> (truncated to 64KB)
            Agent->>Registry: complete(ChatRequest + tool results)
            Registry-->>Agent: LlmResponse
        end
    end

    Agent->>Session: save_session(session)
    Agent->>Bus: dispatch_outbound(OutboundMessage)
    Bus->>Channel: consume_outbound()
    Channel->>Channel: send(OutboundMessage)
```

When `PipelineRegistry::complete()` runs, it:

1. Classifies the request using the **default** pipeline's classifier.
2. Looks up the specialized pipeline for that `TaskType` (falls back to
   default if none is registered).
3. Runs stages 2-6 using the selected pipeline.
4. Calls `router.update()` with the outcome after the response is scored.

---

## 5a. TieredRouter and Permission-Based Routing (Level 1)

The `TieredRouter` replaces the `StaticRouter` when `routing.mode` is set to
`"tiered"` in the configuration. It selects models based on task complexity,
user permissions, and cost budgets.

Source: `crates/clawft-core/src/pipeline/tiered_router.rs`,
`crates/clawft-core/src/pipeline/permissions.rs`

### How It Works

When a `ChatRequest` enters the pipeline, the `TieredRouter` executes an
8-step routing algorithm:

```mermaid
flowchart TB
    A[1. Extract AuthContext] --> B[2. Read permissions]
    B --> C{3. Rate limit check}
    C -->|exceeded| F1[Return fallback]
    C -->|ok| D[4. Filter tiers by max_tier]
    D --> E{5. Select tier by complexity}
    E --> G{Escalation needed?}
    G -->|yes, 3 conditions met| H[Promote to next tier]
    G -->|no| I[Use selected tier]
    H --> J{6. Budget check}
    I --> J
    J -->|over budget| K[Downgrade to cheaper tier]
    J -->|ok| L[7. Select model from tier]
    K --> L
    L --> M[8. Record estimated cost]
```

### AuthContext and Permission Resolution

Every request carries an `AuthContext` (sender_id, channel, permissions).
The `AgentLoop` creates this from the inbound message:

- **CLI channel** -> `admin` (Level 2, full access)
- **All other channels** -> `zero_trust` (Level 0, free tier only)

Permissions are resolved through a 5-layer merge (lowest to highest priority):

| Priority | Layer | Source |
|----------|-------|--------|
| 1 (lowest) | Built-in defaults | `zero_trust_defaults()`, `user_defaults()`, `admin_defaults()` |
| 2 | Global config | `routing.permissions.zero_trust`, `.user`, `.admin` |
| 3 | Workspace config | Project-local `.clawft/config.json` routing section |
| 4 | Per-user override | `routing.permissions.users.{sender_id}` |
| 5 (highest) | Per-channel override | `routing.permissions.channels.{channel}` |

Each layer can override any dimension independently. For example, a per-user
override can set `"level": 2` without specifying `max_tier` -- the `max_tier`
is inherited from the admin defaults.

### Granting Permissions

**Grant a specific user admin access:**
```json
{
  "routing": {
    "permissions": {
      "users": {
        "alice_telegram_123": { "level": 2 }
      }
    }
  }
}
```

**Grant an entire channel higher access:**
```json
{
  "routing": {
    "permissions": {
      "channels": {
        "telegram": { "level": 1 },
        "slack": { "level": 1 }
      }
    }
  }
}
```

**Grant a user access to specific tools only:**
```json
{
  "routing": {
    "permissions": {
      "users": {
        "bob_discord_456": {
          "level": 1,
          "tool_access": ["read_file", "list_dir", "web_search"],
          "cost_budget_daily_usd": 2.00
        }
      }
    }
  }
}
```

### Permission Levels

| Level | Name | Default `max_tier` | Tool Access | Rate Limit | Budget |
|-------|------|-------------------|-------------|------------|--------|
| 0 | `zero_trust` | `free` | None | 10/min | $0.10/day |
| 1 | `user` | `standard` | 7 safe tools | 60/min | $5.00/day |
| 2 | `admin` | `elite` | All (`*`) | Unlimited | Unlimited |

### 16 Permission Dimensions

Each dimension is independently configurable per level, user, or channel:

| Dimension | Type | Description |
|-----------|------|-------------|
| `level` | integer | Permission level (0, 1, 2) |
| `max_tier` | string | Maximum model tier accessible |
| `model_access` | string[] | Model allowlist (globs: `"anthropic/*"`) |
| `model_denylist` | string[] | Model denylist |
| `tool_access` | string[] | Tool allowlist (`["*"]` = all) |
| `tool_denylist` | string[] | Tool denylist |
| `max_context_tokens` | integer | Max input tokens |
| `max_output_tokens` | integer | Max output tokens |
| `rate_limit` | integer | Requests per window (0 = unlimited) |
| `streaming_allowed` | boolean | SSE streaming allowed |
| `escalation_allowed` | boolean | Complexity escalation allowed |
| `escalation_threshold` | float | Escalation trigger point (0.0-1.0) |
| `model_override` | boolean | User can override model selection |
| `cost_budget_daily_usd` | float | Daily budget (0.0 = unlimited) |
| `cost_budget_monthly_usd` | float | Monthly budget (0.0 = unlimited) |
| `custom_permissions` | object | Plugin-defined extensions |

### Tier Selection

The router selects the highest-quality tier the user can afford:

1. Filter tiers where the user's `max_tier` ordinal >= tier ordinal.
2. From allowed tiers, find those whose `complexity_range` contains the task
   complexity.
3. Pick the tier with the highest cost (best quality).
4. If no tier matches and escalation is enabled (3 conditions: user allows it,
   config enables it, complexity > threshold), promote to the next tier above
   `max_tier`.

### Model Selection Strategies

Within a chosen tier, the model is selected by one of four strategies:

| Strategy | Behavior |
|----------|----------|
| `preference_order` | First model in the tier's `models` list (default) |
| `round_robin` | Atomic counter rotates through models across requests |
| `lowest_cost` | First model (tiers sorted by cost) |
| `random` | Hash-based deterministic selection |

### Cost Tracking

The `CostTracker` maintains per-user daily and monthly spend:

- `reserve_budget()` atomically checks and reserves cost before routing
  (prevents TOCTOU races)
- `reconcile_actual()` adjusts spend after the LLM response (actual vs.
  estimated)
- Zero budget (0.0) means unlimited
- Spend resets at `reset_hour_utc` daily
- Persistence via atomic JSON file with 0600 permissions

When a user exceeds their budget, the router downgrades to a cheaper tier.
If all tiers are exhausted, the `fallback_model` is used (if permitted by
the user's `max_tier`).

### Rate Limiting

The `RateLimiter` enforces per-user request limits:

- Two-stage check: global counter (AtomicU64) first, then per-user sliding
  window
- LRU eviction when tracking > 10,000 users
- When rate limited, the router returns the fallback model (if permitted)

### Workspace Ceiling Enforcement

When a workspace config overrides routing permissions, the
`PermissionResolver` and `validate_workspace_ceiling` enforce ceilings
from the **global** routing config:

- Workspace cannot grant a `permissions.*.level` above global
  `routing.max_grantable_level` (default: `1` = user). Operators raise
  this field (e.g. `"max_grantable_level": 2`) when workspaces need to
  grant admin without a code change (WEFT-47).
- Workspace cannot enable escalation if global disables it
- Workspace cannot increase rate limits beyond global
- Workspace cannot increase cost budgets beyond global
- Workspace cannot grant wildcard tool access if global doesn't

This prevents a malicious `.clawft/config.json` from escalating privileges.

```json
{
  "routing": {
    "max_grantable_level": 2,
    "permissions": {
      "user": { "level": 1 },
      "admin": { "level": 2 }
    }
  }
}
```

Missing `max_grantable_level` deserializes to `1` for backward
compatibility with pre-0.8 configs.

### Configuration

See the [Configuration Reference](../reference/config.md#routing) for the
full JSON schema, defaults, and examples.

### Security: AuthContext Injection Prevention

The `auth_context` field on `ChatRequest` is annotated with
`#[serde(skip_deserializing)]`. This means:

- The field is populated server-side by the `AgentLoop`, not from user input
- JSON payloads sent through the gateway API cannot inject an `auth_context`
- Even if a user includes `"auth_context": {"level": 2}` in their request
  JSON, it is silently dropped during deserialization

### Source Files

| File | Description |
|------|-------------|
| `clawft-types/src/routing.rs` | Config types: `RoutingConfig`, `UserPermissions`, `AuthContext`, etc. |
| `clawft-core/src/pipeline/tiered_router.rs` | `TieredRouter` implementing `ModelRouter` trait |
| `clawft-core/src/pipeline/permissions.rs` | `PermissionResolver` with 5-layer merge |
| `clawft-core/src/pipeline/cost_tracker.rs` | `CostTracker` with atomic budget reservation |
| `clawft-core/src/pipeline/rate_limiter.rs` | `RateLimiter` with sliding window |
| `clawft-core/src/routing_validation.rs` | Config validation (17+ rules) |
| `clawft-core/src/tools/registry.rs` | Tool permission enforcement |

---

## 5. IntelligentRouter and 3-Tier Routing

The `IntelligentRouter` is gated behind the `vector-memory` feature flag and
implements 3-tier routing per ADR-026. It replaces the `StaticRouter` when
the feature is enabled.

### Routing Algorithm

The router evaluates three rules in order, taking the first match:

1. **Agent Booster tag check**: If `RoutingContext.tags` contains
   `"AGENT_BOOSTER_AVAILABLE"`, route to Tier 1 immediately.
2. **Policy cache lookup**: Embed the prompt and search the vector store for
   cached routing decisions. If the best match has a similarity score above
   0.85, use the cached tier.
3. **Complexity scoring**: Compute a heuristic complexity score and route
   based on threshold:
   - complexity < 0.30 -> Tier 2
   - complexity >= 0.30 -> Tier 3

### Tier Definitions

| Tier | Model | Latency | Cost | Use Cases |
|------|-------|---------|------|-----------|
| 1 | Agent Booster (WASM) | <1ms | $0 | Simple transforms: var to const, add types, format code. Skip the LLM entirely. |
| 2 | Haiku-class | ~500ms | Low | Simple tasks, short answers, low complexity (<30%) |
| 3 | Sonnet/Opus | 2-5s | Higher | Complex reasoning, architecture, security analysis (>30%) |

### Complexity Scoring

The `compute_complexity()` function applies four heuristics:

| Factor | Calculation | Max Contribution |
|--------|-------------|------------------|
| Word count | `words / 1000` | 0.3 |
| Code fences | Contains ` ``` ` | +0.2 |
| Reasoning keywords | Contains "analyze", "design", "architect", "explain", or "compare" | +0.1 |
| Multi-step indicators | Contains "first", "then", "finally", or "step 1" | +0.2 |

The result is clamped to [0.0, 1.0].

### Policy Learning

Successful routing decisions are stored as vector embeddings in the policy
store. On future requests:

1. The prompt is embedded.
2. The policy store is searched for the 5 nearest neighbors.
3. If the best match has a similarity score above 0.85, the cached tier is
   reused (cache hit).
4. Failed routing decisions (where `success == false`) are not cached.

### Router Types and Methods

```rust
pub struct IntelligentRouter {
    policy_store: VectorStore,
    embedder: Box<dyn Embedder>,
    cost_records: Vec<CostRecord>,
}

impl IntelligentRouter {
    pub async fn new(embedder: Box<dyn Embedder>) -> Self;
    pub async fn route_request(&self, prompt: &str, context: &RoutingContext)
        -> Result<RoutingDecision, RoutingError>;
    pub async fn record_cost(&mut self, model: &str, tokens: u64, cost: f32, latency_ms: u64);
    pub async fn update_policy(&mut self, pattern: &str, tier: u8, success: bool);
    pub fn get_model_stats(&self, model: &str) -> ModelStats;
}
```

---

## 6. Progressive Intelligence Levels

Intelligence capabilities are gated behind feature flags that can be enabled
independently. Each level builds on the one below.

| Level | Feature Flags | Routing | Memory | Scoring | Learning |
|-------|---------------|---------|--------|---------|----------|
| 0 | none (default) | Static routing (always same model) | Substring memory search | NoopScorer (always 1.0) | NoopLearner (discard all) |
| 1 | `ruvllm` | 7-factor complexity, HNSW routing cache (150x faster lookup) | HNSW-indexed vector memory | 5-dimension quality scoring | EMA statistics |
| 2 | + `tiny-dancer` | Sub-ms neural routing, CircuitBreaker, uncertainty estimation | Neural-compressed memory | Neural quality model | Gradient-free adaptation |
| 3 | + `sona` | Per-request adaptation | Semantic memory consolidation | Context-aware scoring | Micro-LoRA per-request, Base-LoRA hourly, EWC++ weekly, ReasoningBank |
| 4 | + `attention` + `temporal-tensor` | Flash attention, MoE routing | Temporal-aware memory with attention | Auto-tiered quantization scoring | Mixture-of-experts learning |
| 5 | + `graph` + `domain-expansion` | Property graph routing | Multi-hop graph memory | Cross-domain scoring | Cross-domain transfer learning |

At Level 0, the only optional feature flag is `vector-memory`, which enables
the `IntelligentRouter`, `VectorStore`, and `SessionIndexer` modules.

---

## 7. Scoring and Learning

### Current State (Level 0)

- **Scoring**: `NoopScorer` returns `QualityScore { overall: 1.0, relevance: 1.0, coherence: 1.0 }` for every response. No analysis is performed.
- **Learning**: `NoopLearner` discards all `Trajectory` records and `LearningSignal` feedback. No state is persisted.

### Feedback Loop

Even at Level 0, the pipeline feeds the `Trajectory` back into the router via
`router.update()`. The `StaticRouter` ignores this call, but the
`IntelligentRouter` uses it to update its policy store (caching successful
routing decisions as vector embeddings for future reuse).

### Planned Scoring (Level 1+, `ruvllm`)

The `ruvllm` feature flag will introduce 5-dimension quality scoring:

| Dimension | Description |
|-----------|-------------|
| Accuracy | Factual correctness of the response |
| Relevance | How well the response addresses the original request |
| Completeness | Whether all aspects of the request were addressed |
| Coherence | Logical flow and readability |
| Safety | Absence of harmful, biased, or inappropriate content |

### Planned Learning (Level 3+, `sona`)

The `sona` feature flag will introduce four learning mechanisms:

| Mechanism | Frequency | Description |
|-----------|-----------|-------------|
| Micro-LoRA | Per-request | Gradient-free parameter perturbation for immediate adaptation |
| Base-LoRA | Hourly | Aggregated low-rank updates from accumulated trajectories |
| EWC++ | Weekly | Elastic weight consolidation to prevent catastrophic forgetting |
| ReasoningBank | Continuous | Persistent store of successful reasoning traces, queried for similar future requests |
| RMM citation-rerank | Per-turn (planned, ws06-memory) | Generator cites which retrieved memory ids it used; `+1/−1` trains a lightweight SONA reranker. Retriever stays frozen (arXiv:2503.08026). See `docs/research/rmm-reflective-memory-management.md` |

---

## 8. Where Data Lives

### Current Storage

| Data | Location | Format | Persistence |
|------|----------|--------|-------------|
| Configuration | `~/.clawft/config.json` | JSON | Disk |
| Session history | `~/.clawft/workspace/sessions/` | JSONL (metadata header + one line per turn) | Disk |
| Long-term memory | `~/.clawft/workspace/memory/MEMORY.md` | Markdown (paragraph-based search) | Disk |
| Session summaries | `~/.clawft/workspace/memory/HISTORY.md` | Markdown | Disk |
| Cost records | In-memory `Vec<CostRecord>` | Struct | Per-session only |
| Routing policies | In-memory `VectorStore` (when `vector-memory` enabled) | Vector embeddings | Per-session only |

### Planned Storage

| Data | Planned Location | Format |
|------|-----------------|--------|
| Routing policies | `~/.clawft/routing/policies.rvf` | RVF (self-contained vector file) |
| Vector memory | `~/.clawft/workspace/memory/memory.rvf` | RVF |
| Quality scores | `~/.clawft/workspace/analytics/scores.jsonl` | JSONL |
| Learning trajectories | `~/.clawft/workspace/analytics/trajectories.jsonl` | JSONL |

### Backup

Currently, the only persistent pipeline-related data is session JSONL files in
`~/.clawft/workspace/sessions/`. These are plain text and can be copied
directly for backup.

Once `.rvf` files are implemented, they will be self-contained binary files
that can be copied for backup without any export step.

---

## 9. Configuration

### Model Routing

The model to use is configured in `agents.defaults.model` using
`"provider/model"` syntax:

```json
{
  "agents": {
    "defaults": {
      "model": "anthropic/claude-opus-4-5"
    }
  }
}
```

The `StaticRouter` splits this string at the first `/` to determine the
provider and model. If no slash is present, the provider defaults to `"openai"`.

Provider credentials are resolved from the `providers` config section or from
environment variables (e.g. `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`).

See the [Configuration Guide](configuration.md) for the full provider list and
environment variable reference.

### Token Budget

When using the live LLM pipeline (tiered routing), the context assembler's
token budget is derived from `routing.tiers[].max_context_tokens` -- the
largest value across all tiers is used. This controls the *input* token
budget (how many tokens of conversation history to send). It falls back
to 128,000 if no tier specifies a value.

```json
{
  "routing": {
    "tiers": [
      { "name": "fast", "max_context_tokens": 8192 },
      { "name": "balanced", "max_context_tokens": 16384 }
    ]
  }
}
```

For the default (non-live) pipeline, the budget comes from
`agents.defaults.max_tokens`:

```json
{
  "agents": {
    "defaults": {
      "max_tokens": 8192
    }
  }
}
```

### Viewing Routing Decisions

- `weft status --detailed` shows the current routing configuration (provider,
  model, pipeline level).
- The `--verbose` flag (or `RUST_LOG=debug`) logs routing decisions at the
  debug level, including the reason field from `RoutingDecision`.
- When using the `IntelligentRouter`, `get_model_stats()` returns per-model
  analytics (total calls, total cost, average latency).

### Enabling the IntelligentRouter

Build with the `vector-memory` feature flag:

```sh
cargo build --release --features vector-memory
```

Or install with:

```sh
cargo install clawft-cli --features vector-memory
```

The `IntelligentRouter` activates automatically when `vector-memory` is
enabled. No additional configuration is required -- it uses the same
`agents.defaults.model` as the Tier 3 fallback.

### ContextRouter (`Config.routing.context_router`) — feature gates (WEFT-51)

`ContextRouter` runs **before** the LLM call inside the agent loop. It
selects skills / tool subsets and may emit a clamped `complexity_hint`;
it **never** picks a model (that remains `TieredRouter` / `StaticRouter`).

| Config value | Implementation | Notes |
|--------------|----------------|-------|
| `"null"` (default) | `NullRouter` | Empty decision; historical behaviour. |
| `"llm-classifier"` | `LlmClassifierRouter` | v1; needs a live `LlmClient`. |
| `"embedding"` | `EmbeddingRouter` | v2; needs `vector-memory` (+ optional `embedding-router`). |
| `"hybrid"` | `HybridRouter` | v2.5; embedding primary + classifier fallback. |
| other | warn + `NullRouter` | Unknown strings degrade gracefully at boot. |

#### `embedding-router` cargo feature

| Build | Index backend (`INDEX_BACKEND`) | Behaviour |
|-------|----------------------------------|-----------|
| `--features embedding-router` (default via `full`) | `"diskann"` | Production Vamana index (`ruvector-diskann@2.1`). |
| `embedding-router` **off**, `vector-memory` **on** | `"brute-force"` | In-memory O(n) cosine over ~35 skills. **Graceful fallback** — `context_router = "embedding"` still constructs and routes. |
| `vector-memory` off | module not compiled | Daemon/weave must not select embedding without the module; slim builds keep `"null"`. |

Compile-time constant: `clawft_core::agent::context_router::INDEX_BACKEND`
(`"diskann"` | `"brute-force"`). Construction logs
`EmbeddingRouter: index built` with `backend=…`.

Construction failures (empty skill registry, embed error at build time)
return `EmbeddingRouterError` / `None` from the daemon helper — boot
falls back to `NullRouter` with a `warn!`, never panics.

**Feature-off test matrix** (must both compile + pass):

```bash
# default (diskann)
scripts/build.sh test clawft-core

# embedding-router off → brute-force floor (WEFT-51)
cargo test -p clawft-core --no-default-features \
  --features native,vector-memory \
  --lib agent::context_router::embedding
```

Source: `crates/clawft-core/src/agent/context_router/embedding/`,
`crates/clawft-weave/src/daemon.rs` (`build_embedding_router_or_warn`).

---

## 10. Phase 3 TODO: Unimplemented Features

The following features are designed but not yet implemented. They are tracked
here as stubs for future development.

### Adaptive Classifier

- [ ] 7-factor complexity scoring (word count, vocabulary diversity, syntactic
  depth, domain specificity, ambiguity, code density, reasoning chain length)
- [ ] HNSW-indexed prompt embeddings for classification by similarity
- [ ] Confidence estimation with fallback to higher tier on low confidence

### Neural Router

- [ ] Sub-millisecond neural routing via `tiny-dancer` WASM inference
- [ ] CircuitBreaker pattern: automatic tier demotion when a provider is slow
  or erroring
- [ ] Uncertainty estimation: route to higher tier when the router is unsure

### Quality Scoring

- [ ] 5-dimension scoring (accuracy, relevance, completeness, coherence, safety)
- [ ] Heuristic baselines: response length, format compliance, tool call
  success rate
- [ ] Neural quality model with calibrated confidence scores

### Learning Backend

- [ ] Trajectory persistence to disk (JSONL or RVF format)
- [ ] Micro-LoRA per-request adaptation
- [ ] Base-LoRA hourly batch updates
- [ ] EWC++ weekly consolidation
- [ ] ReasoningBank with semantic retrieval
- [ ] RMM: stop dumping `MEMORY.md`; retrieve Top-K / citation-rerank Top-M
      (`docs/research/rmm-reflective-memory-management.md`, **WEFT-732** then **WEFT-733**)

### Routing Policy Persistence

- [ ] Save routing policies to `~/.clawft/routing/policies.rvf`
- [ ] Load policies on startup for warm-start routing
- [ ] Policy expiration and garbage collection

### Cost Analytics

- [ ] Persist cost records across sessions
- [ ] Per-tier cost breakdown in `weft status`
- [ ] Budget alerts when cost exceeds configurable thresholds
- [ ] Cost-aware routing: prefer cheaper tiers when quality is sufficient

---

## Source Files

### Pipeline Stages
- `clawft-core/src/pipeline/traits.rs` -- Trait definitions, `ChatRequest`, `RoutingDecision`, `PipelineRegistry`
- `clawft-core/src/pipeline/classifier.rs` -- `KeywordClassifier`
- `clawft-core/src/pipeline/router.rs` -- `StaticRouter` (Level 0)
- `clawft-core/src/pipeline/assembler.rs` -- `TokenBudgetAssembler`
- `clawft-core/src/pipeline/transport.rs` -- `OpenAiCompatTransport` and `LlmProvider`
- `clawft-core/src/pipeline/scorer.rs` -- `NoopScorer`
- `clawft-core/src/pipeline/learner.rs` -- `NoopLearner`
- `clawft-core/src/pipeline/llm_adapter.rs` -- `ClawftLlmAdapter` and `build_live_pipeline`

### Tiered Router (Level 1)
- `clawft-types/src/routing.rs` -- Config types: `RoutingConfig`, `UserPermissions`, `AuthContext`
- `clawft-core/src/pipeline/tiered_router.rs` -- `TieredRouter` (8-step algorithm, 49 tests)
- `clawft-core/src/pipeline/permissions.rs` -- `PermissionResolver` (5-layer merge, 31 tests)
- `clawft-core/src/pipeline/cost_tracker.rs` -- `CostTracker` (atomic budgets, 27 tests)
- `clawft-core/src/pipeline/rate_limiter.rs` -- `RateLimiter` (sliding window, 21 tests)
- `clawft-core/src/routing_validation.rs` -- Config validation (17+ rules, 45 tests)
- `clawft-core/src/tools/registry.rs` -- Permission-gated tool execution
- `clawft-core/src/agent/loop_core.rs` -- AuthContext creation from inbound messages

### Intelligent Router (Level 2, behind `vector-memory`)
- `clawft-core/src/intelligent_router.rs` -- `IntelligentRouter`

---

## 11. Agent Routing (L1)

Agent routing determines which agent instance handles an inbound message based
on the channel and sender context. This operates *before* the 6-stage pipeline;
the agent router picks the agent, then the agent's pipeline processes the
message.

Source: `clawft-types/src/agent_routing.rs`, `clawft-core/src/agent_routing.rs`

### AgentRouter

The `AgentRouter` evaluates an ordered list of `AgentRoute` rules against each
inbound message. The first matching rule wins.

```rust
pub struct AgentRouter {
    routes: Vec<AgentRoute>,
    catch_all: Option<String>,
}
```

Construction methods:

- `AgentRouter::new(config)` -- from an `AgentRoutingConfig`
- `AgentRouter::empty()` -- no rules, no catch-all
- `AgentRouter::with_catch_all(agent)` -- catch-all only, no explicit rules

### AgentRoute Configuration

Each route specifies a channel to match and optional match criteria:

```rust
pub struct AgentRoute {
    pub channel: String,
    pub match_criteria: MatchCriteria,
    pub agent: String,
}

pub struct MatchCriteria {
    pub user_id: Option<String>,
    pub phone: Option<String>,
    pub chat_id: Option<String>,
}
```

All `MatchCriteria` fields are optional. A field that is `None` matches any
value. When multiple fields are set, all must match (AND semantics). An
empty `MatchCriteria` (all `None`) acts as a wildcard for that channel.

### Routing Algorithm

```mermaid
flowchart TB
    A[Inbound message] --> B{sender_id empty?}
    B -->|yes| C{catch_all configured?}
    B -->|no| D[Walk routes in order]
    D --> E{Rule matches?}
    E -->|yes| F["RoutingResult::Agent(rule.agent)"]
    E -->|no, more rules| D
    E -->|no rules left| C
    C -->|yes| G["RoutingResult::CatchAll(catch_all)"]
    C -->|no| H["RoutingResult::NoMatch + warn log"]
```

1. If the message has an empty `sender_id` (anonymous), skip rule evaluation
   and go directly to the catch-all.
2. Walk routes in declaration order. The first rule where `channel` matches
   and all non-`None` criteria match the message wins.
3. If no rule matches and a `catch_all` agent is configured, route there.
4. If no rule matches and no catch-all is configured, return `NoMatch` and
   log a warning. Messages are **not** silently dropped.

### Anonymous Routing

Messages with an empty `sender_id` bypass all routing rules and go directly
to the catch-all agent. If no catch-all is configured, they receive a
`NoMatch` result with a warning log.

### Configuration Example

```toml
[[agent_routes]]
channel = "telegram"
match = { user_id = "12345" }
agent = "work-agent"

[[agent_routes]]
channel = "whatsapp"
match = { phone = "+1555123" }
agent = "personal-agent"

[[agent_routes]]
channel = "slack"
# Empty match criteria = wildcard for all slack messages
agent = "slack-agent"

[agent_routing]
catch_all = "default-agent"
```

### Source Files

| File | Description |
|------|-------------|
| `clawft-types/src/agent_routing.rs` | `AgentRoute`, `MatchCriteria`, `AgentRoutingConfig` types |
| `clawft-core/src/agent_routing.rs` | `AgentRouter` engine with first-match-wins evaluation |

---

## 12. Planning Strategies (L4)

The planning router adds structured reasoning capabilities with configurable
guard rails to prevent runaway loops and cost overruns.

Source: `clawft-core/src/planning.rs`

### Strategies

Two planning strategies are available:

| Strategy | Serialized Name | Description |
|----------|-----------------|-------------|
| `React` | `react` | Reason+Act loop: alternate between reasoning about the current state and taking an action. Each step is a single reasoning-action pair. |
| `PlanAndExecute` | `plan_and_execute` | Generate a full plan first, then execute each step sequentially. |

### PlanningConfig

```rust
pub struct PlanningConfig {
    pub max_planning_depth: u32,            // default: 10
    pub max_planning_cost_usd: f64,         // default: 1.00
    pub planning_step_timeout: Duration,    // default: 60s
    pub circuit_breaker_no_op_limit: u32,   // default: 3
}
```

| Parameter | Default | Description |
|-----------|---------|-------------|
| `max_planning_depth` | 10 | Maximum number of reasoning steps before forced termination |
| `max_planning_cost_usd` | $1.00 | Hard budget cap for LLM calls during a single planning session |
| `planning_step_timeout` | 60s | Maximum wall-clock duration for a single planning step |
| `circuit_breaker_no_op_limit` | 3 | Consecutive non-actionable steps before circuit breaker abort |

### Configuration

```toml
[router.planning]
max_planning_depth = 10
max_planning_cost_usd = 1.0
planning_step_timeout = 60
circuit_breaker_no_op_limit = 3
```

### Guard Rails

The `PlanningRouter.check_guard_rails()` method validates each step against
all limits before execution. It returns `Some(TerminationReason)` if a limit
is reached, or `None` if the step is within bounds.

```rust
pub fn check_guard_rails(
    &self,
    current_step: u32,
    total_cost_usd: f64,
    consecutive_no_ops: u32,
) -> Option<TerminationReason>
```

### Termination Reasons

When a planning session ends, the `TerminationReason` explains why:

| Reason | Trigger |
|--------|---------|
| `Completed` | Planning finished successfully |
| `MaxDepthReached` | `current_step >= max_planning_depth` |
| `BudgetExceeded` | `total_cost_usd >= max_planning_cost_usd` |
| `StepTimeout` | Single step exceeded `planning_step_timeout` |
| `CircuitBreaker` | `consecutive_no_ops >= circuit_breaker_no_op_limit` |
| `Cancelled` | User abort or agent shutdown |

### Partial Results

When planning is terminated early, `explain_termination()` produces a
human-readable summary that includes the termination reason, number of steps
completed, and total cost spent. The `PlanningOutcome` struct carries all
step results so that partial work is not lost:

```rust
pub struct PlanningOutcome {
    pub strategy: PlanningStrategy,
    pub termination_reason: TerminationReason,
    pub steps_executed: u32,
    pub total_cost_usd: f64,
    pub total_duration: Duration,
    pub step_results: Vec<PlanningStepResult>,
    pub explanation: String,
}
```

### Source Files

| File | Description |
|------|-------------|
| `clawft-core/src/planning.rs` | `PlanningRouter`, `PlanningConfig`, `PlanningStrategy`, `TerminationReason` |

---

## 13. Delegation (M1-M3)

Delegation routes tasks to an appropriate execution target based on regex
rule matching and complexity heuristics. The architecture uses an MCP-first
posture: tasks are handled either **locally** or via the **Claude API**
(Anthropic Messages API). There is no subprocess spawning.

Source: `clawft-services/src/delegation/mod.rs`,
`clawft-services/src/delegation/claude.rs`

### DelegationEngine

The `DelegationEngine` compiles delegation rules at construction time and
evaluates them on each `decide()` call. Rules with invalid regex patterns
are logged and skipped.

```rust
pub struct DelegationEngine {
    config: DelegationConfig,
    compiled_rules: Vec<CompiledRule>,
}
```

The `decide()` method takes a task string and a `claude_available` flag:

1. Walk compiled rules in order; first regex match wins.
2. If the matched target is `Claude` but `claude_available` is false, fall
   back to `Local`.
3. If the matched target is `Flow`, resolve it to `Claude` (see below).
4. If no rule matches, use `Auto` mode (complexity heuristic).

### Auto-Decide Heuristic

When no rule matches, `DelegationEngine::complexity_estimate()` scores the
task on a `0.0..1.0` scale using three weighted signals:

| Signal | Weight | Description |
|--------|--------|-------------|
| Text length | 30% | Saturates at 500 characters |
| Question mark density | 20% | Saturates at 3 question marks |
| Complexity keywords | 50% | `deploy`, `refactor`, `architect`, `security`, etc. (20 keywords) |

Routing based on the score:

| Complexity | Score | Target |
|------------|-------|--------|
| Low | < 0.3 | `Local` |
| Medium / High | >= 0.3 | `Claude` (if available and enabled), else `Local` |

### ClaudeDelegator

The `ClaudeDelegator` calls the Anthropic Messages API directly with a
multi-turn tool-use loop. It provides:

- **Tool forwarding**: A caller-supplied closure resolves tool calls during
  the delegation loop.
- **Turn limiting**: Configurable `max_turns` to prevent runaway loops.
- **API error handling**: Structured error variants for HTTP, API, parse,
  and timeout failures.

### DelegationError Variants

| Variant | Description |
|---------|-------------|
| `Http` | HTTP request to the Claude API failed |
| `Api` | Non-2xx status with body capture |
| `InvalidResponse` | Response body could not be parsed |
| `MaxTurnsExceeded` | Delegation loop hit the configured turn limit |
| `ToolExecFailed` | A tool execution failed during delegation |
| `SubprocessFailed` | Non-zero exit code with stderr capture (legacy, retained for compatibility) |
| `OutputParseFailed` | stdout could not be parsed (legacy, retained for compatibility) |
| `Timeout` | Delegation exceeded configured timeout |
| `Cancelled` | User abort or agent shutdown |
| `FallbackExhausted` | All delegation targets exhausted |

### Flow Target Retired (WEFT-179)

`DelegationTarget::Flow` and `claude_flow_enabled` / `claudeFlowEnabled` were
**removed**. Subprocess `FlowDelegator` was never shipped; the product path is
MCP-first multi-agent integration.

**Migration:**

| Old | New |
|-----|-----|
| `"target": "flow"` / `"Flow"` | Still loads as `claude` (serde alias). Prefer `"claude"`. |
| `claudeFlowEnabled` | Remove the key (ignored if present). |
| Subprocess `claude-flow` spawn | MCP session + skills (`allowed-tools`). |

For complex multi-agent coordination, use **skill-based orchestration** via
MCP servers (see Section 14 and `docs/guides/mcp-integration.md`).

### Feature Gate

Delegation is gated behind the `delegate` feature, which is enabled by
default. To build without delegation:

```bash
cargo build --no-default-features
```

### Source Files

| File | Description |
|------|-------------|
| `clawft-services/src/delegation/mod.rs` | `DelegationEngine` with rule matching and complexity heuristics |
| `clawft-services/src/delegation/claude.rs` | `ClaudeDelegator` with Anthropic API tool-use loop, `DelegationError` |
| `clawft-types/src/delegation.rs` | `DelegationConfig`, `DelegationTarget`, `DelegationRule` types |

---

## 14. MCP Server Discovery (M4)

The `McpServerManager` provides runtime management of MCP server connections,
including hot-reload when the configuration file changes.

Source: `clawft-services/src/mcp/discovery.rs`

### McpServerManager

```rust
pub struct McpServerManager {
    servers: HashMap<String, ManagedMcpServer>,
    debounce: Duration,       // default: 500ms
    drain_timeout: Duration,  // default: 30s
}
```

Core operations:

| Method | Description |
|--------|-------------|
| `add_server(config)` | Register a new MCP server (replaces existing by name) |
| `remove_server(name)` | Remove and drain a server |
| `list_servers()` | List all managed servers |
| `get_server(name)` | Look up a server by name |
| `mark_connected(name, tools)` | Update server status and discovered tools |
| `mark_error(name)` | Mark a server as errored |
| `apply_config_diff(new_configs)` | Hot-reload: add, remove, and update servers |

### Server Status

Each managed server has a `ServerStatus`:

| Status | Description |
|--------|-------------|
| `Connected` | Server is connected and ready |
| `Connecting` | Handshake in progress |
| `Draining` | In-flight requests completing before disconnect |
| `Disconnected` | Server is not connected |
| `Error` | Connection failed |

### Internal-Only Servers

The `MCPServerConfig` has an `internal_only` field that defaults to `true`:

```rust
pub struct MCPServerConfig {
    pub command: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub url: String,
    pub internal_only: bool,  // default: true
}
```

When `internal_only` is `true`:

- The MCP session **is** created (the server process starts and the
  handshake completes normally).
- The server's tools are **not** registered in the `ToolRegistry`. They
  will not appear in `schemas()` output and will not be sent to the LLM.

When `internal_only` is `false`:

- The MCP session is created **and** the server's tools are listed and
  registered in the `ToolRegistry`, making them available to the LLM.

This distinction exists to support **infrastructure MCP servers** such as
`claude-flow` and `claude-code`. These servers expose hundreds of tools
(agent management, memory, coordination, etc.) that would overwhelm the
LLM context if loaded directly. Instead, they are kept internal and their
capabilities are accessed through **skills**.

```json
{
  "mcp_servers": {
    "claude-flow": {
      "command": "npx",
      "args": ["--no-install", "@claude-flow/cli", "mcp", "start"],
      "internal_only": true
    },
    "project-tools": {
      "command": "./tools-server",
      "internal_only": false
    }
  }
}
```

In this example, `claude-flow` is internal (session only, no tool
registration) while `project-tools` is external (tools registered and
visible to the LLM).

### Skill-Based Tool Discovery

Instead of loading all MCP tools into the LLM context, skills act as a
discovery layer. A skill declares which tools it needs via `allowed-tools`
glob patterns. The `ToolRegistry::schemas_for_tools()` method filters tool
schemas by those patterns, so only the relevant subset is sent to the LLM
on any given turn:

```rust
// ToolRegistry method
pub fn schemas_for_tools(&self, allowed: &[String]) -> Vec<serde_json::Value>
```

Patterns support glob syntax (`*` and `?`). For example, a skill for
memory operations might declare `allowed-tools: ["memory_*", "search"]`,
and only those tools would be included in that turn's tool list. This
prevents context bloat while keeping the full tool surface available when
needed.

### Hot-Reload Protocol

When `clawft.toml` changes on disk:

1. File watcher detects the change (500ms debounce to coalesce rapid edits).
2. `apply_config_diff()` computes the diff between old and new server lists.
3. **New servers**: Added immediately in `Disconnected` state.
4. **Removed servers**: Marked as `Draining` for up to 30s to complete
   in-flight calls, then disconnected.
5. **Changed servers**: Old server is removed, new config is added (remove
   + add).

`apply_config_diff()` returns `(added, removed, changed)` counts.

### CLI Commands

```sh
weft mcp add <name> <command|url>
weft mcp list
weft mcp remove <name>
```

### Source Files

| File | Description |
|------|-------------|
| `clawft-services/src/mcp/discovery.rs` | `McpServerManager`, `ServerStatus`, `ManagedMcpServer` |
| `clawft-types/src/config/mod.rs` | `MCPServerConfig` with `internal_only` field |
| `clawft-core/src/tools/registry.rs` | `ToolRegistry::schemas_for_tools()` for per-turn filtering |
| `clawft-cli/src/mcp_tools.rs` | MCP session creation with internal-only branching |
