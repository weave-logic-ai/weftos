# H2: RVF Phase 3 Vector Memory -- Difficult Tasks

> Backfilled 2026-04-28.

## 2026-02-20 Difficult: H2.3 RVF segment I/O without committing to an unstable upstream

**Item**: H2.3
**Difficulty**: High
**Why**: The upstream `rvf-runtime` 0.2 binary format was flagged by
its own audit as evolving. Adopting it directly would have made every
agent's memory subject to upstream format churn.
**Approach**: Use `rvf-types` as the *conceptual* model (segment
header + entries + WITNESS chain) but ship a JSON serialization in
tree. Two modules: `rvf_stub.rs` (brute-force fallback) and
`rvf_io.rs` (forward-compatible segment shape). Both behind
`feature = "rvf"`.
**Findings**: Both shapes shipping at once was confusing -- `rvf_io.rs`
never got a real consumer and the audit (WS-O2) flagged the duplication.
2026-04-28: deleted `rvf_io.rs` and pinned `rvf_stub.rs` as the active
path. The forward-compatible shape can return when there's a real
consumer.

## 2026-02-20 Difficult: WASM micro-HNSW under an 8 KB budget

**Item**: H2.8
**Difficulty**: Very High
**Why**: HNSW is normally a megabyte-scale data structure. Browsers
have tight WASM memory budgets and we wanted recall in the agent
loop, not just remote.
**Approach**: Strip the graph to a single layer, cap neighbor count
per node, and reuse the HashEmbedder (16-bit SimHash) for vectors so
each node carries a small footprint. Hard-fail at insert time if the
working set exceeds 8 KB.
**Findings**: The 8 KB cap is enforced by an explicit byte counter,
not estimated. The recall hit versus full HNSW is acceptable for the
target N (hundreds of entries per session). Tests in
`micro_hnsw.rs` (545 lines) include working-set assertions; if those
ever start failing on size, *do not* relax the cap without owner
sign-off.

## 2026-02-20 Difficult: WITNESS hash chain that survives partial writes

**Item**: H2.6
**Difficulty**: High
**Why**: A SHA-256 hash chain over segment writes that crashes mid-
append must either resume cleanly or refuse to load. There is no
in-band "chain is corrupt, please replay" semantics in JSON.
**Approach**: Inline the chain head + parent hash in the segment
file header. On load, recompute the chain and refuse to open if the
recomputed head doesn't match. `read_verified_segment_file` is the
load-bearing entry point.
**Findings**: The witness module ended up co-located with the
embeddings rather than under `security/` because the verification
needs the segment shape. Future move out is possible if the
segment format stabilizes upstream.
