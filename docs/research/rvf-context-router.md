# RVF Context Router — Design Research

**Date**: 2026-04-26
**Status**: Research, pre-implementation
**Audience**: Multi-expert review panel
**Companion docs**:
- `.planning/04-rvf-integration.md` — RVF binary format, segments, WASM microkernel
- `.planning/05-ruvector-crates.md` — full ruvector crate map
- `.planning/08-tiered-router.md` — TieredRouter (model-tier router, already shipped)
- `.planning/development_notes/ruv-ecosystem-analysis-20260414.md` — current crate state
- `.planning/development_notes/adaptive-hnsw-tiered-search.md` — three-tier HNSW pattern (in our codebase)
- `docs/research/gepa-prompt-evolution-analysis.md` — prompt evolution loop

---

## 0. Scope Statement

This document designs the **per-turn context router** for the WeftOS Concierge
chat agent (RPC `agent.chat` in `crates/clawft-weave/src/daemon.rs`).

It does **not** redesign the model-tier router. The tiered router
(`clawft-core::pipeline::tiered_router::TieredRouter`) is finished and picks
free / standard / premium / elite by complexity score and permissions. That is
the **what model** problem.

The context router is the **what skills + which agent profile + which tool
subset** problem. It runs *before* the tiered router on each turn and shapes
the request that the tiered router receives.

The chat agent wraps `clawft-core::agent::loop_core::run_tool_loop` and the
`clawft-tools` registry. Today both `.clawft/skills/` (2 entries:
`claude-code`, `claude-flow`) and `.claude/skills/` (33 entries) plus
`.claude/agents/` (27 categories) coexist on disk, and **nothing reads them
dynamically per turn** — agents currently get a static system prompt.

For v1 we plan an LLM classifier on the free tier. This doc decides what v2
looks like, with the hard constraint that v2 must materially earn its keep
over v1.

---

## 1. Inventory: ruv Ecosystem Building Blocks (Verified)

### 1.1 What is actually in `Cargo.toml` today

From the workspace root (`Cargo.toml` lines 167-172):

| Crate | Version | Status |
|-------|---------|--------|
| `ruvector-cluster` | `2.0` | declared, used only by `clawft-kernel` |
| `ruvector-raft` | `2.0` | declared, used only by `clawft-kernel` |
| `ruvector-replication` | `2.0` | declared, used only by `clawft-kernel` |
| `ruvector-diskann` | `2.1` | declared, used by `clawft-kernel` (feature-gated) |
| `cognitum-gate-tilezero` | `0.1` | declared, used by `clawft-kernel` |

**Crucially**: `ruvllm`, `sona`, `micro-hnsw-wasm`, `rvf-runtime`, `rvf-types`,
`ruvector-tiny-dancer-core`, `ruvector-attention`, `ruvector-temporal-tensor`
are **not yet workspace dependencies**. They are referenced in
`.planning/04-rvf-integration.md` and `.planning/05-ruvector-crates.md` as
*planned* additions. Adding any of them is a real workspace change with audit,
build-time, and binary-size implications.

### 1.2 ruvector-diskann 2.1 — the workhorse we already own

From `.planning/development_notes/ruv-ecosystem-analysis-20260414.md`:

- **Vamana** two-pass graph build, alpha-robust pruning
- **Product Quantization** for compressed distance
- **mmap** graph loading (zero-copy)
- **SIMD distance** optional via `simsimd` feature
- **Performance**: 1.0 recall, 90 µs search, 14 tests in upstream

This is the highest-trust ANN we already have. If the v2 router needs vector
NN, DiskANN should be the default. Note it is **native-only** (mmap, simsimd)
— not WASM-friendly.

### 1.3 The candidate intelligence crates (NOT yet adopted)

These are documented in `.planning/05-ruvector-crates.md`. Versions and
feature-flag behavior are taken from that doc and from upstream crate names.
**None of these are pulled into the workspace yet** — adopting any is a
deliberate choice.

| Crate | What it gives us | Binary cost (native) | WASM | Verified surface |
|-------|------------------|----------------------|------|------------------|
| `ruvllm` (`minimal` feature) | `TaskComplexityAnalyzer`, `HnswRouter` (150x faster pattern match), `QualityScoringEngine`, `SessionManager` | ~2-3 MB | via `ruvllm-wasm` | doc-grounded |
| `ruvllm-wasm` (standalone) | `HnswRouterWasm`, `SonaInstantWasm`, chat templates | 50-200 KB | yes | MCP tools confirm it ships; **HARD CAP ~11 patterns in v2.0.1** (see §1.5) |
| `sona` | `MicroLoRA` rank-2 (instant), `BaseLoRA` rank-8 (background), `EWC++`, `ReasoningBank` | ~100 KB | yes (5 deps) | doc-grounded |
| `ruvector-tiny-dancer-core` | `FastGRNN` (<1ms inference), `CircuitBreaker`, `UncertaintyEstimator`, `Trainer` for KD | ~500 KB-1 MB | **no** (rusqlite, redb, simsimd) | doc-grounded |
| `ruvector-attention` | 40+ attention mechanisms (Flash, MoE, InformationBottleneck, MLA) | ~80 KB | good | doc-grounded |
| `ruvector-temporal-tensor` | 8/7/5/3-bit groupwise symmetric quantization, zero deps | <10 KB | perfect | doc-grounded |
| `micro-hnsw-wasm` | Zero-dep WASM HNSW, 11.8 KB; STDP/LIF online learning at edge | 11.8 KB | perfect | doc-grounded; compile-time `MAX_VECTORS=32/core`, `MAX_DIMS=16` |
| `rvf-runtime` + `rvf-types` + `rvf-index` | Binary container: `VEC`, `INDEX`, `META`, `POLICY_KERNEL`, `COST_CURVE`, `WITNESS` segments; progressive 3-tier HNSW (Layer A/B/C) | ~260 KB | yes | doc-grounded |
| `cognitum-gate-tilezero` (already a dep) | RBAC-style permit/receipt, capability tokens | small | n/a | already in workspace |

