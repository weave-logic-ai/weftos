---
title: "Research Streams (Active + Landed-as-Feature)"
slug: research-streams
workstream_id: "17"
status: active
last_updated: 2026-04-28
release: 0.7.0
review_type: comprehensive-audit
audience: release-gate
owners: [research, kernel, weave, docs]
---

# Workstream 17 — Research Streams (Active + Landed-as-Feature)

## General Description

This workstream tracks the WeftOS / clawft research → feature pipeline. Unlike the
shipping workstreams, "Research streams" is a heterogeneous bucket: it includes
research that has already converted into shipped features (ECC, EML, Democritus
loop, Exochain governance certification), research that is actively in flight
(JEPA / LeWM world model, knowledge-graph paper synthesis), and exploratory
material that has not yet picked a landing target (Pasqal / NVIDIA quantum,
coherence-lattice-alpha, ruv-ecosystem alignment, sonobuoy paper survey,
gaming-robotics symposium output).

The audit is structured deliberately: for every research thread, we record (a)
its current status (landed, active, exploratory, deferred, orphaned), (b) the
artifacts that exist in-tree, (c) the open gaps, deferred items, TODOs, and
unresolved questions, and (d) where the work plugs back into a shipping
workstream. **This is not a 0.7.0 ship-gate.** Most of the items below are
out-of-scope for the release; they are recorded so that follow-up sprints have
a single index of what was studied, what landed, and what is still on the floor.

The architectural anchor for the active research is the LeWM × ECC × WeftOS
diagram in `.planning/symposiums/lewm-worldmodel/diagram.md`, formalised across
ADR-048 → ADR-058. The shipping research anchor is the K3c "ECC cognitive
substrate" track that landed in `clawft-kernel` (causal.rs, hnsw_service.rs,
cognitive_tick.rs, eml_coherence.rs, democritus.rs) plus the `eml-core` crate
and the `weftos/eml.mdx` / `weftos/ecc.mdx` doc set.

## Status & Timeline

| Phase | Stream | Status | Where it lives | Notes |
|-------|--------|--------|----------------|-------|
| Landed | ECC cognitive substrate (K3c) | SHIPPED | `crates/clawft-kernel/src/{causal,crossref,impulse,hnsw_service,calibration,cognitive_tick}.rs`, `docs/.../weftos/ecc.mdx` | 83 ECC tests, 562 total with `--features exochain,ecc`. Boot `boot_ecc()` runtime function still deferred — see "What's Left". |
| Landed | EML coherence (sprint 16) | SHIPPED | `crates/eml-core/`, `crates/clawft-kernel/src/eml_coherence.rs`, `docs/.../weftos/eml.mdx`, `docs/.../weftos/eml-attention.mdx` | Two-tier coherence wired; misbehaving in production (see Democritus). |
| Landed | Democritus two-tier loop (sprint 16) | SHIPPED, MISBEHAVING | `crates/clawft-kernel/src/cognitive_tick.rs::run_democritus_loop` | `kernel.log` shows `DEMOCRITUS: still stuck after 256 checks: Stuck { net_change: 0.0 }` repeatedly — loop is alive but its inputs are flat-lined. |
| Landed | Exochain governance certification | SHIPPED with 2 minor gaps | `crates/clawft-kernel/src/governance.rs`, `crates/clawft-kernel/src/chain.rs`, scattered call sites | `governance-certification.md` — 19 gates, 14/14 high-priority items covered. Two governance gaps remain: `auth_service:rotate_credential` and `auth_service:request_token` log to chain but are not gated. |
| Active | JEPA / LeWM world model | IN FLIGHT | `.planning/lewm-worldmodel-rs-page/{DESIGN-A,DESIGN-B,PLAN}.md`, `.planning/symposiums/lewm-worldmodel/diagram.md`, `docs/src/app/lewm-worldmodel-rs/` | Public marketing/explainer page exists at `/lewm-worldmodel-rs`. ADR-048 → ADR-058 drafted. **No `weftos-worldmodel-core`, `weftos-worldmodel-impls`, `weftos-worldmodel`, `weftos-sensor-pipeline`, `weftos-sensor-pipeline-wire`, `clawft-worldmodel-service`, or `clawft-delegation` crate exists yet** — the diagram describes future state. |
| Active | KG paper synthesis (Phase 1 + Phase 2) | IN FLIGHT, partially landed | `.planning/development_notes/knowledge-graph-paper-survey.md`, `knowledge-graph-paper-survey-phase2.md`, `arxiv-2410-05779-analysis.md`, `arxiv-2603-21852-analysis.md` | EML coherence paper (2603.21852) has fully landed. GraphRAG, CausalRAG, RoMem, SASE not yet implemented. **Cross-cuts with Stream 12 (graphify analyze).** |
| Survey / synthesis | Sonobuoy papers | SURVEY COMPLETE | `.planning/sonobuoy/{SYNTHESIS.md,GAPS.md,RANGING.md,papers/analysis/}` (66 paper analyses, 7 gaps tracked) | Round-2 verification mandate (ADR-062) closed round-1 fabrication issue. G1 closed; G2-G5 open. No code shipped yet. |
| Exploratory | Quantum integrations (Pasqal, NVIDIA) | DEFERRED | `.planning/development_notes/{nvidia-quantum-integration,pasqal-integration}.md` | Pasqal: backend skeleton designed, not implemented. NVIDIA Ising: declined, but cuDensityMat-backed `SimulatorBackend` queued behind `quantum-nvidia` feature flag for v0.7.x post-GUI. |
| Exploratory | Coherence-lattice-alpha | DEFERRED | `.planning/development_notes/coherence-lattice-alpha-integration.md` | AGPL + CC BY-NC + 5 commits + broken internal deps. Conceptual takeaway logged; revisit on relicensing. |
| Exploratory | ruv ecosystem analysis | INFORMATIONAL | `.planning/development_notes/{ruv-ecosystem-analysis-20260414,ruvector-weftos-alignment,openfang-comparison,ruview-eml-contributions}.md` | Used to drive ruvector-cluster / -raft / -replication / -diskann dep choices and OpenFang gap analysis. Not landing as a single feature. |
| Exploratory | Closure-SDK | DECLINED | `.planning/development_notes/closure-sdk-integration.md` | AGPL blocker; conceptual-only. |
| Survey / synthesis | Gaming-robotics symposium | SYMPOSIUM COMPLETE | `.planning/symposiums/gaming-robotics/{FINAL_REPORT.md,robotics/research-report.md,gaming/,integration/,experiments/,slides/}` | 8 experiments planned ($1,102 hardware budget). No experiments executed yet. |
| Survey / synthesis | Other symposiums | VARIES | `.planning/symposiums/{cognitum-seed-gaps,cold-case-ecc,compositional-ui,ontology-navigator,RLM - arxiv-2512.24601}/` | Ontology-navigator drove ADR-treecalc-eml-architecture. Cold-case-ecc drove ECC-XAI infographic + legal deep-dive. Cognitum-seed-gaps drove tiered-kernel-profiles. |

