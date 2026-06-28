# Brain · 05 — RVF Brain & Research Streams

> The intelligence substrate this brain is modeled on, and the research programs.
> Source-of-truth: `.planning/04-rvf-integration.md`, `.planning/05-ruvector-crates.md`,
> `docs/guides/rvf.md`, `crates/clawft-kernel/src/{causal,hnsw_service,cognitive_tick,
> weaver,impulse,embedding}.rs`, `.planning/{sonobuoy,sensors,actors,symposiums}/`.

## 1. RVF / ruvector primer

**RVF (RuVector Format)** is a single-file binary container from the ruvector
project that merges a vector DB, HNSW index, quantization codebooks, routing
policies, and a crypto audit chain — queryable via a ~5.5 KB embedded WASM
microkernel. It is the "brain substrate" because all memory + learned routing +
audit evidence travel as one portable `.rvf` file with no external DB/WAL.

**Segments clawft uses**: VEC (0x01) embeddings · INDEX (0x02) HNSW adjacency ·
MANIFEST (0x05) · QUANT (0x06) codebooks · META (0x07) config/metadata · HOT
(0x08) frequent entries · SKETCH (0x09) access tracking · WITNESS (0x0A)
SHAKE-256 audit chain · POLICY_KERNEL (0x31) routing params · COST_CURVE (0x32)
provider cost/latency/quality.

**Three-tier progressive HNSW**: Layer A (coarse, µs load, ~70% recall) → Layer B
(hot region, ~85%) → Layer C (full graph, ~95%). Sub-500 ms to first useful
retrieval on cold start.

**Temperature quantization** (SKETCH-driven): hot=fp16 (768 B/384-dim vec),
warm=PQ (~48 B, 16×), cold=binary (~48 B, 32×). 10K entries at 20/30/50
hot/warm/cold ≈ 2.0 MB total.

**Planned clawft storage**: `~/.clawft/workspace/memory/memory.rvf`,
`sessions/index.rvf`, `routing/policies.rvf`, `witness.rvf`. Bridged via
`rvf-adapters/agentdb`, exposed through `rvf__` MCP tools. **Status**: planned,
not yet implemented — the current `VectorStore` (`clawft-core/src/vector_store.rs`)
is an in-memory brute-force O(n·d) cosine scan with a SimHash `HashEmbedder` (not
semantic).

## 2. ECC cognitive substrate (implemented in `clawft-kernel`)

- **CausalGraph** (`causal.rs`): lock-free DashMap-backed directed graph; 8 typed
  edges (Causes, Inhibits, Correlates, Enables, Follows, Contradicts, TriggeredBy,
  EvidenceFor) + strength/timestamp/count; forward+reverse indexes;
  spectral_analysis (Fiedler), spectral_analysis_rff (RFF approx),
  spectral_partition; ExoChain-gated destructive ops. The long-term causal memory.
- **HnswService** (`hnsw_service.rs`): single/batch insert, top-k, dedup search,
  multi-key insert, tiered search (Layer A/B/C), recall measurement, file
  persistence, optional EML coherence layer, optional ChainManager witness binding.
- **CognitiveTick** (`cognitive_tick.rs`): kernel heartbeat (50 ms boot-calibrated,
  adaptive); stats tick_count/mean/p95/max compute_us/drift; drives the
  **DemocritusLoop** (`democritus.rs`) — processes Impulses, calls
  spectral_analysis_rff to detect incoherence, classifies impulse → CausalEdgeType,
  adds edge.
- **WeaverEngine** (`weaver.rs`): cognitive modeler integrating git (GitPoller) +
  source watch (FileWatcher) + embeddings (HNSW) + causal reasoning (CausalGraph) +
  learned strategy patterns (WeaverKnowledgeBase) into one feedback loop;
  ConfidenceReport/Gap cached every N ticks; ExportedModel for external use.
- **ImpulseQueue** (`impulse.rs`): Mutex<Vec<Impulse>> signal bus; 5 typed signals
  (BeliefUpdate, CoherenceAlert, NoveltyDetected, EdgeConfirmed, EmbeddingRefined) +
  Custom(u8); emit(type, payload, ttl, strength), drain_ready().