### 1.4 ruvllm MCP tools (live, callable)

The MCP server `mcp__claude-flow__ruvllm_*` confirms what ships in
**ruvllm-wasm v2.0.1**:

```
ruvllm_status            -- probe availability
ruvllm_hnsw_create       -- (dimensions, maxPatterns, efSearch)
ruvllm_hnsw_add          -- index a pattern
ruvllm_hnsw_route        -- (routerId, query, k=3)  -> nearest patterns
ruvllm_microlora_create  -- (inputDim, outputDim, rank ∈ [1,4], alpha)
ruvllm_microlora_adapt   -- (loraId, quality ∈ [0,1], success, learningRate)
ruvllm_sona_create       -- (hiddenDim=64, patternCapacity, learningRate)
ruvllm_sona_adapt        -- (sonaId, quality ∈ [0,1])
ruvllm_chat_format       -- chat template formatter
ruvllm_generate_config   -- generate routing config
```

This is the *de facto* runtime. If we use ruvllm-wasm in-process, this is the
API surface we get.

### 1.5 The 11-pattern wall

From the live MCP schema for `ruvllm_hnsw_create`:

> "Max patterns capacity (limit ~11 in v2.0.1)"

This is a **hard, verified constraint** on ruvllm-wasm's HNSW router today.
Eleven patterns is enough for "Reasoning vs CodeGen vs Conversational vs
Analysis vs Creative" archetypes. It is **not** enough for `.claude/skills/`
(33 entries) or `.claude/agents/*` (60+ subdirectories). Anyone proposing
ruvllm-wasm's HnswRouter as the *primary* skill index is wrong on first
contact with the data shape.

### 1.6 What WeftOS already knows about HNSW routing

From `.planning/development_notes/adaptive-hnsw-tiered-search.md` (our own
crate, v0.6.13, `crates/clawft-core/src/embeddings/hnsw_store.rs`,
`crates/clawft-kernel/src/hnsw_eml.rs`):

- We already ship a **three-tier dimensional HNSW** (coarse 20-d → medium 40-d
  → fine 128-d) with EML-tuned `ef`/keep counts.
- Probe-and-triage uses tree calculus form classification (Atom / Sequence /
  Branch).
- On structured data: 1.61× faster, +10% recall vs. flat.
- On i.i.d. random data: catastrophic failure (recall 0.04). **Tiered HNSW
  requires structured embeddings.** This bites if we naively dump
  random-ish skill descriptions in.

This is the local pattern — we should reuse `clawft-kernel`'s
`hnsw_service.rs` rather than spinning up a parallel index.

### 1.7 What I have not verified

I did not `git clone` the upstream ruv repos in this session — `/tmp/ruv-research`
is empty and the planning instructions explicitly forbid `/tmp` for outputs.
The crate descriptions above are sourced from
`.planning/04-rvf-integration.md` and `.planning/05-ruvector-crates.md`,
which were themselves grounded against the upstream repos at write time
(2026-04-14 and earlier). Versions can drift; the `ruvector-*@2.x` line is
known stable per the 2026-04-14 ecosystem analysis. **Before adopting ruvllm
or sona, verify upstream API matches the planning docs — these crates are
pre-1.0 and the planning docs warn explicitly about API instability.**

---

## 2. Problem Shape: What the Context Router Must Do

### 2.1 Inputs

| Input | Source | Notes |
|-------|--------|-------|
| Latest user turn (text) | `ChatRequest.messages.last()` | Always present |
| Recent message history | `ChatRequest.messages` | Bounded by `TokenBudgetAssembler` |
| Recent tool-call sequence | Trajectory captured by agent loop | Optional in v1, valuable in v2 |
| Active workspace path | platform | Cheap signal; e.g. inside a Rust crate vs. a notes dir |
| User identity | `AuthContext.sender_id` | For per-user adaptation |
| Channel | `AuthContext.channel` | CLI vs Discord vs Telegram vs voice |
| Available skills | filesystem walk of `.clawft/skills/`, `.claude/skills/` | ~35 today, growing |
| Available agent profiles | `.claude/agents/<category>/<role>.md` | ~60+ today |
| Tool registry | `clawft-tools::ToolRegistry` | ~16 tools today |

### 2.2 Outputs