**Timeline anchor**: ECC and EML landed in K3c (March 2026) and sprint 16
(early April 2026) respectively. LeWM ADR batch 048-058 drafted ~April 2026
(diagram + symposium). The marketing page is mid-implementation. The
implementation crates are not in-tree as of master HEAD on 2026-04-28.

## Released Features (Research that Became Features)

### ECC Cognitive Substrate (K3c)

Source: `.planning/development_notes/k3c-ecc-integration.md`, `docs/src/content/docs/weftos/ecc.mdx`.

What the original research argued for: a deterministic, typed, signed
causal-DAG layer beneath the LLM agent loop, plus a small set of services
(causal graph, cross-reference, impulse queue, HNSW vector backend, calibration
benchmark, cognitive tick) wired through the resource tree and exochain.

What landed:

- 6 new kernel modules (causal.rs, crossref.rs, impulse.rs, hnsw_service.rs,
  calibration.rs, cognitive_tick.rs) totalling ~1,950 lines + 83 tests behind
  the `ecc` feature flag.
- `ToolCategory::Ecc`, 7 ECC tool specs, ECC resource-tree namespaces under
  `/kernel/services/ecc/`, `BootPhase::Ecc`, `NodeEccCapability` cluster type.
- Boot calibration < 100ms with 100 synthetic ticks (target was < 2 s).
- `ExoChainHeader`, `EXOCHAIN_MAGIC`, `write_exochain_event`,
  `decode_exochain_payload` redefined locally in `chain.rs` after upstream
  rvf-types removed them. Header `chain_id` width changed from u64 → u32.
- Chain RVF breakage repaired; all 479 pre-existing tests pass; 562 tests pass
  with `--features exochain,ecc`.

What did **not** land — recorded under "What's Left":

- `boot_ecc()` runtime function (creates HnswService + CausalGraph +
  CognitiveTick instances and wires them into `Kernel<P>`). Resource tree
  namespaces and tool catalog are populated, but service instances and
  calibration are not yet executed at boot time.
- WASM browser-target verification of the `ecc` feature exclusion.
- Dedicated chain-event integration tests for ECC operations.
- `clawft-weave` `weaver ecc` subcommands.
- 5 pre-existing clippy warnings remain in `agent_loop.rs`, `chain.rs`,
  `gate.rs` (not introduced by ECC, but unaddressed).
- New RVF segment-type definitions for ECC structures (conceptual, not
  implemented).

### EML Cluster (Sprint 16 + ongoing)

Source: `.planning/development_notes/{eml_model_development,
eml-causal-collapse-research,eml-synergy-scan,hnsw-eml-analysis,
hnsw-eml-deep-analysis,ruview-eml-contributions,coherence-lattice-alpha-integration}.md`,
`.planning/development_notes/sprint-16/eml-coherence.md`,
`docs/src/content/docs/weftos/eml.mdx`, `docs/.../weftos/eml-attention.mdx`,
`crates/eml-core/`.

The research traces back to Odrzywolel 2026 (arXiv:2603.21852v2) — every
elementary mathematical operation can be reconstructed with nested
`eml(a,b) = exp(a) − ln(b)` plus the constant 1. WeftOS uses this for two
distinct purposes:

1. **Function discovery** — for HNSW heuristics, surprise scoring, density
   thresholds, ForceAtlas2 physics constants, and similar hard-coded
   coefficients, replace with a small `EmlModel` that learns the closed-form
   function. Trained weights snap to {0,1}, making the result interpretable
   rather than a black box.
2. **Coherence approximation** — replace 500 µs Lanczos-based λ₂ computation
   with a 34-parameter, depth-3 EML master formula that predicts λ₂ from 7
   graph statistics in ~0.1 µs. Self-trains from exact Lanczos results
   collected during normal operation (two-tier cadence: O(1) fast every tick,
   O(k·m) exact when drift > 5 % or every 100 ticks).

What landed:

- `eml-core` crate (`Cargo.toml`, `src/{lib,model,operator,tree,features,
  events,attention,baseline_attention}.rs`, `examples/{attention_compare,
  attention_gate}.rs`).
- `EmlModel::{new,predict,record,train,distill}`, coordinate-descent training
  with random restarts, multi-head output, JSON serialization, ExoChain
  `EmlEvent` emission.
- `clawft-kernel/src/eml_coherence.rs` — `EmlCoherenceModel`, `GraphFeatures`,
  EML operator wired through `EccSubsystem.eml_coherence`,
  `kernel.ecc_eml_coherence()` accessor, `CausalGraph::coherence_fast(&model)`.
- HTTP facade route `GET /api/ecc/coherence` → `ecc.coherence` RPC returning
  `{ fast, model_trained, training_samples, last_exact }`.
- `ToyEmlAttention` (Iteration 0 hybrid: 5 EmlModels for projections + softmax,
  f64 matmul between them) behind `experimental-attention` feature on
  `eml-core` (~580 lines, 13 unit tests, 4-phase benchmark).

What did **not** land:

- Iteration 1+ end-to-end coordinate-descent loop for QKV models (Q/K/V remain
  at default initialization in Iteration 0).
- Full EML-Transformer (no published working model exists; deep-tree numerical
  stability is open).
- Truly O(1) feature extraction — current `from_causal_graph` is O(n+m) due to
  `connected_components()`. Component count needs incremental maintenance.
- Hardcoded-heuristic replacements catalogued in `eml-synergy-scan.md`
  (graphify analyze surprise scorer, cluster MAX_COMMUNITY_FRACTION,
  ForceAtlas2 6-parameter physics tuner, HNSW ef-adaptive beam, etc.) — most
  rows in that scan are still hardcoded.