- **Embedding backends** (`embedding.rs`): EmbeddingProvider trait;
  MockEmbeddingProvider + LlmEmbeddingProvider; select_embedding_provider() factory.

## 3. How THIS brain is built (chunking + metadata)

The claude-flow MCP tools map onto RVF: `memory_store` → VEC+INDEX;
`agentdb_hierarchical-store` → adds the causal-tier structure; `embeddings_generate`
→ the embedder. This brain follows the project's own pattern.

**Granularity**: one decision/finding/spec per chunk, 100–300 tokens, self-contained.
Good boundaries = one ADR, one symposium decision, one paper finding, one
implementation note, one hardware spec line.

**Metadata schema** per chunk:
```json
{
  "namespace": "weftos/roadmap | weftos/kernel | weftos/architecture | weftos/adr |
                weftos/releases | weftos/bugs | weftos/rvf | weftos/research/*",
  "type": "adr | decision | research | implementation | sensor-spec | actor-spec |
           paper-finding | cost-model | bug | phase",
  "status": "active | superseded | draft | deferred | open | closed",
  "source_file": "<repo-relative path>",
  "date": "YYYY-MM-DD",
  "project_stream": "kernel-ecc | rvf | sonobuoy | sensors | actors | compositional-ui | ...",
  "causal_parents": ["<id>", ...]
}
```
`causal_parents` lets the brain reconstruct a CausalGraph: each chunk → CausalNode,
parents → Enables (precondition) or Follows (temporal) edges. Namespace governance
follows sonobuoy ADR-089 (one namespace per stream, no cross-namespace writes
except admin-scoped). The brain is stored in the `weftos/*` namespaces of the
ruvector store and queryable with `memory_search_unified` /
`agentdb_hierarchical-recall`.

## 4. Research streams

**Sonobuoy / underwater acoustics** — the largest research program
(`.planning/sonobuoy/`). Two paper-analysis rounds, 42 papers (Round 1 had 14
fabricated citations → drove ADR-062 verification-first; Round 2: 24 DOI/arXiv-
verified). Grounded in Urick's sonar equation, Wenz noise, KRAKEN/BELLHOP, and the
K-STEMIT 5-branch spatio-temporal architecture (temporal GLU, spatial GraphSAGE,
physics-prior, classification, active-imaging SAS). Hardware: 2"-PVC buoy, ESP32-S2
TX + ESP32-S3 RX split, 3 hydrophone variants, Airmar P79 imaging tier, cost ladder
~$45→~$11K/10-buoy fleet. FL = FedAvg + Deep Gradient Compression + Multi-Krum.
~25 ADR candidates (053–077) + ADR-078 ranging + 081–094 symposium ADRs. **Status**:
planning/research; no Rust crate scaffolded.

**ESP32 edge-sensor + actor firmware** (`.planning/sensors/`, `.planning/actors/`).
Foundational decision: **Node vs Actor** — a Node emits signed *measurements*
(`substrate/<node-id>/sensor/...`), an Actor emits signed *acts*
(`substrate/<actor-id>/ink/strokes/...`); both carry ed25519 keys, distinct even
when co-located. First Node: ESP32-S3 + INMP441 I2S mic → whisper.cpp HTTP service
(lifecycle-separated, not FFI) → `substrate/derived/transcript/mic`. First Actor:
Inkpad (CrowPanel DIS08070H, 800×480, GT911) emitting ink-stroke acts. **Status**:
active spike (this is what the uncommitted leaf-display work serves).

**Symposium design sessions** (`.planning/symposiums/`, 9 dirs): `compositional-ui`
(most mature — 10 sessions, 19 ADRs: primitive canon, WSP transport, event-sourced
diff tree, app manifest, IDE bridge), `cold-case-ecc` (ECC + explainable AI +
legal/compliance), `sonobuoy` (business+technical tracks, PPTX decks, ADRs 081–094),
`gaming-robotics`, `ontology-navigator` (EML/TreeCalc for ontology layer),
`RLM-arxiv-2512.24601` (RL-from-memory paper → ECC mapping), `cognitum-seed-gaps`
(tiered profiles), `krause-docs-generator` (client docs service), `lewm-worldmodel`.