```rust
pub struct ContextRoutingDecision {
    /// Skills to load into the system prompt, by id, in priority order.
    pub skills: Vec<SkillSelection>,
    /// Optional agent profile to activate (null = base concierge).
    pub agent_profile: Option<String>,
    /// Tool ids the model should see this turn (subset of registry).
    pub tool_subset: ToolSubset,
    /// Suggested complexity boost ([-0.3, +0.3]) handed to TieredRouter.
    /// MUST be small and clamped — see §6.
    pub complexity_hint: f32,
    /// Confidence in the routing decision; below `fallback_threshold`
    /// the daemon falls back to the v1 LLM classifier.
    pub confidence: f32,
    /// Audit blob written to SOUL.journal.md and to RVF WITNESS segment.
    pub trace: ContextRoutingTrace,
}

pub struct SkillSelection {
    pub id: String,            // e.g. "claude-flow"
    pub source: SkillSource,   // Clawft | Claude
    pub score: f32,            // for explainability
}

pub enum ToolSubset {
    All,                       // pass through registry
    Allowlist(Vec<String>),    // only these tool ids
    Denylist(Vec<String>),     // all except these
}
```

The decision must be **cheap, explainable, and fallback-safe**. Three-figure
microsecond budget on the happy path; under ~50 ms total even with embedding
generation; never hard-fails (a degraded all-skills fallback always works).

### 2.3 Anti-goals

- **No new tier system.** The TieredRouter owns model-tier selection.
- **No re-classification of complexity.** Complexity stays in the
  `KeywordClassifier` (Level 0) / `ComplexityAnalyzer` (Level 1, ruvllm).
  Context router can hint, not override.
- **No skill execution.** The router selects; the agent loop executes.
- **No per-step routing.** One decision per chat turn, not per tool call.

---

## 3. Architecture Options

### 3.1 Option A — LLM classifier (the v1 baseline)

**Shape**: free-tier model receives a prompt listing all skills and agents,
emits structured JSON.

**Pros**

- Zero new infrastructure. Reuses `TieredRouter` free tier.
- Naturally handles novel phrasings (LLM generalization).
- Free to iterate — change the prompt, change the behavior.
- Bootstraps training data for later options (see §5).

**Cons**

- Adds an LLM round-trip to every chat turn (200-800 ms even on Groq free).
- Token cost: listing 35 skill descriptions + 60 agent descriptions inline is
  ~3-6 K input tokens per turn. Free tier ≠ free quota.
- Tail latency owned by an external provider.
- Cannot be cached effectively across users (turn text varies).
- Prompt-fragility: schema drift, JSON-mode regressions, refusals.

**Verdict**: correct v1. Wrong long-term steady state.

### 3.2 Option B — Embedding-classifier (HNSW retrieval, no training)

**Shape**:

```
turn text  --(embed)-->  query vec
                              |
                        DiskANN / kernel-HNSW
                              |
                        top-k skill descriptions
                              |
                  rules layer (sticky context, agent
                  profile mapping, tool allowlist)
                              |
                  ContextRoutingDecision
```

Embed each skill's `description + name + first paragraph` once at boot, store
in our existing `clawft-kernel` `HnswService`, query per turn.

**Pros**

- **Latency**: `clawft-kernel`'s tiered HNSW reports p99 ~97 µs at 5K
  vectors. Plus embedding (~10-100 ms local model, ~80-200 ms API).
- **No training data required.** Skill descriptions *are* the index.
- **Updates instantly**: drop a new skill on disk, re-index, done. No
  retraining, no preference data.
- **Fits our infra**: reuses `hnsw_service.rs` and `hnsw_store.rs`.
- **Explains itself**: cosine score per skill is the explanation.
- **Scales beyond 11 patterns** — DiskANN handles millions; ruvllm-wasm
  HnswRouter does not, so we don't use it for this.

**Cons**

- **Embedding cost.** Need an embedder. Three sub-options:
  1. **Local ONNX `all-MiniLM-L6-v2`** (~22 MB model, ~10 ms embed). Pulls in
     `ort` / `tract`. Native only.
  2. **API embedding** (`text-embedding-3-small` or Voyage): 80-200 ms,
     network-coupled, $.02/1M tokens.
  3. **Hash + character n-gram fallback** (`HashEmbedding` from ruvector-core
     idiom): zero cost, no semantic generalization. Useful only as a tie
     breaker.
- **Synonym / paraphrase weakness**. "fix this build error" vs. "the cargo
  step blew up" land in different parts of the embedding space unless the
  embedder is good. MiniLM is okay; hash-embedding is not.