- Phase-rotation temporal KG (RoMem, arXiv:2604.11544) — would replace hard
  age-pruning with geometric shadowing on CausalEdge; not yet integrated.

### Democritus Two-Tier Coherence Loop (Sprint 16)

Source: `.planning/development_notes/sprint-16/democritus-loop.md`,
`crates/clawft-kernel/src/cognitive_tick.rs::run_democritus_loop`.

What the research argued for: continuous SENSE → THINK(fast) → DETECT DRIFT →
THINK(exact) → COMMIT cycle running as a tokio background task, using
`EmlCoherenceModel::predict()` for the fast path and `CausalGraph::
spectral_analysis(50)` for the exact path. Two-tier with adaptive cadence and
periodic retrain (every 1000 exact samples).

What landed (sprint 16, committed):

- `run_democritus_loop()` async function gated behind `#[cfg(feature = "ecc")]`.
- `EccSubsystem::eml_coherence` changed to `Option<Arc<Mutex<EmlCoherenceModel>>>`
  for thread-safe sharing; loop spawned via `tokio::spawn` after
  `tick.start().await`.
- Mutex held briefly only for `predict`/`record`; never during spectral
  analysis.
- Drift threshold 0.05; exact every 100 ticks; train every 1000 exact samples;
  `spectral_k = 50`.
- Boot log entry: "DEMOCRITUS two-tier coherence loop spawned".

**Live behavioural gap (open)**: `.weftos/runtime/kernel.log` (verified
2026-04-28 at 15:36) shows the warning chain:

```
... DEMOCRITUS: still stuck after 1 checks: Stuck { net_change: 0.0 }
... DEMOCRITUS: still stuck after 2 checks ... 4 ... 8 ... 16 ... 32 ... 64 ...
... DEMOCRITUS: still stuck after 128 checks: Stuck { net_change: 0.0 }
... DEMOCRITUS: still stuck after 256 checks: Stuck { net_change: 0.0 }
```

The exponential-backoff suppression (cognitive_tick.rs L440-491) is doing its
job — but it is suppressing a `Stuck { net_change: 0.0 }` state that has been
continuous since at least 10:47 UTC on the day of this audit. The loop is
alive (the warnings are firing on a geometric schedule), but `net_change` is
flat-lined: there is no coherence variation to predict, which means either (a)
the causal graph in the running daemon is empty / static, (b)
`detect_conversation_cycle` is mis-tuned for the current workload, or (c) the
EML model is predicting a constant. **This is a P1 follow-up, not a 0.7.0 ship
blocker, but it should be filed as an issue.**

### Exochain Governance Certification

Source: `.planning/development_notes/exochain-{certification-critical,
certification-medium,certification-nonkernel,fix-plan,governance-audit}.md`,
`governance-certification.md`.

What the research argued for: every state-modifying public method in the kernel
gets ExoChain logging; critical/high paths additionally get a governance gate
in front of the mutation. Two complementary mechanisms — `GateBackend` trait
(auth, config, app, cluster, capability, environment, a2a, cron) and
`GovernanceEngine::evaluate()` with `EffectVector` (profile_store,
hnsw_service, causal, wasm_runner, http_api).

What landed:

- 19 distinct governance check call sites across 12 files (up from the
  original 2).
- All 14 high-priority items from the audit checklist are gated.
- Sandbox layer (`clawft-core::sandbox`) emits `chain_event!` markers via
  tracing; daemon subscribes and forwards.
- Non-kernel crates use `tracing::info!(target: "chain_event", ...)` since they
  cannot depend on `ChainManager` directly.
- Daemon constructs `GovernanceGate` (threshold 0.8, exec-guard
  Blocking/Judicial, cron-warn Warning/Executive) with chain manager attached.
- 32/32 MEDIUM-severity items certified (one CONDITIONAL PASS for
  `hnsw_service.rs:load_from_file` — static constructor, no chain manager
  available).

What did **not** land — open governance gaps:

- `auth_service:rotate_credential` — chain-logged but **not gated**. Any agent
  with IPC access could rotate any credential without policy check.
- `auth_service:request_token` — chain-logged but **not gated**. Token
  issuance is gated only by the `allowed_agents` check on the credential.
- `auth_service:revoke_token` — PARTIAL (chain-logged, not gated).
- `EffectVector` is "context-only" on several gates (auth, config, a2a, cron) —
  the gate runs but the `EffectVector` is empty / heuristic. Risk weighting
  on these gates is implicit, not explicit.

## Active Research

### JEPA / LeWM World Model (`feature/lewm-worldmodel`, ADR-048 → ADR-058)

This is the one research stream that is mid-flight at 0.7.0 cut.

**Architectural spine** (from `.planning/symposiums/lewm-worldmodel/diagram.md`):

- Two views: ① full-system panel diagram (sensor plane → observation wire →
  consumers split → training layer → exochain spine), ② real-time H-O-E-A
  cycle (HYPOTHESIZE → OBSERVE → EVALUATE → ADJUST) running on three
  concurrent timescales (1 ms DEMOCRITUS servo, 10 Hz planner, ≪1 Hz edge
  intelligence retrains, continuous streaming-merge checkpoints).
- **Decoupling invariant** (ADR-058): ECC remains authoritative per node; the
  latent world model is an additive consumer that publishes impulses, never
  short-circuits causal edges. When the cluster service is absent, the loop
  runs unchanged — graceful degradation.
- **SIGReg manifold** (ADR-050): isotropic-Gaussian `N(0, I)` in 192 dims,
  Welford-based `sigreg_health` measurable in production, version-tagged,
  ExoChain-attested. Auto-rollback when `sigreg_health < 0.85` for 30 s.
- **Three trainable RVF-hosted small models per sensor class** (ADR-057), hot-
  swap at tick alignment.
- **Three observation topics** under `mesh.sensor.v1.{encoded,consensus,control}`
  (ADR-053), CBOR + Ed25519 framing, observational-only packets, ExoChain-
  indexed every frame.
- **Two training surfaces** (ADR-055): offline per-sensor-class edge
  intelligence (RVF-delivered, hot-swappable) and online streaming-merge
  world-model training with a four-condition AND rollback gate (cluster SIGReg
  health, held-out probing accuracy, VoE surprise differentiation, temporal-
  straightening score).
