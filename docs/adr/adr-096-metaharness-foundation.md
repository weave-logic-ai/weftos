# ADR-096: MetaHarness as foundational agent/fusion evolution layer

- **Status**: Draft (Proposed — foundation adoption; not a `weft` runtime hard dep)
- **Date**: 2026-07-31
- **Deciders**: Pending (platform / agent / sensor-fusion maintainers)
- **Related**:
  - ADR-017 (GEPA prompt evolution — parallel flywheel on prompts)
  - ADR-022 (ExoChain audit — optional promote events)
  - ADR-045 / tiered router (in-tree cost routing — compose with MH router)
  - ADR-078 / ADR-095 / Graph Views (fusion operational model)
  - ADR-090 (LeWM decoupling — MH must not violate R1–R5)
  - rUv / ruflo ADR-150 (MetaHarness optional integration surfaces)
  - rUv ADR-148/149 (cost-optimal router)
  - **ADR-097** (universal data-surface governance)
  - `@metaharness/flywheel` (receipts + signed promote)
  - `@metaharness/darwin` (“freeze the model, evolve the harness”)
- **Source**:
  - `docs/research/metaharness-foundation.md`
  - `docs/research/ruv-worldgraph-vs-weftos.md`
  - `docs/research/rmm-reflective-memory-management.md` (SONA memory-reranker weights are a flywheel champion — evaluate → receipt → `confirm=true`; paper ablation shows RL-updating the retriever collapses)
  - `docs/plans/release-0.8-readiness-review-2026-07-31.md` §2
  - Live `metaharness_score` on weftos (2026-07-31): harnessFit 75, taskCoverage 65, memoryUsefulness 51

## Context

WeftOS agent work already uses Ruflo/claude-flow and has run MetaHarness
**score/OIA** for 0.8 readiness. Sensor fusion is now defined operationally as
**Graph Views** that churn with live multi-source data. Without a governed
**evaluate → receipt → promote** loop, ViewSpecs, attach policies, promote gates,
and agent harness configs will either stagnate or change without proof.

rUv MetaHarness provides that loop as a first-party sibling to ruflo:

- **Score / genome / OIA** — readiness and safety signals  
- **Router** — cost-optimal model routing with receipts (`savings` skill)  
- **Flywheel** — candidate evaluation into immutable receipts; explicit promote  
- **Darwin** — mutate harness policy surfaces with the model frozen  

Grok Build is a primary WeftOS development host but is **not** yet a named
`metaharness --host` adapter; adoption must work via **project rules + scripts +
Ruflo MCP** first, host package later.

## Decision (Draft)

### 1. MetaHarness is foundational for **agentic development and fusion policy**

WeftOS **adopts MetaHarness** as the standard layer for:

| Concern | Mechanism |
|---------|-----------|
| Harness readiness | `metaharness score` / genome / OIA |
| Cost routing evidence | router receipts / savings ledger |
| Policy evolution under churn | flywheel evaluate + signed promote |
| Optional self-improvement | Darwin on **harness/policy** surfaces only |

It is **not** foundational for the shipped `weft` daemon binary path: production
nodes must run without Node/metaharness installed.

### 2. Four constraints (mirror ruflo ADR-150)

1. **Removable** — delete MH packages → workspace still builds (`scripts/build.sh`).  
2. **Optional dependency** — MH lives in dev/optional Node tooling, never `weft` link.  
3. **Graceful degradation** — scripts/MCP return degraded when MH absent.  
4. **CI** — agent/harness CI may require MH; core Rust gate must not.

### 3. Graph Views / sensor fusion evolve through flywheel discipline

When fusion data or sources churn:

1. Mutate **ViewSpec / attach / soft-edge / promote-gate** candidates (not silent edits only).  
2. Evaluate against anchors + fixtures (and later soak).  
3. Write **immutable receipts**.  
4. **Promote** only with confirm + keys (or explicit human merge of receipt-backed PR).  
5. Store winning patterns in AgentDB `patterns` for multi-host recall (Grok/Claude).  

ECC authority and LeWM R1–R5 remain non-negotiable (ADR-090); Darwin/flywheel
must not evolve “WM overrides ECC” policies into champions.

### 4. Grok adoption path (pathfinder)

| Phase | Action |
|-------|--------|
| Now | `.grok/rules` + skills + team bus + `scripts/metaharness/*` + Ruflo MCP |
| Next | **Dual-host pattern recall** + genome/foundation readiness (not ADR-041 vanity) |
| Later | Documented Grok overlay → candidate **`@metaharness/host-grok`** contribution |

Grok remains **executor**; Ruflo/MetaHarness remain **orchestrator / evolve**.
Upstream MH hosts today do not include Grok — WeftOS is early ground truth for
that composition. Ecosystem strategy:
`docs/research/ruv-ecosystem-synergy-flywheel.md`.

### 5. Explicit non-goals

- Auto-evolving production ViewSpecs without receipts.  
- Replacing GEPA, LeWM training, or ECC DEMOCRITUS with Darwin.  
- Requiring MetaHarness on edge devices or Android splat nodes.  
- Blocking 0.8 ship on flywheel Phase 2.

## Consequences

### Positive

- Shared evolve language across fusion, agents, and multi-host (Grok included).  
- Alignment with rUv WorldGraph-class twins that also expect provenance + governed change.  
- Measurable harness improvement (score dimensions already tracked).  

### Negative / risks

- Another Node toolchain surface to pin (already true via ruflo).  
- Weak anchors evolve wrong policies — mitigate with fixture anchors + safety inspect.  
- Key management for promote if using signed receipts.  

### Neutral

- Score snapshot (fit 75 / coverage 65 / memory 51) is the baseline to beat.

## Follow-ups

- [x] `docs/research/metaharness-foundation.md`  
- [x] `docs/research/ruv-worldgraph-vs-weftos.md`  
- [x] Phase 1 scripts + `.grok/rules/metaharness.md`
- [x] Phase 1 tasks (WEFT-725), patterns (WEFT-726), ViewSpec anchors (WEFT-727)
- [x] ADR-097 universal data-surface governance (WEFT-728)  
- [ ] Mint harness tasks (gate, plane-dag, fusion-view)  
- [ ] AgentDB pattern store for fusion/release wins  
- [ ] Optional: promote `.planning/research/cosmos3-*.md` into `docs/research/`  
- [ ] Accept this ADR when Phase 1 lands

## References

1. ruflo ADR-150 MetaHarness integration surfaces; ADR-153 Darwin; ADR-148 router.  
2. `@metaharness/flywheel` — verifiable self-improvement; promote with receipts.  
3. `@metaharness/darwin` — freeze model, evolve harness.  
4. MCP: `metaharness_score`, `metaharness_flywheel` (claude-flow/ruflo).  
5. WeftOS Graph Views: `docs/research/graph-views.md`.  
