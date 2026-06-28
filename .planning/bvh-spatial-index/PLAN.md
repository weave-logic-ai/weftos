# BVH-on-RVF Spatial-Temporal Index — Implementation Plan

**Status**: Draft (companion to ADR-056)
**Created**: 2026-05-11
**Owner**: ECC / cognitive-substrate workstream
**Source**: ADR-056, scorch_and_awe concept paper (2026-05-03)

## User intent

> "We need to plan out how to add the BVH to our existing indexes;
> it will form the 3D tree for the ECC to bind to."

The ECC cognitive substrate already exposes HNSW (similarity) and
causal-edges (ExoChain walk). It cannot answer geometric overlap.
A new BVH index sits next to those, behind a `SpatialBackend` trait,
gated by the same `ecc` feature, anchored to the chain from v1.

ADR-056 carries the irreversible decisions (AABB, tagged-union
registry, ChainAnchor binding, leaf identity kinds). This document
carries the work breakdown.

## Goal shape (what shipping looks like)

A consumer asks `SpatialService` "what leaves intersect this AABB on
branch B?" and gets an O(log n) answer. The answer is reproducible
across nodes because every BVH mutation went through ExoChain. New
leaf primitive types (beam, capsule, sphere, frustum, swept-AABB) can
register at runtime without recompiling the kernel.

The first consumer is the `weaver ecc spatial` CLI verb, which
exercises every query type end-to-end. The second consumer is the
agent context router (`crates/clawft-core/src/agent/context_router/`),
which uses spatial overlap to find contextually-co-located entries
where HNSW similarity alone is too coarse.

## Crate layout

```
crates/clawft-bvh/
  Cargo.toml                  # no_std + alloc; no tokio; serde + ciborium
  src/
    lib.rs                    # public surface
    aabb.rs                   # Aabb, Vec3, Ray, Frustum
    leaf.rs                   # Leaf, LeafId, IdentityKind, BranchId
    tree.rs                   # BvhNode, BvhTree (build + refit + query)
    query.rs                  # point / aabb / sphere / ray / frustum / knn
    registry.rs               # tag → narrow-phase interpreter dispatch
    branch.rs                 # COW branch derivation + diff
    store.rs                  # BvhStore (the integrated thing)
    chain.rs                  # ChainSink trait (kernel plugs in here)
    determinism.rs            # phase-boundary commit + ExoChain-seq sort
  tests/
    broadphase.rs             # broad-phase against brute force
    determinism.rs            # same insertions → same tree across nodes
    branch_diff.rs            # COW + diff invariants
    registry.rs               # tag dispatch + missing-tag failure modes

crates/weftos-leaf-types/src/spatial/
  mod.rs
  tags.rs                     # canonical SpatialLeafTag registry
  primitives.rs               # AabbWire, SphereWire, RayWire, etc.

crates/clawft-kernel/src/
  spatial_backend.rs          # SpatialBackend trait + SearchResult + errors
  spatial_bvh.rs              # clawft_bvh::BvhStore → SpatialBackend adapter
  spatial_service.rs          # SystemService registration + ChainSink wiring

crates/clawft-weave/src/commands/
  ecc_cmd.rs                  # +Spatial subcommand (insert/query/branch/diff)
```

The `clawft-bvh` crate is `no_std + alloc` so it ships unchanged into
WASM (browser, WASI) and embedded sensor builds. Tokio + chain wiring
lives at the kernel boundary, behind the `ChainSink` trait that
`clawft-bvh` exposes for plug-in.

## Phases

### Phase A — `clawft-bvh` standalone broad-phase (no chain)

Single PR. Establishes the crate, the trait surface, and the in-
memory broad-phase. **No kernel integration, no chain binding yet.**

1. Create `crates/clawft-bvh/` with the layout above.
2. Implement `Aabb`, `Vec3`, `Ray`, `Frustum`, `Leaf`, `LeafId`.
3. Build a top-down median-split BVH (SAH later — premature for v1).
4. Implement broad-phase queries: point, AABB, sphere, ray, frustum,
   knn (with distance-bound pruning).
5. Implement the tagged-union registry: `register_tag(u32,
   NarrowPhaseFn)`, `narrow_phase(tag, query, payload)`.
6. Tests: brute-force differential test for every query type
   (random scenes, random queries, exact-equal expected answer set).

Acceptance:
- `cargo test -p clawft-bvh` green on `--all-targets`.
- `scripts/build.sh check` clean.
- Documented public surface (`#![warn(missing_docs)]`).
- Brute-force differential test runs ≥ 10k random scenarios.

Plane: one work item under cycle `0.8.x`, type `Feature`.

### Phase B — `weftos-leaf-types::spatial` canonical tags

Pull-request after Phase A merges. Adds the cross-consumer registry.