- **Three deployment topologies** (ADR-054): single, hot-standby, peer-to-
  peer.

**What is being built (per diagram)**:

- 3 new workspace crates: `weftos-worldmodel-core` (`no_std`, traits),
  `weftos-worldmodel-impls` (candle-backed), `weftos-worldmodel` (facade).
- `weftos-sensor-pipeline`, `weftos-sensor-pipeline-wire`,
  `clawft-worldmodel-service` binary, in-tree `clawft-delegation`.
- ViT-tiny encoder + AdaLN-modulated predictor, end-to-end SIGReg (Epps-Pulley
  on M random 1D projections). No EMA, no stop-gradient scaffolding.
- `LatticeApi` (ADR-052): observe / observe_node / predict / plan / recall /
  subscribe_surprise / subscribe_drift exposed via `ServiceApi`.

**What exists in-tree at 2026-04-28**:

- ADR batch 048-058 (drafted on `feature/lewm-worldmodel`).
- Symposium diagram + system view + H-O-E-A cycle ASCII art.
- Marketing/explainer page for `weftos.weavelogic.ai/lewm-worldmodel-rs` —
  scaffolded under `docs/src/app/lewm-worldmodel-rs/` with 18 client/RSC
  components (HeroZoomFrame, InversionFlip, SystemPanelSvg, PanelPopup,
  ConsumersSplit-equivalent (FlowingSystemDiagram), TrainingLayer-equivalent
  (DeepDive), HoeaLoop, AdrIndex, CrossViewDissolve, LatentDots, SigRegManifold,
  SurpriseTimeline, plus 5 sensor visualisations under `components/sensors/`).
  Built on Next.js 16 RSC + `motion` v12, dark blueprint-schematic palette,
  `prefers-reduced-motion` hard branch.
- DESIGN-A.md (12-chapter visual-maximalist storyboard) and DESIGN-B.md
  (motion-mechanics + perf discipline; ≤25 KB gz motion JS, ≤80 KB gz total
  route, ≤40 KB SVG, LCP < 2.0 s, CLS = 0, scroll INP < 120 ms).
- Reconciled PLAN.md taking A's 12-beat narrative on B's mechanics.
- Page draft PR posture is **DO NOT MERGE — awaiting visual confirmation**.

**What does NOT exist yet** (the actual implementation):

- None of the 3+3 crates listed above have been created on master. This is
  the largest implementation gap in the workstream. Search of
  `crates/` finds no `*worldmodel*`, `*sensor-pipeline*`, or `*delegation*`
  directories.
- No `mesh.sensor.v1.*` topics on the mesh; `crates/clawft-kernel/src/mesh*`
  does not yet host these wire definitions.
- No `LatticeApi` `ServiceApi` registration.
- No `pred_φ` / `LatentPlanner` (CEM / MPPI-warm / gradient).
- No SIGReg `sigreg_health` Welford monitor in production code paths.
- No four-condition AND rollback gate.
- ExoChain attestation of (a_t, z_t, z_{t+1}, surprise) tuples not yet wired.
- The "weaver A1 amendment" referenced in the diagram (ECC authoritative per
  node) is documented in the symposium materials but not in a separate ADR
  pointer block.

### Knowledge-Graph Paper Synthesis (Phase 1 + Phase 2)

Source: `.planning/development_notes/{knowledge-graph-paper-survey,
knowledge-graph-paper-survey-phase2,arxiv-2410-05779-analysis,
arxiv-2603-21852-analysis}.md`. **Cross-cuts with Stream 12 (graphify analyze).**

Phase 1 (April 2026) surveyed 8 graphify-relevant papers and tagged
implementation priority:

- **P0 — Implement Now**:
  - GraphRAG (Edge et al. 2024, arXiv:2404.16130) — community summaries during
    pipeline analyze, hierarchical aggregation at query time.
  - CausalRAG (Wang et al. 2025, ACL Findings, arXiv:2503.19878) — causal
    chain tracing during retrieval, leveraging existing typed edges.
  - SASE (Liu et al. 2024, CIKM, arXiv:2408.05765) — parameter-free linear-time
    spectral clustering using k-order graph convolution + Random Fourier
    Features, replacing the current label-propagation in `cluster.rs`.

Phase 2 (sprint 16, 7 newer papers, 2604.* series):

- RoMem (Li et al. 2604.11544) — phase-rotation temporal KG, replaces
  hard-deletion edge pruning with geometric shadowing. `P1`.
- LightRAG (Guo et al. 2410.05779) — dual-level keyword retrieval, 610× fewer
  tokens than GraphRAG; would slot into `suggest_questions()`. `P2`.
- Odrzywolel (2603.21852v2) — already landed via EML coherence and HNSW-EML
  models. **The only Phase 2 paper that fully landed.**
- Plus 5 others (paper IDs in `phase2.md`) — `P1`/`P2`/`P3`.

**What landed**: only the EML / Odrzywolel result.

**What's open**: GraphRAG, CausalRAG, SASE, RoMem, LightRAG implementations
are all sketched and prioritised, with implementation plans, but none are in
the codebase. These are tracked under "Task List" below as carry-over items
into Stream 12.

## Survey / Synthesis Output

These are not "research that will land as a feature in 0.7.0". They are
canonical references that downstream sprints draw on.

### Sonobuoy Paper Survey (66 paper analyses + GAPS + RANGING)

Source: `.planning/sonobuoy/{SYNTHESIS.md,GAPS.md,RANGING.md,papers/analysis/}`.

- 42 papers analyzed across 12 categories in two rounds. Round 1 had a 14/18
  fabrication rate; ADR-062 formalises the "verify every citation" mandate.
  Round 2 (24/24 verified) closed the credibility gap.
- 66 paper analysis files now in `papers/analysis/`. Architecture grew from
  4 to 5 branches (added active-imaging / synthetic aperture sonar).
- Two deployment profiles: `sonobuoy-tactical` (hours-days, expendable) and
  `sonobuoy-pam` (months-years, refurbishable, HARP-class).
- 4-tier on-buoy / at-shore power hierarchy grounded by Rybakov 2020,
  MLPerf Tiny 2021, MCUNet 2020, acoupi 2026 (µW analog gate → 5 mW Cortex-M4
  → 50 mW Cortex-M7 → 200 W shore GPU).
