# rUv ecosystem synergy flywheel (not score hacks)

**Status:** Living strategy (RuvNet Brain–grounded)  
**Date:** 2026-08-01  
**Companions:** ADR-096/097, ADR-090, `.metaharness/flywheel/GAPS.md`,  
`.metaharness/flywheel/STRING.md`, `docs/research/ruv-worldgraph-vs-weftos.md`,  
`.grok/rules/ruflo-grok.md`

---

## 0. The flywheel string

```
SEE → WIRE → BUILD → UPSTREAM
```

| | |
|--|--|
| **SEE** | Exists; agents can’t find it |
| **WIRE** | Exists; not on the agent path |
| **BUILD** | Missing in WeftOS |
| **UPSTREAM** | Must live in rUv/Cognitum (or we contribute reference) |

**Darwin view of the same loop:** traverse the capability tree → compare
node-by-node → classify → mutate harness only → measure (foundation/genome/
alignment) → promote. Prefer SEE/WIRE; alignment and score are compatible when
score is a side-effect of real surface.

**Sensor-synergy analogy:** multi-source fusion over a **code/feature DAG**
(crates, ADRs, hosts, gates) — not a single score sensor. Same spirit as Graph
Views over BVH + live + chain.

**Future substrate:** a **WeftOS brain** (mirror of ruvbrain / `search_ruvnet`)
so agents can crawl *our* graph as easily as rUv’s — then Darwin/compare is a
graph join, not a scavenger hunt. See §11.

---

## 1. Thesis

The MetaHarness **scorecard is a shallow canary**, not the product goal.

The flywheel exists so WeftOS **stays cross-compatible with the rUv + Cognitum
stack as it ships** — Ruflo orchestration, AgentDB/RVF memory, MetaHarness
promote discipline, RuVector substrate, WorldGraph-class twins, **and Cognitum
(gate receipts, MaaS/Fugu metering, Seed/edge, identity)** — while WeftOS remains
the **Rust agent OS** (kernel, mesh, ECC, edge/hardware).

A concrete pathfinder bet:

> **WeftOS may be among the first serious products to run production agent work
> as Grok (executor) + Ruflo (orchestrator/ledger) + MetaHarness (evolve/promote),
> with a real OS underneath — and with Cognitum-shaped governance/metering where
> cloud or multi-tenant inference is used.** Upstream MetaHarness hosts today are
> Claude Code, Codex, OpenCode, Hermes, OpenClaw, RVM, Copilot, GitHub Actions —
> **not Grok**. We are building that overlay in-repo (`.grok/`, team bus, rules)
> and should treat it as an **upstream contribution surface**, not a private fork
> culture.

Score hacks that only fatten README tokens without shared contracts are
**rejected**. Alignment is proven by **shared interfaces, patterns, receipts,
dual-host recall, and Cognitum-compatible gates/keys** — not by ADR-041 vanity.

---

## 2. Division of labor (locked)

| Layer | Owner | Must remain |
|-------|--------|-------------|
| **Grok Build** | Executor | edits, shell, tests, subagents, synthesize |
| **Ruflo / claude-flow MCP** | Orchestrator | memory, swarm, hooks, team bus, doctor |
| **MetaHarness flywheel** | Promote gate | evaluate → receipt → promote; frozen gate |
| **@metaharness/darwin** | Optional WRITE | mutate harness under `.metaharness/variants/` only |
| **WeftOS Rust product** | Runtime | builds/runs **without** Node/MH (ADR-150 mirror) |

This matches ruflo **ADR-150** (optional MH, graceful degradation) and agentic-flow
**ADR-075/076** (meta-harness = freeze model, evolve harness).

---

## 3. rUv stack map → WeftOS (synergy axes)

### 3.1 Host / orchestration

| rUv | Status | WeftOS alignment |
|-----|--------|------------------|
| MetaHarness **9 hosts** (`host-claude-code` … `host-rvm`) | Shipped adapters | WeftOS uses **Claude + Grok overlays**; no `@metaharness/host-grok` yet |
| Ruflo team bus / swarm | Shipped MCP | `scripts/grok-team-bus.mjs`, `.grok/skills/agent-teams-grok` |
| Ruflo metaharness plugin (score/genome/mint/evolve) | Plugin surface | Local `scripts/metaharness/*`; MCP parity incomplete |
| `@metaharness/router` (ADR-148/149) | Cost-optimal routing | Not wired as primary WeftOS router; opportunity |

