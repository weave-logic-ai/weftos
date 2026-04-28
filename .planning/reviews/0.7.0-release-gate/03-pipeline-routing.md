---
title: "Pipeline & Routing"
slug: pipeline-routing
workstream_id: "03"
status: shipped-with-deferrals
period: "2026-02-13 → 2026-04-28"
versions_landed:
  - 0.6.8 (eml-attention iter 0, experimental)
  - 0.6.9 (eml-attention iter 1)
  - 0.6.10 (eml-attention iter 2 / SafeTree)
  - 0.6.11 (LlmClassifierRouter / E1)
  - 0.6.12 (EmbeddingRouter / E2)
  - 0.6.13 (HybridRouter plumbing / E3)
  - 0.6.14 (RetryModel wired into RetryPolicy)
  - 0.6.15 (gemini provider; multi-provider bug fix; xAI)
  - 0.6.16-0.6.18 (D1-D11 pipeline reliability)
  - 0.6.19 (clawft-service-llm + tool-call wire format + null-content deserializer)
  - 0.6.20 (TieredRouter / sprint 01-tiered-router)
related_plans:
  - .planning/sparc/phase4/01-tiered-router/
  - .planning/sparc/phase4/05-pipeline-reliability/
  - .planning/development_notes/01-tiered-router/
  - .planning/development_notes/05-pipeline-reliability/
  - .planning/development_notes/eml_model_development.md
  - .planning/development_notes/eml_model_development_assessment.md
  - .planning/development_notes/eml-causal-collapse-research.md
  - .planning/development_notes/eml-synergy-scan.md
  - .planning/development_notes/hnsw-eml-analysis.md
  - .planning/development_notes/hnsw-eml-deep-analysis.md
  - .planning/development_notes/ruview-eml-contributions.md
  - docs/plans/agent-core-v1.md (Phase E1/E2/E3)
  - docs/plans/chat-agent-v1.md (§16 v0→v3 router roadmap)
  - docs/research/rvf-context-router.md
related_adrs:
  - ADR-017 (GEPA Prompt Evolution)
  - ADR-018 (Hermes Models as clawft-llm Provider)
  - ADR-019 (Registry Trait in clawft-types)
  - ADR-045 (Tiered Router with Permission-Based Model Selection)
sprint_refs:
  - 01-tiered-router (Phases A through I, 9 SPARC plans)
  - 05-pipeline-reliability (D1-D11, 4 sub-phases)
  - sprint-16/eml-coherence
  - sprint-17 (12 EML wrappers)
  - agent-core-v1 (Phase E1/E2/E3 router phasing)
completion_pct: 78
open_task_count: 23
risk: medium
last_updated: 2026-04-28
---

# Pipeline & Routing

## General Description

This workstream covers the agent pipeline (`crates/clawft-core/src/pipeline/`),
the LLM provider layer (`crates/clawft-llm/`), the daemon-local LLM service
(`crates/clawft-service-llm/`), the EML learning substrate (`crates/eml-core/`),
and the pre-LLM `ContextRouter` family
(`crates/clawft-core/src/agent/context_router/`).

The pipeline is a 6-stage chain:

```
Classifier → ModelRouter → Assembler → Transport → Scorer → Learner
```

defined in `pipeline/traits.rs`. Stage 5 (Scorer) and Stage 6 (Learner) are
GEPA-inspired (ADR-017): Level 0 ships `NoopScorer`/`NoopLearner`; Level 1
ships `FitnessScorer` + `TrajectoryLearner`; the genetic-mutation production
loop is still the design target, not yet wired end-to-end.

The model router has two implementations selected via `routing.mode`:
- `StaticRouter` (Level 0, `pipeline/router.rs`): always returns the configured
  default `provider/model`, ignores complexity, permissions, and budget.
