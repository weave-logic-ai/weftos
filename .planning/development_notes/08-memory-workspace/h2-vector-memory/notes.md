# H2: RVF Phase 3 Vector Memory -- Notes

> Backfilled 2026-04-28. Use this file for ongoing development
> findings.

## Implementation map

- `crates/clawft-core/src/embeddings/mod.rs` -- `Embedder` trait,
  module exports gated by `vector-memory` and `rvf` features.
- `hash_embedder.rs` -- SimHash-based local embedder (always-on under
  `vector-memory`).
- `api_embedder.rs` -- HTTP embeddings client (under `rvf`).
- `hnsw_store.rs` -- main HNSW index, 18 unit tests, adaptive tiered
  search since `b5e07640` (2026-04-17).
- `micro_hnsw.rs` -- WASM/browser variant with 8 KB working-set
  budget.
- `quantization.rs` -- hot/warm/cold tier quantization.
- `witness.rs` -- WITNESS hash-chain segments.
- `rvf_stub.rs` -- the live brute-force JSON-backed store (called by
  `memory_bootstrap.rs`).
- `progressive.rs` -- progressive embedder pipeline.
- `crates/clawft-core/src/memory_bootstrap.rs` -- indexes existing
  `MEMORY.md` content into `RvfStore`.

## Useful invariants

- `vector-memory` feature gates HashEmbedder + HNSW. `rvf` feature
  gates the full RVF stack. The two are not independent: `rvf`
  implies `vector-memory`.
- WASM builds use `micro_hnsw`, not `hnsw_store`. Don't import
  `hnsw_store` from a WASM target.
- `RvfStore` (in `rvf_stub.rs`) is the live module. `rvf_io.rs` was
  removed 2026-04-28 -- if you see references in older PRs, they're
  stale.
- `WitnessChain` is inline in `rvf_io::SegmentFile` (now removed).
  When the segment-style shape returns, it must keep the chain
  inline; do not move WITNESS to a side-car file.

## Tips

- `instant-distance` panics on empty input vectors. Validate
  `embedding.is_empty()` before insert.
- Adaptive HNSW (the `b5e07640` change) uses tiered dimensional
  search. The tier picker is monotone in N; benchmarks live in
  `crates/clawft-core/benches/`.
- For repro: SimHash is deterministic over the input bytes, so
  golden tests over `HashEmbedder` are stable across builds.

## Known follow-ups (see audit + Plane)

- WS-O6 / WEFT-MW-6 -- vector index drift versus `MEMORY.md`.
- WS-O2 / WEFT-93 -- rvf_stub vs rvf_io fate (resolved this commit:
  `rvf_io.rs` removed).