1. New `weftos-leaf-types/src/spatial/` module with `SpatialLeafTag`
   enum + `u32` discriminants frozen via `#[repr(u32)]`.
2. Initial canonical tags (one per primitive class the concept paper
   names): `Sphere`, `Aabb`, `Obb`, `Capsule`, `SweptAabb`, `Frustum`,
   `RadialSphereEvent`, `BeamTrace`, `SensorRead4D`. Each tag has a
   CBOR-serialized payload struct in `primitives.rs`.
3. `clawft-bvh` re-exports the tags so the registry uses the same
   discriminants everywhere.

Acceptance:
- Tag discriminants stable across `cargo build` (round-tripped through
  a snapshot test).
- CBOR round-trip test for every primitive struct.
- `audit-surface.sh` clean (new fixtures, if any, conform to
  `weftos-design`).

Plane: one work item under `0.8.x`, type `Feature`, references ADR-031.

### Phase C — `clawft-kernel::spatial_*` adapter + service

Pull-request after Phase B merges. Plugs `clawft-bvh` into ECC.

1. `spatial_backend.rs`: `SpatialBackend` trait (verbatim from
   ADR-056 §8) + error type mirroring `VectorError`.
2. `spatial_bvh.rs`: thin adapter — `BvhBackend(Arc<Mutex<BvhStore>>)`
   implementing `SpatialBackend`. Epoch versioning follows
   `vector_hnsw.rs` patterns.
3. `spatial_service.rs`: `SpatialService` implementing
   `SystemService` (ADR-035). Health, status, config (`SpatialConfig`
   in `clawft-types`).
4. Kernel wiring: registration in `clawft-kernel::lib.rs` next to
   `HnswService`. Boot-time config from `KernelConfig::spatial`.
5. **ChainSink wiring**: kernel implements `clawft_bvh::ChainSink`
   over the existing `ChainManager` (CBOR encoded; one event per
   `insert_leaf` / `remove_leaf` / `derive_branch` /
   `rebalance_seal`). Dual-signed per ADR-028.

Acceptance:
- `scripts/build.sh test --features ecc` green.
- `scripts/build.sh check --features ecc` green.
- New integration test in `clawft-kernel/tests/` that boots a kernel,
  inserts 100 leaves, restarts the kernel, replays the chain segment,
  and confirms the BVH state is byte-identical.
- `weaver ecc spatial status` returns service health.

Plane: one work item under `0.8.x`, type `Feature`, references
ADR-022, ADR-035, ADR-041.

### Phase D — Determinism phase + COW branches

Pull-request after Phase C merges. Lifts the in-memory store to a
chain-anchored branch model.

1. `BvhStore::derive(branch_meta) -> BranchId`: emits a `bvh.derive`
   chain event and returns a child store handle that COW-shares
   nodes with the parent.
2. Determinism-phase commit: pending mutations buffer per branch and
   commit at `seal_phase(exochain_seq_end)` — sort by
   `(priority_tier, exochain_seq)`, lazy rebalance, write witness
   chain entry, seal.
3. `branch_diff(a, b, region)` — returns `DiffEntry` per leaf that
   differs in `region` between branches `a` and `b`.
4. `derive_branch(parent, meta)` consumes the parent's chain segment
   head as the base for COW deltas.

Acceptance:
- Deterministic replay test: same chain-event sequence applied to
  two empty BVHStores yields byte-identical trees, branches, and
  diff outputs.
- Branch-diff over volume V: insert N leaves in branch A, remove K,
  add M in branch B; `branch_diff(A, B, V)` returns exactly the
  K + M leaves intersecting V.

Plane: one work item under `0.8.x`, type `Feature`.

### Phase E — Consumer integration (`weaver ecc spatial`)

Pull-request after Phase D merges. Adds the CLI verb and the first
real consumer.

1. `weaver ecc spatial insert --tag <name> --aabb <x,y,z,x,y,z>
   [--payload <hex>]`
2. `weaver ecc spatial query (point|aabb|sphere|ray|frustum|knn)
   [--branch <id>] [--limit N]`
3. `weaver ecc spatial branch (derive|diff) ...`
4. `weaver ecc spatial status` — service health, leaf count per
   branch, last sealed phase.

Acceptance:
- E2E test in `clawft-weave/tests/`: round-trip insert → query →
  branch → diff via the CLI, confirming chain events are emitted
  and replayable.
- Documented in `docs/clawft-agent-guide.md` next to the existing
  `weaver ecc search` / `weaver ecc causal` reference.

Plane: one work item under `0.8.x`, type `Feature`.

### Phase F (deferred — concept paper §10.1) — BVH × HNSW fingerprinting