- `TieredRouter` (Level 1, `pipeline/tiered_router.rs`): the 1,650-line ADR-045
  implementation. Composes 4 named tiers (`free`/`standard`/`premium`/`elite`),
  a `PermissionResolver` with 5-layer merge (built-in → global → workspace →
  per-user → per-channel; CONS-007 made channel highest priority), a
  `CostTracker` with atomic `reserve_budget()` (FIX-07), a sliding-window
  `RateLimiter`, and 4 selection strategies (`PreferenceOrder`, `RoundRobin`,
  `LowestCost`, `Random`).

Independently, the **pre-LLM `ContextRouter`** lives in
`crates/clawft-core/src/agent/context_router/` and is the chat-agent-v1 v0→v3
roadmap. This is *not* the same router as `ModelRouter`: it never picks a model
and never escalates a tier (per `docs/research/rvf-context-router.md`); it
emits a `ContextDecision { skills, archetype, tool_subset, complexity_hint }`
that is consumed before the LLM call. The catalog:

| Version | Type | Status |
|---|---|---|
| v0 | `NullRouter` | shipped (default) |
| v1 | `LlmClassifierRouter` | shipped 0.6.11 (E1) |
| v2 | `EmbeddingRouter` (ruvector-diskann@2.1, feature-gated `embedding-router`) | shipped 0.6.12 (E2) |
| v2.5 | `HybridRouter` (plumbing only — primary + fallback chain) | shipped 0.6.13 (E3); rerank deferred |
| v3 | `MicroLoraRouter` | **deferred** — gated on `ruvllm-wasm` lifting 11-pattern HNSW cap |

LLM transport has two parallel paths:
- `OpenAiCompatTransport` (`pipeline/transport.rs` + `pipeline/llm_adapter.rs`):
  the production path, wraps `clawft-llm::OpenAiCompatProvider` with
  `RetryPolicy` and (for tiered mode) per-provider adapter dispatch.
- `ServiceLlmAdapter` (`pipeline/service_llm_adapter.rs`, native-only): bridges
  `clawft-service-llm::LlmClient` (the daemon's narrow HTTP client to a single
  llama-server / OpenRouter endpoint) into the same `LlmProvider` trait.
  `clawft-service-llm` was deliberately split out of `clawft-llm` to keep the
  daemon's "POST one prompt to localhost" path free of the browser-targeted
  provider abstraction.

EML score-fusion in the pipeline is **not yet implemented**. The only EML
production wiring in this workstream is `clawft-llm/src/eml_retry.rs`
(`RetryModel`: 3-input EML model learning retry delays from
`(error_ordinal, attempt, hour_of_day)`), which lands the dependency edge but
not the score-fusion path described in
`.planning/development_notes/eml-synergy-scan.md`. The toy EML attention work
(`eml-core::attention::ToyEmlAttention`) is shipped as **experimental** behind
the `experimental-attention` feature gate.

## Status & Timeline

| Sprint / Phase | Window | Status | Notes |
|---|---|---|---|
| 05-pipeline-reliability D1-D11 | Weeks 2-5 (sprint phase4) | **Done** 2026-02-20 | All 11 items done, 2,407 tests passing |
| 01-tiered-router A→I + security review | 2026-02-18 → 2026-02-20 | **Done** | 12,730 lines of spec, 235+ tests planned, 12 fix batches applied |
| EML iteration 0/1/2 | 0.6.8 → 0.6.10 | **Experimental** | G1 identity gate deferred to Iteration 3 |
| Sprint 16 EML coherence | 2026-04 (kernel-side) | **Partial** | Two-tier cadence "not yet wired" per `sprint-16/eml-coherence.md:54` |
| chat-agent-v1 §16 router phasing E1→E3 | 2026-04 | **Done plumbing** | v3 MicroLoRA deferred; v2.5 rerank deferred |
| Element 09 / FlowDelegator | (downstream) | Unblocked by D6+D9 | not in this workstream |
| GEPA loop (ADR-017) end-to-end | — | **Not wired** | `mutation.rs` exists; `evolution_ready` flag emitted but no production deploy gate |
| ContextRouter v3 (`MicroLoraRouter`) | — | **Deferred upstream** | Blocked by ruvllm-wasm 11-pattern HNSW cap |