- **Cold start**: first turn pays for the embedder load (~50-200 ms one-time).
- **Embedding pollution**: if descriptions all sound the same ("agent that
  helps you with X"), retrieval collapses. We need to pre-process skill
  descriptions to be diverse and informative.

**Verdict**: strongest v2 candidate. Cheapest to build, easiest to debug,
covers ~80% of the value.

### 3.3 Option C — Trained MicroLoRA classifier

**Shape**:

```
turn embedding (e.g. 384-d MiniLM)
        |
   MicroLoRA (rank 2, base = small frozen MLP)
        |
   logits over skill ids
        |
   softmax  -> top-k skills + confidence
```

ruvllm's MicroLoRA is rank 1-4, designed for instant adaptation
(<1 ms per `microlora_adapt` call per the MCP tool).

**Pros**

- Inference is sub-millisecond once the embedding is in hand.
- `ruvllm_microlora_adapt(loraId, quality)` is a one-call online learning
  step — every chat turn becomes a training signal.
- The *mechanism* matches our SONA narrative (per-request adaptation,
  background consolidation).
- Compatible with the existing `LearningBackend` trait.

**Cons**

- **Needs labels.** A classifier needs (turn_embedding, skill_id) pairs. We
  do not have them. The honest source is "log the v1 LLM classifier's
  decisions and treat them as gold". That works (see §5) but inherits the
  v1's biases and only converges to ~v1 quality.
- **Cold start is brutal.** Until you have hundreds of labels for each skill,
  the classifier is worse than retrieval.
- **EWC++ is necessary** the moment you have >5 skills, otherwise newly added
  skills overwrite earlier ones in the LoRA weights. Adds complexity.
- **Calibration is hard.** Softmax confidence is not real confidence; we'd
  need temperature scaling at minimum to use it as a fallback gate.
- **Skill set churn.** Output dim = number of skills. Adding a skill changes
  the model shape unless we use a fixed-K head with empty slots. ruvllm-wasm
  MicroLoRA requires fixed `outputDim` at create time.

**Verdict**: premature optimization unless we already have working retrieval
and substantial logged data. Reserve for v2.5/v3.

### 3.4 Option D — SONA pattern-router

**Shape**: SONA's pattern store maps `query_embedding → known_pattern_id`
with online instant adaptation (`ruvllm_sona_create(hiddenDim=64,
patternCapacity=N)`).

**Pros**

- Ships *bundled* the right primitives: capacity-bounded pattern memory,
  quality-feedback adaptation, decay.
- The semantics — "this turn looked like that turn, which used these skills,
  and got quality 0.86" — match what we want exactly.
- Fits our verbal architecture story (the Concierge "remembers good moves").

**Cons**

- Same calibration / cold-start problems as Option C.
- `patternCapacity` is finite; old patterns get evicted under pressure (this
  is by design — but it means the router can forget).
- We have not run sona in production; the planning doc explicitly flags
  "sona's learning quality unproven at clawft's scale".
- Pattern store is a black box without our HNSW's introspection / explain
  tooling.

**Verdict**: SONA's natural place is not "the router" — it is "the
adaptation loop *behind* the retrieval router." Treat it as a re-ranker /
preference signal, not the index.

### 3.5 Option E — Hybrid (recommended for v2)

```
                       turn text
                          |
                   [ Embedder ]
                          |
              turn_embedding (e.g. 384-d)
                          |
            +-------------+--------------+
            |                            |
     [ DiskANN/HNSW  ]            [ Sticky-context  ]
     skill descriptions             rules engine
            |                            |
     top-k skills + scores         conversation flags
            |                            |
            +-------------+--------------+
                          |
              [ Reranker (cheap) ]
              · score boost from SOUL preference
              · score boost from sticky context
              · clip if below floor
                          |
              [ MicroLoRA / SONA layer ] (optional, gated)
              · only active once N labels collected
              · refines top-k order
                          |
                ContextRoutingDecision
                          |
                  [ confidence gate ]
                  if confidence < τ_fallback
                       → call v1 LLM classifier
                       → store its output as new label
                          |
                          v
                    final decision
```

**Pros**

- **Always-improving floor.** Even on day 0 with no SONA training, retrieval
  works.
- **SONA layer is *additive***. Toggle off with a feature flag and we degrade
  to plain retrieval, not chaos.
- **LLM fallback is the safety net** that doubles as a label generator. The
  router converges to "rarely fall back" as preference data accumulates.
- **Composable with TieredRouter**: complexity hint is small, explainable,
  bounded.

**Cons**

- More moving parts. Three knobs (retrieval k, rerank weight, fallback
  threshold) that interact.
- Requires monitoring to know when it's working vs. silently degrading.

**Verdict**: the right v2.

---

## 4. Data Shape

### 4.1 What the router sees per turn

```rust
pub struct RoutingFeatures<'a> {
    /// The latest user turn text.
    pub user_turn: &'a str,
    /// Last N messages, default N=4. Used only for sticky-context heuristics
    /// in v2; the LLM classifier sees the full history.
    pub recent_history: &'a [LlmMessage],
    /// Last K tool-call ids in this session, default K=8. Empty on first turn.
    pub recent_tools: &'a [String],
    /// Channel ("cli", "discord", "telegram", "voice", ...).
    pub channel: &'a str,
    /// Sender id (opaque). Used for per-user SONA bucket.
    pub sender_id: &'a str,
    /// Workspace heuristics (Cargo.toml present? .git? notes/ dir?).
    pub workspace_hints: WorkspaceHints,
    /// Cached embedding of the last 1-2 turns, if available, for delta-boost
    /// (sticky context — "we were just talking about X").
    pub prior_turn_embedding: Option<&'a [f32]>,
}
```

### 4.2 What gets emitted

Already shown in §2.2 (`ContextRoutingDecision`). Two things worth calling
out:

- `confidence` is in `[0, 1]`. Below `τ_fallback` (default `0.55`) the
  daemon calls the v1 LLM classifier. The threshold is a config knob in
  `.clawft/config.json`.
- `complexity_hint` is in `[-0.3, +0.3]` and gets *added* to the
  classifier's complexity score before the TieredRouter sees it. Hard-clamp
  in code; do not allow ±1.0 hints.

### 4.3 Skill descriptor shape

This is the corpus indexed by the embedding-retrieval router. Each skill
becomes one indexed record:

```yaml
id: claude-flow
source: clawft        # or "claude"
title: "Claude-Flow Coordination"
description: |
  Multi-agent swarm coordination via claude-flow CLI; spawn, hive-mind,
  memory store/search, hooks, session management.
trigger_phrases:
  - "spawn agents"
  - "swarm"
  - "hive mind"
exemplar_turns:
  - "kick off a swarm to refactor the pipeline crate"
  - "search memory for authentication patterns"
tool_hint: ["delegate_tool", "shell_tool"]
```

Index text = `title + "\n" + description + "\n" + trigger_phrases + "\n" +
exemplar_turns`. The exemplars do most of the retrieval work — descriptions
alone collapse in embedding space.

`exemplar_turns` are written by hand for the first ~10 skills, then mined
from chat logs once we have them (see §5).

---

## 5. Training-Data Path

The honest order of operations:

1. **Day 0 — synthetic exemplars only.** For each skill, hand-author 3-5
   exemplar turns. ~100-200 lines of YAML across all skills. This is what
   the embedder sees on day 0. No training needed.

2. **Days 1-30 — log v1 classifier.** Every time the LLM classifier runs,
   write the `(turn_embedding, decision, post-hoc_quality_signal)` to a
   `routing-trace.rvf` file (POLICY_KERNEL segment style). This is the
   bootstrap label corpus for any trained model. Be honest: these labels
   are biased toward whatever the v1 prompt said.

3. **Days 30+ — implicit feedback mining.** Two cheap signals:
   - **Tool match**: did the chosen skill's `tool_hint` overlap with the
     tools the agent actually invoked? Match → +1, mismatch → -1.
   - **User correction**: did the user reply with "no, I meant…" / "wrong
     skill" / "use X instead"? Trivial regex catches the majority.
   These become the quality signal fed to `sona_adapt` /
   `microlora_adapt`.

4. **Months 2+ — explicit feedback (optional).** Lightweight thumbs-up /
   thumbs-down on the assistant turn. Only valuable if surface area exists
   for it (egui panel, Discord reactions, voice "no, that was wrong").
   Defer until we know we want it.

**Reality check**: option 4 rarely materializes. Plan around 1-3.

5. **`SOUL.journal.md` mining.** The chat agent appends self-observations to
   `.clawft/SOUL.journal.md`. If a journal entry says "I should have used
   `claude-flow` skill instead of `claude-code` for that swarm question",
   that's a labeled correction. Parse those entries weekly into preference
   pairs `(turn_embedding, preferred_skill_id, dispreferred_skill_id)`. See
   §7.

---

## 6. Integration with `TieredRouter`

The two routers must not duel. Concrete contract:

| Stage | Owner | Input | Output |
|-------|-------|-------|--------|
| Context routing | `ContextRouter` (new) | `RoutingFeatures` | `ContextRoutingDecision` |
| Task classification | `KeywordClassifier` / `ruvllm::ComplexityAnalyzer` | `ChatRequest` | `TaskProfile { complexity, task_type, keywords }` |
| Model-tier routing | `TieredRouter` (existing) | `ChatRequest`, `TaskProfile`, `AuthContext` | `RoutingDecision { provider, model, tier }` |

Order: **Context → Classifier → TieredRouter**. The context router runs
first because it shapes the system prompt and tool list; the classifier
analyzes the *resulting* request; the tiered router picks the model.

The `complexity_hint` flows like this:

```
let mut profile = classifier.classify(&request);
profile.complexity = (profile.complexity + ctx_decision.complexity_hint)
    .clamp(0.0, 1.0);
let routing = tiered_router.route(&request, &profile).await;
```

`complexity_hint` is bounded `[-0.3, +0.3]` so the context router can
*nudge* tier selection but never single-handedly cross a tier boundary.
Skills that obviously need premium-tier reasoning ("design a CRDT") add
+0.2; chitchat skills add 0.0; nothing adds more than +0.3.

**Risk: dueling routers.** Mitigation:

- Context router cannot pick the model. It cannot even *suggest* a model.
- It can hint complexity within a clamped range.
- It can hint at tools, which TieredRouter does not touch.
- All hints are **logged in the trace** — if quality drops after a context
  router rollout, we can diff the logs against TieredRouter's pre-hint
  decisions. See §10 for monitoring.

The existing `ChatRequest.complexity_boost` field (visible in
`crates/clawft-core/src/pipeline/assembler.rs` test code at line 170) is
the actual carrier — we already added the field for exactly this kind of
hint. Reuse it.

---

## 7. Self-Update Path: SOUL Journal as Preference Signal

The chat agent already writes `.clawft/SOUL.journal.md`. We can mine it for
two distinct training signals:

### 7.1 Explicit corrections

Pattern: `"should have used X instead of Y"` / `"wrong skill"` / `"better
choice would have been"`. A regex + a short LLM judge on candidates
produces preference pairs:

```
(turn_embedding, preferred_skill, dispreferred_skill, weight=1.0)
```

These feed `sona_adapt` (boost preferred) and a separate dispreference
penalty applied to the LoRA logits. Ship behind a feature flag — start
read-only ("count the corrections") before any model update.

### 7.2 Implicit drift

If the router systematically picks skill X but SOUL entries express dissatis-
faction in the same window, that's a drift signal. Don't try to fix it
automatically in v2 — emit a `routing.drift` event, surface it in `weft
status`, let a human inspect. Auto-correction loops on top of drift
detection are a v3 problem.

### 7.3 Closed loop sketch

```
chat turn -> ContextRoutingDecision  -- WITNESS append (turn_id, skills)
                       |
                    agent loop
                       |
                  outcome + tools
                       |
            implicit signals (§5.3)
                       |
        SOUL.journal append (turn_id, observations)
                       |
            weekly mining job (cron / `weft soul mine`)
                       |
            (turn_emb, preferred_skill, dispreferred_skill) pairs
                       |
            applied via sona_adapt / microlora_adapt
                       |
            new router weights persisted to .rvf  (POLICY_KERNEL segment)
                       |
            daemon hot-swap on next tick (§8)
```

**Honest caveat**: closed loops without humans-in-the-loop go bad. Audit
every adapt call (`WITNESS` segment with the labeled pair and resulting
weight delta). Make it easy to roll back a week. Don't let online learning
touch production weights without a shadow-mode evaluation phase.

---

## 8. Concrete v2 Implementation Outline

### 8.1 File layout

```
crates/clawft-core/src/pipeline/
  context_router/
    mod.rs                 -- ContextRouter trait + ContextRoutingDecision
    null_router.rs         -- "all skills, all tools" baseline (always works)
    llm_router.rs          -- v1: small LLM classifier (free tier)
    embedding_router.rs    -- v2: embed + HNSW retrieval (Option B)
    hybrid_router.rs       -- v2: retrieval + sona/microlora rerank (Option E)
    skill_corpus.rs        -- builds the indexed corpus from .clawft/.claude
    rules.rs               -- sticky-context, channel rules, conv flags
    fallback.rs            -- confidence-gated fallback dispatch
    trace.rs               -- ContextRoutingTrace + WITNESS append helpers

crates/clawft-core/src/embeddings/
  embedder.rs              -- Embedder trait (already in tree, extend)
  embedder_minilm.rs       -- ONNX MiniLM impl (feature = "local-embed")
  embedder_api.rs          -- API embedding impl (already exists for memory)
  embedder_hash.rs         -- HashEmbedding fallback

crates/clawft-weave/src/
  daemon.rs                -- new RPC handle_agent_chat()
                              uses ContextRouter then TieredRouter

.clawft/
  models/
    router.rvf             -- skill index + (optional) MicroLoRA weights
                              segments: VEC | INDEX | POLICY_KERNEL | META
    embedder.onnx          -- optional, pulled on first run
  config.json              -- routing.context section (see §8.4)
  SOUL.journal.md          -- already exists
```

### 8.2 Trait shape

```rust
#[async_trait]
pub trait ContextRouter: Send + Sync {
    /// Pick skills, agent, tool subset, and a complexity hint.
    /// Must be cheap on the happy path (<10ms excluding embed).
    async fn route(
        &self,
        features: RoutingFeatures<'_>,
    ) -> Result<ContextRoutingDecision>;

    /// Record outcome for online learning. Optional — most impls no-op.
    fn observe(&self, turn_id: &str, outcome: &TurnOutcome) {}

    /// Hot-reload skill corpus or model weights. Returns a version tag.
    /// Called by the daemon when .clawft/skills/ changes (notify watcher)
    /// or when .clawft/models/router.rvf is updated atomically.
    async fn reload(&self) -> Result<RouterVersion>;
}

pub struct TurnOutcome {
    pub chosen_skills: Vec<String>,
    pub tools_actually_used: Vec<String>,
    pub user_correction: Option<UserCorrection>,
    pub quality_score: Option<f32>,
}
```

`Arc<dyn ContextRouter>` lives on the daemon and is selected at boot from
`config.routing.context.kind`:

```
kind = "off"        -- NullRouter (default)  -> no behavior change
kind = "llm"        -- v1 LlmRouter (free tier)
kind = "embedding"  -- v2 EmbeddingRouter (Option B)
kind = "hybrid"     -- v2 HybridRouter (Option E, recommended)
```

### 8.3 Daemon wiring

In `crates/clawft-weave/src/daemon.rs`:

```rust
async fn handle_agent_chat(
    params: AgentChatParams,
    state: Arc<DaemonState>,
) -> Result<AgentChatResult> {
    let features = RoutingFeatures::from_request(&params, &state)?;

    let ctx = state.context_router.route(features).await
        .unwrap_or_else(|e| {
            warn!(?e, "context router error; falling back to NullRouter decision");
            ContextRoutingDecision::all_skills()
        });

    state.witness.append(WitnessEntry::context_routing(&ctx)).await?;

    let request = build_chat_request(&params, &ctx);

    let mut profile = state.classifier.classify(&request);
    profile.complexity = (profile.complexity + ctx.complexity_hint).clamp(0.0, 1.0);

    let decision = state.tiered_router.route(&request, &profile).await;

    // existing pipeline.complete(...) path
    let result = state.pipeline.complete(request, decision, &profile).await?;

    let outcome = TurnOutcome::from(&result, &ctx);
    state.context_router.observe(&result.turn_id, &outcome);

    Ok(result.into())
}
```

### 8.4 Config

```jsonc
{
  "routing": {
    "mode": "tiered",   // existing TieredRouter
    "context": {
      "kind": "hybrid",
      "embedder": "minilm-onnx",     // or "api" or "hash"
      "skills_dirs": [".clawft/skills", ".claude/skills"],
      "agents_dirs": [".claude/agents"],
      "top_k_skills": 3,
      "fallback_threshold": 0.55,
      "complexity_hint_clamp": 0.30,
      "sona": {
        "enabled": false,             // default off; flip on after warmup
        "hidden_dim": 64,
        "pattern_capacity": 256,
        "learning_rate": 0.01,
        "shadow_mode_until_pairs": 200
      },
      "microlora": {
        "enabled": false,             // default off
        "rank": 2,
        "alpha": 1.0
      },
      "model_path": ".clawft/models/router.rvf"
    }
  }
}
```

### 8.5 Hot swap

`.clawft/models/router.rvf` is replaced atomically (`tmp + rename`). The
daemon has a `notify`-based watcher (already used elsewhere in the codebase)
that calls `context_router.reload()` on change. Reload acquires a write
lock on the inner `ArcSwap<RouterState>`; in-flight requests using the
previous state finish on the previous state. No restart required. Roll-
back is `mv router.prev.rvf router.rvf`.

---

## 9. Recommendation Stack

| Phase | Router | Owns | Cost | When |
|-------|--------|------|------|------|
| v0 | `NullRouter` | nothing — passes all skills | $0, 0 ms | now |
| v1 | `LlmRouter` (free-tier classifier) | per-turn LLM call | ~$0.0002, 200-800 ms | next sprint |
| v2 | `EmbeddingRouter` (Option B) | embed + HNSW retrieval over skill corpus | ~$0 native, 10-100 ms | after v1 has 30 days of logs |
| v2.5 | `HybridRouter` (Option E) | + SONA pattern store, + LLM fallback | ~$0 (fallback rate < 10%), 10-100 ms | when shadow mode shows ≥10% accuracy lift over v2 |
| v3 | + MicroLoRA reranker | trained on logged labels + SOUL preference pairs | ~0 ms inference, weekly retrain | when v2.5 plateaus |

**Hard rule**: do not skip a step. Each stage exists to validate the next
stage's value and to provide labels.

---

## 10. Risks & Open Questions

### 10.1 Where this likely breaks

| Risk | Likelihood | Severity | Mitigation |
|------|-----------|----------|-----------|
| Skill descriptions are too generic; embedding retrieval collapses | High | High | Mandatory `exemplar_turns` field; LLM-judge any new skill's exemplars at PR time |
| Embedder is slow / unavailable, killing turn latency | Medium | High | Hash-embedding fallback always available; embedder load is one-time at boot |
| Tiered HNSW catastrophic-fail on near-uniform skill embeddings (recall 0.04) | Medium | Medium | Use flat HNSW or DiskANN until corpus >200 skills; tiered HNSW only after probe says "Branch" form (per `adaptive-hnsw-tiered-search.md`) |
| ruvllm-wasm 11-pattern HNSW cap mistakenly used as primary index | Already considered, just noting | Critical if missed | Use DiskANN / kernel-HNSW for skill index; ruvllm-wasm only for archetype routing |
| MicroLoRA forgets old skills when new ones are added | High if rolled too early | High | EWC++ from sona; freeze old slots; prefer fixed-K head |
| SONA / MicroLoRA online learning silently degrades | High | Critical | Mandatory shadow-mode period (`shadow_mode_until_pairs`); WITNESS audit; one-week rollback ready |
| Dueling routers (context boosts complexity, tiered router escalates, cost spike) | Medium | High | Hard clamp `complexity_hint ∈ [-0.3, +0.3]`; alert if hint causes a tier boundary crossing on >5% of turns |
| `.claude/skills/` and `.claude/agents/` ship descriptions optimized for Claude.app, not for retrieval | High | Medium | Cache an *augmented* descriptor per skill in `.clawft/derived/`; don't index raw upstream files |
| SOUL.journal mining loop becomes an unsupervised auto-edit of the router | Inevitable if not designed against | High | All adapt calls go through `WitnessLog`; no closed loop to production weights without human approve in v2 |
| 33+ skills means 33+ vectors in HNSW (fine) but 60+ agent profiles means many more (still fine) — but cross-product (skill × agent × tool subset) explodes if we ever try to enumerate it | Low | Medium | Three independent decisions, not a joint decision. Skills first; agent profile from a small map; tool subset from selected skills' `tool_hint`. |

### 10.2 What's premature optimization

- **Trained MicroLoRA classifier in v2.** Without thousands of logged labels,
  it will be worse than retrieval. Defer.
- **Cross-domain transfer (`ruvector-domain-expansion`).** Cute, no
  near-term return. Defer.
- **Hyperbolic / Poincaré-ball embeddings** for the skill index. Skills are
  ~flat. The hierarchy claim doesn't pay rent at this scale.
- **Quantum or graph-attention rerankers.** Not even close.
- **Putting the router in WASM** (`micro-hnsw-wasm`). The router lives in
  the daemon, which is native. WASM router is a Phase-3 daemon-in-WASI
  problem.

### 10.3 What's the smallest v2 that earns its keep

The minimum that beats v1:

1. Embedding retrieval over hand-authored skill descriptors (Option B).
2. ONNX `all-MiniLM-L6-v2` as default embedder (or API if ONNX unavailable).
3. Use `clawft-kernel`'s existing `HnswService` (flat HNSW for ≤200 vecs).
4. Fallback to v1 LLM classifier when top-1 cosine score < `0.45`.
5. Log every decision to `routing-trace.rvf` (WITNESS segment).
6. **No SONA. No MicroLoRA.** Both off until we have logs.

This earns its keep when:

- p99 routing latency < `LLM-classifier p99 / 5` (target 5× faster than v1).
- Fallback rate stabilizes < 25% within 30 days.
- Tool-match implicit feedback (chosen skill's `tool_hint` ∩ tools used) is
  ≥ 0.6 averaged over a week.

Hit those three, ship Hybrid as v2.5. Miss any one, fix retrieval before
adding adaptation.

### 10.4 Open questions for the review panel

1. **Embedder choice.** Local ONNX MiniLM (one-time 22 MB download, no
   network) vs. API (~80-200 ms / call, costs money, depends on provider).
   I lean ONNX for the default; API for users who already have the key.
   **Open**: who pays the binary-size cost of ONNX runtime?
2. **Per-user routing buckets.** Should the router personalize by `sender_id`
   (each user gets a SONA bucket) or be global? Personalization is a
   privacy and storage question, not just an algorithmic one.
3. **Skill descriptor authority.** Who owns the augmented descriptors for
   `.claude/skills/`? They are upstream-controlled; we can't edit them in
   place. Proposal: a `.clawft/derived/skill-overrides/<id>.yaml` layer that
   merges over upstream.
4. **Agent-profile selection.** Is the agent profile chosen by skill (each
   skill names a default profile) or by an independent classifier? Skill →
   profile is simpler and probably correct. **Open** — confirm with the
   panel.
5. **Where does the RVF schema live?** The `router.rvf` segments are not
   yet defined. Propose: extend `rvf-types`'s segment registry with
   `CTX_SKILL_INDEX` (`0x40`), `CTX_LORA_HEAD` (`0x41`),
   `CTX_PREF_PAIRS` (`0x42`). Coordinate with upstream before claiming codes.
6. **Thread through `agent.chat` how?** The new RPC sits next to existing
   `agent.spawn` / `agent.send`. Does it own its own session, or does it
   reuse the agent loop's session? Proposal: own a transient session,
   delegate to `run_tool_loop` per turn — no new session machinery.

---

## 11. Action Items After Panel Review

Listed for traceability; no implementation lives in this doc.

- [ ] Confirm ruvllm-wasm 11-pattern cap with upstream; record the version
      where this is fixed (if any) so we can re-evaluate ruvllm-wasm's
      HnswRouter for archetype routing.
- [ ] Verify `sona`, `ruvllm`, `micro-hnsw-wasm` upstream API matches the
      planning docs; pin exact versions before any adoption.
- [ ] Coordinate RVF segment codes (`CTX_*`) with the upstream `rvf-types`
      registry.
- [ ] Author skill descriptors (YAML) for the existing 35 skills and 60+
      agents. This is hand-authored grunt work; no shortcut.
- [ ] Add `routing.context` config section to `clawft-types::config`.
- [ ] Decide ONNX vs API embedder default (open question §10.4 #1).
- [ ] Implement v1 `LlmRouter` first (it generates training data); ship to
      production as the gate for v2.
- [ ] Add `weft routing trace` and `weft routing replay` CLI commands —
      without these, we cannot debug the router in production.
- [ ] Set p99 latency, fallback rate, and tool-match metrics in `weft
      status`; set alert thresholds before turning on Hybrid.

---

## Appendix A — Quick Reference: Per-Turn Latency Budget (v2 Hybrid, target)

| Stage | Target | Worst case | Notes |
|-------|--------|-----------|-------|
| Build `RoutingFeatures` | 50 µs | 200 µs | mostly hash lookups |
| Embed user turn | 10 ms (ONNX) / 150 ms (API) | 50 ms / 800 ms | one network call worst case |
| HNSW retrieval (k=3, ≤200 vecs) | 100 µs | 1 ms | flat HNSW; tiered if corpus grows |
| Sticky/rules layer | 50 µs | 200 µs | hashmap lookups |
| Optional SONA rerank | 1 ms | 3 ms | per ruvllm-wasm spec |
| Confidence gate | 5 µs | 10 µs | comparison |
| WITNESS append | 200 µs | 5 ms (fsync) | append-only |
| **Total (happy path, ONNX)** | **~12 ms** | **~60 ms** | |
| **Total (LLM fallback)** | adds 200-800 ms | adds 2 s | only on low-confidence turns |

Compared to v1 (always-LLM): expected median latency improvement is ~20-50×;
expected token cost improvement is ~95% (only fallback turns pay LLM cost).

---

## Appendix B — Why Not Just Use ruvllm-wasm's HnswRouter Directly

For completeness — this is the option a reasonable person would propose
first, and it is wrong, in this specific way:

- ruvllm-wasm v2.0.1 caps HNSW at ~11 patterns. We have 35+ skills today.
- ruvllm-wasm is tuned for *archetype* routing (Reasoning / CodeGen /
  Analysis), not skill indexing.
- We already have a higher-quality, native HNSW (`clawft-kernel`,
  DiskANN-2.1) with SIMD distance and 1.0 recall on production data.

Use ruvllm-wasm where it shines: 5-7 stable archetypes, online adaptation,
WASM-portable. Specifically: as the **task_type prior** that flows into
`TaskProfile.task_type`, not as the skill index. That's a smaller, future
change that does not block this work.