- Federated learning layer defined: FedAvg + Deep Gradient Compression +
  Multi-Krum + Split Learning, mapping onto existing `mesh_*.rs` + Raft +
  gossip + rvf-crypto.
- 25 ADR candidates emerge: ADR-053 → ADR-077.
- **Gaps**: G1 closed (sensor-position uncertainty, `RANGING.md` 983 lines,
  ADR-078 drafted). G2-G5 open (Helmholtz-PINN 3D collapse, FNO thermocline,
  SAS unknown velocity, sub-kbps FL). G6-G7 administrative.

### Symposium Reports

- **gaming-robotics** (`.planning/symposiums/gaming-robotics/`): 5 expert teams
  + creator answers + integration architecture + 8 experiments planned at
  $1,102 hardware cost, 7-week timeline. Central thesis: "DEMOCRITUS
  cognitive loop IS a servo control loop"; PERCEIVE-THINK-ACT (PTA) framework
  unifies all WeftOS applications. **Sidecar model adopted** (WeftOS as native
  binary sidecar; game engine connects via TCP). No experiments executed yet.
- **lewm-worldmodel**: diagram only (covered in "Active Research").
- **cognitum-seed-gaps**: drove `tiered-kernel-profiles.md` and SPRINT.md.
- **cold-case-ecc**: drove the ECC-XAI infographic (v1, v2), legal deep dive,
  `case-examples.md`, deck-spec.md, research-foundations.md.
- **compositional-ui**: cross-referenced into Stream 8 (GUI dev panel +
  protocol-spec, see `08-weftos-gui.md` lines 466-477), Stream 13
  (App Substrate Surface ADR set, see `13-app-substrate-surface.md`
  lines 479-490), and Stream 15 (IDE bridge protocol ADR-018, see
  `15-mcp-integration.md` line 309). Symposium output is wired.
- **ontology-navigator**: drove `adr-identity-iri.md` and
  `adr-treecalc-eml-architecture.md` plus 13 session findings + synthesis.
- **RLM - arxiv-2512.24601**: CLOSED (paper-specific session).
  Output is fully self-contained in
  `.planning/symposiums/RLM - arxiv-2512.24601/` (00-synthesis,
  01-paper-summary, 02-weftos-mapping, 03-adoption-candidates,
  04-gaps-and-risks). Adoption candidates listed in `03-` are deferred
  to 0.8.x+ research cycles; no follow-up workstream owns them yet.
  Revisit trigger: when KG / RoMem work in Stream 17 (WEFT-514/515)
  reaches a point where recursive-LM patterns become relevant.

### Quantum Integrations

- **Pasqal** (`pasqal-integration.md`): full hardware spec audit, cloud API +
  SDK details, blockade-radius math, neutral-atom qubit encoding, EMU-TN /
  EMU-MPS / EMU-SV emulator targets. No backend implementation yet.
- **NVIDIA Ising** (`nvidia-quantum-integration.md`): **DEFER**. Ising-
  Calibration and Ising-Decoding solve QPU calibration and surface-code
  decoding — both one abstraction layer below Pasqal Cloud, which we already
  consume. CUDA-Q + cuQuantum could provide a third `QuantumBackend` (analog
  Rydberg simulation locally), Apache-2.0 / BSD-3-Clause source; cuQuantum SDK
  binary is NVIDIA redistributable. **One conditional pilot**: cuDensityMat-
  backed `SimulatorBackend` behind `quantum-nvidia` feature flag,
  Python-sidecar first, FFI later, queued for v0.7.x post-GUI.

### ruv Ecosystem Analysis (April 2026 sweep)

- ruvector workspace at v2.1.0; our 4 dep crates (ruvector-cluster, -raft,
  -replication, -diskann) all stable; DiskANN matured (ADR-144) with Vamana
  + PQ + NAPI bindings, 14 tests, 1.0 recall, 90 µs search.
