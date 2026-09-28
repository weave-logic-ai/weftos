# MetaHarness as WeftOS foundation — flywheel, mutate, multi-host (incl. Grok)

**Status:** Living foundation plan (rUv Brain–grounded)  
**Date:** 2026-07-31  
**Companions:** ADR-096 (draft), ADR-095 / Graph Views,  
`docs/research/ruv-worldgraph-vs-weftos.md`,  
`docs/research/rmm-reflective-memory-management.md` (SONA memory-reranker
weights = flywheel champion, `confirm=true`; never silent-promote, never
RL the frozen retriever),  
`docs/plans/release-0.8-readiness-review-2026-07-31.md` §2 (score snapshot)

---

## 1. Why this is foundational (not optional polish)

WeftOS is becoming a system that **churns**:

- Sensor fusion Graph Views attach/detach live sources.
- Structure extract and splat pipelines revise world-model leaves.
- Agents, Plane DAG, and MCP surfaces change weekly.
- Cost/model routing and tool policy need evidence, not vibes.

rUv MetaHarness is built for exactly that regime:

| Package / surface | Job |
|-------------------|-----|
| `metaharness` CLI | Mint / score / genome / analyze harnesses |
| `@metaharness/router` | Cost-optimal model routing + receipts |
| `@metaharness/flywheel` | Evaluate candidates → receipts → **signed promote** |
| `@metaharness/darwin` | **Freeze the model; evolve the harness** (7 mutation surfaces) |
| ruflo / claude-flow MCP | `metaharness_score`, `metaharness_flywheel`, evolve, OIA audit |
| Host adapters | claude-code, codex, pi-dev, hermes, openclaw, rvm, … |

**Architectural constraint (ruflo ADR-150):** MetaHarness is an **optional
augmentation**, never a hard runtime dep of the kernel — graceful degrade if
absent. WeftOS must keep the same rule for **shipping `weft` binaries**, while
still treating MetaHarness as **required for agentic development / fusion
policy evolution** on the monorepo.

Slogan to operationalize:

> **Freeze the model. Evolve the harness. Promote only what proves lift.**

For fusion: freeze foundation models (and ECC R1–R5); evolve **ViewSpecs,
attach policies, promote gates, agent context packs**; promote only with
receipts as data and sensors evolve.

---

## 2. Current WeftOS state (measured 2026-07-31)

`metaharness score` / MCP `metaharness_score` on repo root:

| Dimension | Score | Gap |
|-----------|------:|-----|
| harnessFit | **75** | OK (>70) |
| compileConfidence | **100** | |
| taskCoverage | **65** | Need gate/plane/fusion/release tasks |
| toolSafety | **90** | |
| memoryUsefulness | **51** | AgentDB underused by harness config |
| archetype | mcp-server-harness | |
| scaffoldReady | true | 6/6 hard constraints |

Already present:

- `ruflo` / `@claude-flow/cli` pinned in root `package.json` (AgentDB owner).
- `node_modules/metaharness` + `@metaharness/*` (transitive).
- Release readiness already ran score + OIA dry-run (§2 of readiness review).
- Claude-side skills: `savings` (routing ledger), ruflo-metaharness plugin pattern.
- **Grok:** project `.grok/rules`, skills, ruflo doctrine — **no first-class
  MetaHarness host adapter named `grok`** in current metaharness host list
  (`claude-code|codex|pi-dev|hermes|openclaw|rvm|copilot|opencode|github-actions`).

---

## 3. What “adopt in Grok” means

Grok Build is already the **executor** (edit/shell/tests) with Ruflo as
orchestrator (`.grok/rules/ruflo-grok.md`). MetaHarness adoption in Grok is:

### 3a. Project layer (do now — no upstream host package required)

1. **Document** MetaHarness as foundation (this file + ADR-096).  
2. **`.grok/rules`** — when to score / flywheel / store patterns; never silent
   promote.  
