---
title: "Knowledge Graph & Graphify"
slug: knowledge-graph-graphify
workstream_id: "12"
release_gate: "0.7.0"
last_updated: 2026-04-28
audit_kind: "comprehensive"
audit_scope:
  - clawft-graphify
  - sprint-17 KG-NNN tasklist
  - GraphRAG / CausalRAG / SASE / RANGER / RoMem / TRACE / SGKR / CodaRAG / TransFIR
    paper surveys
  - HNSW + ruvector / RVF integration (ruvector-diskann@2.1, shaal PR #352)
  - ADR-009 (sparse-lanczos), ADR-011 (no-frankensearch),
    ADR-029 (rvf-crypto-fork), ADR-031 (rvf-wire-mesh-format)
related_workstreams:
  - "02 — kernel"
  - "11 — agent-core (E2/E3 routers using ruvector)"
status: "active sprint, mostly landed; integration + perf gaps remain"
verdict: "NOT-A-SHIP-GATE; this is an inventory of total depth"
---

# Knowledge Graph & Graphify

## General Description

`clawft-graphify` is the WeftOS knowledge-graph extraction, analysis, and
query engine. It ports the original Python `graphify` (11 K LOC, 20 modules,
126 tests; `.planning/graphify-rs/MASTER_PLAN.md`) into the Rust workspace
and connects it to the kernel's spectral / causal stack
(`crates/clawft-kernel/src/causal.rs`, `causal_predict.rs`,
`hnsw_service.rs`, `vector_*.rs`).

Two domains share one substrate:

1. **Code assessment** — tree-sitter AST extraction (10 grammars), cross-file
   import resolution, community detection (label propagation), structural
   analysis (god nodes, surprising connections, suggested questions, graph
   diff), and seven export formats (JSON, GraphML, HTML/vis.js, Obsidian,
   wiki, VOWL, Cypher). Wiring lives in `crates/clawft-graphify/src/`
   (~19 K lines across 49 files; see `wc -l` inventory below).

2. **Forensic analysis** — persons / events / evidence / locations / timelines
   with `gap_analysis()`, `coherence_score()`, and `counterfactual_delta()`
   in `domain/forensic.rs`.

Sprint 17 ("Knowledge Graph & Analysis Upgrade",
`.planning/development_notes/sprint-17-tasklist.md`) layered 18 KG-NNN
tasks on top derived from the 22-paper surveys
(`knowledge-graph-paper-survey{,-phase2}.md`), the EML synergy scan
(`eml-synergy-scan.md`), and the shaal ruvector PR #352. As of this audit,
**11 of 18 KG-tasks have at least a first-cut implementation in code**;
the remainder are P1/P2 backlog items or are blocked on upstream
ruvector / ruvllm-wasm releases.

A second, parallel Sprint 17 lives in `.planning/sprint17.md`
("Security Hardening + Ontology Graph Pipeline") and contains the
WebVOWL/VOWL/OWL-RDF ontology-graph items (OG-1..OG-4). These are
graphify-adjacent; OG-1 already shipped (`export/vowl.rs`, 231 lines).

Workstream 12 also owns the HNSW retrieval surface that agent-core
streams (workstream 11) consumes for the E2 `EmbeddingRouter` and E3
`HybridRouter` (see `docs/handoff.md`, `docs/research/rvf-context-router.md`).
The **11-pattern HNSW cap in `ruvllm-wasm` v2.0.1** that blocks the v3
`MicroLoraRouter` lives in this workstream's blast radius
(`docs/research/rvf-context-router.md:118-128`).

---

## Status & Timeline

| Phase | Window | Status |
|-------|--------|--------|
| Graphify Rust port — Phase 1A (core model + AST) | 2026-04-04 | Shipped (`.planning/graphify-rs/phase1a-notes.md`, 37 unit tests) |
| Phase 2 (analysis + export) | 2026-04-04 | Shipped (analyze.rs / cluster.rs / export/ / report.rs) |
| Phase 3 (kernel bridge + domain layers) | 2026-04-04 | Shipped (`bridge.rs`, `domain/code.rs`, `domain/forensic.rs`; 79 tests) |
| Phase 4-5 (semantic/vision extract, ingest, watch, hooks, exports, CLI) | 2026-04-04 | Shipped (`phase45-notes.md`, 61 graphify + 13 weave tests) |
| Sprint 16 (vector hardening + hybrid HNSW/DiskANN backend) | 2026-03/04 | Shipped (`sprint-16/vector-hardening.md`, `vector-hybrid.md`; 70 vector tests) |
| Sprint 17 (KG-NNN paper synergy) — KG-001..KG-010, KG-014, KG-016 | 2026-04-16 → present | **In flight** — most landed, see Task List for residue |
| Sprint 17 (Ontology Graph Pipeline) — OG-1..OG-4 | 2026-04-16 → present | OG-1 (VOWL export) shipped; OG-2..OG-4 open |
| Adaptive HNSW tiered search v0.6.13 | 2026-04-17 | Shipped (`adaptive-hnsw-tiered-search.md`) — control 0.69 / tiered 0.79 recall, 1.61× faster |
| Ontology Navigator symposium follow-on (Phase 5 codebase schema agent) | Sprint 22 (planned) | Not started |
| RVF context router v2 (EmbeddingRouter via `ruvector-diskann@2.1`) | 2026-04-27 | Shipped in agent-core-v1 (workstream 11) — this workstream owns the substrate |
| RVF context router v3 (`MicroLoraRouter`) | Deferred | **Blocked on `ruvllm-wasm` lifting 11-pattern HNSW cap** |
| 0.7.0 GUI release | TBD | Sprint 17 KG work continues on `0.6.x` series; v0.7.0 = GUI complete (per sprint-17-tasklist.md header) |

**Active branch:** `development-0.7.0` (clean at audit time).
Sprint 17 is open and KG-NNN work is proceeding against `0.6.x`; v0.7.0
is the GUI-complete milestone.

---

## Released Features

### Extraction & ingest
- AST extraction via tree-sitter for 10 languages: Python, JS/TS, Rust,
  Go, Java, C, C++, Ruby, C#, plus generic. Each behind a `lang-*`
  feature gate; `lang-all` rolls them up
  (`crates/clawft-graphify/Cargo.toml`).
- Cross-file resolution in `extract/cross_file.rs` (currently
  Python-leaning; planned extension hook).
- Generic `LanguageConfig`-driven extractor in `extract/ast.rs` (469 lines).
- Tree-calculus AST shape detection in `extract/treecalc.rs` (855 lines)
  feeding the topology dispatcher.
- Detection / classification / sensitive-file filtering in
  `extract/detect.rs` (599 lines).
- BLAKE3 content-hash cache in `cache.rs` with `EXTRACTOR_VERSION`
  invalidation, atomic writes, GC.
- URL ingestion (`ingest.rs`, 540 lines) with SSRF protection
  (file://, localhost, 10/172.16-31/192.168/127 blocked) and
  arxiv / tweet / webpage / PDF / image fetchers behind a `HttpClient`
  trait.
- File watcher (`watch.rs`, 187 lines) — polling default, `notify` crate
  behind feature gate, debounce, code-vs-non-code filter.
- Git hooks (`hooks.rs`, 254 lines) — install / uninstall / status for
  post-commit and post-checkout, calls `weaver graphify rebuild`.
- Vault integration (`vault/{mod,frontmatter,links,analyze,suggest}.rs`)
  for Obsidian-style document graphs (v0.6.11).

### Build & analysis
- `KnowledgeGraph` over `petgraph::Graph<Entity, Relationship, Directed>`
  with `HashMap<EntityId, NodeIndex>`; idempotent insert; subgraph
  extraction; remove-entity with edge cleanup.
- `EntityId = BLAKE3(domain || type_disc || name || source_file)` plus
  `from_legacy_string()` for Python compatibility.
- 26 entity types (12 code + 12 forensic + File + Concept + Custom),
  23 relationship types, frozen discriminants for ID stability.
- Community detection via deterministic label propagation with
  oversized-community recursive splitting (>25 % of graph, ≥10 nodes)
  and cohesion scoring (`cluster.rs`, 710 lines).
- God-node ranking (degree-based, file/concept-aware), surprising-edge
  scoring (7 features + EML model), suggested-question generation
  (5 strategies, deterministic), graph-diff (`analyze.rs`, 1792 lines).
- EML scorer models in `eml_models.rs` (704 lines) — surprise scorer,
  cluster threshold, query fusion (KG-001), community summary, others.
- Topology / triage dispatch in `topology.rs` and `topology_infer.rs`
  feeding ADR-treecalc-eml architecture.

### Sprint 17 KG-NNN (delivered in tree)
- **KG-001 — EML score fusion for `weaver graphify query`**: shipped.
  `QueryFusionModel` in `eml_models.rs:453+`; wired in
  `clawft-weave/src/commands/graphify_cmd.rs:277` ("KG-001 hybrid score
  fusion").
- **KG-002 — community summary generation (GraphRAG)**: shipped.
  `summary.rs` (425 lines), `KnowledgeGraph::community_summaries`
  field, `generate_community_summaries()`, query-side dispatch in
  `graphify_cmd.rs:265`.
- **KG-003 — causal chain tracing (CausalRAG)**: shipped.
  `trace_causal_chain()` in `clawft-kernel/src/causal.rs:3060`.
- **KG-004 — RFF spectral embedding (SASE)**: shipped.
  `causal.rs:3171` "Random Fourier Feature Spectral Analysis"
  alongside Lanczos. Performance comparison vs Lanczos on test graphs
  is the open piece (see Task List).
- **KG-005 — information-gain pruning**: shipped in
  `clawft-kernel/src/causal_predict.rs:243` "Information Gain Pruning
  (KG-005)".
- **KG-006 — BFS dependency-graph retrieval (SGKR)**: shipped at
  `clawft-graphify/src/model.rs:422,866` "BFS Data Flow Tracing".
- **KG-007 — MCTS graph exploration (RANGER)**: shipped at
  `analyze.rs:998` with tests at `analyze.rs:1701`.
- **KG-008 — entity dedup via HNSW (CodaRAG)**: shipped at
  `model.rs:529,1008` "Entity Deduplication".
- **KG-009 — geometric shadowing for memory decay (RoMem)**: shipped
  at `causal.rs:1755`, tests at `causal.rs:3286`.
- **KG-010 — multi-hop traversal with priors (TRACE)**: shipped at
  `analyze.rs:1229` and `analyze.rs:1602` "Multi-hop Beam Search".
- **KG-011 — LogQuantized for DiskANN (shaal PR #352)**: **stub only**
  — `vector_quantization.rs:22-86`, `is_available() -> false` with
  `TODO(KG-011): Check ruvector-core version once PR #352 merges`.
  Config types ship; activation gated on upstream merge.
- **KG-012 — unified SIMD distance kernel (shaal PR #352)**: **stub
  only** — `vector_quantization.rs:89-153`, `is_available() -> false`,
  `TODO(KG-012)` mirrors KG-011.
- **KG-014 — codebook cold-start (TransFIR)**: shipped at
  `causal.rs:1948,3496` "VQ Codebook Cold-Start".
- **KG-016 — conversational graph exploration**: shipped as
  `clawft-graphify/src/conversation.rs` (533 lines) — multi-turn
  `ConversationContext` with focus / visited / topic-stack / followups.

### Vector / HNSW (workstream 12 substrate; consumed by 02 + 11)
- Three-tier adaptive HNSW search via tree-calculus triage
  (Atom / Sequence / Branch) and EML parameter selection — shipped
  v0.6.13. Coarse 20-dim index, medium 40-dim re-rank, fine 128-dim
  re-rank. ExoChain events `hnsw.eml.{observe,recall,trained,triage}`.
- `VectorBackend` trait + four implementations (`vector_hnsw.rs`,
  `vector_diskann.rs` stub, `vector_hybrid.rs` hot+cold) — Sprint 16.
- Epoch-based versioning, optimistic concurrency control
  (`insert_with_epoch` → `EpochConflict`), soft-delete + tombstone +
  compaction, capacity limits (`StoreFull`) — Sprint 16.

### Export & visualization
- JSON (`node_link_data` Python-compat), GraphML, HTML/vis.js
  interactive, Obsidian vault + canvas, Wikipedia-style wiki articles,
  VOWL (OG-1, WebVOWL-compatible), Cypher (Neo4j). All under
  `export/*.rs`; ~2 K lines.
- Force-directed and tree layouts (`layout/{force,tree,slicer,triage,
  positioned}.rs`) — Reingold-Tilford trees + Barnes-Hut force
  layouts. `layout/mod.rs:49` notes Sugiyama (layered) layout is a
  TODO (currently falls back to tree layout).

### CLI surface (`weaver graphify …`)
- `ingest`, `query`, `export`, `diff`, `rebuild`, `watch`,
  `hooks {install|uninstall|status}` — wired in
  `clawft-weave/src/commands/graphify_cmd.rs`.

### Bridge & integration
- `GraphifyBridge` (feature `kernel-bridge`) connects KG to
  `CausalGraph`, `HnswService`, `CrossRefStore` with `EmbeddingProvider`
  trait. `RelationType → CausalEdgeType` mapping in `bridge.rs`
  (914 lines).
- `GraphifyAnalyzer` registers as the 9th kernel `Analyzer` per
  ADR-023. Findings: god nodes → complexity, surprising connections
  → dependencies, singleton communities → architecture.

---

## What's Left — Total Depth

This is the comprehensive inventory. Items here are NOT ranked for v0.7.0
ship-readiness; they are simply the open surface area in workstream 12.

### TODOs / FIXMEs in source

| File:Line | Marker | Description |
|-----------|--------|-------------|
| `crates/clawft-graphify/src/layout/mod.rs:49` | `TODO: Sugiyama` | `Geometry::Layered` falls back to `layout_as_tree` instead of a Sugiyama / hierarchical-graph layered layout |
| `crates/clawft-kernel/src/vector_quantization.rs:83` | `TODO(KG-011)` | Check `ruvector-core` version once PR #352 merges; until then `LogQuantizedConfig::is_available()` returns `false` |
| `crates/clawft-kernel/src/vector_quantization.rs:150` | `TODO(KG-012)` | Same gate for `SimdDistanceConfig::is_available()` (UnifiedDistanceParams) |
| `crates/clawft-graphify/src/extract/lang/python.rs:39-43` | rationale prefixes | Python rationale-comment scraper recognizes `# HACK:`, `# TODO:`, `# FIXME:` (intentional, not a TODO marker itself) |

The graphify crate has no other in-source TODO/FIXME/XXX markers, but
several modules contain dead-code allowances and `unwrap()` calls in
non-test paths in `ingest.rs` (regex compilation lines 112-185, 200, 225,
251) that should be `OnceLock` / `Lazy` (not strictly TODOs but technical
debt of the same colour).

### Deferred items (Sprint 17 P1 / P2)

| ID | Title | Status | Where it lands |
|----|-------|--------|----------------|
| KG-011 | LogQuantized for DiskANN | **Stub-only**, `is_available()=false` | `clawft-kernel/src/vector_quantization.rs:22-86` |
| KG-012 | Unified SIMD distance kernel | **Stub-only**, `is_available()=false` | `clawft-kernel/src/vector_quantization.rs:89-153` |
| KG-013 | Spatio-temporal GNN for sonobuoy (K-STEMIT) | **Not started** | Would land in new `clawft-sensor/` crate or sonobuoy firmware |
| KG-015 | EA-Agent entity alignment | **Not started** | Multi-repo dedup; LLM-agent based |
| KG-017 | Knowledge distillation for edge EML (SevenNet-Nano) | **Not started** | Distill depth-4 EML → depth-2 for WASM/ESP32 |
| KG-018 | Newman modularity scoring | **Not started** | Alternative to current cohesion scoring in `cluster.rs` |
| OG-2 | OWL/RDF ingestion (Turtle, JSON-LD) | **Not started** | Add `oxigraph` or `sophia` crate; map RDF triples → graphify |
| OG-3 | Rust-native force-directed layout for headless | Partial — basic force layout exists in `layout/force_layout.rs` but Barnes-Hut O(n log n) and the SVG positioned-output pipeline still need work | `layout/force_layout.rs` (216 lines) |
| OG-4 | VOWL visual encoding rules in SVG export | **Not started** | Layer onto `export/html.rs` or new SVG export |

Within the agent-core handoff:

- **`MicroLoraRouter` (v3 context router)** — explicitly deferred until
  `ruvllm-wasm` lifts the documented 11-pattern HNSW cap
  (`docs/research/rvf-context-router.md:118-128`,
  `docs/handoff.md:65`). E3's `HybridRouter` left a
  `TODO(agent-core-v1 phase E3+)` marker. This is the single largest
  cross-stream blocker between WS12 (graphify/HNSW) and WS11 (agent-core).

### Carry-over from `phase45-notes.md` "Remaining Work"

- Full extraction-pipeline integration in `weaver graphify rebuild`
  (currently the rebuild CLI exists but the integration path against the
  real extraction pipeline is partially stubbed).
- Real HTTP client injection for URL ingestion — `StubHttpClient` is the
  default; production reqwest-based client is not yet wired.
- `notify`-crate watcher behind a feature gate (currently polling-only by
  default; `notify` is an optional dep).
- MCP server (Phase 6 of the master plan).
- Benchmarks (Phase 6) — `benches/extraction.rs`, `benches/graph_ops.rs`
  named in master plan; not present in tree.

### Carry-over from `sprint-16/vector-hybrid.md` "Next steps"

- Wire `VectorBackend` into `DemocritusLoop` (currently uses raw
  `HnswService`).
- Add `ecc.vector-config` RPC endpoint to show active backend.
- Implement the real DiskANN backend when `ruvector-diskann` publishes
  the production crate (the workspace already declares
  `ruvector-diskann = "2.1"` per `docs/research/rvf-context-router.md`,
  but `vector_diskann.rs` still uses a brute-force `HashMap` linear-scan
  stub).
- Add `diskann` feature flag gating for the real implementation.
- Benchmark hybrid vs. pure HNSW for ECC workloads.

### Open questions

1. **Spectral analysis path selection** — Lanczos (ADR-009, sparse
   O(k·m)) vs RFF (KG-004) vs EML-approximated lambda₂
   (`arxiv-2603-21852-analysis.md`). Need a benchmark on graphs of
   1 K / 10 K / 100 K nodes and a clear rule for which path runs when.
2. **Incremental graph updates** — `arxiv-2410-05779-analysis.md`
   identifies LightRAG's set-union-with-dedup as a P1 win (10-100×
   faster re-analysis). `pipeline.rs::Pipeline::run_incremental()` is
   sketched in the analysis doc but not implemented in tree.
3. **Multi-key HNSW indexing** — LightRAG-style multi-key embedding
   (entity name, type, context) per LightRAG analysis P2. Not
   implemented; would touch `hnsw_service.rs::insert()` and the bridge
   embedding flow.
4. **Edge embeddings** — embed relationships not just entities, for
   "how does X interact with Y?" queries (LightRAG P5).
5. **Graph-aware HNSW re-ranking** — re-rank HNSW neighbors by graph
   topology (LightRAG P4).
6. **HNSW-EML opportunities not yet implemented** — see
   `hnsw-eml-deep-analysis.md` priority list. Items 3 (cosine
   decomposition / dimension selection, 10-30× distance speedup),
   4 (search-path prediction, 2-5× search speedup), and especially
   8 (progressive dimensionality, 5-20× search speedup) are open. Items
   5 (neighbor-quality), 7 (layer probability), 9 (cache-aware
   prefetch), 10 (PQ correction) are research-grade.
7. **Eleven-pattern HNSW wall** — `ruvllm-wasm` v2.0.1 caps
   `HnswRouter` at ~11 patterns. This is a hard architectural blocker
   for v3 context routing and for any "many-pattern" use case in
   graphify-driven retrieval. Need either an upstream PR to lift the
   cap or a switch to native `ruvector-diskann@2.1` (already a
   workspace dep, but native-only — no WASM).
8. **EML synergy candidates not yet wired** — `eml-synergy-scan.md`
   enumerates ~30 hand-tuned heuristics across `analyze.rs`,
   `cluster.rs`, `export/html.rs`, `pipeline.rs`, `report.rs`,
   `domain/forensic.rs`, `build.rs` that are EML-replaceable but
   currently still hardcoded. Of those, only the surprise scorer,
   cluster threshold, and query fusion (KG-001) are EML-wired.
9. **Hyperedges** — `Hyperedge` type exists at `model.rs` and
   `KnowledgeGraph::hyperedges` is populated by analysis but no
   first-class hyperedge detection algorithm runs in the pipeline yet
   (`pipeline.rs` does not call a `discover_hyperedges()` step).
10. **Sugiyama layered layout** — `layout/mod.rs:49`. Falling back to
    tree layout silently is a behavioural footgun for users who request
    `Geometry::Layered`.
11. **Vault domain hyperedges** — `vault/` ships v0.6.11 frontmatter and
    wikilink analysis but the SUGGEST → ratify → CRDT pipeline from the
    Ontology Navigator symposium (Sprint 22 plan, Phase 5 codebase
    schema agent) is not started.
12. **Forensic gap-analysis scaling** — `phase3-notes.md` flags
    `gap_analysis()` is O(n·m) and "should be optimized with indexes
    for large graphs". Acceptable for typical case sizes (<10 K
    entities) but a known cliff.
13. **EML coherence two-tier cadence** — the canonical two-tier pattern
    (fast EML every tick + slow ground truth periodically) is laid out
    in `eml-coherence.md` and `hnsw-eml-analysis.md` §6 but the slow-path
    sampling cadence in `hnsw_store.rs` is still a fixed
    `rebuild_threshold = 100`. Adaptive rebuild (HNSW-EML §2g) is open.

### Orphaned work / not-tied-to-an-ID

- `topology_infer.rs` (353 lines) and `alignment.rs` (453 lines) are
  shipped but are sparsely referenced from `pipeline.rs`. They power the
  Topology dispatcher and the cross-graph alignment story but are
  pre-Sprint-17 work; their interaction with KG-001..KG-018 is not
  documented.
- `vision_extract.rs` (246 lines) is feature-gated (`vision-extract`)
  and shipped, but no end-to-end test runs through it (the
  feature-gate is off by default and no fixture in
  `crates/clawft-graphify/schemas/`).
- `validation.rs` (213 lines) implements JSON-shape validation only;
  schema-based edge validation is flagged "Minimal" in
  `.planning/symposiums/ontology-navigator/session-4-pipeline-findings.md`.
- The `clawft-llm` workspace dep is declared optional in
  `Cargo.toml:67` but **no feature in this crate enables it** — semantic
  extraction takes an `FnOnce(String) -> Future` callback instead. This
  is intentional (testability) but means the LLM bridge feature flag
  named in `phase45-notes.md` is effectively dead.
- ADR-049 ("pending" per `MASTER_PLAN.md:3`) is the graphify port ADR;
  ADR-050..ADR-053 candidates from `knowledge-graph-paper-survey-phase2.md
  §513-527` (Temporal Phase Rotation, Dependency-Graph Retrieval, Entity
  Dedup HNSW Pre-filter, Codebook Cold-Start, Spatio-Temporal Sensor
  Architecture) — none of these have ADR documents in `docs/adr/` even
  though their implementations have largely landed in tree.
- The Sprint 17 dual focus (security hardening in `sprint17.md` vs KG
  upgrades in `development_notes/sprint-17-tasklist.md`) means the
  ontology graph pipeline (OG-1..OG-4) is sometimes confused with the
  KG-NNN tasks. OG-1 (VOWL export) is shipped; OG-2..OG-4 are open and
  need a clear owner.

### Cross-references to other workstreams

- **Workstream 02 (kernel)** — owns `causal.rs`, `causal_predict.rs`,
  `hnsw_service.rs`, `vector_*.rs`, `vector_quantization.rs`. KG-003,
  KG-004, KG-005, KG-009, KG-011, KG-012, KG-014 implementations live
  there but are conceptually graphify retrieval features.
- **Workstream 11 (agent-core)** — consumes the HNSW substrate via
  `EmbeddingRouter` (E2, `ruvector-diskann@2.1`) and `HybridRouter`
  (E3, with `TODO(agent-core-v1 phase E3+)` marker for v3 MicroLora).
  The 11-pattern HNSW cap blocks v3 here.

---

## Task List

### Sprint 17 P0 (KG-001..KG-007)

| ID | Title | Effort | Status |
|----|-------|--------|--------|
| KG-001 | EML score fusion for `weaver graphify query` | M | **Done** — `QueryFusionModel` in `eml_models.rs:453+`; CLI dispatch at `graphify_cmd.rs:277` |
| KG-002 | Community summary generation (GraphRAG) | M | **Done** — `summary.rs`, `KnowledgeGraph::community_summaries`, query at `graphify_cmd.rs:265` |
| KG-003 | Causal chain tracing (CausalRAG) | S | **Done** — `causal.rs:3060` `trace_causal_chain` |
| KG-004 | Random Fourier spectral embedding (SASE) | M | **Done in tree** — `causal.rs:3171`. **Open**: benchmark vs Lanczos on 1 K / 10 K / 100 K node graphs and document the size-threshold dispatch. |
| KG-005 | Information-gain pruning | S | **Done** — `causal_predict.rs:243` |
| KG-006 | BFS dependency-graph retrieval (SGKR) | M | **Done** — `model.rs:422,866` BFS data-flow tracing |
| KG-007 | MCTS graph exploration (RANGER) | L | **Done** — `analyze.rs:998`; tests at `analyze.rs:1701` |

### Sprint 17 P1 (KG-008..KG-014)

| ID | Title | Effort | Status |
|----|-------|--------|--------|
| KG-008 | Entity dedup via HNSW (CodaRAG) | S | **Done** — `model.rs:529,1008` |
| KG-009 | Geometric shadowing for memory decay (RoMem) | M | **Done** — `causal.rs:1755`; tests at `causal.rs:3286` |
| KG-010 | Multi-hop traversal with priors (TRACE) | M | **Done** — `analyze.rs:1229`; beam search at `analyze.rs:1602` |
| KG-011 | LogQuantized for DiskANN (shaal PR #352) | M | **Stub** — config types in `vector_quantization.rs:22-86`, `is_available()=false`. Blocked on upstream merge. |
| KG-012 | Unified SIMD distance kernel (shaal PR #352) | S | **Stub** — config in `vector_quantization.rs:89-153`. Blocked on upstream merge. |
| KG-013 | Spatio-temporal GNN for sonobuoy (K-STEMIT) | L | **Not started** — would create `clawft-sensor/` or live in sonobuoy firmware |
| KG-014 | Codebook cold-start (TransFIR) | S | **Done** — `causal.rs:1948,3496` VQ codebook |

### Sprint 17 P2 (KG-015..KG-018)

| ID | Title | Effort | Status |
|----|-------|--------|--------|
| KG-015 | EA-Agent entity alignment | L | **Not started** — multi-repo dedup, LLM-agent driven |
| KG-016 | Conversational graph exploration | M | **Done** — `conversation.rs` (533 lines) |
| KG-017 | Knowledge distillation for edge EML (SevenNet-Nano) | M | **Not started** — depth-4 → depth-2 distillation for WASM/ESP32 |
| KG-018 | Newman modularity scoring | S | **Not started** — alternative to cohesion in `cluster.rs` |

### Ontology Graph Pipeline (sprint17.md OG-1..OG-4)

| ID | Title | Status |
|----|-------|--------|
| OG-1 | VOWL JSON export | **Done** — `export/vowl.rs` (231 lines) |
| OG-2 | OWL/RDF ingestion (Turtle, JSON-LD) | **Open** — needs `oxigraph` or `sophia` |
| OG-3 | Rust-native force-directed layout | **Partial** — basic force layout in `layout/force_layout.rs`; Barnes-Hut + positioned-SVG export still open |
| OG-4 | VOWL visual encoding rules | **Open** — layer onto `export/html.rs` or new SVG |

### Carry-over from earlier phases

| Source | Item | Status |
|--------|------|--------|
| `phase45-notes.md` | Full extraction pipeline integration in `weaver graphify rebuild` | Open |
| `phase45-notes.md` | Real reqwest-based HTTP client for URL ingestion | Open (`StubHttpClient` is default) |
| `phase45-notes.md` | `notify`-crate watcher behind feature gate | Partial (feature exists; default is polling) |
| `phase45-notes.md` | MCP server (Phase 6) | Open |
| `phase45-notes.md` | Benchmarks (`benches/extraction.rs`, `benches/graph_ops.rs`) | Open |
| `phase3-notes.md` | Index-based optimization for `gap_analysis()` (currently O(n·m)) | Open — only matters >10 K entities |
| `MASTER_PLAN.md` | ADR-049 (graphify port) | "pending" — never written |
| `MASTER_PLAN.md` | Layout `cypher.rs` export named in plan but realized as Cypher in `export/wiki.rs` flow only; no standalone `export/cypher.rs` | Verify-or-add |
| `MASTER_PLAN.md` | `export/svg.rs` named in plan; not in tree | Open |

### Sprint 16 carry-over (vector substrate consumed by WS12)

| Source | Item | Status |
|--------|------|--------|
| `vector-hybrid.md` | Wire `VectorBackend` into `DemocritusLoop` | Open |
| `vector-hybrid.md` | `ecc.vector-config` RPC | Open |
| `vector-hybrid.md` | Real DiskANN backend (replace brute-force stub) | Blocked on `ruvector-diskann` publishing the production crate. Workspace already declares `ruvector-diskann = "2.1"` per `rvf-context-router.md:53` but `vector_diskann.rs` is still a `HashMap` linear-scan stub. |
| `vector-hybrid.md` | `diskann` feature flag for real impl | Open |
| `vector-hybrid.md` | Hybrid vs pure HNSW benchmark for ECC | Open |
| `vector-hardening.md` | Persist tombstones in save/load format | Deferred to vector-sync work (WS4 / Gap #11) |

### LightRAG / arxiv-2410-05779 implementation opportunities

| Priority | Item | Where | Status |
|----------|------|-------|--------|
| P1 | Incremental graph updates (set-union dedup) | `pipeline.rs::run_incremental()` | Open |
| P2 | Multi-key HNSW indexing | `hnsw_service.rs::insert()` | Open |
| P3 | Dual-level question classification (local/global) | `analyze.rs::suggest_questions()` | Open — adds `QuestionLevel` enum |
| P4 | Graph-aware HNSW re-ranking | bridge between `hnsw_service.rs` and `analyze.rs` | Open |
| P5 | Relationship embeddings | `hnsw_service.rs` | Open |

### EML / arxiv-2603-21852 implementation opportunities

| Priority | Item | Where | Status |
|----------|------|-------|--------|
| Actionable | Lambda₂ approximation via shallow EML regression | `clawft-kernel/src/eml_coherence.rs` (already two-tier pattern) | Open — fast O(1) coherence check using EML formula trained on (graph_features → λ₂) data |
| Actionable | Learned surprise scoring (replace hand-crafted `surprise_score` with EML composite) | `analyze.rs` + `eml_models.rs::SurpriseScorerModel` | Partial — model exists but only gates the existing 7-feature linear logic; full EML composite is open |
| Monitor | Interpretable EML attention for graph attention | `ruvector-attention` (external dep) | Watch upstream |
| Monitor | EML symbolic regression of calibration curves | `clawft-kernel/src/calibration.rs` | If ECC calibration goes adaptive |

### HNSW-EML opportunities (`hnsw-eml-analysis.md`, `hnsw-eml-deep-analysis.md`)

| # | Item | Speedup / recall | Effort | Status |
|---|------|------------------|--------|--------|
| 2b/1 | Adaptive ef (beam width per query) | 1.5-3× search | S | Open — feature wires into `HnswStore::query()` |
| 2a/2 | Learned distance function | +2-5 % recall, ~3× distance cost | M | Open |
| 2g/6 | Learned rebuild threshold | 1.1-1.3× | S | Open |
| 2e | Multi-entry-point routing | 1.3-2× | M | Open — requires custom HNSW or fork of `instant-distance` |
| 2f/9 | Prefetch prediction (cache-aware traversal) | 1.2-1.5× memory-bound | M | Open |
| 2c/7 | Learned layer assignment | 1.1-1.5× | L | Open — requires custom HNSW |
| 2d/5 | Learned neighbor pruning | +1-2 % recall | L | Open — requires custom HNSW |
| 2h/10 | Learned PQ codebooks (DiskANN) | +5-15 % recall (compressed) | L | Open — DiskANN-only |
| 4 | Search-path prediction (region → entry-node lookup) | 2-5× | M | Open — biggest single win per deep analysis |
| 8 | Progressive dimensionality (per-layer projected distances) | 5-20× | L | Open — combines dimension-selection + path-prediction |
| 3 | Cosine-similarity decomposition (dimension selection) | 10-30× distance | M | Open |

### Cross-stream blockers

| ID | Description | Blocks | Workaround |
|----|-------------|--------|-----------|
| 11-PAT-CAP | `ruvllm-wasm` v2.0.1 documents an ~11-pattern HNSW cap (`docs/research/rvf-context-router.md:118-128`) | v3 `MicroLoraRouter` (WS11 E3+); any "many-pattern" routing in graphify | Use native `ruvector-diskann@2.1` for native targets; live with 11-pattern cap on WASM until upstream lifts it |
| RVD-2.1-PUB | `ruvector-diskann` 2.1 declared in workspace but production crate not yet published; current backend is a brute-force `HashMap` stub | KG-011 LogQuantized, KG-012 SIMD distance, full DiskANN backend | Use HNSW backend; hybrid backend already merges hot HNSW + cold-stub |
| SHAAL-352 | Upstream `ruvector-core` PR #352 (LogQuantized + UnifiedDistanceParams) not yet merged | KG-011, KG-012 activation | Stubs ship `is_available()=false`; configs are wire-ready |

### Open ADRs

| ID | Title | Status |
|----|-------|--------|
| ADR-049 | Graphify port (named in `MASTER_PLAN.md:3` as pending) | **Not written** |
| ADR-050 | Dependency-graph retrieval in graphify (SGKR adoption) | Candidate per `knowledge-graph-paper-survey-phase2.md:519`; KG-006 shipped without ADR |
| ADR-051 | Entity deduplication via HNSW pre-filter (CodaRAG) | Candidate per `phase2.md:521`; KG-008 shipped without ADR |
| ADR-052 | Codebook cold-start for emerging entities (TransFIR) | Candidate per `phase2.md:523`; KG-014 shipped without ADR |
| ADR-053 | Spatio-temporal dual-branch architecture for sensor systems (K-STEMIT) | Candidate per `phase2.md:525`; KG-013 not started |

---

## Sources

### Primary planning docs
- `.planning/development_notes/sprint-17-tasklist.md` — KG-001..KG-018 with effort, source paper, target files
- `.planning/sprint17.md` — security + ontology graph pipeline OG-1..OG-4
- `.planning/graphify-rs/MASTER_PLAN.md` (1112 lines) — port plan, source inventory, crate layout, phase tasks
- `.planning/graphify-rs/architecture.md` — module-level architecture
- `.planning/graphify-rs/analysis-algorithms.md` — clustering / surprise / question / diff algorithms
- `.planning/graphify-rs/analysis-extraction.md` — AST extraction strategy
- `.planning/graphify-rs/phase1a-notes.md`, `phase3-notes.md`, `phase45-notes.md` — landed-work notes per phase

### Paper surveys
- `.planning/development_notes/knowledge-graph-paper-survey.md` — 15 papers, P0/P1/P2 with WeftOS applicability per paper, implementation roadmap, cross-cutting themes
- `.planning/development_notes/knowledge-graph-paper-survey-phase2.md` — 7 additional papers (RoMem, TRACE, SevenNet-Nano, SGKR, CodaRAG, TransFIR, K-STEMIT), synergy map, ADR candidates ADR-049..ADR-053
- `.planning/development_notes/arxiv-2410-05779-analysis.md` — LightRAG analysis, 5-priority implementation list
- `.planning/development_notes/arxiv-2603-21852-analysis.md` — EML "all elementary functions" paper, lambda₂ approximation proposal
- `.planning/development_notes/eml-synergy-scan.md` — ~30 hardcoded heuristics in graphify identified as EML candidates

### HNSW / vector deep dives
- `.planning/development_notes/adaptive-hnsw-tiered-search.md` — three-tier search v0.6.13 results
- `.planning/development_notes/hnsw-eml-analysis.md` — 8 HNSW-EML opportunities, top-3 implementation plans
- `.planning/development_notes/hnsw-eml-deep-analysis.md` — 10 deep opportunities including search-path prediction, progressive dimensionality
- `.planning/development_notes/sprint-16/vector-hybrid.md` — hybrid HNSW+DiskANN backend status, "Next steps" list
- `.planning/development_notes/sprint-16/vector-hardening.md` — epoch / soft-delete / capacity / OCC

### Ontology Navigator symposium (graphify-adjacent)
- `.planning/symposiums/ontology-navigator/synthesis.md` — Phase 5 codebase schema agent, runs `graphify + cluster → inferred schema`
- `.planning/symposiums/ontology-navigator/qa-decisions.md` — schema inference from graphify output
- `.planning/symposiums/ontology-navigator/session-4-pipeline-findings.md` — graphify+vault status table
- `.planning/symposiums/ontology-navigator/adr-treecalc-eml-architecture.md` — graphify topology integration

### Architectural decision records
- `docs/adr/adr-009-sparse-lanczos.md` — sparse Lanczos at O(k·m), v0.2 deliverable
- `docs/adr/adr-011-no-frankensearch.md` — raw HNSW sufficient at <10 K entries; revisit at v0.3
- `docs/adr/adr-029-rvf-crypto-fork-strategy.md` — `weftos-rvf-crypto` and `weftos-rvf-wire` published forks
- `docs/adr/adr-031-rvf-wire-mesh-format.md` — RVF wire segments as zero-copy mesh format
- `docs/adr/adr-023-assessment-as-kernel-service.md` — graphify as 9th `Analyzer`

### Cross-stream
- `docs/handoff.md` — agent-core-v1 ships, v3 MicroLoraRouter deferred on 11-pattern HNSW cap
- `docs/research/rvf-context-router.md` — ruvector inventory, 11-pattern wall, v1/v2/v3 router design
- `crates/clawft-graphify/Cargo.toml` — feature gates (ast-extract, semantic-extract, vision-extract, code-domain, forensic-domain, html-export, neo4j-export, kernel-bridge, lang-*)

### Source inventory (audit-time line counts)
- 49 files, 19 487 total lines under `crates/clawft-graphify/src/`
- Largest modules: `analyze.rs` 1792, `model.rs` 1162, `bridge.rs` 914, `extract/treecalc.rs` 855, `cluster.rs` 710, `eml_models.rs` 704, `extract/detect.rs` 599, `build.rs` 575, `domain/forensic.rs` 542, `conversation.rs` 533, `topology.rs` 522, `export/html.rs` 519, `layout/slicer.rs` 485, `extract/ast.rs` 469, `report.rs` 454, `alignment.rs` 453, `layout/mod.rs` 452, `summary.rs` 425, `pipeline.rs` 411
- KG-NNN tags grep: `crates/clawft-graphify/src/{summary,model,analyze,conversation,eml_models}.rs`, `crates/clawft-kernel/src/{causal,causal_predict,vector_quantization}.rs`, `crates/clawft-weave/src/commands/graphify_cmd.rs`, `crates/clawft-types/src/config/kernel.rs`

<!-- TRIAGED-STAMP:BEGIN -->
## Triaged into Plane — 2026-04-28

All open items in this audit have been filed as Plane work items in the WeftOS workspace under the `ws12-knowledge-graph` label.

- **Range**: WEFT-351 … WEFT-387 (37 items)
- **Per cycle**: 0.8.x: 35, 0.9.x: 2
- **Triage spec**: `.planning/reviews/0.7.0-release-gate/triage/`
- **WEFT-N → name map**: `.planning/reviews/0.7.0-release-gate/triage/weft-mapping.json`

Per the project rule (CLAUDE.md → "Plane is the authoritative work tracker"): future updates to these items happen in Plane, not in this audit doc. This doc remains the source-of-truth for the original survey.
<!-- TRIAGED-STAMP:END -->