**Synergy play:** document + test **Grok as host** so patterns work when rUv
eventually adds `host-grok` (or accepts our overlay as reference). Do not
require Claude Code for WeftOS release engineering.

### 3.2 Memory / patterns

| rUv | WeftOS |
|-----|--------|
| AgentDB `.rvf`, hierarchical tiers, pattern-store / ReasoningBank | `seed-patterns.sh`, COW memory crate, AgentDB via Ruflo |
| RMM (Tan et al. ACL 2025, arXiv:2503.08026) — topic merge + citation-RL rerank | WeftOS gap: `MEMORY.md` dump. Landing: frozen VectorStore/HNSW + SONA `observe` ±1. Canonical `docs/research/rmm-reflective-memory-management.md`. Plane **WEFT-732** then **WEFT-733** |
| Namespace `patterns` multi-host recall | ADR-096 §3 — store winning ViewSpec/harness patterns |
| Causal graph / Reflexion | ECC + ExoChain (product authority, not MH) |

**Synergy play:** pattern keys and namespaces stay **compatible with Ruflo
`memory_store` / `agentdb_pattern-*`** so Claude and Grok sessions share
memory. Avoid inventing a third pattern store.

### 3.3 World / fusion twin

| rUv | WeftOS |
|-----|--------|
| **WorldGraph** — typed graph twin + provenance | **Graph Views** + BVH + chain |
| **worldgraph** metaharness CLI (coding harness for twin crates) | Own MetaHarness tasks for fusion-view |
| OccWorld occupancy predictor | LeWM (ADR-090) — ECC remains authority |

**Synergy play:** ViewSpec evolve uses the **same promote discipline** as MH
flywheel (receipts, anchors). Schema vocabulary can converge with WorldGraph
edge provenance fields without merging crates.

### 3.4 Substrate / crypto / mesh

| rUv | WeftOS |
|-----|--------|
| RuVector HNSW, RVF, graph-node | In-tree / forked RVF, vector path, BVH on RVF |
| QUIC / mesh siblings | WeftOS mesh QUIC path |
| `@metaharness/harness` control plane (receipts, bandit, default-deny) | Product governance + MH receipts for **policy** only |

**Synergy play:** keep **pin discipline** on ruflo/ruvector (already in
`package.json` `weftos.rufloPin`). Flywheel should **fail closed** if pins
drift without a receipted bump.

### 3.5 Evolve / promote

| rUv primitive | WeftOS use |
|---------------|------------|
| `metaharness score` | Canary only |
| `metaharness genome` | READ readiness (READY signal) |
| `@metaharness/flywheel` | Measure + promote gate for **policy** |
| `@metaharness/darwin` | Optional WRITE on `.metaharness/**` only |
| Witness / sign harness | Future for `.harness/` or task manifests |

### 3.6 Cognitum (platform + edge + MaaS) — required alignment pillar

Cognitum is **not** a side brand. In rUv’s own docs it is the **multi-tenant
governance / cost / safety platform** that MetaHarness rides on for hosted
inference, plus the **Seed/edge hardware + cog** stack and the **commerce/API**
surface. WeftOS already ships one concrete integration:

| Cognitum surface | What it is (rUv/Cognitum source) | WeftOS today |
|------------------|----------------------------------|--------------|
| **`cognitum-gate-tilezero`** | Permit / Defer / Deny + cryptographic receipts (ruvector crate) | Workspace dep + `tilezero` feature (implies `exochain`) |
| **api.cognitum.one** | Catalog, payments, entitlements, `cog_` API keys, MCP listing | Not product-wired; CHANGELOG “Cognitum Seed gap sprint” history |
| **Cognitum MaaS / Fugu (ADR-203)** | Metered tiered `/v1/chat/completions`, dual OpenAI/Anthropic, budget reserve-and-commit, Darwin pods + **human approval gate** | Optional future LLM provider path (not default local/Ollama) |
| **`cog_` key scheme** | `X-API-Key` / Bearer `cog_…` → hash lookup; never log plaintext | Provider boundary must match when cloud routing is enabled |
| **Meta-proxy** | Signed `metaharness proxy install` artifacts | Dev tooling only if MH proxy used |
| **Seed + cogs** | Edge vector store, cognitive-pipeline cog, Pi Zero envelope | Parallel to WeftOS edge-pad / sensor / leaf hardware — **interop opportunity**, not merge |
| **cognitum-learn** | KB ingest/query on Seed (`/api/v1/store/*`) | Distinct from AgentDB; map as optional edge memory tier |
| **OAuth / identity** (Musica ADR-175 pattern) | PKCE + keychain; env token fallback; local planner degrade | Template for any WeftOS “sign in to Cognitum” CLI/desktop path |