**This-session commit `8b05d868`** (`fix(service-llm): accept null content on
tool-call turns`) sits at the head of `clawft-service-llm/src/client.rs` and
patches a Nemotron-via-OpenRouter wire-format incompatibility. It is included
in 0.6.19 / development-0.7.0 and is the most recent change in scope.

## Released Features

The following items are shipped and verified:

**Routing core**
- `RoutingConfig` / `ModelTierConfig` / `PermissionsConfig` / `EscalationConfig`
  / `CostBudgetConfig` / `RateLimitConfig` / `UserPermissions` / `AuthContext`
  / `TierSelectionStrategy` (typed enum, not `Option<String>`) — all in
  `crates/clawft-types/src/routing.rs` per FIX-01 / CONS-001.
- `TieredRouter` with the 5-layer permission merge (`PermissionResolver` in
  `pipeline/permissions.rs`), 4 selection strategies, escalation chain, and
  fallback model
  (`crates/clawft-core/src/pipeline/tiered_router.rs`).
- `CostTracker` with atomic `reserve_budget()` (FIX-07 TOCTOU mitigation,
  `cost_tracker.rs:156-285`), per-user daily/monthly buckets, persistence
  (mode 0600 on Unix, FIX-12), and budget reconciliation between estimated
  and actual spend.
- Sliding-window `RateLimiter` keyed by `sender_id` with optional global
  aggregate cap (FIX-08, `rate_limiter.rs`).
- `validate_workspace_ceiling()` enforcing FIX-04 ceiling rules (workspace
  configs cannot grant level > 1, increase budgets, increase rate limits, or
  add tools beyond global allowlist), `crates/clawft-core/src/routing_validation.rs:506`.
- D6 `sender_id` end-to-end propagation: `InboundMessage` → `ChatRequest` →
  `RoutingDecision` → `CostTracker.update()` (verified by integration test).

**Pre-LLM context routing (agent-core-v1 Phase E)**
- `NullRouter` (v0, default); `LlmClassifierRouter` (v1, E1, 0.6.11);
  `EmbeddingRouter` (v2, E2, 0.6.12, behind `embedding-router` feature using
  `ruvector-diskann@2.1`); `HybridRouter` (v2.5 plumbing, E3, 0.6.13).
- Daemon attaches the configured router via `Config.routing.context_router`
  (`crates/clawft-core/src/bootstrap.rs:687-692`).

**Transport / providers**
- `OpenAiCompatProvider` (clawft-llm), `ProviderRouter::strip_prefix()` for
  `provider/model` routing, multi-provider adapter map for tiered mode
  (`pipeline/llm_adapter.rs:438-477`), `LocalProvider`, `FailoverChain`,
  `RetryPolicy<P>` with structured `ProviderError` (D3) and configurable
  `RetryConfig` (D4).
- `RetryModel` (eml-core-backed): learns optimal retry delay from
  `(error_ordinal, attempt, hour_of_day)`, wired into `RetryPolicy::with_model()`
  (commit 97b5857f).
