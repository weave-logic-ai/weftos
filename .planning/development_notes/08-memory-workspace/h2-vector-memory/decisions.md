# H2: RVF Phase 3 Vector Memory -- Decisions

> Backfilled 2026-04-28 from source comments and module docs in
> `crates/clawft-core/src/embeddings/{mod.rs,hnsw_store.rs,rvf_io.rs,rvf_stub.rs,witness.rs,quantization.rs,micro_hnsw.rs}`,
> the SPARC plan
> (`.planning/sparc/phase4/08-memory-workspace/02-phase-H2-hnsw-embedder.md`,
> `03-phase-H2-advanced-rvf-witness-quantization.md`), the audit
> (`.planning/reviews/0.7.0-release-gate/06-memory-workspace.md`), and
> git history (`a67b9e5c`, `b5e07640`).

## 2026-02-20 Decision: HNSW backed by `instant-distance`, not by `hnsw-rs`

**Context**: H2.1 needs an in-process HNSW index for top-K search over
agent memory. Two viable Rust crates: `instant-distance` and
`hnsw-rs`. Both implement HNSW; build/dependency footprints differ.
**Options**:
1. `hnsw-rs` -- richer feature set, heavier dependency tree.
2. `instant-distance` -- minimal surface, builds quickly on WASI.
3. Roll our own (rejected -- HNSW is enough nuance to get wrong).
**Decision**: `instant-distance`.
**Rationale**: Smaller compile time, fewer transitive deps, ergonomic
`Builder` API. The crate is mature enough for the volumes the H2
target supports (10k-100k entries per agent).
**Consequences**: Different distance metrics require wrapping; we use
cosine similarity over normalized vectors. Implementation in
`crates/clawft-core/src/embeddings/hnsw_store.rs` (~1005 lines + 18
tests). Adaptive tiered search (added `b5e07640`, 2026-04-17) extended
this with the `Adaptive HNSW` layer.

## 2026-02-20 Decision: dual `Embedder` impls (HashEmbedder default, ApiEmbedder optional)

**Context**: H2.2 requires real embeddings, but offline-first agents
shouldn't carry an unconditional API dependency.
**Options**:
1. Only ApiEmbedder (e.g. OpenAI text-embedding-3-small).
2. Only local (HashEmbedder via SimHash).
3. Both, with HashEmbedder the no-feature-flag default.
**Decision**: Option 3.
**Rationale**: HashEmbedder is deterministic, fast, and runs in WASM.
ApiEmbedder gives semantic quality where the network is available.
The trait shape (`Embedder: Send + Sync`) lets callers pick at runtime.
**Consequences**: The `vector-memory` feature gates HNSW + HashEmbedder;
the `rvf` feature gates ApiEmbedder + the rest of the H2 stack. Two
matrices to build; CI covers both. Implementation:
`hash_embedder.rs` (297 lines, default) and `api_embedder.rs` (385 lines,
behind `rvf`).

## 2026-02-20 Decision: H2.3 RVF segment I/O lands as a brute-force JSON store, not the upstream binary format

**Context**: The `rvf-runtime` 0.2 audit flagged its binary segment
format as tightly coupled to the upstream's evolving on-disk schema.
A 0.7.0 release that ships a fresh on-disk format every minor would
churn agent memory.
**Options**:
1. Use `rvf-runtime` 0.2 directly. Tight coupling, churn risk.
2. Implement a local fallback using `rvf-types` as the conceptual
   model and a portable JSON serialization.
3. Defer H2.3 to 0.8.x.
**Decision**: Option 2 -- ship `rvf_stub.rs` (in-memory + JSON)
*and* `rvf_io.rs` (segment-style JSON via `rvf-types`). Both modules
sit behind `feature = "rvf"`.
**Rationale**: `rvf_stub` is the brute-force fallback that
`memory_bootstrap.rs` calls today. `rvf_io.rs` was added as the
forward-compatible segment shape (header + entries + WITNESS chain
inline) so future migrations can promote without re-modeling.
**Consequences**: Two modules carry overlapping shapes. The audit
(WS-O2 / WEFT-93 / MW-15) flagged the dual implementation as confusing.
Resolved 2026-04-28 by removing `rvf_io.rs` (no callers) and adding a
module note to `rvf_stub.rs` documenting it as the live brute-force
implementation.

