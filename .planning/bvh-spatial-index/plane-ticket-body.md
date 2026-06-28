## What this ticket is

The single re-entry point for the BVH-on-RVF spatial-temporal index
work. **At the start of cycle 0.8.x**, an agent or maintainer claims
this ticket, re-reads the ADR + plan below, and decomposes Phase A–E
into individual work items (one per phase). This ticket then
transitions to **Done** once those phase tickets exist.

Filed as a single placeholder rather than five phase tickets
on purpose — the maintainer at 0.8.x kickoff should re-validate the
phase shape against whatever else has shifted in the workspace
before committing to a five-PR cadence.

## Source of truth

- **ADR-056** (Accepted 2026-05-13) — `docs/adr/adr-056-bvh-spatial-index.md`
  - Irreversible decisions: new `clawft-bvh` crate, `SpatialBackend`
    trait mirroring `VectorBackend`, AABB at node level, tagged-union
    leaf-primitive registry in `weftos-leaf-types::spatial`,
    `Object | Event` leaf identity kinds, ExoChain-seq ordering at the
    determinism phase, ChainAnchor binding from v1 (per ADR-022's
    mandatory-audit rule), CBOR (ADR-030) over rvf-wire (ADR-031).
- **Companion plan** — `.planning/bvh-spatial-index/PLAN.md`
  - Crate layout, five phases with acceptance criteria, five open
    questions deferred to in-phase resolution, risks + rollback table.
- **Upstream concept paper** — `scorch_and_awe/docs/concepts/bvh-on-rvf-spatial-temporal-index.md`
  (cross-linked from that doc back to this ticket via the
  "Implementation tracking (cross-project)" footer).

## Acceptance criteria for this ticket

This ticket is **Done** when:

1. The 0.8.x reviewer has re-read ADR-056 + PLAN.md and confirmed
   the phase shape is still right (or flagged what changed and why).
2. Five phase work items (A–E from PLAN.md) exist in Plane under
   the appropriate cycle, each linked back to ADR-056 + PLAN.md,
   each with acceptance criteria copied from the corresponding
   PLAN.md section.
3. The five new tickets carry the same labels and workstream tag
   as this one (`ws02-kernel` + `ws17-research`).

## Why not file the five phase tickets now

User direction at planning time (2026-05-13): file a single
placeholder rather than five forward-dated phase tickets. The
phases may evolve before 0.8.x kicks off, especially if a real
consumer (likely scorch_and_awe or a sensor-substrate workstream)
emerges and pins requirements that change the broad-phase API or
the chain-coupling cadence.

## Cross-project notes

- `scorch_and_awe` will eventually draft its own ADR-0004 (named
  "forthcoming" in the concept paper's header). The clawft side
  does not block on it; the spatial index is the substrate, not a
  game-specific feature.
- A future Phase F — BVH × HNSW fingerprinting (concept paper §10.1)
  — is **explicitly deferred** until a real consumer pins
  requirements. Concept paper §12.3 Q7 stands.