- `clawft-service-llm` (daemon's narrow HTTP client): tool-call wire format,
  `complete_with_tools`, env-driven OpenRouter takeover via
  `OPENROUTER_API_KEY`, `LLM_MODEL`, `LLM_SERVICE_URL`, single-permit
  `Semaphore` to match llama-server's single-batch model.
- `service_llm_adapter.rs`: `ServiceLlmAdapter` bridges `LlmClient` into
  `LlmProvider`. The agent loop can route via the daemon's RPC instead of the
  general clawft-llm provider.
- D2 streaming failover correctness; D7 `StreamCallback` accepts `FnMut`;
  D8 bounded message bus; D9 MCP transport request-ID multiplexing.

**Provider catalog** (clawft-llm): 11 OpenAI-compatible providers (openai,
anthropic, groq, deepseek, openrouter, gemini, xai, mistral, together,
hermes/local, plus the `local-provider` shim).

**EML substrate**
- `eml-core` standalone crate (36 tests, zero WeftOS deps, e116d6b4).
- 12 EML self-learning models across the WeftOS stack (690a10d1).
- Sprint 17 added 18 KG-backed EML wrappers + 170 tests.
- Toy attention iterations 0/1/2 (experimental) with go/no-go benchmark
  harness (`crates/eml-core/examples/attention_gate.rs`).

**Score-fusion**
- `FitnessScorer` (Level 1 multi-objective: task_completion, efficiency,
  tool_accuracy, coherence with configurable weights and error-indicator
  phrases; `pipeline/scorer.rs:46-100`).
- `TrajectoryLearner` (Level 1 GEPA-inspired ring buffer + evolution-readiness
  flag; `pipeline/learner.rs:46-120`).

## What's Left — Total Depth

### TODOs / FIXMEs in code

Scope-wide grep for `TODO`/`FIXME`/`XXX`/`HACK`/`todo!`/`unimplemented!` across
`crates/clawft-core/src/pipeline/`, `crates/clawft-llm/`,
`crates/clawft-service-llm/`, `crates/eml-core/`, and
`crates/clawft-core/src/agent/context_router/` returns three live items:

- **`pipeline/rate_limiter.rs:59`** — `TODO(Element-09): Expose via admin
  metrics endpoint (L2)`. The `pending_count()`-equivalent is implemented but
  not surfaced through any HTTP/RPC admin route. Owner: Element-09 (admin
  surface), not this workstream's hot path.
- **`pipeline/rate_limiter.rs:284`** — `TODO(Element-09): Used by admin
  maintenance endpoint (L2)`. Same owner; the maintenance method (manual LRU
  flush) is wired but not exposed.
- **`agent/context_router/hybrid.rs:44-50`** — `TODO(agent-core-v1 phase E3+):
  wire MicroLoraRouter (v3) once ruvllm-wasm lifts the 11-pattern HNSW cap
  (docs/research/rvf-context-router.md:118-128)`. Plus the comment notes the
  v2.5 sona-backed rerank is also deferred until ruv-ecosystem stability
  clears (`ruv-ecosystem-analysis-20260414.md`).

The `eml-core` and `clawft-llm` crates are clean of TODO markers. The
`clawft-service-llm` crate is clean. The pipeline crate is clean apart from
the two Element-09 items above.

Browser transport (`clawft-llm/src/browser_transport.rs:422-427`) carries an
inline comment about preferring `gloo_timers::future::sleep` for backoff over
the current `futures_util::future::ready(()).await` no-op — annotated as
implementation choice, not a hard TODO.

### Deferred items

**From `01-tiered-router` consensus log (`development_notes/01-tiered-router/consensus-log.md:443-449`)**:
- **CONS-002 (OPEN)**: DashMap vs `RwLock<HashMap>` for `CostTracker` and
  `RateLimiter`. Implemented as `RwLock<HashMap>` (no new dep). The
  performance trade-off was never benchmarked under contention — the entry
  remains `OPEN` pending real production data.
- **CONS-003 (NEEDS REVIEW)**: Escalation security model. Mitigated by FIX-04
  + FIX-06; final review marked "at Gate C+G" but no completion stamp logged.
- **CONS-006 (OPEN)**: Config validation boundary. The validation lives in
  `routing_validation.rs` but the boundary between deserialization-time
  rejection (serde) and post-load validation (`validate_workspace_ceiling`)
  was never finalized.

**From `01-tiered-router/sparc/security-review.md`**: the 22 findings produced
8 fuzz-target proposals (`fuzz_targets/{routing_config_parsing,
permission_resolution, cost_tracker_concurrent, rate_limiter_unique_ids,
auth_context_threading, tool_permission_glob, escalation_chain,
budget_persistence}`) — no fuzz harness exists yet under
`crates/clawft-core/fuzz/` or anywhere in the workspace.

**From `agent-core-v1.md:99` Phase E3**:
- v2.5 sona-backed rerank step: explicitly deferred (placeholder only).
- v3 `MicroLoraRouter`: deferred until `ruvllm-wasm` upstream lifts the
  11-pattern HNSW cap. Tracked in `docs/research/rvf-context-router.md:118-128`.
- v1→v2 promotion gate (7-day fallback rate < 25%) is "policy, not code" —
  the metric harness logs to substrate but no automated promotion exists.

**From `sprint-16/eml-coherence.md:54-60`**: two-tier coherence cadence
(`coherence_fast()` every tick, `spectral_analysis()` on drift, `model.train()`
every 1000 exact) is **not yet wired** despite the 34-param EML model existing
in `clawft-kernel/src/eml_coherence.rs`. Note: this is kernel-side; included
here for cross-reference because the same `EmlModel` substrate is shared.

**From `eml_model_development_assessment.md:60-70`**:
- Iteration 1 G1 PASS on per-position-mean (57.8% MSE reduction) but original
  identity-task target re-scoped to Iteration 3.
- Iteration 2 (SafeTree) shipped 0.6.10 with relaxed gate.
- Iteration 3 gate: multi-param coordinated perturbation on SafeTree; target
  ≥80% MSE reduction at `(seq_len=4, d_model=8)` and `final_mse < 5e-2`. Not
  attempted yet.
- Iterations 4-5+ (full EML-Transformer, hybrid scoring) explicitly
  aspirational.

**From `eml-synergy-scan.md`**: 80+ hardcoded heuristics across graphify,
kernel, LLM, assessment, and bench subsystems are listed as EML candidates.
The scan is exploratory — none of these have been migrated. Notable
pipeline-relevant ones:
- `assessment/effects.rs` weighted scores (`-0.5*err - 0.3*resource +
  0.2*throughput`).
- `pipeline/scorer.rs` weights (0.4/0.2/0.2/0.2) currently hand-tuned; could
  be `EmlModel::new(2, 4, 1)` learning per-task fitness.
- LLM provider selection cost-vs-quality trade-off ($0.01/1K threshold etc.)

**From `eml-causal-collapse-research.md`**: the EML closed-form discovery
target for `delta_lambda_2` (kernel coherence) is research-stage, not in
flight.

**From `hnsw-eml-deep-analysis.md`**: 10 EML opportunities for HNSW (adaptive
ef, learned distance, cosine decomposition, search-path prediction, etc.).
Items 3-10 are research; items 1-2 (adaptive beam, learned distance) are not
yet implemented.

**From `ruview-eml-contributions.md`**: cross-project research on motion
detection / vital-sign anomaly weights as EML candidates. Out of scope for
0.7.0 but tracked.

**From `01-tiered-router` SPARC plan I (testing & docs)**: `weft status`
routing-info integration was added per GAP-C08, but the production telemetry
sink for routing decisions (per-tier dispatch counts, fallback rates, budget
exhaustion events) is not surfaced through any user-facing metrics endpoint.

**From `pipeline-reliability/d-perf/notes.md:14-16`**: D1 parallel-tool
implementation explicitly notes "Per-path advisory locks not implemented yet
(future hardening)". The SPARC plan listed advisory locks as the D1 mitigation
for race-condition risk (likelihood=Medium, impact=High, score=6) — that
mitigation is *not* shipped. The current implementation simply runs tool calls
in parallel via `futures::join_all` and trusts the test suite, with no
filesystem-level guard against two tools writing the same path concurrently.

### Open questions and known limitations

**Routing**
- **Q1**: Should `RoutingConfig` gain an explicit `max_grantable_level` field?
  `routing_validation.rs:489-491` notes the field is intentionally a constant
  for now: `Until max_grantable_level is added to RoutingConfig, this constant
  provides the default ceiling`. Promotion to a config field would let
  operators raise the ceiling deliberately for known-good workspaces without
  patching code.
- **Q2**: Do channel-level overrides correctly interact with admin-level
  users? CONS-007 made channel > user priority (so `#general` can pin all
  users to free tier even if admin), but the security review (T-02) flagged
  that admin `model_override: true` still bypasses tier filtering. There is no
  explicit test for "admin in restricted channel" yet.
- **Q3**: Fallback model permission check. The security review (sec 2.3)
  recommended subjecting the fallback model to the same tier check, but the
  current code path in `tiered_router.rs` does not appear to gate the fallback
  by `max_tier`. A misconfigured `routing.fallback_model: anthropic/claude-opus-4-5`
  would let zero_trust users hit elite tier. Needs verification.
- **Q4**: Cost-tracker integrity. Persistence file uses mode 0600 but no HMAC
  / checksum. Security review §2.4 recommended HMAC keyed on a server-side
  secret; not implemented. A local user with file access can reset their
  spend by editing the JSON.
- **Q5**: Rate limiter `window_seconds = 0` is documented as "functionally
  unlimited" but Phase H validation does not reject `window_seconds: 0`.
- **Q6**: Information disclosure via `RoutingDecision.reason`. Security review
  T-06 flagged that the reason string still includes `"complexity={:.2},
  tier={}, level={}, user={}"` — verbatim user metadata. Not redacted.

**Context router**
- **Q7**: How is the v1→v2 promotion gate enforced operationally? The
  fallback-rate metric is emitted to substrate but there is no scheduled job
  or admin command to flip the config from `llm-classifier` to `embedding`
  when the gate clears.
- **Q8**: `HybridRouter` "structurally empty decision" predicate
  (`hybrid.rs:100`) treats `Some(vec![])` for `tool_subset` as non-empty
  (intentional — `Some([])` means "no tools at all" is an explicit signal).
  Is the contract documented for plugin authors?
- **Q9**: What does the daemon do when `Config.routing.context_router` is set
  to `"embedding"` but the `embedding-router` feature is not compiled in?
  `index.rs` already handles `EmbeddingRouterError::EmptyRegistry`, but the
  cargo-feature gate path was not exhaustively tested for misconfigured
  builds.

**EML / score fusion**
- **Q10**: Is EML score fusion in scope for the 0.7.0 pipeline? The synergy
  scan flagged `pipeline/scorer.rs` weights as a candidate but the work was
  not assigned to a sprint. `FitnessScorer` weights remain literal constants
  (0.4/0.2/0.2/0.2) hand-tuned via `FitnessScorerWeights::default()`.
- **Q11**: The `FitnessScorer.error_indicators` allowlist of refusal phrases
  ("I can't", "as an AI", etc.) is hand-curated. Localization, jailbreak
  resilience, and false-positive rate are unknown.
- **Q12**: The `RetryModel` (`eml_retry.rs`) trains from observed retry
  outcomes but the persistence path for the trained model is unclear — when
  the daemon restarts, does the learned retry curve survive? The `Default`
  impl resets to untrained.

**EML attention**
- **Q13**: When does Iteration 3 (multi-param coordinated perturbation) start?
  No tracking issue or plan stub.
- **Q14**: Is the `experimental-attention` feature gate actually wired in CI
  (build / test)? The benchmark example exists, but no GH actions step
  references it.

**Transport**
- **Q15**: `clawft-service-llm` and `clawft-llm` overlap. The lib.rs comments
  argue the split (daemon-narrow vs general provider abstraction), but as the
  daemon adds streaming / multi-provider features, the split rationale erodes.
  Is consolidation planned?
- **Q16**: The null-content deserializer (commit 8b05d868) covers the Nemotron
  case. Are there other upstreams that emit non-string content (arrays of
  content blocks, tool-call structured content)? OpenAI's vision API uses
  array-of-blocks; the current `String` field with `null→""` mapping does not
  handle that.

### Orphaned work

- **Genetic mutation loop** (`pipeline/mutation.rs`, 413 lines) is a complete
  GA implementation (selection, crossover, mutation operators, population
  state) but is **not invoked** from any production code path.
  `TrajectoryLearner` sets `evolution_ready: true` after threshold but no
  caller reads the flag and triggers mutation. ADR-017's "self-improvement
  flywheel" is shipped as code, not as a running feature.
- **`pipeline/permissions.rs`** is 757 lines with the full 5-layer resolver
  but the audit-log emission path (per security review §2.2 recommendation:
  `Log all escalation events and model_override events at warn level for
  audit`) is not exhaustive — escalation events log, but `model_override`
  bypasses do not appear to.
- **`scripts/build.sh gate`** (the 11-check phase gate referenced in
  `CLAUDE.md`) doesn't have an explicit pipeline-pass step distinguished from
  `cargo test`. Pipeline-specific regression coverage relies on the workspace
  test suite.
- **EML attention `experimental-attention` feature** ships in eml-core but is
  not consumed by any pipeline code. The 12 EML wrappers from sprint 17 live
  in their respective domains (KG, graphify) but the pipeline scorer/learner
  is the obvious next consumer and has not been adapted.
- **Sprint 16 EML coherence two-tier cadence** — kernel-side, but the same
  `EmlModel::record/train` pattern was meant to replicate into pipeline
  scorer. Never propagated.
- **`weft status` routing info** added per GAP-C08 — the panel side surfaces
  current tier / budget remaining, but historical routing decision logs (per
  the security-review §2.3 audit recommendation) are not.
- **Fuzz targets** for the security review's 8 attack surfaces — not
  scaffolded in any `fuzz/` directory.
- **`hashbrown::HashMap` vs `std::HashMap`** — the standard map is used
  everywhere in pipeline; CONS-002's DashMap question never materialized as
  a benchmark or PR.
- **MCP namespace collision** (security review T-02 attack vector
  `exec__shell` via wildcard `["*"]`) — mitigation not landed; tool-name
  validation in `permissions.rs` is exact-match only.

## Task List

| ID | Description | Source | Owner | Severity |
|---|---|---|---|---|
| 03-01 | Expose rate-limiter metrics via admin endpoint | `rate_limiter.rs:59` | Element-09 | low |
| 03-02 | Expose rate-limiter LRU maintenance via admin endpoint | `rate_limiter.rs:284` | Element-09 | low |
| 03-03 | Wire `MicroLoraRouter` (v3) once `ruvllm-wasm` lifts 11-pattern HNSW cap | `hybrid.rs:44`, `agent-core-v1.md:99` | upstream + agent-core | medium |
| 03-04 | Wire v2.5 sona-backed rerank step | `hybrid.rs:49`, `agent-core-v1.md:99` | agent-core (after sona) | medium |
| 03-05 | Add `max_grantable_level` field to `RoutingConfig` | `routing_validation.rs:489` | this workstream | medium |
| 03-06 | Apply tier check to fallback model | security review §2.3, Q3 | this workstream | high |
| 03-07 | HMAC the cost-tracker persistence file | security review §2.4, Q4 | this workstream | medium |
| 03-08 | Reject `window_seconds: 0` in Phase H validation | security review §2.5, Q5 | this workstream | low |
| 03-09 | Redact / truncate `RoutingDecision.reason` to avoid info disclosure | security review T-06, Q6 | this workstream | medium |
| 03-10 | Audit-log `model_override` bypasses (escalation already logs) | security review §2.3 | this workstream | medium |
| 03-11 | Add MCP tool-name namespace validation against wildcard `["*"]` | security review T-02 | this workstream | high |
| 03-12 | Scaffold fuzz targets for 8 attack surfaces | `01-tiered-router/security-review.md` | this workstream | medium |
| 03-13 | Resolve CONS-002 (DashMap benchmark) | `consensus-log.md:443` | this workstream | low |
| 03-14 | Resolve CONS-003 final review (escalation security) | `consensus-log.md:443` | this workstream | medium |
| 03-15 | Resolve CONS-006 (config validation boundary) | `consensus-log.md:443` | this workstream | low |
| 03-16 | Wire D1 per-path advisory locks for parallel tool execution | `d-perf/notes.md:14` | this workstream | medium |
| 03-17 | Wire `evolution_ready` flag → `mutation.rs` GA loop (ADR-017 flywheel) | `pipeline/mutation.rs`, `learner.rs` | this workstream | medium |
| 03-18 | Persist `RetryModel` learned weights across daemon restarts | `eml_retry.rs`, Q12 | this workstream | low |
| 03-19 | Surface routing-decision history via admin endpoint | security review §2.3 | this workstream | low |
| 03-20 | Iteration 3 EML attention (multi-param coordinated perturbation) | `eml_model_development_assessment.md` | research track | research |
| 03-21 | Wire sprint-16 two-tier EML coherence cadence (kernel-side) | `sprint-16/eml-coherence.md:54` | kernel workstream | low |
| 03-22 | Decide consolidation of `clawft-service-llm` vs `clawft-llm` | Q15 | architecture review | low |
| 03-23 | Handle non-string `content` (vision blocks / structured) in `LlmClient` | Q16 | this workstream | medium |

## Sources

- `crates/clawft-core/src/pipeline/{mod,assembler,classifier,cost_tracker,
  learner,llm_adapter,mutation,permissions,rate_limiter,router,scorer,
  service_llm_adapter,tiered_router,traits,transport}.rs`
- `crates/clawft-core/src/agent/context_router/{hybrid,llm_classifier,
  embedding/{mod,index,tests}}.rs`
- `crates/clawft-core/src/routing_validation.rs`
- `crates/clawft-types/src/routing.rs`
- `crates/clawft-llm/src/{browser_transport,config,eml_retry,error,failover,
  lib,local_provider,openai_compat,provider,retry,router,sse,types}.rs`
- `crates/clawft-service-llm/src/{client,lib}.rs`
- `crates/eml-core/src/{attention,baseline_attention,events,features,lib,
  model,operator,tree}.rs`
- `.planning/sparc/phase4/01-tiered-router/{00-orchestrator, A..I plans,
  completion-report, consensus-log, gap-analysis-{coverage,docs,security,
  types}, planning-summary, remediation-plan, security-review}.md`
- `.planning/sparc/phase4/05-pipeline-reliability/{00-orchestrator,
  01-phase-DPerf-parallel-tools-caching, 02-phase-DReliability-errors-retry-failover,
  03-phase-DBus-observability-transport, 04-element-05-tracker}.md`
- `.planning/development_notes/01-tiered-router/{consensus-log,
  planning-summary, phase-A/decisions}.md`
- `.planning/development_notes/05-pipeline-reliability/{README,
  d-{perf,reliability,observability,transport}/{notes,decisions,
  blockers,difficult-tasks}}.md`
- `.planning/development_notes/{eml_model_development,
  eml_model_development_assessment, eml-causal-collapse-research,
  eml-synergy-scan, hnsw-eml-analysis, hnsw-eml-deep-analysis,
  ruview-eml-contributions}.md`
- `.planning/development_notes/sprint-16/eml-coherence.md`
- `docs/adr/{adr-017-gepa-prompt-evolution, adr-018-hermes-llm-provider,
  adr-019-registry-trait, adr-045-tiered-router-permissions}.md`
- `docs/plans/{agent-core-v1, chat-agent-v1}.md`
- `docs/research/rvf-context-router.md`
- Git history: commits `8b05d868`, `1d378372`, `a7e848cd`, `a05e22ac`,
  `97b5857f`, `81ef9e41`, `dc55c875`, `08eb90df`, `c18dde11`, `50de58cc`,
  `07ceb059`, `2df59531`

<!-- TRIAGED-STAMP:BEGIN -->
## Triaged into Plane — 2026-04-28

All open items in this audit have been filed as Plane work items in the WeftOS workspace under the `ws03-pipeline` label.

- **Range**: WEFT-27 … WEFT-58 (32 items)
- **Per cycle**: 0.7.x: 11, 0.8.x: 16, 0.9.x: 2, 1.0.x: 3
- **Triage spec**: `.planning/reviews/0.7.0-release-gate/triage/`
- **WEFT-N → name map**: `.planning/reviews/0.7.0-release-gate/triage/weft-mapping.json`

Per the project rule (CLAUDE.md → "Plane is the authoritative work tracker"): future updates to these items happen in Plane, not in this audit doc. This doc remains the source-of-truth for the original survey.
<!-- TRIAGED-STAMP:END -->