- 5 active ecosystem focus areas: mcp-brain SIMD (ADR-149), brain hypothesis
  engine (ADR-148), boundary-first detection (PR #347), KV-cache compression
  (ADR-147), Musica audio separation (PR #337).
- `ruvector-weftos-alignment.md` maps ruvector cognitive-container patterns
  onto WeftOS K0-K5 boot phases: `BootPhaseMask` adopted.
- `openfang-comparison.md`: OpenFang (RightNow AI, Feb 2026, 5,592 stars in
  4 days) ahead of clawft on channel breadth (40 vs 13), autonomous "Hands"
  agents, Tauri 2.0 desktop, 16-layer security stack, OpenAI-compatible API,
  P2P wire protocol (OFP), JS+Python SDKs, ratatui TUI, migration tools.
- `ruview-eml-contributions.md`: WiFi DensePose Rust port, 18-crate structure,
  CSI feature extraction. Conceptual fit but no integration target.

## What's Left — Total Depth

This section enumerates EVERY open item across the entire research pipeline,
including items already implied above. The audit prompt asked for total depth;
this list is therefore deliberately exhaustive. Items are tagged by status
(open / deferred / orphaned).

### TODOs / FIXMEs / HACKs in research-driven code paths

Direct grep of `crates/clawft-kernel/src/{eml_coherence,cognitive_tick,causal,
causal_predict,democritus}.rs` and `crates/eml-core/src/*` for `TODO|FIXME|XXX|
HACK` returned **zero** matches. The research code is unusually clean. The
remaining gaps are all in research / planning markdown, summarised below.

Ancillary code-adjacent TODO references found:

- `.planning/development_notes/sprint-16/security-audit.md:108` — "A TODO
  comment acknowledges the need for rate limiting but none is" (incomplete in
  source, references rate-limiting gap that touches `democritus` exposure).
- `.planning/development_notes/sprint-16/browser-wasm-features.md:46` —
  ComplexityAnalyzer flags files > 500 lines and TODO/FIXME/HACK markers; not
  a TODO itself, but a reminder that file-size budget enforcement is research-
  derived policy.
- `.planning/lewm-worldmodel-rs-page/PLAN.md:188` — "All 12 beats present OR
  MVP cut with clear TODOs" (acceptance criterion).

### Deferred items (research-driven, not yet implemented)

**ECC**:

- D1. `boot_ecc()` runtime function not yet wired into `Kernel<P>` boot
  sequence. (Covered in K3c notes §1.)
- D2. WASM browser-target verification of the `ecc` feature flag exclusion
  (blake3 + vector-memory have native deps).
- D3. Dedicated chain-event integration tests for ECC operations.
- D4. `clawft-weave` `weaver ecc` subcommands (CLI surface).
- D5. New RVF segment-type definitions for ECC structures (currently
  conceptual).
- D6. 5 pre-existing clippy warnings (`agent_loop.rs:30,138,219`,
  `chain.rs:1239`, `gate.rs:427`).

**EML**:

- D7. Truly O(1) feature extraction in `eml_coherence.rs` —
  `from_causal_graph` is O(n+m) due to `connected_components()`. Component
  count needs incremental maintenance.
- D8. EML-Transformer Iteration 1+ — Q/K/V models remain at default
  initialization in Iteration 0; full end-to-end coordinate-descent loop is
  open.
- D9. Per `eml-synergy-scan.md`, dozens of hardcoded heuristics (graphify
  analyze surprise scorer L209-269, cluster `MAX_COMMUNITY_FRACTION`,
  ForceAtlas2 6-parameter physics tuner, HNSW ef beam, peripheral-hub
  detection L254-255, bridge node `.take(3)` cut, 9 lines of HTML viz physics
  constants L246-251 + 253) are still hardcoded.
- D10. Coordinate-descent on multiple interdependent EML models is an open
  problem.
- D11. Numerical stability scaffolding for nested exp/ln at scale (the
  CIFAR-10 MLP run blew up; full LLM-scale would require heroic stability
  engineering).

**Democritus**:

- D12. **Live behavioural bug** — kernel.log shows continuous
  `Stuck { net_change: 0.0 }` since 2026-04-28 10:47 UTC. Investigate whether
  the running daemon's causal graph is empty/static, or if
  `detect_conversation_cycle` thresholds are mis-tuned, or if the EML model is
  predicting a constant. Suppression backoff is correct; the underlying
  signal is the issue. **Should be filed as a bug, not a feature gap.**
- D13. Rate limiting for the `democritus` exposure surface (referenced in
  sprint-16/security-audit.md TODO).

**Exochain governance**:

- D14. `auth_service:rotate_credential` — needs governance gate.
- D15. `auth_service:request_token` — needs governance gate.
- D16. `auth_service:revoke_token` — partial certification; needs gate.
- D17. `EffectVector` heuristics on auth/config/a2a/cron gates are
  context-only; explicit risk dimensions deferred.
- D18. 8 agents / 48-task fix plan in `exochain-fix-plan.md` is partially
  consumed; remaining medium-severity rows (cap matrix in critical/medium
  certifications) carry over.

**LeWM world model** (the largest open area):

- D19. `weftos-worldmodel-core` crate — not created.
- D20. `weftos-worldmodel-impls` crate — not created (candle-backed ViT-tiny
  encoder + AdaLN-modulated predictor).
- D21. `weftos-worldmodel` facade crate — not created.
- D22. `weftos-sensor-pipeline` crate — not created.
- D23. `weftos-sensor-pipeline-wire` crate — not created.
- D24. `clawft-worldmodel-service` binary — not created (3 deployment
  topologies: single, hot-standby, peer-to-peer).
- D25. `clawft-delegation` crate — not created.
- D26. `mesh.sensor.v1.{encoded,consensus,control}` topic definitions on the
  mesh wire (CBOR + Ed25519 + ExoChain index).
- D27. `LatticeApi` (`ServiceApi` registered): observe, observe_node, predict,
  plan, recall, subscribe_surprise, subscribe_drift.
- D28. SIGReg `sigreg_health` Welford monitor in production paths.
- D29. SIGReg auto-rollback at `sigreg_health < 0.85` for 30 s.
- D30. `pred_φ` predictor (z_t, a_t → ẑ_{t+1}).
- D31. `LatentPlanner` (CEM default / MPPI-warm / gradient shooting), 10 Hz
  background thread.
- D32. Four-condition AND rollback gate (cluster SIGReg health, held-out
  probing, VoE surprise differentiation, temporal-straightening).
- D33. Two training surfaces (offline edge intelligence per sensor class via
  RVF segment hot-swap; online streaming-merge with per-class importance-
  weighted replay).
- D34. ExoChain attestation of (a_t, z_t, z_{t+1}, surprise) tuples.
- D35. ADR-058 decoupling-invariant 5 formal rules — invariant codified in
  text but no compile-time / runtime check.
- D36. Marketing page (`docs/src/app/lewm-worldmodel-rs/`) — draft PR posture
  is "DO NOT MERGE" pending visual confirmation; not landed.
- D37. Per-sensor-class trainable RVF-hosted small models (transmit-gate /
  aggregate / encode), hot-swap at tick alignment, auto-rollback.

**KG paper synthesis (carry-over to Stream 12)**:

- D38. GraphRAG community summaries in pipeline analyze + aggregation in
  `run_query()` (P0).
- D39. CausalRAG `causal_trace()` BFS/DFS along Causes/Enables/EvidenceFor
  edges (P0).
- D40. SASE k-order graph convolution + Random Fourier Features clustering
  (P0).
- D41. RoMem phase-rotation temporal KG (P1).
- D42. LightRAG dual-level keyword retrieval (P2).
- D43. ~5 other 2604.* papers from Phase 2 (P1/P2/P3).

**Sonobuoy gaps (carry-over)**:

- D44. G2 — Helmholtz-PINN 3D collapse (open).
- D45. G3 — FNO thermocline collapse (open).
- D46. G4 — SAS unknown velocity (open; partially covered by G1's Doppler).
- D47. G5 — sub-kbps FL (open).
- D48. `clawft-sonobuoy-ranging` crate scaffolding (G1 follow-up, scheduled
  for v2).

**Quantum / exploratory**:

- D49. Pasqal backend skeleton — designed, not implemented.
- D50. cuDensityMat-backed `SimulatorBackend` behind `quantum-nvidia` feature
  flag, queued for v0.7.x post-GUI.

**Gaming-robotics symposium**:

- D51. 8 experiments planned, none executed. $1,102 hardware budget unspent.

**Coherence-lattice-alpha**:

- D52. Two-channel Shannon + Fiedler pattern logged as "prior art for
  `eml_coherence` future refinement". No code action.

### Open questions

- Q1. ECC: Should `boot_ecc()` be folded into the standard `boot()` flow or
  remain a feature-flag-gated fork? (Affects `Kernel<P>` struct shape.)
- Q2. EML: Is the 192-dim SIGReg manifold the right choice for the latent
  contract, given current encoder models? The diagram fixes 192 dims; this
  binds the wire format.
- Q3. Democritus stuck-state diagnosis (see D12). Is `Stuck` the right
  classification when `net_change == 0.0`, or should the empty/static case be
  a distinct state?
- Q4. LeWM ADR-058 decoupling invariant — what are the runtime checks that
  would catch a violation (e.g., world model short-circuits a causal edge)?
- Q5. ExoChain governance gates on auth_service rotate/request/revoke — what
  policy expresses "agent X may rotate but not revoke"?
- Q6. Sonobuoy 5th branch (active-imaging / SAS) — does it land as a feature,
  or stay as a doc-only architecture for now?
- Q7. Is there a single "research → feature" pipeline doc, or does each ADR
  serve as its own pipeline marker? (The audit found no single index.)

### Orphaned research (tracked, no current consumer)

- **Closure-SDK** — quaternion cognitive architecture, AGPL blocker; declined.
- **Coherence-lattice-alpha** — physics preprint, 5 commits, broken internal
  deps, AGPL + CC BY-NC; declined except for conceptual takeaway.
- **NVIDIA Ising** — Calibration and Decoding both at the wrong abstraction
  layer; declined.
- **OpenFang feature gaps** — channel breadth, autonomous Hands, desktop
  Tauri, 16-layer security, OpenAI-compatible API, P2P OFP wire protocol,
  agent marketplace, JS/Python SDKs, ratatui TUI, migration tools — none of
  these are research, but the comparison surfaces no fewer than 10
  distinct gap targets that no workstream owns.
- **WiFi DensePose / RuView** — interesting but no integration target.
- ~~**`compositional-ui` symposium** outputs not visibly cross-referenced into
  Stream 9/10 audits.~~ — RESOLVED (WEFT-540): compositional-ui is
  cross-referenced from streams 8, 13, and 15. See "Other Symposiums"
  section above for the explicit citation map.
- ~~**`RLM - arxiv-2512.24601` symposium** — paper-specific session, no
  follow-up tracked.~~ — CLOSED (WEFT-540): rationale and revisit
  trigger recorded in "Other Symposiums" section above.
- The Closure-SDK conceptual takeaway is logged but not assigned a future
  revisit trigger.

## Task List

(Carry-over items only — these are the action items the audit produces. Most
are out-of-scope for 0.7.0 and should be filed in tracker / ADRs in follow-up
sprints.)

| # | Task | Source | Stream(s) | Priority | Owner |
|---|------|--------|-----------|----------|-------|
| T1 | File bug: `DEMOCRITUS: still stuck after N checks` continuous since 10:47 UTC; investigate empty-graph vs mis-tuned `detect_conversation_cycle` vs constant-EML-prediction. | D12, kernel.log | 17 | **P0** | kernel |
| T2 | Wire `boot_ecc()` runtime function into `Kernel<P>` boot sequence. | D1 | 17 | P1 | kernel |
| T3 | Verify `ecc` feature exclusion on `wasm32-unknown-unknown`. | D2 | 13 (wasm) + 17 | P2 | wasm |
| T4 | Add governance gates to `auth_service:rotate_credential`, `request_token`, `revoke_token`. | D14, D15, D16 | 17 + 8 (security) | P1 | security |
| T5 | Make `auth/config/a2a/cron` `EffectVector` explicit (not context-only). | D17 | 17 + 8 | P2 | security |
| T6 | Implement `weaver ecc` CLI subcommands. | D4 | 17 + 4 (weave) | P2 | weave |
| T7 | Define new RVF segment types for ECC structures and persistence. | D5 | 17 + persistence | P2 | core |
| T8 | Resolve 5 pre-existing clippy warnings in `agent_loop.rs`, `chain.rs`, `gate.rs`. | D6 | 17 + lint | P3 | core |
| T9 | Add incremental component-count maintenance for O(1) `eml_coherence` feature extraction. | D7 | 17 | P2 | kernel |
| T10 | EML-Transformer Iteration 1: end-to-end coordinate-descent loop for Q/K/V. | D8 | 17 | P3 | research |
| T11 | Drive `eml-synergy-scan.md` rows from "scan" to "implementation"; pick top 5 by impact. | D9 | 17 + 12 | P2 | research |
| T12 | RoMem phase-rotation temporal KG on CausalGraph. | D41 | 17 + 12 | P3 | research |
| T13 | GraphRAG community summaries in pipeline analyze. | D38 | 17 + 12 | P1 | research+graphify |
| T14 | CausalRAG `causal_trace()`. | D39 | 17 + 12 | P1 | research+graphify |
| T15 | SASE clustering replacing label-propagation. | D40 | 17 + 12 | P2 | graphify |
| T16 | LightRAG dual-level keyword retrieval. | D42 | 17 + 12 | P3 | graphify |
| T17 | Process remaining Phase 2 papers (~5) into priority list. | D43 | 17 | P3 | research |
| T18 | Land ADR-058 decoupling invariant runtime checks. | D35, Q4 | 17 + 11 (ADRs) | P1 | research |
| T19 | Create `weftos-worldmodel-core` crate (`no_std`, traits). | D19 | 17 + workspace | P0 (next sprint) | research |
| T20 | Create `weftos-worldmodel-impls` crate (candle-backed ViT-tiny + AdaLN). | D20 | 17 | P0 | research |
| T21 | Create `weftos-worldmodel` facade crate. | D21 | 17 | P0 | research |
| T22 | Create `weftos-sensor-pipeline` + `-wire` crates. | D22, D23 | 17 | P0 | research |
| T23 | Create `clawft-worldmodel-service` binary (3 deployment topologies). | D24 | 17 | P1 | research |
| T24 | Create `clawft-delegation` crate. | D25 | 17 | P1 | research |
| T25 | Add `mesh.sensor.v1.{encoded,consensus,control}` topics on mesh wire (CBOR + Ed25519, ExoChain-indexed). | D26 | 17 + mesh | P0 (after crates) | mesh |
| T26 | Implement `LatticeApi` with 7 methods. | D27 | 17 | P1 | research |
| T27 | Wire SIGReg `sigreg_health` Welford monitor + auto-rollback at 0.85 / 30 s. | D28, D29 | 17 | P1 | research |
| T28 | Implement `pred_φ` predictor + `LatentPlanner` (CEM default). | D30, D31 | 17 | P1 | research |
| T29 | Implement four-condition AND rollback gate. | D32 | 17 | P1 | research |
| T30 | Two training surfaces (offline edge intelligence + online streaming merge). | D33 | 17 | P2 | research |
| T31 | Per-sensor-class trainable RVF-hosted small models (transmit-gate / aggregate / encode), hot-swap at tick alignment. | D37 | 17 | P2 | research |
| T32 | ExoChain attestation of `(a_t, z_t, z_{t+1}, surprise)` tuples. | D34 | 17 + 17 governance | P1 | research |
| T33 | Land marketing page `/lewm-worldmodel-rs` after visual confirmation. | D36 | 17 + docs | P2 | docs |
| T34 | Sonobuoy: scaffold `clawft-sonobuoy-ranging` crate (G1 follow-up). | D48 | 17 + future | P3 | research |
| T35 | Sonobuoy: drive G2-G5 to closure or accept as deferred. | D44, D45, D46, D47 | 17 | P3 | research |
| T36 | Pasqal: implement backend skeleton. | D49 | 17 + future | P3 | research |
| T37 | NVIDIA: scaffold cuDensityMat `SimulatorBackend` behind `quantum-nvidia` feature flag (post-GUI v0.7.x). | D50 | 17 | P3 | research |
| T38 | Gaming-robotics: kick off first experiment (any of the 8). | D51 | 17 + future | P3 | research |
| T39 | Cross-link symposium output (`compositional-ui`, `RLM - arxiv-2512.24601`) into responsible streams or mark closed. | "Orphaned" | 17 + various | P3 | docs |
| T40 | ~~Decide whether to add a single "research → feature" pipeline index (vs ADR-only).~~ DECIDED 2026-04-30 (WEFT-541): **ADR-only**. The "Released Features" section of this audit doc + each ADR's "Status: Accepted" + the "Status & Timeline" table at top serve as the de-facto landed-as-feature index. Adding a parallel index file duplicates source-of-truth and drifts. If audit-time discovery becomes painful again, revisit at the next release-gate review. | Q7 | 17 + ADRs | P3 | docs |
| T41 | Decide ECC `boot_ecc()` fold-vs-fork. | Q1 | 17 | P2 | kernel |
| T42 | Decide on 192-dim SIGReg latent dimensionality. | Q2, ADR-050 | 17 | P1 | research |
| T43 | Decide ECC governance "rotate but not revoke" policy expression. | Q5 | 17 + 8 | P2 | security |
| T44 | Decide whether sonobuoy 5th (active-imaging) branch lands as a feature or stays doc-only. | Q6 | 17 | P3 | research |

## Sources

- `.planning/lewm-worldmodel-rs-page/{DESIGN-A.md, DESIGN-B.md, PLAN.md}`
- `.planning/symposiums/lewm-worldmodel/diagram.md`
- `.planning/symposiums/gaming-robotics/{FINAL_REPORT.md, robotics/research-report.md}` plus `cognitum-seed-gaps`, `cold-case-ecc`, `ontology-navigator`, `compositional-ui`, `RLM - arxiv-2512.24601`
- `.planning/development_notes/k3c-ecc-integration.md`
- `.planning/development_notes/sprint-16/{democritus-loop, eml-coherence, security-audit, browser-wasm-features}.md`
- `.planning/development_notes/{eml_model_development, eml_model_development_assessment, eml-causal-collapse-research, eml-synergy-scan, hnsw-eml-analysis, hnsw-eml-deep-analysis, ruview-eml-contributions, coherence-lattice-alpha-integration}.md`
- `.planning/development_notes/{exochain-certification-critical, exochain-certification-medium, exochain-certification-nonkernel, exochain-fix-plan, exochain-governance-audit, governance-certification}.md`
- `.planning/development_notes/{knowledge-graph-paper-survey, knowledge-graph-paper-survey-phase2, arxiv-2410-05779-analysis, arxiv-2603-21852-analysis}.md`
- `.planning/development_notes/{nvidia-quantum-integration, pasqal-integration, ruv-ecosystem-analysis-20260414, ruvector-weftos-alignment, openfang-comparison}.md`
- `.planning/sonobuoy/{SYNTHESIS, GAPS, RANGING, README, k-stemit-sonobuoy-mapping}.md`, `.planning/sonobuoy/{papers/analysis/, gaps/}`
- `crates/eml-core/` (Cargo.toml + src/{lib,model,operator,tree,features,events,attention,baseline_attention}.rs + examples/)
- `crates/clawft-kernel/src/{causal, crossref, impulse, hnsw_service, calibration, cognitive_tick, eml_coherence, democritus, governance, chain}.rs`
- `docs/src/app/lewm-worldmodel-rs/{page.tsx, copy.ts, lewm.css, layout.tsx, components/}`
- `docs/src/content/docs/weftos/{ecc, eml, eml-attention}.mdx`
- `.weftos/runtime/kernel.log` (live evidence of Democritus stuck-state, lines 10538-11220, sampling 2026-04-28 10:47 → 15:36 UTC)

<!-- TRIAGED-STAMP:BEGIN -->
## Triaged into Plane — 2026-04-28

All open items in this audit have been filed as Plane work items in the WeftOS workspace under the `ws17-research` label.

- **Range**: WEFT-502 … WEFT-549 (48 items)
- **Per cycle**: 0.7.x: 1, 0.8.x: 21, 0.9.x: 23, 1.0.x: 3
- **Triage spec**: `.planning/reviews/0.7.0-release-gate/triage/`
- **WEFT-N → name map**: `.planning/reviews/0.7.0-release-gate/triage/weft-mapping.json`

Per the project rule (CLAUDE.md → "Plane is the authoritative work tracker"): future updates to these items happen in Plane, not in this audit doc. This doc remains the source-of-truth for the original survey.
<!-- TRIAGED-STAMP:END -->