**Honest Cognitum MaaS framing (their public docs):** governance/cost/safety
platform on commodity models — **orchestration is a cost lever, not an accuracy
lever**. That matches WeftOS: local/air-gapped remains first-class; Cognitum is
the **metered multi-tenant / approval-gated** path when you leave the LAN.

**Synergy plays:**

1. **Gate receipts** — keep TileZero Permit/Defer/Deny as the product-side
   cousin of MH flywheel + MaaS approval gates (same *shape*: no silent promote).
2. **Optional cloud LLM** — when WeftOS agents use cloud inference, prefer
   Cognitum-compatible auth (`cog_` / OAuth) and budget discipline over raw
   OpenAI keys in production deployments.
3. **Seed / edge** — treat Cognitum Seed vector APIs and WeftOS edge crates as
   **peers** (pair, push, query) rather than competing OS kernels.
4. **Pods** — MaaS `pod = {domain × host × tier}` maps cleanly to Ruflo swarm +
   Grok host + optional cost tier; document the mapping, don’t invent a third
   pod runtime.
5. **Degrade** — no Cognitum network → full local path (same as Musica
   deterministic local planner). Never block `weft` on cloud.

---

## 3.7 Four intervention modes (how agents “see” capability)

Alignment work is not only “build new.” Most wins are **making what already
exists visible and callable** to agents, hosts, and shallow scanners.

| Mode | Meaning | Typical work | Score / agent effect |
|------|---------|--------------|----------------------|
| **SEE** | Capability exists; agents/tools don’t know where | Index, patterns, task cards, MCP tool lists, `doctor`, skill routing | Instant readiness lift without new product code |
| **WIRE** | Capability exists; not on the path agents use | Connect scripts → MCP → Grok/Claude; seed AgentDB; surface genome next to score; TileZero tests in gate | “Suddenly visible” = often a wiring PR |
| **BUILD** | Missing product feature | New ViewSpec, provider, probe, CI fixture | Real capability + measures |
| **UPSTREAM** | Lives in rUv/Cognitum; we need them to expose or accept our surface | `host-grok`, monorepo inventory, Seed API, Fugu provider contract | Shared ecosystem; we ship reference + PR/issue |

**Rule of thumb:** prefer **SEE → WIRE → BUILD → UPSTREAM**. Hacking score
without one of these four is rejected. Many “low scores” are **SEE/WIRE gaps**
(crates, TileZero, team bus, genome) already in tree.

### Examples already in WeftOS

| Asset | Mode needed |
|-------|-------------|
| 54 workspace crates / edge tree | **SEE** (platform matrix, score surface) — mostly done |
| `metaharness genome` READY | **WIRE** score.sh next to scorecard — done |
| Grok + Ruflo team bus | **SEE/WIRE** skills + rules — pathfinding |
| `cognitum-gate-tilezero` | **WIRE** (tests/CI smoke) more than BUILD |
| Dual-host patterns | **WIRE** round-trip CI (S6) |
| `@metaharness/host-grok` | **UPSTREAM** (+ our reference package S1) |
| Fugu/`cog_` LLM path | **BUILD** optional provider (C1) or **UPSTREAM** client libs |
| WorldGraph schema bridge | **BUILD** adapter or **UPSTREAM** shared schema |
| Shallow ADR-041 inventory | **UPSTREAM** inventory deep-walk / agent-OS archetype |

Agents should discover via: `.metaharness/` tasks, Ruflo `memory`/`patterns`,
`search_ruvnet` / ruvbrain, `doctor`/genome, CONTRIBUTING front door — not only
root README tokens.

---

## 4. What the flywheel should optimize (true objectives)

Prefer SEE/WIRE first; BUILD/UPSTREAM when the contract is missing.
Not “raise ADR-041 memoryUsefulness” as a primary goal.