## 2026-02-20 Decision: WITNESS hash chain is inline in segment files

**Context**: H2.6 adds a SHA-256 hash chain over segment writes for
audit. Two storage choices: separate witness file or inline header.
**Options**:
1. Separate `<segment>.witness` file.
2. Inline the chain head + parent hash inside the segment file
   header.
3. Append-only journal at the agent root.
**Decision**: Option 2 -- inline within the segment file.
**Rationale**: Keeps the audit chain co-located with the data it
protects. A separate file can be deleted independently and break
verification. The append-only journal pattern is reserved for the
future substrate-backed path.
**Consequences**: The `WitnessChain` lives in `witness.rs` and is
embedded by `rvf_io::SegmentFile` (forward-compatible) and read on
load by `read_verified_segment_file`. `rvf_stub` does not yet carry
WITNESS, since the brute-force JSON store predates the chain design.

## 2026-02-20 Decision: WASM micro-HNSW with an 8 KB working-set budget

**Context**: H2.8 -- the browser/WASM build cannot afford a full HNSW
graph (megabyte+ index). Agents in the browser still need top-K
recall over a small working set.
**Options**:
1. No vector recall in the browser; remote-only.
2. Mini HNSW with a hard budget cap.
3. Linear scan over a small in-memory list.
**Decision**: Option 2 -- micro-HNSW with an 8 KB budget.
**Rationale**: Linear scan beats mini-HNSW only for very small N; at
the H2 target (hundreds of entries per session), the index pays for
itself even with the budget cap. 8 KB is small enough to live in the
WASM memory headroom we already had.
**Consequences**: `crates/clawft-core/src/embeddings/micro_hnsw.rs`
(545 lines) is the implementation. Browser tests must keep the
budget assertion (no creeping growth in working-set size).

## 2026-02-20 Decision: temperature quantization (hot/warm/cold)

**Context**: H2.7 -- agent memory has highly skewed access patterns.
Recent context is hot, older context is mostly cold. Storing
everything as f32 wastes RAM/disk on the cold tier.
**Options**:
1. f32 everywhere (simple, expensive).
2. Three tiers: hot (f32), warm (f16), cold (8-bit PQ).
3. Tier on-demand, no automatic promotion/demotion.
**Decision**: Option 2 with explicit `Temperature` markers.
**Rationale**: Tied to access recency, not semantic. Demotes cleanly
on segment compaction. Hot tier stays exact; cold tier accepts
recall loss for the older entries.
**Consequences**: Implementation in
`crates/clawft-core/src/embeddings/quantization.rs` (596 lines).
Tier transitions happen at compact time, not on every write.

## 2026-04-28 Decision: drop `rvf_io.rs` (orphan), keep `rvf_stub.rs` as the live path

**Context**: WS-O2 / WEFT-93 / MW-15 -- both modules ship under
`feature = "rvf"`, but only `rvf_stub.rs` has callers
(`crates/clawft-core/src/memory_bootstrap.rs`). `rvf_io.rs` is
forward-compatible orphan code with no consumers.
**Options**:
1. Keep both, document that `rvf_io.rs` is the future path.
2. Delete `rvf_io.rs`, add a module note to `rvf_stub.rs`.
3. Delete `rvf_stub.rs` and migrate callers (high churn).
**Decision**: Option 2.
**Rationale**: Dual implementations rot fast and confuse new
contributors. The brute-force JSON store is the live path; the
segment-style shape can return when there's a real consumer.
**Consequences**: `rvf_io.rs` deleted in this release. `rvf_stub.rs`
gains a module-doc paragraph explaining it's the active path and
the segment-style design is on hold. `embeddings/mod.rs` no longer
re-exports `rvf_io`. Generated rustdoc for `rvf_io` will rebuild on
the next docs run.