3. **`.grok/skills`** — thin wrappers or pointers to `savings`, flywheel status,
   `metaharness score`, fusion View evolve checklist.  
4. **Scripts** under `scripts/metaharness/` — score, genome, flywheel status,
   optional evolve dry-run; callable from Grok and Claude equally.  
5. **AgentDB `patterns`** — store successful ViewSpecs, promote gates, routing
   outcomes (raise memoryUsefulness).  
6. **Tasks** — mint harness tasks for: `scripts/build.sh gate`, plane-dag ready,
   fusion View smoke, release dry-run (raise taskCoverage → ≥80).

### 3b. Host adapter layer (upstream / later)

- Prefer contributing **`@metaharness/host-grok`** (or document Grok as
  “claude-shaped + `.grok/` overlay”) rather than forking metaharness.
- Until then: treat **Grok + Ruflo MCP + project rules** as the host binding
  (same as ruflo-grok doctrine: Grok executes, Ruflo coordinates).

### 3c. What not to do

- Do not make `weft` daemon **require** Node/metaharness at runtime.  
- Do not auto-evolve production ViewSpecs without flywheel **confirm + keys**.  
- Do not conflate GEPA prompt evolution (ADR-017) with Darwin harness
  evolution — both are flywheels; different surfaces (prompts vs harness
  policies / ViewSpecs).

---

## 4. Mutation surfaces for WeftOS (extend Darwin’s seven)

Darwin’s seven (upstream): `planner`, `contextBuilder`, `reviewer`,
`retryPolicy`, `toolPolicy`, `memoryPolicy`, `scorePolicy`.

**WeftOS-specific surfaces** (candidates for flywheel anchors — not all in
Darwin 0.x today; may start as **versioned JSON/TOML configs** under
`.metaharness/weftos/` or `config/` with the same evaluate→receipt→promote
discipline):

| Surface | Churn driver |
|---------|----------------|
| `viewSpecPolicy` | Which sources attach; windows; caps (Graph Views F1–F5) |
| `fusionSoftEdgePolicy` | ANN k, min_score, DiskANN vs HNSW cold |
| `promoteGatePolicy` | When View components become BVH leaves (F9) |
| `batchAnalyticsPolicy` | When to spill WCC/PageRank (ADR-095 activation) |
| `lewmImpulsePolicy` | What WM may inject (must respect ADR-090 R1–R5) |
| `planeDagPolicy` | Cycle priority / claim rules for agents |
| Classic Darwin 7 | Agent coding / release / MCP operator harnesses |

**Flywheel loop on data churn:**

```text
sensor/agent churn
  → harvest evaluation sample (anchors + live tasks)
  → propose candidate policy / ViewSpec mutation
  → sandbox score (no net secrets; safety inspect)
  → immutable receipt
  → human or keyed promote (CAS)
  → champion ViewSpec / harness config
  → AgentDB pattern store + ExoChain optional audit event
```

---

## 5. Integration architecture (foundation)

```text
┌────────────────── hosts ──────────────────┐
│  Grok Build · Claude Code · Codex · …     │
│  (execute tools; load project rules)      │
└───────────────────┬───────────────────────┘
                    │
┌───────────────────▼───────────────────────┐
│  Ruflo / claude-flow MCP (optional)       │
│  score · genome · flywheel · evolve · OIA │
└───────────────────┬───────────────────────┘
                    │
┌───────────────────▼───────────────────────┐
│  MetaHarness packages (devDependency)     │
│  score / router / flywheel / darwin       │
│  receipts under .metaharness/             │
└───────────────────┬───────────────────────┘
                    │ policies / ViewSpecs (promoted)
┌───────────────────▼───────────────────────┐
│  WeftOS runtime (no MH required)          │
│  Graph Views · BVH · LeWM · ECC · mesh    │
└───────────────────────────────────────────┘
```

**Four ruflo constraints (ADR-150) — adopt verbatim:**

