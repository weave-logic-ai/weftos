# ADR-056: BVH-on-RVF Spatial-Temporal Index over ECC

**Date**: 2026-05-11 (Proposed) / 2026-05-13 (Accepted)
**Status**: Accepted
**Deciders**: ECC / cognitive-substrate maintainers; clawft-kernel maintainers (artifact-shape, crate-placement, and chain-coupling resolved via planning conversation 2026-05-13)
**Source**: `/home/aepod/dev/scorch_and_awe/docs/concepts/bvh-on-rvf-spatial-temporal-index.md` (Concept Paper, 2026-05-03), `.planning/bvh-spatial-index/PLAN.md`

## Context

The ECC (Embedded Cognitive Core, gated by the `ecc` feature in
`clawft-kernel`) currently exposes three index families to consumers:

- **HNSW** (`crates/clawft-kernel/src/hnsw_service.rs`,
  `vector_hnsw.rs`) — k-NN over feature vectors, behind the
  `VectorBackend` trait (`vector_backend.rs`).
- **Causal edges** — ordered walks over ExoChain
  (`clawft-substrate`, ADR-020 ChainLoggable, ADR-022 mandatory audit,
  ADR-030 CBOR, ADR-041 ChainAnchor, ADR-043 BLAKE3/SHAKE256).
- **Cross-references (UNID)** — direct lookup, surfaced through
  `weaver ecc crossrefs`.

What ECC cannot answer today is **"what is where, at what time, with
what shape and payload."** Spatial queries — sphere/AABB overlap, ray
cast, frustum cull, point-in-region, volume-temporal diff — degrade
to linear scans over the leaf set. The scorch_and_awe concept paper
(2026-05-03) frames this as a missing fourth index — a **bounding-
volume hierarchy** that composes spatial broad-phase (O(log n)) with
RVF's existing COW branches, witness chains, lineage tracking, and
membership filters. The composition is qualitatively distinct from
either layer alone: a **4D spatial-temporal index with extensible
non-scalar modal payloads**.

This ADR commits clawft to building that index inside the ECC
cognitive-substrate umbrella so it sits alongside HNSW and causal-
edges as a peer query class. ECC consumers (governance, agent
context routing, sensor surfaces, future scorch_and_awe-style
simulation consumers) bind to a single new trait and pick up
spatial broad-phase without re-plumbing chain wiring, gating, or
audit.

### What this is not

- **Not a physics engine.** The BVH indexes events that simulation
  produces; it never simulates.
- **Not a replacement for HNSW.** HNSW answers feature similarity;
  BVH answers geometric overlap. They compose, they do not compete
  (cf. concept paper §10, ADR-011 "raw HNSW sufficient" is preserved
  — this ADR does not introduce a similarity index).
- **Not a TSDB.** Scalar time-series belong in `clawft-edge-bench` /
  external TSDB. BVH leaves require geometric extent.
- **Not a CVMG fork.** clawft has no separate causal-mesh-graph crate;
  the equivalent here is the ExoChain + ECC causal-edges store, and
  this ADR cross-keys BVH leaves into that store by chain sequence,
  not into a separate graph.

## Decision

### 1. New crate: `clawft-bvh`

A new workspace crate `crates/clawft-bvh/` houses the BVH itself
plus the leaf-primitive registry. The crate has **no dependency on
`clawft-kernel`**; the kernel imports it, not the other way around.
This keeps the spatial primitive reusable from `clawft-substrate`
(branch-diff streaming), from `clawft-cli` (offline replay), and
from external embedders that want only the broad-phase without the
ECC service shell.

The kernel adds `clawft-kernel/src/spatial_*.rs` modules behind the
existing `ecc` feature, mirroring the `vector_*` family:

- `spatial_backend.rs` — `SpatialBackend` trait (the consumer-facing
  contract; analogous to `VectorBackend`).
- `spatial_bvh.rs` — adapter wrapping `clawft_bvh::BvhStore` as a
  `SpatialBackend`.
- `spatial_service.rs` — `SpatialService` registered against the
  `ServiceRegistry` (ADR-035 ServiceApi-layered protocol).

### 2. AABB as the canonical bounding primitive

All BVH internal nodes carry axis-aligned bounding boxes. AABB is
the universal broad-phase volume: cheapest test, supports swept
motion (swept-AABB ray cast), and admits simple tree refit. **Leaf
primitives may carry richer narrow-phase shapes** (sphere, OBB,
capsule, swept-AABB, frustum) — those live in the leaf payload
behind the registry tag and are not exposed at the broad-phase
level. (Concept paper §12.1 Q1 — Resolved.)

### 3. Tagged-union leaf-primitive registry

Each leaf is `(AabbBound, identity_kind, tag: u32, payload: Bytes)`.
The `tag` is a stable u32 keyed against a deterministic registry
exported from `weftos-leaf-types` so non-kernel consumers (the
mesh, the dashboard, external auditors) can interpret leaves
without recompiling. Adding a new leaf primitive type registers a
new tag and a narrow-phase interpreter function; the BVH itself
never recompiles. This mirrors RVF's existing kernel/eBPF/WASM
segment-tag pattern (ADR-031) and avoids the trait-object
deserialization fragility called out in the concept paper §12.2 Q2.