| Objective | Measure | Cross-compat proof |
|-----------|---------|-------------------|
| **Dual-host recall** | Pattern stored under Grok is retrieved under Claude (and reverse) via Ruflo AgentDB | Shared `patterns` namespace |
| **Grok+Ruflo pathfinding** | Full feature loop without Claude Code host | Team bus + metaharness scripts + MCP |
| **Cognitum gate kinship** | TileZero feature builds; Permit/Defer/Deny path tested | `cognitum-gate-tilezero` + exochain |
| **Cognitum optional cloud** | Cloud LLM path uses `cog_` / documented degrade-to-local | Provider boundary + no hard cloud dep |
| **Seed peer story** | Documented map edge/Seed vs WeftOS edge (not dual OS) | Research + optional probe |
| **Fusion promote safety** | ViewSpec candidate → eval → receipt → human promote | Same shape as MH promote + MaaS approval gate |
| **Removability** | `scripts/build.sh gate` with no Node MH / no Cognitum network | ADR-150 + air-gap first |
| **World twin vocabulary** | ViewSpec fields mappable to WorldGraph provenance | Research crosswalk |
| **Pin honesty** | ruflo/metaharness/cognitum-gate versions pinned | Receipt notes version |
| **Router optional** | When `@metaharness/router` or Fugu tiers used, metering/savings visible | ADR-148 / MaaS dials |

---

## 5. Gaps (ecosystem, not vanity)

### Pathfinder (Grok × Ruflo) — high value

1. **No upstream `host-grok`** — WeftOS is inventing the overlay; capture as
   contribution-ready package shape (`.grok/rules`, skills, team bus contract).
2. **MCP parity** — ruflo metaharness tools (score/genome/evolve/mint) not all
   exercised from Grok session; local scripts are the fallback (good) but
   parity matrix should be tracked.
3. **Team bus** — host-agnostic bus is the right design; keep it from
   depending on Claude `SendMessage`.

### Twin / memory

4. **WorldGraph schema export/import** — not yet a wire format bridge to Views.
5. **AgentDB pattern contract tests** — seed script exists; need round-trip test
   Claude↔Grok (or two namespaces same DB).
6. **RVF / COW memory story** vs agenticow COW — document composition, avoid
   dual sources of truth.

### Cognitum

7. **TileZero coverage** — dep present; continuous tests / receipt format docs
   still a tracked concern (Plane history).
8. **No first-class Cognitum LLM provider** in clawft/weft (optional) — raw
   OpenRouter/OpenAI more common in agent tooling.
9. **Seed interop** — WeftOS edge/hardware is real; no documented pair/push/query
   contract vs Cognitum Seed store API.
10. **MaaS approval-gate mapping** — conceptual only (human promote); no pod
    budget reservation wired into agent runs.

### Evolve

11. **Darwin not on** — correct until approved; when on, only
    `.metaharness/variants`.
12. **Production promote keys** — process-local signer only.
13. **Holdout suite** still score-shaped; should add **interop fixtures**
    (pattern round-trip, ViewSpec anchors, gate without MH, tilezero smoke).

---

## 6. Flywheel policy levers (alignment-oriented)

Gen-0 policy (`.metaharness/flywheel/policy-root.json`) should grow levers like:

| Lever | Meaning |
|-------|---------|
| `primary_score` | `weftosFoundation` not ADR-041 |
| `require_genome_ready` | READ layer honesty |
| `dual_host_patterns` | require pattern seed + namespace convention |
| `grok_ruflo_path` | require `.grok/rules/ruflo-grok.md` + team-bus script |
| `ruflo_pin_check` | fail if pin mismatch undocumented |
| `worldgraph_crosswalk` | require research doc + ViewSpec provenance fields |
| `darwin_surfaces` | `metaharness_only` |
| `frozen_surfaces` | ADR-090, gate, dual-sign |

Mutating these via Darwin is **policy text / harness docs**, never Rust ECC.

---

## 7. Contribution surfaces to rUv (when ready)

| Contribution | Benefit |
|--------------|---------|
| **Reference Grok host overlay** (docs + fixture repo layout) | Fills host gap in MH 9-host matrix |
| **Interop notes: Graph Views ↔ WorldGraph provenance** | Twin stack coherence |
| **Agent OS archetype** (or monorepo genome notes) | Scorecard less wrong for OS monorepos |
| **Patterns namespace conventions** for dual-host | Ruflo memory multi-host |

Do not fork MetaHarness for score formulas. Prefer **upstream issues +
reference implementations**.

---

## 8. Explicit anti-goals

- Optimizing ADR-041 dimensions as primary KPIs  
- Auto-promoting ViewSpecs or ECC policy without human + receipt  
- Making `weft` depend on Node / MH / Ruflo at runtime  
- Replacing LeWM/GEPA/ECC with Darwin  
- Treating WorldGraph as drop-in replacement for BVH  

