# RVF Integration Guide

RVF (RuVector Format) is a universal binary format from the
[ruvector](https://github.com/ruvnet/ruvector) project. A single `.rvf` file
merges a vector database, routing policies, quantization dictionaries, and a
cryptographic audit trail into one self-contained artifact. It is designed for
embedded and edge deployment where a multi-service stack is impractical.

This document explains how clawft's vector subsystem works today, what changes
RVF will introduce, and the concrete steps planned for Phase 3.

---

## Table of Contents

1. [Overview](#1-overview)
2. [Current Implementation](#2-current-implementation)
3. [RVF Format Deep Dive](#3-rvf-format-deep-dive)
4. [Planned Architecture](#4-planned-architecture)
5. [Integration with Routing and Scoring](#5-integration-with-routing-and-scoring)
6. [Quantization and Storage](#6-quantization-and-storage)
7. [Feature Flags](#7-feature-flags)
8. [Key ruvector Crates](#8-key-ruvector-crates)
9. [Data Management and Backup](#9-data-management-and-backup)
10. [Phase 3 Roadmap](#10-phase-3-roadmap)

---

## 1. Overview

RVF integration is **planned but not yet implemented**. The current vector
store uses brute-force cosine similarity over in-memory vectors. A comment in
`vector_store.rs` summarizes the intent:

> For larger datasets, swap to an HNSW-based backend (e.g. RVF) when available.

When the integration lands, clawft will replace its in-memory vector store with
RVF-backed storage, gaining HNSW indexing, tiered quantization, policy-aware
routing, and a tamper-evident audit trail -- all in a single binary file per
concern.

---

## 2. Current Implementation

The `vector-memory` feature flag in `clawft-core` enables the vector subsystem.
Everything lives in memory for the lifetime of the process; nothing is persisted
to disk.

### VectorStore

Defined in `clawft-core/src/vector_store.rs`.

```rust
pub struct VectorStore {
    entries: Vec<VectorEntry>,
}

pub struct VectorEntry {
    pub id: String,
    pub text: String,
    pub embedding: Vec<f32>,
    pub tags: Vec<String>,
    pub metadata: HashMap<String, serde_json::Value>,
    pub timestamp: u64,
}
```

Search iterates every entry and computes cosine similarity against the query
vector. Results are sorted by descending score and truncated to `top_k`.

**Complexity**: O(n * d) per query, where `n` is the number of entries and `d`
is the embedding dimension.

### HashEmbedder

Defined in `clawft-core/src/embeddings/hash_embedder.rs`.

| Property | Value |
|----------|-------|
| Algorithm | SimHash (word-level hashing via `DefaultHasher`) |
| Dimensions | 384 (default) |
| Normalization | L2 unit length |
| Deterministic | Yes |
| API calls | None |
| Case sensitivity | Case-insensitive (lowercased before hashing) |

The `HashEmbedder` is a baseline. It produces consistent embeddings suitable
for exact-match and near-duplicate detection, but it does not capture semantic
meaning the way learned embedding models do. It is not suitable for production
semantic search.

### Embedder Trait

```rust
#[async_trait]
pub trait Embedder: Send + Sync {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbeddingError>;
    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbeddingError>;
    fn dimension(&self) -> usize;
}
```

Any replacement embedder (LLM API, local ONNX model) must implement this trait.

### Limitations

- **No persistence**: entries are lost when the process exits.
- **No indexing**: brute-force search does not scale beyond tens of thousands of
  entries.
- **Hash-based embeddings**: the SimHash embedder captures word overlap, not
  semantic similarity.

---

## 3. RVF Format Deep Dive

An RVF file is a sequence of typed segments, each prefixed with a header that
declares the segment type, length, and optional compression. Readers skip
segments they do not understand, which makes the format forward-compatible.

### Segment Types

| Segment | Code | Use in clawft |
|---------|------|---------------|
| VEC | `0x01` | Memory and session embeddings |
| INDEX | `0x02` | HNSW adjacency lists for fast approximate search |
| META | `0x07` | Agent configuration, skill metadata |
| HOT | `0x08` | Frequently-accessed entries (kept uncompressed for speed) |
| SKETCH | `0x09` | Access frequency tracking (Count-Min Sketch or similar) |
| WITNESS | `0x0A` | Cryptographic audit trail (hash chain) |
| QUANT | `0x06` | Quantization dictionaries (codebooks for PQ) |
| POLICY_KERNEL | `0x31` | Routing policy parameters |
| COST_CURVE | `0x32` | Provider cost, latency, and quality curves |

A single `.rvf` file can contain any combination of these segments. For
example, `memory.rvf` would contain VEC, INDEX, HOT, SKETCH, and QUANT
segments, while `policies.rvf` would contain POLICY_KERNEL and COST_CURVE.

---

## 4. Planned Architecture

### Storage Layout

```
~/.clawft/workspace/memory/memory.rvf   (~2-5 MB for 10K entries)
~/.clawft/sessions/index.rvf            (~1-3 MB for 1K sessions)
~/.clawft/routing/policies.rvf          (~100 KB)
~/.clawft/witness.rvf                   (~500 KB - 5 MB)
```

Each file is self-contained. No external database or index server is required.

### Component Mapping

| Current component | RVF replacement |
|-------------------|-----------------|
| `VectorStore` (in-memory `Vec<VectorEntry>`) | VEC + INDEX segments in `memory.rvf` |
| `HashEmbedder` | LLM embedding API or local ONNX model (separate concern) |
| No persistence | RVF file I/O with mmap reads |
| No routing storage | POLICY_KERNEL + COST_CURVE in `policies.rvf` |
| No audit trail | WITNESS segment in `witness.rvf` |
| Session JSONL files | INDEX + META segments in `index.rvf` |

### Search Path (After RVF)

1. Receive query text.
2. Generate embedding via the configured `Embedder` implementation.
3. Search the HNSW index in the INDEX segment of `memory.rvf`.
4. Return top-k results with metadata from the VEC and META segments.

The brute-force O(n * d) scan is replaced by HNSW's approximate nearest
neighbor search at O(log n) per query.

---

## 5. Integration with Routing and Scoring

The `IntelligentRouter` (defined in `clawft-core/src/intelligent_router.rs`)
will use RVF to persist and query routing decisions.

### Flow

1. **Store policies**: The `IntelligentRouter` writes routing policies as
   vectors into the POLICY_KERNEL segment of `policies.rvf`.
2. **On new request**: Embed the task description and search the policy store
   using HNSW.
3. **Cache hit** (match score > 0.85): Use the cached tier assignment directly.
4. **Cache miss**: Compute complexity via the 7-factor model, route to the
   appropriate tier, and store the new policy vector for future lookups.
5. **Track performance**: Cost curves in the COST_CURVE segment record provider
   latency, cost, and quality metrics over time.

This creates a self-improving routing loop: successful routing decisions are
embedded and indexed, so similar future requests skip the complexity computation
entirely.

---

## 6. Quantization and Storage

RVF supports temperature-based quantization, where vectors are stored at
different precision levels based on access frequency.

### Temperature Tiers

| Temperature | Format | Size (384-dim) | Access pattern |
|-------------|--------|----------------|----------------|
| Hot | fp16 | ~768 bytes | Frequently accessed |
| Warm | Product Quantization (PQ) | ~48 bytes | Occasional access |
| Cold | Binary quantization | ~48 bytes | Archival / rare access |

The SKETCH segment tracks access frequency. When a vector's access count drops
below a threshold, it is demoted to a lower temperature tier. Promotion happens
on access.

### HNSW Indexing Layers

The INDEX segment supports progressive construction:

| Layer | Recall | Use case |
|-------|--------|----------|
| A | ~70% | Fastest search, initial build |
| B | ~85% | Balanced recall and speed |
| C | ~95% | Highest accuracy, full build |

The recommended approach is to build Layer A first and upgrade to B and C as
the dataset grows. This avoids paying the full indexing cost up front for small
datasets.

---

## 7. Feature Flags

### Core Feature Flags (clawft-core Cargo.toml)

```toml
[features]
rvf = ["dep:rvf-runtime", "dep:rvf-types", "dep:rvf-index"]
rvf-agentdb = ["rvf", "dep:rvf-adapters-agentdb", "dep:ruvector-core"]
rvf-crypto = ["rvf", "dep:rvf-crypto"]
ruvllm = ["dep:ruvllm"]
intelligent-routing = ["ruvllm", "tiny-dancer"]
sona = ["dep:sona"]
```

- `rvf` -- Base RVF support: file I/O, segment parsing, HNSW index.
- `rvf-agentdb` -- AgentDB adapter for PolicyMemoryStore and
  SessionStateIndex.
- `rvf-crypto` -- WITNESS segment with cryptographic audit trail.
- `ruvllm` -- 7-factor complexity scoring and HNSW-based routing.
- `intelligent-routing` -- Full routing stack (requires `ruvllm` and
  `tiny-dancer`).
- `sona` -- Self-learning: MicroLoRA, ReasoningBank, EWC++.
  Planned memory path (ws06-memory): same engine reranks *retrieved memory
  nodes* from generator citations (RMM arXiv:2503.08026). See
  `docs/research/rmm-reflective-memory-management.md`. Do not RL-update the
  VEC/HNSW retriever from those rewards.

### CLI Aggregate Features (clawft-cli Cargo.toml)

```toml
ruvector = [
    "clawft-core/rvf-agentdb",
    "clawft-core/intelligent-routing",
    "clawft-core/sona",
    # ...
]
ruvector-full = ["ruvector", "clawft-core/rvf-crypto"]
```

- `ruvector` -- Enables the full ruvector stack without crypto.
- `ruvector-full` -- Everything in `ruvector` plus the WITNESS audit trail.

Users building from source can opt in:

```bash
cargo build --features ruvector       # standard ruvector integration
cargo build --features ruvector-full  # includes cryptographic audit trail
```

---

## 8. Key ruvector Crates

| Crate | Purpose | Notes |
|-------|---------|-------|
| `ruvllm` | 7-factor complexity scoring, HNSW routing, quality scoring | Minimal feature footprint |
| `sona` | Self-learning: MicroLoRA, ReasoningBank, EWC++; planned RMM memory-citation rerank (`docs/research/rmm-reflective-memory-management.md`) | ~30 KB WASM |
| `ruvector-core` | AgenticDB: PolicyMemoryStore, SessionStateIndex | Core storage abstractions |
| `ruvector-attention` | 40+ attention mechanisms | Optional, for advanced routing |
| `micro-hnsw-wasm` | Zero-dependency WASM HNSW search | 11.8 KB compiled |
| `ruvector-temporal-tensor` | Tiered quantization (hot/warm/cold) | < 10 KB |
| `ruvector-tiny-dancer-core` | FastGRNN, CircuitBreaker | Lightweight inference |
| `rvf-runtime` | RVF file I/O, segment parsing | Required by the `rvf` feature |
| `rvf-types` | Segment type definitions, header structs | Required by the `rvf` feature |
| `rvf-index` | HNSW index construction and search | Required by the `rvf` feature |
| `rvf-crypto` | WITNESS segment, hash chain verification | Required by `rvf-crypto` feature |
| `rvf-adapters-agentdb` | Bridges ruvector-core to RVF storage | Required by `rvf-agentdb` feature |

All crates are designed for minimal binary size and optional WASM compilation.

---

## 9. Data Management and Backup

### Current State

- Memory is stored in `MEMORY.md` (Markdown format, human-readable).
- Sessions are stored as JSONL files.
- The `weft memory show` command displays stored memory.
- The `weft memory search` command searches with substring matching on
  MEMORY.md / HISTORY.md paragraphs.
- A parallel vector index (`memory.rvf`) is built from MEMORY.md by
  `bootstrap_memory_index` (feature `rvf`). **WEFT-84:** the index is
  rebuilt when MEMORY.md is newer than the index (mtime check), and can
  always be force-rebuilt with `weft memory reindex`.

### After RVF

- Memory is stored in `memory.rvf` (binary, self-contained).
- Sessions are stored in `index.rvf`.
- The same `weft memory show` and `weft memory search` commands work, but
  search uses HNSW instead of brute-force.
- Stale indexes after MEMORY.md edits are handled by mtime-aware bootstrap
  or `weft memory reindex` (no manual delete required).

### Backup

RVF files are self-contained binary files. To back up:

```bash
cp ~/.clawft/workspace/memory/memory.rvf /path/to/backup/
cp ~/.clawft/sessions/index.rvf /path/to/backup/
cp ~/.clawft/routing/policies.rvf /path/to/backup/
cp ~/.clawft/witness.rvf /path/to/backup/
```

No external database dump or index rebuild is needed. The files contain both
the vectors and their indices.

Planned CLI commands for structured export and import are part of the Phase 3
roadmap.

---

## 10. Phase 3 Roadmap

The following tasks are planned for the RVF integration phase:

- [ ] Integrate `rvf-runtime` and `rvf-types` crates as dependencies under the
      `rvf` feature flag.
- [ ] Replace the brute-force `VectorStore` with an HNSW-backed implementation
      that reads and writes RVF INDEX segments.
- [ ] Replace `HashEmbedder` with an LLM embedding API or local ONNX model for
      production-quality semantic embeddings.
- [ ] Implement RVF file I/O for memory persistence (load on startup, flush on
      write).
- [ ] Add `weft memory export` and `weft memory import` CLI commands for
      portable data transfer.
- [ ] Implement POLICY_KERNEL storage so the `IntelligentRouter` persists
      routing policies across restarts.
- [ ] Add WITNESS segment support for a tamper-evident audit trail of agent
      actions.
- [ ] Implement temperature-based quantization (hot/warm/cold) to reduce
      storage for infrequently accessed vectors.
- [ ] Ensure WASM compatibility via `micro-hnsw-wasm` for browser and edge
      deployments.