(Concept paper §12.2 Q2 — Resolved: tagged-union registry adopted.)

The tag registry is the contract that all consumers must agree on.
It lives in `weftos-leaf-types/src/spatial/tags.rs` and is
versioned via the same lockstep semver as the rest of the workspace
(ADR-001).

### 4. Leaf identity kinds: `Object` vs. `Event`

Each leaf is created with an immutable `identity_kind: ObjectKind |
EventKind` flag.

- **Object leaves** (units, sensors, persistent entities) keep a
  stable `LeafId` across branches; each branch holds a revision.
- **Event leaves** (terrain modifications, beam traces, sensor reads,
  one-shot observations) are immutable per branch with a
  `parent_leaf` reference up the lineage chain.

Branch-diff semantics, garbage collection, and replay scrubbing
branch on this flag. (Concept paper §12.1 Q5 — Resolved.)

### 5. ExoChain-seq ordering at the determinism phase

BVH mutations are batched and committed at a deterministic phase
boundary (the **determinism phase**). Within a phase, pending
insertions are sorted by `(priority_tier, exochain_seq)`, lazy
rebalance runs once, the witness-chain entry is written, and the
branch is sealed. Within-tier ties resolve by ExoChain sequence
ascending. This matches the existing ECC tick model and the F7
within-tier resolution rule used elsewhere in the substrate.
(Concept paper §12.1 Q3 + Q6 — Resolved.)

### 6. ChainAnchor binding from v1 (per ADR-022, ADR-041)

Every BVH mutation (`insert_leaf`, `remove_leaf`, `derive_branch`,
`rebalance_seal`) emits a chain event with CBOR payload (ADR-030).
The chain event is signed under the existing ADR-028 dual-sig regime
and routed through the `ChainAnchor` trait (ADR-041) when external
anchoring is configured. **The BVH has no in-memory-only mode that
bypasses the chain** — ADR-022 mandates audit for all state-changing
operations, and the BVH-on-RVF promise of tamper-evident spatial
replay collapses if mutations can be applied off-chain.

For latency-sensitive call sites, an in-memory query cache reads
from the current branch without re-deriving on each call; the
write path is still chain-mediated.

### 7. COW branches via `derive()` on the chain-anchored segment

A BVH branch is a chain segment. `BvhStore::derive(parent, meta)`
emits a chain event (`bvh.derive`) and produces a child store that
shares the parent's nodes COW-style; only modified subtrees are
stored as deltas. Branch-diff over a volume V is a two-branch
overlap query that returns leaves whose presence differs. State-
at-time-T is constructed by walking the chain segment to sequence
T and replaying. (Concept paper §4.2.)

### 8. Trait shape: `SpatialBackend`

```rust
pub trait SpatialBackend: Send + Sync {
    fn insert(&self, leaf: Leaf) -> SpatialResult<LeafId>;
    fn remove(&self, id: LeafId) -> bool;
    fn get(&self, id: LeafId) -> Option<Leaf>;

    fn query_point(&self, p: Vec3) -> Vec<LeafId>;
    fn query_aabb(&self, bb: Aabb) -> Vec<LeafId>;
    fn query_sphere(&self, center: Vec3, radius: f32) -> Vec<LeafId>;
    fn query_ray(&self, ray: Ray, max_t: f32) -> Vec<RayHit>;
    fn query_frustum(&self, frustum: &Frustum) -> Vec<LeafId>;
    fn query_knn(&self, p: Vec3, k: usize) -> Vec<LeafId>;

    fn derive_branch(&self, meta: BranchMeta) -> SpatialResult<BranchId>;
    fn branch_diff(&self, a: BranchId, b: BranchId, region: Aabb)
        -> Vec<DiffEntry>;

    fn epoch(&self) -> u64;
    fn len(&self) -> usize;
    fn backend_name(&self) -> &str;
}
```

Mirrors the `VectorBackend` trait conventions (epoch versioning,
`backend_name`, `Send + Sync` for `Arc` sharing). The
`SpatialService` wraps an `Arc<dyn SpatialBackend>` exactly like
`HnswService` wraps `Mutex<HnswStore>`.

The narrow-phase interpreter dispatch (`tag → fn`) lives in the
leaf-primitive registry, not on the trait — adding a new primitive
type is a registry change, not a trait change.

### 9. Feature gating

- `clawft-bvh` crate: always built (no feature gates). The crate is
  pure broad-phase + registry; it has no `tokio` / `clawft-kernel`
  deps and works in `no_std` with `alloc` (matching
  `weftos-leaf-types`).
- `clawft-kernel/src/spatial_*.rs`: behind the existing `ecc`
  feature, identical to the `vector_*` family.