---

## 9. Near-term iteration (no big approval)

1. Extend **flywheel measure** evidence with alignment checks (Grok, patterns,
   WorldGraph crosswalk, **Cognitum gate dep**, synergy doc).  
2. Keep **genome + foundation** as readiness; scorecard secondary.  
3. One **interop fixture** test: patterns-manifest keys non-empty + seed dry-run.  
4. ADR-096 “Next”: dual-host recall + Cognitum optional cloud story > ADR-041 vanity.

## Needs approval (big)

| ID | Change |
|----|--------|
| **S1** | Package **host-grok reference** for rUv contribution |
| **S2** | Optional `@metaharness/router` + savings ledger |
| **S3** | Darwin evolve on harness-only surfaces |
| **S4** | ViewSpec ↔ WorldGraph schema bridge |
| **S5** | Production promote keys + CI replay verify |
| **S6** | Dual-host pattern round-trip CI |
| **C1** | Cognitum/Fugu-compatible optional LLM provider (`cog_` + local degrade) |
| **C2** | Seed peer map + probe (vs WeftOS edge) |
| **C3** | TileZero receipt format + CI smoke |

---

## 10. One-liner

**SEE → WIRE → BUILD → UPSTREAM** — stay a good citizen of rUv + Cognitum and a
pathfinder for Grok+Ruflo; let honest scores rise as a side-effect, never as
the only sensor.

---

## 11. Darwin loop + WeftOS brain (design intent)

### 11.1 Darwin view: traverse → compare

Not “evolve random files.” The productive Darwin/flywheel cycle:

```
┌─────────────────────────────────────────────────────────────┐
│  1. TRAVERSE  capability DAG (WeftOS brain + ruvbrain)      │
│  2. COMPARE   node-by-node (feature, ADR, crate, host, gate)│
│  3. CLASSIFY  SEE | WIRE | BUILD | UPSTREAM                 │
│  4. MUTATE    harness/policy only (.metaharness, docs, pins)│
│  5. MEASURE   foundation + genome + alignment + receipts    │
│  6. PROMOTE   human confirm / keys — freeze model           │
└─────────────────────────────────────────────────────────────┘
```

Each generation picks **one node class** (or one lever on policy-root), not a
free-form rewrite of the OS. Frozen: ADR-090, gate phases, dual-sign, denylist.

### 11.2 Sensor synergy on code/features

Graph Views fuse multi-source *spatial* sensors. The flywheel should fuse
multi-source *capability* sensors the same way:

| Sensor | Signal |
|--------|--------|
| Foundation score | tasks, views, crates, targets, domains |
| Genome | READY / risk / topology |
| Alignment axes | Grok path, patterns, Cognitum gate, pins |
| ruvbrain join | “does rUv have a sibling node?” |
| WeftOS brain join | “can *we* retrieve our own ADR/crate?” |
| Dual-host recall | pattern round-trip |

A “gap” is a **fusion result** (missing SEE or WIRE or BUILD or UPSTREAM), not
“ADR-041 memory is 53.”

### 11.3 WeftOS brain (like ruvbrain) — why + shape

**Problem:** ruvbrain can crawl 60+ repos; WeftOS agents still greps and memory.
Comparing graphs is asymmetric → false “BUILD” when the answer was SEE.

**Proposal (needs approval to implement fully — S7):**

| Piece | Role |
|-------|------|
| **Corpus** | ADRs, crates/*/README, FEATURE_GATES, research, ViewSpecs, patterns-manifest, CHANGELOG |
| **Index** | AgentDB / RuVector / embeddings under `.metaharness/brain/` or `.swarm` |
| **API** | `search_weftos` / `weftos_brain_query` (MCP or CLI) — same *feel* as `search_ruvnet` |
| **Compare job** | `scripts/metaharness/crosscut.sh` — join ruvbrain hits × weftos-brain hits → table of modes |
| **Removable** | optional; `weft` never depends on it (ADR-150) |

Until S7 ships, the manual crosscut is: ruvbrain query + platform surface + ADR
list + alignment measure (what this session started).

### 11.4 Approval IDs (brain + Darwin)

| ID | Change |
|----|--------|
| **S7** | Scaffold **WeftOS brain** (index + search CLI/MCP, optional) |
| **S8** | **crosscut** job: traverse/compare → classify JSON for Darwin/proposer |
| **S3** | Darwin evolve (already listed) consuming S8 classifications |
