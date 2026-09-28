# uvRTH tessellation ranks — sparse regional holograph

**Date:** 2026-09-26  
**Status:** Doctrine note (not an ADR). Canonical LOD in ADR-079 stays **L0–L5** until we promote this ladder.  
**Related:** `uvrth-name-candidate.md`, ADR-079.

The sparse **regional** world model is the novel bit: tesserae that **do not exist until observed**. Nested scale is not new (Powers of Ten, SI prefixes, discrete self-similar cosmology’s micro/macro/mega). What is new for WeftOS is using that ladder as a **Region Tessellated Holograph** — empty ranks stay empty, Graph Views stay **per region / per rank**, never one planet-wide graph.

The names people reach for (and forget) sit between **SI prefixes** and **complex-systems rungs**:

| Remembered | Usual name | SI-ish | uvRTH rank | ADR-079 LOD | Example |
|------------|------------|--------|------------|-------------|---------|
| nano | nano | 10⁻⁹ | **nano** | below L5 | piezo face, bolt, leaf on a VectorRef |
| micro | micro | 10⁻⁶ | **micro** | L5 fine | object AABB, one hydrophone station |
| mini | milli / object | 10⁻³ | **mini** | L5–L4 | furniture, 0.4 m array station, buoy |
| — | **meso** (often skipped) | 10⁰ | **meso** | L4 | room, quilt patch, 1.2 m tow section |
| normal | human / site | 10¹–10² m | **normal** | L3 | campus, lake triangle, USV+hose |
| macro | macro | 10³–10⁴ m | **macro** | L2 | city, fishing ground, coastal cell |
| major | mega | 10⁵–10⁶ m | **major** | L1 | basin, EEZ, state |
| super | giga / planetary | 10⁷ m | **super** | L0 | planet ellipsoid, Urth root `region/urth` |
| inf | unbounded | — | **inf** | hub | graph-of-graphs, `/engraph center`, no single mesh |

**Meso** is the rung everyone drops and then misses — room/quilt, between object and site.

**Inf** is not “draw the universe.” It is the **open rank**: queries across stores, no tessera instantiated for empty cosmos.

---

## How the universe rhyme works (and where it stops)

Self-similar nesting (atomic → stellar → galactic, or nano → macro → mega) is the *analogy*: each rank is the same kind of thing (a **region** with a BVH, a dual holograph, optional VectorRef) at a different cell size.

It is **not** a claim that uvRTH simulates physics at every rank. A nano tessera is an object leaf, not a molecular dynamics run. Generative fill at any rank stays **cosmetic / non-metric** (ADR-079).

Sparse rule, all ranks:

- Unobserved cell = **no leaf** (or explicit `unobserved`), not a hallucinated city of mahi.
- Densify only from capture, open feeds, or promoted Graph Views (F9).
- A mahi *patch* is a **normal/macro** tessera with confidence; a whale *corridor* is a chain of Event leaves across ranks over time.

---

## IDs (proposal, not shipped)

`region/urth/<rank>/<cell>` e.g. `region/urth/meso/quilt-…`, `region/urth/normal/lake-triangle-…`.

Keep L0–L5 in code until a migration ADR. This ladder is the **spoken** tessellation; L-numbers stay the wire.

---

## Novelty, said once

Nested scale: old. Dual appearance/structure: ADR-078. **Sparse regional holograph with honest empty tesserae from nano through inf:** that is the uvRTH bet.