Out of scope here. Captured for forward continuity. A future ADR
(next available number when drafted — ADR-057 is taken by substrate
read ACLs as of 2026-05-12) will compose neighborhood fingerprints
with HNSW for "find historical configurations resembling this one"
queries. **Do not implement until a real consumer pins requirements**
(concept paper §12.3 Q7, preserved here).

## API surface preview (`clawft-bvh::lib.rs`)

```rust
pub mod aabb;
pub mod leaf;
pub mod query;
pub mod registry;
pub mod store;

pub use aabb::{Aabb, Frustum, Ray, Vec3};
pub use leaf::{IdentityKind, Leaf, LeafId, BranchId};
pub use query::{RayHit, DiffEntry};
pub use registry::{NarrowPhaseFn, SpatialLeafTag, TagRegistry};
pub use store::{BvhStore, BvhStoreConfig, ChainSink, BvhError};
```

`SpatialBackend` (in `clawft-kernel`) consumes this through
`BvhStore`; consumers outside the kernel use `BvhStore` directly
when they don't need chain anchoring (e.g. offline tooling).

## Configuration surface (`clawft-types::config`)

```rust
pub struct SpatialConfig {
    pub enabled: bool,
    pub backend: SpatialBackendKind,    // Bvh (only variant today)
    pub max_leaves: usize,              // store-full guard
    pub phase_commit_threshold: usize,  // pending mutations before
                                        // a forced phase seal
    pub branch_retention: BranchRetentionPolicy, // count or by age
}
```

Defaults: `enabled = false` (opt-in), `max_leaves = 1_000_000`,
`phase_commit_threshold = 4096`, retention `KeepLast(64)`.

## Open questions (do **not** block phases A–D)

- **OQ1 — narrow-phase recursion ban**: ADR-056 §3 forbids re-
  recursion (a leaf's narrow-phase calling into another spatial
  structure). We should encode this as a `#[forbid(...)]` or a
  registry-time check. **Resolve in Phase A.**
- **OQ2 — supersession schedule for high-churn leaves**: how does
  the BVH cooperate with ExoChain supersession rules when sensor-
  net workloads churn N leaves/sec? **Resolve in Phase D**, once
  branch retention is exercised against synthetic load.
- **OQ3 — branch retention vs. RVF compaction**: the concept paper
  §12.1 Q4 leans on RVF compaction; our chain layer does not yet
  expose a compaction hook. **Spike during Phase D**, file a
  follow-up ADR if a compaction trait emerges.
- **OQ4 — multi-tenant membership filters**: ADR-056 §1 names
  membership filters as a v1 capability, but the trait surface
  in §8 doesn't include them. **Resolve before Phase E** — add a
  `filter: &MembershipFilter` arg to query methods or a wrapper
  layer.
- **OQ5 — BVH × HNSW fingerprinting**: deferred per ADR-056 and
  concept paper §12.3. Do not start until Phase E ships.

## Plane workflow

Per `.claude/CLAUDE.md` "Plane is the authoritative work tracker."

On Phase A start:
1. Create one Plane work item per phase (A–E) in cycle `0.8.x`.
2. Link each to ADR-056 and to this PLAN.md.
3. Acceptance criteria copied verbatim from the relevant phase
   above.

On phase merge:
- Transition the phase's work item to **Done** with the merge SHA.
- File follow-ups for any open question that didn't get resolved
  in-phase.

## Risks & rollback

| Risk | Rollback |
|---|---|
| `clawft-bvh` build fails on `no_std + alloc` for sensor targets | Drop `no_std`; mark crate `std`-only and document the constraint. Reversible in Phase A. |
| Chain-event volume is unacceptable for high-frequency workloads | Batch per-phase commits more aggressively; add a `BatchPolicy::SealEvery(N)` knob. Reversible in Phase D. |
| Tagged-union registry races on tag registration across consumers | Bake all known tags into `weftos-leaf-types::spatial::tags` at compile time; refuse dynamic registration. Reversible in Phase B (small surface change). |
| AABB-only broad-phase loses too much tightness | Add an OBB-broad-phase variant behind a second `SpatialBackend` impl. Not a v1 problem; would be a follow-up ADR. |
| Determinism-phase commit latency blocks interactive queries | Pre-commit query against the buffered-pending-leaves set. Already in the trait surface (`query_*` returns pending + sealed). Reversible in Phase D. |

## References

- ADR-056 (this plan's parent decision)
- Concept paper: `/home/aepod/dev/scorch_and_awe/docs/concepts/bvh-on-rvf-spatial-temporal-index.md`
- `crates/clawft-kernel/src/vector_backend.rs` — trait shape
- `crates/clawft-kernel/src/hnsw_service.rs` — service shape
- `.claude/skills/plane-workflow/SKILL.md` — Plane lifecycle
- `scripts/build.sh` — mandatory build/test entry point