- `weaver ecc spatial …` CLI: behind `ecc` in `clawft-weave`.

No new top-level feature flag is introduced. Existing `ecc` builds
pick up BVH automatically.

## Consequences

### Positive

- ECC gains a query class it cannot answer today (spatial overlap),
  unlocking O(log n) replacements for current linear scans in
  agent context routing, governance scope checks, sensor-substrate
  region queries, and any future simulation consumer.
- The `SpatialBackend` trait is one seam: kernel services, the
  weaver CLI, the dashboard, and external embedders all consume
  the same interface. Backend swaps (BVH today, a future R-tree or
  KD-tree variant) require no consumer changes.
- ChainAnchor binding from v1 means the tamper-evident-replay
  promise of the concept paper holds from the first commit — there
  is no "feature on / feature off" governance mode where chain
  audit silently drops.
- The tagged-union registry shape mirrors the existing RVF segment-
  tag pattern (ADR-031), keeping the format model consistent across
  the workspace.
- New leaf primitive types are additive: register a tag, register
  an interpreter, ship. No BVH or kernel recompilation required for
  downstream registrants.
- The `clawft-bvh` crate is reusable outside the kernel — useful
  for `clawft-substrate` branch-diff streaming, offline replay via
  `clawft-cli`, or extraction as an open-source spatial-temporal
  index (per concept paper §11.2).

### Negative

- **Write-path latency floor**: every mutation is a chain event.
  For high-frequency insertion (sensor-stream batching), the cost
  is dominated by signature + chain-write, not by the BVH itself.
  Mitigation: leaves batch into one chain event per determinism
  phase, not per-leaf.
- **Cross-consumer registry coordination**: the tag registry in
  `weftos-leaf-types` becomes a contract. Adding a tag is non-
  breaking; renaming or repurposing one is a wire-format break and
  must follow ADR-031's deprecation rules.
- **Storage growth**: even with COW deltas, every mutation grows
  the chain segment. Q4 (concept paper §12.1) is addressed by
  RVF compaction + supersession cascade, but consumers that churn
  leaves at high rates will need to design supersession schedules.
- **Determinism-phase commit overhead**: pending insertions must
  buffer until the phase boundary, which is incompatible with use
  cases that demand "leaf visible to query immediately after insert."
  Mitigation: pending leaves are queryable from the in-memory
  buffer before seal, but their `LeafId` is not stable until the
  phase commits. Consumers that need stable IDs must wait for
  seal.
- **AABB-only broad-phase loses tightness** for narrow geometry
  (beams, capsules). Narrow-phase recovers the tightness at the
  leaf, but tight-geometry-heavy workloads will see more candidate
  leaves than an OBB-broad-phase tree would. The concept paper §12.1
  Q1 resolution accepts this trade for cheaper tests + swept motion.

### Neutral

- This ADR does not introduce a new similarity index — ADR-011
  ("no FrankenSearch") stands. HNSW remains the only feature-vector
  index in the workspace.
- The concept paper's §10 BVH + HNSW composition ("Temporal
  Similarity Search via HNSW Fingerprinting") is **deferred** to a
  future ADR, exactly as the concept paper §12.3 Q7 defers it.
  This ADR does not block that work but does not commit to it
  either. (The ADR-057 slot has since been allocated to substrate
  per-path read ACLs; the fingerprinting decision will take the
  next available number when it is drafted.)
- No upstream-contribution decision is made re: ruvector
  (concept paper §12.3 Q8). The crate ships in-tree first.

## Implementation pointer

Phased work breakdown, crate layout, API sketch, and the per-phase
acceptance criteria live in
`.planning/bvh-spatial-index/PLAN.md`. Plane work items are filed
under cycle `0.8.x` (initial scaffolding) and `0.9.x` (consumer
integrations) — this is not a 0.7.0 release-gate item.

## References

- Concept paper: `/home/aepod/dev/scorch_and_awe/docs/concepts/bvh-on-rvf-spatial-temporal-index.md`
- ADR-011 — No FrankenSearch (raw HNSW sufficient); preserved
- ADR-019 — Registry trait pattern (followed for leaf tags)
- ADR-020 — ChainLoggable (BVH mutations adopt)
- ADR-022 — Mandatory ExoChain audit (binding rule for §6)
- ADR-028 — Dual signing (Ed25519 + ML-DSA-65) for chain events
- ADR-030 — CBOR codec for chain payloads
- ADR-031 — rvf-wire as mesh wire format (informs leaf serialization)
- ADR-035 — ServiceApi layered protocol (followed for `SpatialService`)
- ADR-041 — ChainAnchor trait (external anchoring of BVH chain head)
- ADR-043 — BLAKE3 / SHAKE-256 migration (BVH segments use these)
- `crates/clawft-kernel/src/vector_backend.rs` — trait shape mirrored
- `crates/clawft-kernel/src/hnsw_service.rs` — service shape mirrored
- `crates/weftos-leaf-types/` — registry crate; gains `spatial/` module
