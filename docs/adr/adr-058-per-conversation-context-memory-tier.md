# ADR-058: Per-conversation context memory tier (session-scoped RVF + HNSW)

**Date**: 2026-06-28
**Status**: Accepted (2026-06-28)
**Deciders**: Main-thread design discussion 2026-06-28 (long-context agent-loop / local-model serving thread)
**Depends-On**: ADR-018 (Hermes models as clawft-llm provider), ADR-022 (ExoChain mandatory audit), ADR-056 (BVH-on-RVF spatial-temporal index over ECC), ADR-011 (raw HNSW sufficient — no FrankenSearch), ADR-031 (RVF wire mesh format), ADR-020 (ChainLoggable), ADR-028 (dual signing Ed25519 + ML-DSA-65), ADR-030 (CBOR exochain codec), ADR-057 (substrate per-path read ACLs)
**Relates-To**: ADR-059 (embedding provider implementation — Qwen3 via ort); ADR-017 (GEPA prompt evolution — promotion lineage); the context-compression gap tracked in `crates/clawft-core/src/agent/context.rs`; RMM / citation-attributed memory (`docs/research/rmm-reflective-memory-management.md`, ws06-memory)

## Context

The clawft agent loop (`crates/clawft-core/src/agent/loop_core.rs`) is moving
toward **cloud-independent operation on a locally-served open-weight model**
(ADR-018) — the working target is Hermes 4.3-36B (Seed-OSS base) served via
llama.cpp on a 128 GB Apple-silicon host. A long-running agent session grows
its context every turn (dialogue, tool outputs, file reads), and that growth
is the binding cost: it inflates both the transformer **KV cache** (RAM +
attention compute) and the **prefill** latency of re-ingesting the rolling
context each turn.

We surveyed the runtime options for bounding/accelerating the KV cache on
backends we can actually run on this host (llama.cpp, mlx_lm, Ollama),
verified against upstream as of 2026-06. The findings constrain the design:

1. **No importance-based KV eviction exists in any local backend.** H2O /
   SnapKV / PyramidKV / TOVA / FastGen are research / vLLM-CUDA only (they
   require the full per-step attention matrix, which forces eager attention
   and abandons FlashAttention). The most we get locally is a **sliding
   window**: mlx_lm `--max-kv-size` (`RotatingKVCache`) or llama.cpp
   context-shift.