1. **Removable** — uninstall MH → `weft` still builds/runs.  
2. **Optional in package.json** — devDependency / optionalDependency only.  
3. **Graceful degradation** — scripts/MCP report `degraded: true`.  
4. **CI-gate** — smoke that score works when present; never fail core gate solely
   because MH absent (unless `WEFT_REQUIRE_METAHARNESS=1` on agent CI).

---

## 6. Concrete phases

### Phase 0 — Document + measure (this PR)

- [x] Research notes + draft ADR-096  
- [x] Cross-link Graph Views / WorldGraph  
- [ ] Baseline score committed in readiness or `.metaharness/score-latest.json`
      (optional regenerate in CI agent lane)

### Phase 1 — Dev harness spine (next coding session)

- [ ] `scripts/metaharness/score.sh` → JSON + markdown  
- [ ] `scripts/metaharness/flywheel-status.sh`  
- [ ] `.metaharness/README.md` — receipts layout, promote rules  
- [ ] `.grok/rules/metaharness.md` — Grok must consult score/flywheel for
      fusion policy and harness changes  
- [ ] Mint ≥3 tasks: gate, plane-dag ready, fusion-view-smoke  
- [ ] AgentDB store patterns for successful releases / View decisions  
- Target: taskCoverage ≥80, memoryUsefulness ≥70

### Phase 2 — Fusion flywheel anchors

- [ ] Versioned `ViewSpec` fixtures under `config/views/` or
      `.metaharness/weftos/views/`  
- [ ] Anchor tasks: identity precision/recall on fixture multi-cam/co-observe  
- [ ] Flywheel **evaluate-only** on ViewSpec mutations; promote manual  

### Phase 3 — Darwin / evolve (opt-in)

- [ ] `npx metaharness-darwin evolve` on **agent harness** trees only first  
- [ ] Never auto-evolve kernel safety or ADR-090 predicates  
- [ ] Signed promote keys in operator env, not repo  

### Phase 4 — Grok host productization

- [ ] Upstream or local `@metaharness/host-grok` if API stabilizes  
- [ ] Or document “Grok = executor + Ruflo MCP + `.grok` overlay” as supported  

---

## 7. Relation to existing WeftOS flywheels

| Loop | Surface | MetaHarness role |
|------|---------|------------------|
| DEMOCRITUS / ECC modes | Cognitive tick | Domain; MH does not replace |
| GEPA (ADR-017) | Prompt genomes | Parallel; can share receipt discipline |
| LeWM train/rollback | WM weights | Domain; MH evolves **impulse policy** not weights |
| Graph View fusion | ViewSpec / promote | **Primary MH application** for sensors |
| Cost router | Model tier | `@metaharness/router` + savings ledger |
| Plane DAG agents | Task claim/close | Tasks + memory patterns |

---

## 8. Security / promote discipline

From flywheel ADR-322-class tools:

- Evaluation **never** mutates the active champion.  
- Promote requires **confirm=true** + approved Ed25519 public key path.  
- Darwin: one surface per generation; safety inspect before sandbox.  
- Exit 99 = safety disqualify.  
- WeftOS: promote of ViewSpec that changes promote-to-BVH gates should emit
  ExoChain audit when wired (ADR-022).

---

## 9. Success criteria (foundation “done enough”)

1. Any Grok or Claude session can run `scripts/metaharness/score.sh` and see
   dimensions without hunting MCP tool names.  
2. Fusion policy changes leave a **receipt** or explicit “no MH” waiver in PR.  
3. memoryUsefulness ≥70 via real pattern stores.  
4. taskCoverage ≥80 with gate/plane/fusion tasks.  
5. Kernel still builds with `node_modules` deleted (optional MH).  

---

## 10. Document history

| Date | Change |
|------|--------|
| 2026-07-31 | Initial foundation plan from rUv Brain + live score + Grok host gap |