2. **llama.cpp context-shift is now default-OFF** (PR #15416) and
   regression-prone (#16693, #16983) because it breaks template-dependent
   models. It is not a load-bearing strategy.
3. **Precomputed-KV-block retrieval is not viable locally.** CacheBlend
   (EuroSys'25), RAGCache, PromptCache, KVLink et al. are CUDA/vLLM/SGLang
   research. Even the SOTA (CacheBlend) must recompute 10–20% of tokens plus
   the entire first layer and re-apply RoPE to recover cross-chunk attention.
   The naive "cache a KV block per indexed chunk and splice it in" is **lossy
   for any chunk not pinned at position 0** (RoPE position mismatch; the
   chunk's KV never saw cross-attention to its prefix).
4. **llama.cpp KV slot save/restore** (`--slot-save-path`, `/slots/{id}`) is
   real and fast, but **prefix-only**: contiguous KV from position 0. Usable
   to skip re-prefill of a *stable head*, not arbitrary retrieved chunks.

Net consequence: **the externalized context tier must store text (embedded),
re-prefilled on retrieval — not reusable KV tensors.** This is not a
limitation we can engineer around locally today; it is the shape of the
solution.

The substrate to build on already exists, partly stubbed:

- `crates/clawft-weave/src/rvf_codec.rs` / `rvf_rpc.rs` — RVF (RuVector
  Format): an append-only, **content-hash-integrity** segment stream
  (ADR-031). A per-conversation event log is RVF's native shape.
- The kernel's `HnswService` delegates to the `instant-distance` crate
  (per `crates/clawft-casestudy-gen-qsr/src/recall.rs`; ratified sufficient
  by ADR-011).
- `crates/clawft-services/src/rvf_tools.rs` exposes RVF vector ops as MCP
  tools but is **currently an in-memory stub** pending the `rvf-runtime`
  backend.
- `clawft_core::vector_store::VectorStore` + `HashEmbedder` (a *hash*
  embedder, not semantic) behind the `vector-memory` feature; today's agent
  memory is `MEMORY.md` with substring fallback (`crates/clawft-tools/src/memory_tool.rs`).
- `crates/clawft-core/src/agent/context.rs` has a `compress_context` path,
  but it is primitive: first-sentence extraction of old turns and a
  whitespace token estimator (`count_tokens`), default budget 8192.

And — decisively — **the conversation is already partly on the chain.** The
kernel `ChainManager` (`crates/clawft-kernel/src/chain.rs`, `ChainLoggable` +
`append_loggable`, ADR-020/022) already logs `agent.chat.turn` events to
ExoChain (the chain-tail witness panel renders them). ExoChain is an
append-only, hash-linked, signed (Ed25519 + ML-DSA-65, ADR-028), checkpointed
event log. Separately, **ECC (Embedded Cognitive Core, `ecc` feature in
`clawft-kernel`) already exposes a family of indexes as peer query classes,
all cross-keyed into ExoChain by chain sequence** (per ADR-056): HNSW
(`hnsw_service.rs`/`vector_hnsw.rs`, `VectorBackend`), causal-edges (ordered
walks over the chain), UNID cross-references, and — accepted but not yet
built — a BVH spatial-temporal index (`clawft-bvh`/`SpatialBackend`). RVF
itself carries COW branches, witness chains, and lineage tracking (ADR-056),
and the system is modeled as a forest of trees (ADR-046). The substrate for
"one event log, many derived indexes, non-destructive branch operations"
therefore already exists — this ADR consumes it rather than inventing a
parallel store.

## Decision

Treat **ExoChain as the single source of truth** (the immutable trunk), and
build context management as **non-destructive branch operations** over it. The
agent loop never gets a parallel store; it gets a **query/graft layer** that
selects branches of the chain via ECC's indexes and grafts them (by COW
reference) into the working context. Retrieval re-injects **text** into the
prompt; KV tensors are never externalized.

```
L1  transformer KV cache   hot, in-window   serving: q8_0 KV + capped --ctx-size
L2  session graft set      warm, this run   NEW — fused query over ECC indexes, scoped to this
                                            conversation's chain sequences; ephemeral, rebuildable
L3  durable trunk          cold, durable    ECC/MEMORY; promotion = graft onto trunk + chain event
ExoChain                   spine            append-only signed log; chain sequence = universal key
```

### Indexes are projections over ExoChain (ECC)

Per ADR-056, ECC already exposes index families as peer query classes, all
keyed by chain sequence. This ADR consumes them for context assembly rather
than building a bespoke vector store:

| Index | Question it answers | Status |
|---|---|---|
| HNSW / `VectorBackend` | semantically similar content | exists |
| causal-edges (chain walk) | what *led to* / depends on this state | exists |
| UNID cross-references | this exact entity | exists |
| temporal | recent / within `[t1,t2]` | mostly free (chain is ordered) + BVH T-axis |
| BVH / `SpatialBackend` | near in space-time (4D) | **ADR-056, not yet built** |

A turn's context is a **fused, budget-bounded query across the relevant
indexes** — semantic recall *plus* causal ancestors of the current state
(pull the chain that produced a failure, not just lexically similar text)
*plus* recency — de-duped **by chain sequence** (the shared key makes fusion
and provenance trivial) and rank-fused (RRF-style). New indexes (BVH, future
others) are **additive**: they register as another projection keyed off the
same chain sequence and join the fusion without re-plumbing.

**Phasing (decided 2026-06-28):** v1 binds **one index per conversation thread**
— the semantic index (i.e. "the model"/embedder) — to keep the first cut
simple. Multi-index fusion (causal + temporal + BVH) is **v2**. The chain-
sequence key and the fusion interface are designed in from the start so v2 is
purely additive, but the v1 deliverable is single-index.

### L2: a session-scoped graft set, not a store

1. On agent-loop start, establish a **session-scoped view**: ECC index queries
   are filtered to *this conversation's* chain sequences, optionally backed by
   an ephemeral per-session HNSW for hot recall. There is **no** new
   source-of-truth store — the chain already holds the canonical events.
2. The view is **disposable and rebuildable** from the chain at session end.
   Scoping to the session keeps retrieval precision high (no cross-session
   contamination) and the ANN small; the durable record lives on the trunk.
3. Addressing reuses the **chain sequence + RVF content hash** as the universal
   key, so identical tool outputs / repeated file reads dedupe for free and
   every grafted item carries verifiable provenance.

### Graft / prune / promote — non-destructive by construction

The metaphor is horticulture on an immutable tree, and it maps to real RVF/
chain primitives (COW branches + append-only chain, ADR-056):

- **Graft** = bring a branch (a subtree of chain events — a turn, a tool
  trajectory, a causal chain) into the working context **by COW reference**,
  selected via the indexes above. Copying does **not** remove it from origin;
  the chain entry is untouched. Cross-conversation grafting (pulling another
  session's branch into this one) is the same operation with a wider scope.
- **Prune** = evict a graft from the live window when it ages out. The branch
  **remains on the chain** (origin retained); pruning only drops it from the
  working set, and it can be re-grafted later via retrieval. This is the
  application-level eviction that bounds L1 (no engine context-shift needed).
- **Promote (graft-to-trunk)** = L2→L3. A distilled branch is grafted onto the
  durable trunk and the promotion is itself a chain event (see below).

Because the chain is append-only and RVF branches are COW, **every operation
is non-destructive**: the origin always persists, lineage is automatic.

**This IS the ECC causal model, not a parallel one — "causal collapse modeled on
the git tree."** graft / prune / promote map directly onto the kernel's node
lifecycle on the `CausalGraph` (ADR-056): `NodeState::{Speculative, Frontier,
Committed, Stale, Pruned}` (node `metadata.state` + `Contradicts`/prune on
revision, witness-chain commit via `chain_seq`). A speculative branch that doesn't
cohere is held off `main_line` and pruned; a coherent one **collapses (commits)**
to the trunk — the same branch / merge / prune semantics as git. The agent uses
this kernel model directly; the full clawstage→ECC mapping (origin of the concept,
`~/dev/mentra/clawstage`) is in `.planning/voice-ecc-synthesis.md`.

### Embedder (decision summary — full implementation in ADR-059)

The semantic index needs an embedder in two lanes that must agree — **lab**
(Python/MLX, `bin/embed`: benchmarking + fine-tuning) and **prod** (Rust
`clawft-kernel` via `ort`, on the hot path; today the undersized all-MiniLM-L6-v2
384-d/256-ctx default). **Decision: one model across both lanes —
`Qwen3-Embedding-0.6B`, MRL-512, f32 storage for v1** (the only candidate that is
the best MLX lab pick *and* a published ONNX export runnable by `ort`, 32K-ctx,
Apache, code-capable, and a clean fine-tune base; rejected NV-Embed-v2 / Conan-v2 /
jina-v4 (non-commercial + off-arch), gemini-* (cloud), and lab≠prod two-model
splits that break vector comparability).

The provider **implementation** — the decoder-ONNX plumbing in `ort`, the HF
`tokenizers` crate, last-token pooling, the query instruction prefix, the `candle`
escape hatch, the consistency / cosine-parity contract, and the storage decision
(store **f32@512**; ruvector int8 *storage* quant deferred, and distinct from the
broken int8 *model export*) — is specified in **ADR-059**.

**Validation (`bin/embedlab`, 2026-06-28):** parity gate PASSES — MLX↔ONNX cosine
**0.9996** (100% of probes ≥ 0.99), MRL-512 best, `model_fp16.onnx` (int8 *model*
export broken). Probe set small → recall deltas within noise; decision rests on
parity + MRL-512. ruvector int8 *storage* quant (Scalar, per-vector global min/max;
LogQuantized/RaBitQ NOT in 2.1.0) is a separate, deferred concern → **store f32@512
for v1**. Full numbers, the storage-quant test spec, and the implementation are in
**ADR-059**. Results: `~/llm/docs/models/results/embedlab.{json,md}`.

### Population is controlled, not automatic

"Control what is populated" now means controlling **what is eligible to be
grafted** — i.e. what gets chain-logged / fed to ECC indexing vs. held
off-chain vs. excluded entirely:

- **On-chain (provenance-grade), per ADR-022**: turns and state-changing tool
  calls — already logged. These are graftable with full signed provenance.
- **Off-chain ephemeral (this session's view only)**: read-only retrievals and
  scratch that do not warrant a permanent immutable record. Indexed for recall
  this run, dropped (or promoted) at session end. This respects ADR-022's
  *state-changing* scope rather than over-logging the chain.
- **Large payloads are content-addressed**: a big tool output / file read is
  stored as an RVF content-addressed blob and the chain event holds only the
  **hash + metadata** — no chain bloat, free dedup, integrity preserved.
- **Populate the graft-eligible set** with: full tool outputs (the large
  items), dialogue turns as they age out, generated artifacts, externally-
  retrieved documents.
- **The aging move**: when a turn or tool output ages past the verbatim
  recent-window, write a **short summary into the prompt (lossy)** AND embed
  the **full text into L2 (lossless, recall on demand)**. The window carries
  the gist; the full text is one retrieval away. This supersedes the
  first-sentence `compress_context` placeholder.
- **Exclude** (deny-list): secrets/credentials, injected `<system-reminder>`
  content, transient noise. A max-chunk size caps any single segment.

### Retrieval rides the prefix-stability cache contract

The assembled prompt MUST be ordered:

```
[stable head: system prompt + tool schema + pinned facts]
  → [L2-retrieved text block (top-k, deterministically ordered)]
  → [rolling dialogue tail]
```

- The **stable head is byte-identical turn-to-turn** so its KV is reused
  (`--cache-reuse`, or `--slot-save-path` keyed by a content hash of the head
  for sub-second restore across process restarts).
- The retrieved block + tail are re-prefilled each turn. This is cheap
  **because the live window is capped** (target 32–48k), which is the whole
  point of pushing the rest to L2.
- Retrieval ordering MUST be cache-aware: never reshuffle the stable head, or
  all prefix-KV reuse is forfeited.

### Serving configuration (L1)

- Run with `-ctk q8_0 -ctv q8_0` (halves KV/token) and an explicit
  `--ctx-size` matched to the capped live window — **not** the model's full
  512k native context.
- Do **not** rely on `--context-shift` (default-off, regression-prone).
  Window bounding is done at the application level by capping the live set
  and evicting to L2.
- `bin/serve-llamacpp` (in the `~/llm` model-lab repo) is the reference
  serving entry point; `--slot-save-path` keys the stable head only.

### L2 → L3 promotion (graft-to-trunk) at session end

Because the session view is discarded per run, a **session-end rollup MUST
promote durable facts to the trunk** before the view is dropped (else "fresh
each run" becomes "amnesia each run"). This reuses the rollup/recall pattern in
`crates/clawft-casestudy-gen-qsr/{rollup,recall}.rs`. The promotion boundary is
itself a population-control point: only facts worth keeping graft onto L3.

**Promotion signals (decided 2026-06-28)** — promotion is multi-signal, not a
single heuristic: (a) task-analysis + task-completion scoring; (b) explicit
"important to remember" marks (e.g. a `memory_store` tool call); (c) a
**mandatory post-task postmortem/review** that examines the finished task and
nominates items for promotion. The postmortem is a first-class loop stage, not
an afterthought — it is the primary durable-memory gate.

**Promotion is a chain event** (`memory.promote` kind via `ChainLoggable`,
ADR-020/022), recording which chain sequences were distilled into which durable
fact. This gives auditable lineage of long-term memory and feeds GEPA prompt-
lineage (ADR-017): the durable trunk is itself witnessed.

### Provenance dividend

Because every grafted item references a chain sequence (and on-chain items are
signed/checkpointed, ADR-028), retrieved context carries a **verifiable
origin** — the agent can produce the *witness chain* for any conclusion, and
the governance engine can gate on provenance. For a governed local agent this
is a differentiating property, and it falls out almost for free because turns
are already on-chain.

## Consequences

### Positive

- Effectively unbounded session context with a **bounded live window**:
  compression (lossy gist) + L2 retrieval (lossless recall) compose to keep
  L1 small while preserving access to everything the session has seen.
- Bounding the window at 32–48k vs letting it grow to 128k saves ~10–13 GiB
  of q8_0 KV (see numbers below) and, more importantly, keeps prefill latency
  and attention cost low — the real bottleneck on a dense 36B.
- Reuses substrate already committed to — ExoChain (ADR-022), ECC index
  families and the BVH peer (ADR-056), raw HNSW (ADR-011), RVF COW branches
  (ADR-031) — instead of a parallel store. Chain sequence as the universal key
  gives dedup, fusion, and provenance for free.
- Extensible by construction: semantic + causal + temporal recall work today;
  BVH (ADR-056) and any future index plug in as additional projections keyed
  off the same chain sequence, with no change to the source of truth.
- Model-independent: the same hierarchy serves any local model behind the
  ADR-018 provider; it does not depend on a particular context window.
- Honest about the toolchain: stores text, not KV, so it works on llama.cpp /
  mlx today without waiting for position-independent KV splicing.

### Negative

- **L3 promotion is now load-bearing.** "Fresh each run" becomes "amnesia each
  run" if the session-end rollup is skipped or loses durable facts. The rollup
  must be reliable and tested.
- Requires real components to replace stubs: a **semantic embedder** (the
  `HashEmbedder` is insufficient for recall quality; candidate: mlx-embeddings
  via the `~/llm` `bin/embed` path or an in-process ONNX embedder) and the
  **`rvf-runtime` backend** behind `rvf_tools.rs`.
- Retrieval adds per-turn latency (embed query + HNSW search + re-prefill of
  the retrieved block). Must be budgeted against the prefill it saves.
- The `compress_context` rewrite (summary-into-window) needs an LLM-summarize
  path and a real tokenizer to replace `count_tokens`, or the token budget is
  inaccurate.

### Neutral

- The session view is per-conversation by default, but **cross-session recall
  is just a wider-scoped graft** (pull another session's branch by COW
  reference) — the chain spans all sessions, so this is a scoping decision, not
  a new mechanism. Default scope is this conversation; widening is opt-in.
- Cross-conversation grafting must respect the ADR-057 read-ACL model: a branch
  is grafted only if the current Actor is allowed to read its origin chain
  paths. First implementation keeps grafts within one Actor's own sessions, so
  ADR-057 does not gate v1.
- Precomputed-KV-block retrieval (CacheBlend-style) remains the aspirational
  ceiling. Revisit only if a clawft serving path moves to CUDA, or if
  llama.cpp/mlx gain position-independent KV splicing.

## KV-size vs window (Hermes-4.3-36B: 64 layers, 8 KV heads, head_dim 128)

q8_0 ≈ 0.133 MiB/token; f16 ≈ 0.25 MiB/token.

| Live window | q8_0 KV | f16 KV |
|---|---|---|
| 32k  | ~4.3 GiB  | ~8.0 GiB |
| 48k  | ~6.4 GiB  | ~12.0 GiB |
| 64k  | ~8.5 GiB  | ~16.0 GiB |
| 128k | ~17.0 GiB | ~32.0 GiB |

Capping at 32k vs 128k (q8_0): **17.0 → 4.3 GiB, ~12.7 GiB saved.** On 128 GB
the RAM is survivable at 128k regardless; the dominant motivation for the cap
is **prefill latency and decode throughput**, not RAM exhaustion.

## Acceptance criteria (for ratification → MUST-HAVE)

- [ ] A **graft API** over ECC indexes: query (scoped to a session's chain
      sequences) → candidate branches (**v1: single semantic index**) →
      COW-reference graft into the working set. Keyed by chain sequence; no
      parallel source-of-truth store.
- [ ] **v1: single index per thread** (semantic / embedder). Fusion interface
      defined and chain-sequence-keyed so it is additive, but multi-index
      fusion (causal + temporal + BVH, ADR-056) is **v2, not a v1 deliverable**.
- [ ] The ADR-059 embedder (Qwen3-Embedding-0.6B, MRL-512) wired into ECC HNSW for
      `agent.chat.turn` / tool-output events (replacing `HashEmbedder` and
      all-MiniLM); large payloads stored as RVF content-addressed blobs referenced
      by chain hash. (Provider + cosine-parity gate: **ADR-059**.)
- [ ] `context.rs` assembles the prompt in the prefix-stable order above and
      caps the live window; aged items summarized in-window AND graftable from
      the chain.
- [ ] Population policy: on-chain (state-changing, ADR-022) vs off-chain
      ephemeral vs excluded (secrets/`<system-reminder>`); max-chunk size;
      content-hash dedup.
- [ ] **Prune = evict-from-window, origin retained on chain** (no
      `--context-shift` reliance); re-graft via retrieval verified.
- [ ] Session-end **graft-to-trunk promotion** emits a `memory.promote` chain
      event recording source chain sequences; tested; view dropped only after.
- [ ] `compress_context` upgraded from first-sentence extraction to
      summary-into-window with a real tokenizer (`count_tokens` replaced).
- [ ] Serving reference (`bin/serve-llamacpp`) documents q8_0 KV + capped
      `--ctx-size` + stable-head `--slot-save-path`; does not depend on
      `--context-shift`.
- [ ] Integration test: a fact stated early, pruned from the window, is
      re-grafted correctly later in the same session — with its witness chain.

## Resolutions and deferrals (design review 2026-06-28)

1. **Eviction trigger — resolved: task-sizing first; the loop is cattle, not a
   pet.** The primary way context stays bounded is **right-sizing tasks to fit
   the window** — decompose work to the proper size; the decomposition is
   itself scorable and should improve over time — not clever in-window
   eviction. The agent-loop instance is **disposable**: because ExoChain is the
   source of truth, a loop can be killed and rebuilt from the chain at any point
   at the cost of time only. A token-budget watermark on the live set remains
   the fallback trigger for the loop itself.

2. **Retrieval granularity — resolved: simplest first, no corner-painting.**
   v1 = one segment per turn / per tool output. Chain-sequence keying keeps a
   later move to semantic re-chunking purely additive.

3. **Promotion policy — resolved: multi-signal + mandatory postmortem.** See
   "Promotion signals" above: task-analysis/completion scoring, explicit
   "important to remember" marks, and a post-task postmortem/review that
   nominates items for promotion. Many triggers, not one heuristic.

4. **Cross-session / cross-Actor graft — OUT OF SCOPE for this ADR.** It belongs
   to the embedder/model layer underneath, not the context-tier contract.
   Default scope stays single-conversation; widening is decided where the
   model/embedder lives.

5. **Embedder choice — DECIDED; implementation in ADR-059.**
   `Qwen3-Embedding-0.6B` across lab (MLX) + prod (`ort`/ONNX) + fine-tuning,
   MRL-512, f32 storage; `gte-modernbert-base` encoder fallback; rerank
   `Qwen3-Reranker-0.6B`. Validated by `~/llm/bin/embedlab` (parity 0.9996).
   **Eval baseline for home-built models: MTEB(eng,v2) Retrieval as the
   anti-forgetting guardrail + CoIR/RTEB for code + our `code_recall.jsonl` as
   the objective; re-run MTEB-Retrieval per MRL dim.** **Direction:** the embedder
   is a **closed-loop training target** — the chain plus postmortem/promotion
   outcomes are labeled recall data, so the 0.6B model can be fine-tuned on the
   agent's own retrieval successes/failures. See ADR-059 for the provider,
   storage-quant test, and lanes.

6. **Fusion weights — resolved: single index in v1** (see Decision → Phasing).
   No fusion in v1; one index (the embedder/model) per thread. Fixed weights
   when multi-index lands in v2; task-adaptive weighting (bias causal on
   failure/debug turns) revisited with the pipeline scorer (ADR-017).

7. **Graft granularity for causal branches — deferred to a specialized agent.**
   How deep to graft a causal-ancestor subtree (vs the token budget) is a job
   for a specialized context-curator/graft agent that learns to do it well.
   Out of v1 (which is single-index regardless).

## Update — the L2 tier is the frontier-walk substrate (2026-06-29, see ADR-062)

ADR-062 (ECC graph-walk conversation) builds directly on this tier and adds two framings:
- The L2 `SessionView` is the **frontier-walk substrate** of the conversation: a turn enters at the
  frontier, walks (recall), mutates, and renders responses *from* nodes. graft/prune/promote are the
  walk's recall/evict/collapse steps and map to the **ADR-046 forest's graft/shake** vocabulary.
- The **central structural change**: today the live conversation path uses HNSW recall *in
  isolation*; ADR-062 requires it to **traverse the ADR-046 forest** — fuse HNSW recall with
  CausalGraph lineage (`traverse_forward`/`trace_causal_chain`) + `CrossRefStore` links, and commit
  nodes **per-turn** (Speculative→Frontier→Committed) rather than only at conversation end.

## Update — single conversation store (M3 store collapse, 2026-07)

The M3 store collapse (design `.planning/hermes-loop/m3-store-collapse-design.md`)
retires the duplicate `SessionManager` JSONL turn store: the LLM-context
assembly input (the in-memory `Session` this tier's graft splices into) is now
**hydrated from the `ConversationSink`** rather than a second per-turn store.
The assembly path (`ContextBuilder::build_messages` + `session.get_history`)
and this tier's graft splice are unchanged; only the *source* of the hydrated
`Session.messages` moved (sink history, filtered to the store-1 subset). One
durable store now feeds sink → chain → tier → forest.

## Update — Reflective Memory Management (2026-09-11)

Google Cloud AI Research RMM (Tan et al., ACL 2025, arXiv:2503.08026) is the
reserved **semantic re-chunking** follow-on from resolution #2 above, plus the
missing retrieval-attribution loop this ADR does not specify.

- **L3 bank organization (prospective):** topic units with merge-or-insert,
  not more append-only `MEMORY.md`. Session-end hook is ConversationSink /
  chain after M3, not `SessionManager`.
- **L1 inject path (retrospective):** `ContextBuilder` still dumps the entire
  `MEMORY.md` as `# Relevant Memory:`. RMM says frozen retriever Top-K →
  lightweight reranker Top-M with generator citations as `+1/−1`. That
  reranker is SONA (WEFT-46), not a second store, and **not** RL on the
  embedder (paper ablation collapsed Acc 58.8→31.0).
- **ADR-059 `Qwen3-Reranker-0.6B`** remains the optional offline cross-encoder.
  RMM's linear adapter is the *online* citation-RL loop; they can stack; they
  are not substitutes.
- Canonical note + Plane tickets: `docs/research/rmm-reflective-memory-management.md`,
  **WEFT-732** (retrospective) then **WEFT-733** (prospective). Do not
  silently promote reranker weights (ADR-096).

**Updated 2026-10-04:** the L1 retrospective path is built (WEFT-732), opt-in via
`agents.memory_recall.enabled` (default off, so the full-file dump is still the
default and the fail-open path). When on, `ContextBuilder::build_messages_with_query`
injects at most `top_m` of `top_k` retrieved `MEMORY.md` snippets tagged `[m1]`…, and
`AgentLoop` feeds `+1` cited / `-1` ignored to the reranker (SONA under
`hybrid-rerank`, identity otherwise); the retriever is never updated. Code:
`crates/clawft-core/src/agent/memory_recall.rs`. The L3 prospective path is built too
(WEFT-733): the WEFT-347 consolidator, now run from `AgentLoop` when
`agents.memory_consolidation.enabled`, merges facts into topic nodes in `MEMORY.md`
(merge-or-insert by a deterministic topic key) instead of appending blocks
(`crates/clawft-core/src/agent/learning/topics.rs`).
