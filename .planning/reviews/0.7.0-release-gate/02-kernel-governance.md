---
title: "Kernel & Governance"
slug: kernel-governance
workstream_id: "02"
status: in-progress
period_start: 2026-03-25
period_end: 2026-04-28
last_updated: 2026-04-28
versions_landed:
  - 0.6.13   # mesh boot integration (TCP transport default, [kernel.mesh] config)
  - 0.6.17   # leaf push protocol (out of scope for kernel core but kernel-adjacent)
  - 0.6.18   # graphify ingest fix (non-kernel)
  - 0.6.19   # cluster peer persistence + optional TCP IPC relay + StreamWindowAnchor
related_plans:
  - .planning/development_notes/sprint-16/democritus-loop.md
  - .planning/development_notes/sprint-16/chain-attestation.md
  - .planning/development_notes/sprint-16/eml-coherence.md
  - .planning/development_notes/sprint-16/security-audit.md
  - .planning/development_notes/sprint-16/mesh-k6-transport.md
  - .planning/development_notes/sprint-16/vector-hardening.md
  - .planning/development_notes/sprint-16/vector-hybrid.md
  - .planning/development_notes/sprint-16/wasmtime-upgrade.md
  - .planning/development_notes/sprint-16/http-facade.md
  - .planning/development_notes/exochain-certification-critical.md
  - .planning/development_notes/exochain-certification-medium.md
  - .planning/development_notes/exochain-certification-nonkernel.md
  - .planning/development_notes/exochain-fix-plan.md
  - .planning/development_notes/exochain-governance-audit.md
  - .planning/development_notes/governance-certification.md
  - .planning/development_notes/k0-k3-gap-analysis.md
  - .planning/development_notes/k0-k5-final-gap-analysis.md
  - .planning/development_notes/k3c-ecc-integration.md
  - .planning/development_notes/k6-decision-coverage.md
  - .planning/development_notes/k6-developer-readiness.md
  - .planning/development_notes/k6-readiness-audit.md
  - .planning/development_notes/k6-test-strategy-review.md
  - .planning/development_notes/mesh-boot-integration.md
  - .planning/development_notes/mesh-time-sync.md
related_adrs:
  - adr-020-kernel-phase-responsibilities
  - adr-020-chainloggable             # duplicate ADR-020 number — see Open questions
  - adr-021-cli-kernel-compliance
  - adr-022-exochain-mandatory-audit
  - adr-023-assessment-as-kernel-service   # status "Proposed", not yet Accepted
  - adr-024-noise-protocol-encryption
  - adr-025-ed25519-node-identity
  - adr-026-quic-primary-transport
  - adr-027-selective-libp2p
  - adr-028-post-quantum-dual-signing       # duplicate ADR-028 number — collides with adr-028-weftos-kernel
  - adr-028-weftos-kernel                   # duplicate ADR-028 number
  - adr-030-cbor-exochain-codec
  - adr-031-rvf-wire-mesh-format
  - adr-032-dashmap-concurrency
  - adr-033-three-branch-governance
  - adr-034-effect-algebra-scoring
  - adr-039-swim-failure-detection
  - adr-040-lww-crdt-process-table
  - adr-041-chainanchor-trait
  - adr-042-three-operating-modes
  - adr-043-blake3-shake256-migration
  - adr-047-self-calibrating-tick
sprint_refs:
  - sprint-11
  - sprint-12
  - sprint-13
  - sprint-14
  - sprint-16
completion_pct: 78
open_task_count: 64
risk: high
---

# Kernel & Governance

## General Description

`clawft-kernel` (~82 KLOC, 75 files, ~3,500 tests with
`exochain,ecc,mesh`) is the WeftOS kernel: boot, process/service/health,
ExoChain audit log, three-branch governance, the DEMOCRITUS cognitive
tick, the ECC substrate (causal/HNSW/EML/impulse/crossref), the K6 mesh
framework (transport/framing/Noise/heartbeat/Kad/mDNS/IPC/chain sync/
tree sync/assess sync), the WASM sandbox, the resource tree manager,
and cluster membership. `exo-resource-tree` (~1.8 KLOC) holds the
Merkle-backed tree + mutation log; permission and delegation are
deliberate K0 stubs.

K0–K5 are functionally complete (`k0-k5-final-gap-analysis.md`,
2026-03-25). K6 mesh transport is largely implemented (~3,500 lines,
133 tests) and wired into boot in v0.6.13. K6 design coverage is
strong (D1–D15, M1–M11, C1–C5 all in plan); security has one missing
item (S10 key rotation) and one partial (S7 browser restrictions).
ExoChain certification is near-complete but not clean: **2 CRITICAL
FAIL** (`auth_service.rotate_credential`, `auth_service.request_token`
both missing governance gates), 3 PARTIAL, 1 MEDIUM FAIL
(`wasm_runner/tools_fs.rs` is unaudited), and the **tracing →
ChainManager bridge in the daemon is missing**, so 12 chain events
emitted by non-kernel crates land in stdout but never reach ExoChain.

v0.6.19 ships cluster peer persistence and the optional TCP IPC relay
(`[kernel.ipc_tcp]`), plus StreamWindowAnchor and signed IPC envelopes.
Most K6 mesh code is single-process / unit-test-only; no two-node
integration harness exists, and K6.4/K6.5 sub-phases (chain replay,
tree Merkle diff, SWIM semantics, CRDT gossip) are designed but not
exercised end-to-end. DEMOCRITUS "still stuck after N checks" was
hardened in 0.6.19 (edge-triggered + exp-backoff + RFF exact path);
the warning is still expected on empty causal graphs and idle
conversations and should be documented as such.

Out of scope: agent loop / chat path / LLM transport / router tier.

## Status & Timeline

| Date       | Event                                                                 |
|------------|-----------------------------------------------------------------------|
| 2026-03-25 | K0–K5 final gap analysis: 263/293 items checked; K3c ECC lands       |
| 2026-03-26 | K6 readiness / decision-coverage / developer-readiness / test reviews |
| 2026-04-03 | ADR-020/021/022 (kernel/CLI/chain triad) + 024–028 (K6 security) accepted |
| 2026-04-03 | ADR-030/031/032/033/034/039/040/041/042/043/047 batch-accepted        |
| 2026-04-04 | Sprint-16 sweep (DEMOCRITUS, EML, vector hardening+hybrid, mesh-k6, wasmtime v33, http-facade, security-audit) |
| 2026-04-04 | ExoChain audit: 5 CRITICAL / 16 HIGH / 30+ MEDIUM gaps cataloged      |
| 2026-04-04 | Critical-High cert: 15 PASS, 3 PARTIAL, 2 FAIL of 21                  |
| 2026-04-04 | MEDIUM cert: 30/32 PASS, 1 conditional, 1 FAIL                        |
| 2026-04-04 | Non-kernel cert: 12/12 emit chain events; daemon bridge MISSING       |
| 2026-04-04 | Governance gate cert: 19 sites, 14/14 high-priority, 3 minor gaps     |
| 2026-04-11 | Custody attestation + host revocation (Cognitum Seed #5, #3)          |
| 2026-04-17 | v0.6.13 mesh boot integration (MeshRuntime at boot phase 5d); mesh-time-sync.md drafted |
| 2026-04-22 | v0.6.19: cluster peer persistence + TCP IPC relay + StreamWindowAnchor + DEMOCRITUS hardening |
| 2026-04-28 | This audit                                                            |

## Released Features

Shipped in the `development-0.7.0` line through v0.6.19.

- **Boot (K0–K5)**: `Kernel<P>` state machine, 11 BootPhase variants,
  BootLog, `weaver kernel status|services|ps`, `weaver console`,
  `weaver boot-log`. `crates/clawft-kernel/src/boot.rs:160-1613`.
- **Process/service/health (K0)**: DashMap-backed ProcessTable,
  ServiceRegistry, aggregated HealthSystem, KernelConfig.
- **Supervisor + RBAC + ExoChain (K1)**: `spawn_and_run`, GateBackend,
  CapabilityGate, GovernanceGate, TreeManager+ChainManager wiring,
  agent.spawn/exit/restart chain events, JSON+RVF chain persistence
  with checkpoint restore.
- **A2A IPC (K2 + K2b)**: direct PID-to-PID, request-response with
  timeout, TopicRouter pub/sub, IpcScope, dead-subscriber GC,
  watchdog sweep, `shutdown_all(timeout)`, suspend/resume, MCP
  `ipc_send` / `ipc_subscribe`.
- **K2.1 Symposium**: SpawnBackend enum, dual-signing scaffolding,
  MessageTarget::Service/ServiceMethod, ServiceEntry, AuditLevel.
- **WASM sandbox (K3)**: Wasmtime-backed `WasmToolRunner` (wasmtime
  29 → 33), fuel/memory/timeout, ToolRegistry integration, WASI
  no-preopens, ServiceApi (C2), dual-layer A2ARouter gate (C4),
  chain-anchored contracts (C3), shell→WASM pipeline (C5).
- **ECC cognitive substrate (K3c)**: CausalGraph DAG, HnswService,
  CognitiveTick (ADR-047 self-calibrating), CrossRef
  (BLAKE3 UniversalNodeId), ImpulseQueue (HLC), Calibration,
  NodeEccCapability. Behind `ecc`.
- **Container manager (K4)**: ContainerManager + lifecycle types,
  ChainAnchor trait + MockAnchor (`chain.rs:2180-2212`).
- **App framework (K5)**: AppManifest, AppManager
  install/start/stop/remove/list/inspect, namespaced agents/tools,
  partial-start rollback, lifecycle hooks.
- **DEMOCRITUS two-tier loop (sprint 16)**: SENSE → THINK(fast EML) →
  DETECT DRIFT → THINK(exact RFF; Lanczos still available) → LOG →
  COMMIT. Spawned at `boot.rs:1497-1521`. v0.6.19 hardening:
  Option-sentinel for `last_exact_coherence` (Finding #1),
  edge-triggered + exp-backoff stuck warning (#2), bounded
  `coherence_history` VecDeque (#3), RFF Laplacian as steady-state
  exact path (#4). `cognitive_tick.rs:295-542`.
- **EML coherence model**: depth-3, 34-param master formula,
  coordinate-descent training, 7 graph features. Behind `ecc`.
  Note: `from_causal_graph` is O(n+m), not the advertised O(1)
  (`connected_components`).
- **K6 mesh scaffolding**: 14 `mesh_*.rs` modules, ~3,500 lines, 133
  tests — mesh_tcp/ws (listeners), mesh_noise (snow XX), mesh_kad/mdns
  (libp2p selective), mesh_framing (16 MiB cap; FrameType 0x00–0x0E,
  AssessmentSync 0x0E), mesh_heartbeat (SWIM ADR-039), mesh_process
  (LWW CRDT ADR-040), mesh_chain (stubbed replay), mesh_tree, mesh_ipc,
  mesh_listener, mesh_bootstrap, mesh_runtime, mesh_assess (sprint 16),
  mesh_dedup, mesh_log, mesh_service / mesh_service_adv. Wired into
  boot at phase 5d in v0.6.13.
- **Cluster peer persistence (v0.6.19)**: atomic tmp+rename to
  `.weftos/runtime/cluster_peers.json` on every membership change;
  rehydrates on boot. Fixes the "cluster: degraded" regression.
- **TCP IPC relay (v0.6.19)**: `[kernel.ipc_tcp]` TCP→Unix-socket relay
  for Windows/WSL/remote bridges (auth stays on the Unix path).
- **StreamWindowAnchor + signed IPC envelopes (v0.6.19)**: BLAKE3
  rolling window commits, `agent.register` signed envelopes,
  `ipc.subscribe_stream`.
- **Custody attestation + host revocation**: `CustodyAttestation`,
  `ChainManager::generate_attestation()`, `custody.attest` RPC,
  `weaver custody attest`; persistent `RevocationList` at
  `.weftos/runtime/revoked_hosts.json`, `cluster.add_peer_checked()`,
  `weaver cluster revoke|unrevoke|revoked`.
- **Vector hardening (sprint 16)**: `epoch: AtomicU64` +
  `current_epoch()`, `insert_with_epoch()` optimistic concurrency,
  soft-delete tombstones + compaction, `max_vectors`. 70 tests.
- **Vector hybrid backend (sprint 16)**: `VectorBackend` trait, HNSW /
  DiskANN-stub / Hybrid (hot+cold, LRU). 35 tests.
- **HTTP REST facade (sprint 16, gaps #6/#7/#8)**: `http_facade.rs`,
  13 routes, SSE `/events`, `POST /custody/witness`, behind
  `http-api`. 42 tests.
- **Wasmtime v29 → v33 upgrade**: closes 10 Dependabot alerts on
  cranelift/wasmparser/wasm-encoder transitive vulns.
- **Three-branch governance (ADR-033) + 5-D EffectVector (ADR-034) +
  TileZeroGate** (`cognitum-gate-tilezero` feature),
  `EVENT_KIND_CAPABILITY_REVOKED`.
- **MEDIUM-tier ExoChain coverage**: 30 of 32 MEDIUM call sites
  certified (causal / artifacts / cron / environment / container /
  process / agency / cluster.update_state / mesh_service /
  mesh_artifact / mesh_ipc / persistence / reconciler).
- **Non-kernel chain-event scaffolding**: `chain_event!` macro and
  `tracing::info!(target: "chain_event", ...)` emit points in core
  / graphify / weave (sandbox, session, workspace, tools, graphify
  build/ingest/pipeline/hook, project init). Bridge to ChainManager
  is **missing** — see What's Left.

## What's Left — Total Depth

### TODOs / FIXMEs in code

From `grep -rn "TODO\|FIXME\|XXX\|HACK\|unimplemented\|todo!()"
crates/clawft-kernel/src crates/exo-resource-tree/src`:

- `vector_quantization.rs:83` — `TODO(KG-011)`: ruvector-core PR #352
  not merged; `LogQuantizedConfig::is_available()` hardwired `false`.
- `vector_quantization.rs:150` — `TODO(KG-012)`: same; SIMD branch-free
  distance (+14% QPS) is config-only.
- `quantum_register.rs:9` — spectral embedding is a TODO.
- `quantum_braket.rs:74-160`, `quantum_pasqal.rs` — entire backends
  return `Err(QuantumError::NotImplemented)` for submit/cancel/
  status/metadata. Behind `quantum-braket`/`quantum-pasqal` features.
- `mesh_runtime.rs:509-535` — explicit `// Chain sync stubs` block.
  `build_chain_sync_request` produces requests; `handle_chain_sync_response`
  parses but **never replays** events into `ChainManager`. K6.4
  chain replay is not done.
- `wasm_runner/registry.rs:318-326` — `unsafe impl Send/Sync` on
  `WasmToolAdapter`/`ToolRegistry` (security-audit L-1; acceptable
  for 1.0).
- `tools_extended.rs:616-629` — `unsafe` raw pointer walk
  (`*mut serde_json::Map`) in `ensure_section` (L-2; sound but
  fragile).
- `chain.rs:1617-1624` — `std::mem::transmute_copy` for ML-DSA-65
  key extraction with `size_of` assertion (L-3; upstream-fragile).
- `clawft-services/src/api/handlers.rs:130,133` — TODO CSP middleware
  and TODO `tower::limit::RateLimitLayer`.
- `clawft-services/src/api/bridge.rs:282, 287, 395, 467` — unimplemented
  bridge endpoints (skill install/uninstall, memory delete, config
  persistence).
- `clawft-services/src/api/mod.rs:309-315` — auth middleware commented
  out (`// Enable once the UI has a login flow`); `mod.rs:295` —
  `CorsLayer::permissive()` is the default when `cors_origins` is
  empty (security-audit M-1, M-2).
- `exo-resource-tree/src/permission.rs:23` — explicit K0 stub: always
  returns `Allow`.
- `exo-resource-tree/src/delegation.rs:14` — explicit K0 stub: type
  only, no grant/revoke/sig/expiry/chain-validate.
- `boot.rs:1415` — vector backend log says `"Vector backend: DiskANN
  (stub)"`; `vector_diskann.rs` is a brute-force `HashMap` with
  linear-scan cosine, awaiting `ruvector-diskann` publication.
- `assessment/mod.rs:4` and `assessment/analyzers/complexity.rs:1,14,
  109,111` — mentions of TODO/FIXME/HACK only as scanner targets,
  not unfinished work.

Kernel + exo-resource-tree are otherwise free of `todo!()` /
`unimplemented!()` in production paths; remaining `panic!()` / `unwrap()`
are in `#[cfg(test)]` modules.

### Deferred items (from plans / handoff / ADRs)

#### CRITICAL / HIGH ExoChain & Governance gaps still open

From `exochain-certification-critical.md`, where 21 items were checked:

- **FAIL** — `auth_service.rs:rotate_credential` (line 325) is chain-logged
  but **not** governance-gated. Must add `gate.check("auth.credential.rotate")`.
- **FAIL** — `auth_service.rs:request_token` (line 354) is chain-logged but
  **not** governance-gated. Token-issuance frequency / scope should pass
  `gate.check("auth.token.issue")`.
- **PARTIAL** — `auth_service.rs:revoke_token` (line 441) — defense-in-depth
  governance gate not added (lower priority than the two FAILs).
- **PARTIAL** — `hnsw_service.rs:clear` (line 230) — chain payload is
  `serde_json::json!({})` with no `entries_destroyed` or `epoch`.
- **PARTIAL** — `environment.rs:set_active` (line 326) reuses
  `EnvironmentError::NotFound` for governance denial; needs a dedicated
  `GovernanceDenied` variant.

#### MEDIUM ExoChain coverage gap

- `wasm_runner/tools_fs.rs` — every filesystem mutation tool
  (`fs.write_file`, `fs.create_dir`, `fs.remove`, `fs.copy`, `fs.move`,
  `fs.glob`) is **unaudited**. No `chain_manager` field, no `cfg(feature
  = "exochain")` block, no `cm.append()` call, no event constants. Audit
  prescribes adding `EVENT_KIND_WASM_FS_WRITE` /
  `EVENT_KIND_WASM_FS_REMOVE` / `..._CREATE_DIR` / `..._COPY` /
  `..._MOVE` constants and threading a chain manager into each tool.

#### Non-kernel chain-event bridge MISSING

- **`exochain-certification-nonkernel.md` blocking gap** — all 12 chain
  events emitted by `clawft-core`, `clawft-graphify`, and `clawft-weave`
  fire correctly via `tracing::info!(target: "chain_event", ...)`, but
  the `clawft-weave/src/main.rs` daemon uses a vanilla
  `tracing_subscriber::fmt()` with **no** custom layer that filters on
  `target == "chain_event"` and forwards to `ChainManager::append()`.
  Net effect: those 12 events live only in stdout. Required: a
  `ChainEventLayer` `tracing::Layer` impl composed alongside `fmt`.
- Minor non-kernel gaps: `ToolRegistry::register_with_metadata` skips
  the chain (only `register` emits); `sandbox.rs::check_tool` /
  `check_network` / `check_file_read` / `check_file_write` log only to
  the in-memory audit log, not the chain; `save_query_result` in
  `clawft-graphify/src/ingest.rs` does not emit a chain event.

#### Governance gate certification gaps (small but real)

From `governance-certification.md`:

- **GAP-1 MEDIUM** — `config_service.rs:delete_typed` (line 380) has no
  governance gate; sibling `delete` (line 259) does. Bypass risk.
- **GAP-3 LOW** — `cron.rs:remove_job` (line 177) is not gated; `add_job`
  is. Removing a governance-mandated audit-rotation cron is uncovered.
- **GAP-2 LOW** — `clawft-core/src/agent/sandbox.rs::check_command` has
  no `GovernanceGate` consultation (intentional split, but DiD candidate).

#### Sprint-16 security-audit MEDIUM follow-ups

- **M-1** — API auth middleware disabled in `clawft-services/src/api/mod.rs:309`.
- **M-2** — CORS `permissive()` default.
- **M-3** — No rate-limiting on `/api/*`.
- **M-4** — No token revocation in `TokenStore` (`crates/clawft-services/src/api/auth.rs`).
- **M-5** — ExoChain has no replay protection; recommend optional
  `idempotency_key` field on `ChainEvent`.
- `cargo audit` is not installed in CI; security-audit recommends adding
  it to `scripts/build.sh gate`.

#### K6 mesh-framework deferrals

From `k0-k5-final-gap-analysis.md` and `k6-readiness-audit.md`:

- **CMVG delta sync (RVF exchange)** — deferred from K3c to K5/K6.5
  (cross-node transport prerequisite).
- **CRDT merge for CausalGraph** — deferred to K5 (single-node first).
- **Spectral analysis offloading** to peers — deferred (requires
  cluster membership + mesh).
- **WASM-compiled cognitive modules** — deferred to K4+.
- **Platform traits (Android, ESP32)** — deferred to K4+ (hardware-specific).
- **Full DEMOCRITUS bidirectional flow** — deferred to K5 (multi-node
  topology required).
- **K6: Network transport, raft, cross-node replication** — formally
  scoped to K6 but only partially implemented; chain replay (K6.4),
  tree Merkle diff (K6.4), SWIM heartbeat semantics (K6.5), CRDT
  process-table gossip (K6.5) all need code beyond the current stubs
  (`mesh_runtime.rs:509-535`).

From `k6-decision-coverage.md` (security side):

- **S10 — Key rotation** is **NOT IN SPARC PLAN**. The security panel
  defines a 5-step dual-signed-chain-event protocol but nothing in the
  K6 plan owns it. Without key rotation, a compromised node key forces
  cluster-wide manual intervention.
- **S7 — Browser node restrictions** has only partial coverage.
  No exit criterion tests `IpcScope::Restricted` as the browser default
  and no governance `browser_policy` rules exist.
- **D7 — Ruvector reuse exit criteria** are partial: only CRDT gossip
  is covered. ruvector-cluster (SWIM membership), ruvector-raft
  (consensus), ruvector-replication (log replication) integration is
  unscoped for K6.
- **S4** — no explicit assertion that the Noise parameter string is
  `"Noise_XX_25519_ChaChaPoly_BLAKE2b"`.

From `k6-developer-readiness.md` (open design questions):

- **Q1** — chain merge strategy: leader-based or DAG? Blocks K6.4
  implementation.
- **Q2** — wire format for `KernelMessage`: JSON or RVF? Pseudocode
  uses `serialize_rvf()` but the formal decision is open.
- **Q4** — full libp2p-kad or lighter DHT?
- **Q5** — split-brain handling. Blocks K6.4.
- Several missing protocol-message struct definitions: `MeshStream`,
  `TransportListener`, `EncryptedPeer`, `WeftHandshake`,
  `JoinRequest` / `JoinResponse`, `ChainSyncRequest` /
  `ChainSyncResponse` (defined but not wired), `TreeSyncRequest` /
  `TreeSyncResponse`, `ServiceEndpoint`, `ProcessAdvertisement` /
  `ServiceAdvertisement`, `Frame`.
- File-layout discrepancy: `mesh_adapter.rs` appears in the K6.3
  phase breakdown but not in the "Files to Create" table; `mesh/handshake.rs`
  is a sub-directory while every other mesh file uses flat
  `mesh_*.rs`.
- No multi-node integration test harness exists. From
  `k6-test-strategy-review.md` recommendations: build
  `InMemoryTransport`, `MockPeer`, `MockClock`, `FaultyTransport` and
  add `crates/clawft-kernel/src/mesh_test_support.rs`.

#### From `k0-k3-gap-analysis.md` — older deferrals still open

- **K0** — `weave console` boots kernel and opens interactive REPL
  status was "NOT DONE" in March; `console.rs` only formats `BootEvent`s.
  K5 final claims it is done but the K0 audit could not find a
  `KernelConsole::run_repl()` etc. Spot-check needed.
- **K0** — `weave console --attach` to running kernel: NOT DONE.
- **K0** — `boot-log` command replays boot events: REPL missing.
- **K0** — Rustdoc builds without warnings: NOT VERIFIED in K0 audit
  (later reported clean by K5 final, with 3 collapsible-if clippy
  warnings).
- **K2** — `MCP tools ipc_send and ipc_subscribe`: gap-analysis says
  "deferred"; K5 final says shipped. Reconcile.

#### Vector / DiskANN / SIMD deferrals

- DiskANN backend is a brute-force `HashMap<u64, StoredEntry>` with
  linear cosine search (`vector_diskann.rs`); real DiskANN integration
  pending `ruvector-diskann` publication. `boot.rs:1415` log says
  "DiskANN (stub)". Swap in `diskann` feature flag when crate ships.
- HNSW tombstones are in-memory only; persistence requires extending
  the save/load format. Deferred to "vector sync work in WS4 / Gap #11"
  per `vector-hardening.md`.
- SIMD branch-free distance (`SimdDistanceConfig::is_available` always
  `false`) — waiting on PR #352 in `ruvector-core`.
- Log-quantized vectors (`LogQuantizedConfig::is_available` always
  `false`) — same upstream dependency.
- `VectorBackend` not yet wired into `DemocritusLoop` (still uses raw
  `HnswService`); see `vector-hybrid.md` "Next steps".

#### Sprint-16 mesh K6 transport "Next Steps"

From `mesh-k6-transport.md`:

- Wire `AssessmentTransport` into the daemon's mesh event loop (only
  used in unit tests today).
- Add `weft assess mesh-status` CLI subcommand.
- Implement assessment diff propagation (push only changed findings).
- Add QUIC transport (ADR-026) alongside existing TCP.

From `mesh-boot-integration.md` "Future Work":

- QUIC transport (quinn + snow Noise) — TCP currently default.
- Mesh as a `SystemService` (proper start / stop / health_check
  lifecycle).
- Cluster service wired to mesh peer discovery.
- Mesh health metrics in observability subsystem.

#### Mesh time sync (`mesh-time-sync.md`) — DESIGN ONLY, NOT IMPLEMENTED

- `authority_time` / `authority_id` heartbeat fields.
- Authority election (GPS > NTP > local monotonic).
- Offset smoothing EMA + outlier rejection.
- `MeshRuntime::mesh_time()` API.
- WiFi TSF integration (ESP32 / embedded).
- Required by ECC causal-edge timestamps, ExoChain `chain_seq`
  monotonicity across nodes, topology browser temporal dimension,
  robotics sensor fusion.

#### HTTP REST facade follow-ups

From `http-facade.md` "Next Steps":

- Wire axum handlers in `clawft-services/src/api/` that call the facade
  types.
- Add `custody.attest` RPC method to daemon dispatch (WS2 dependency)
  — landed in `chain-attestation.md`, double-check connection.
- Implement actual SSE streaming loop in the axum handler using
  `poll_events()`.
- Add integration tests once `ProfilesConfig` / `PairingConfig` types
  land in `clawft-types`.

#### exo-resource-tree K1 deliverables (still open)

- ACL-based permission checks with delegation shortcut (replace the
  always-`Allow` stub at `permission.rs:24-31`).
- `EffectiveAclCache`.
- Integration with `CapabilityChecker`.
- `DelegationCert` grant / revoke with Ed25519 signatures.
- Certificate chain validation.
- Time-bounded delegation with expiry.

#### Other deferred / orphaned items

- **AppManager persistence** — apps installed during one daemon session
  are lost on restart (`k0-k5-final-gap-analysis.md §6`). Need on-disk
  manifest store.
- **ContainerManager live integration** — types are tested but live
  Docker integration requires a real daemon.
- **Training data collection for WASM execution metrics (D18)** —
  deferred from K3 to K5.
- **Training data pipeline + SONA reuptake spike** — deferred from K4
  to K5; K5 has not yet shipped these.
- **K5 deliverable `docs/guides/kernel.md`** — file does not exist.
- **`docs/weftos/k-phases.md`** — documentation drift, shows K2.1 as
  "PENDING" and K3/K4/K5 as "STUBBED" when all are complete.
- **CRDT replication / conflict resolution for resource tree** — no
  Merkle proof generation, no tree diff API, no remote mutation
  signing (`MutationEvent.signature` is always `None`).
- **Quantum backends** — Pasqal and Braket are interface-only stubs
  returning `NotImplemented` (kernel Cargo.toml metadata says
  "EXPERIMENTAL (0.6.x): quantum backends — interface only").
- **One kernel-lib aggregate test hangs** when the full suite is run;
  targeted runs pass (CHANGELOG 0.6.19 "Known Issues").
- **Workspace clippy debt** — ~150 errors across `clawft-types/src/goal.rs`,
  `clawft-rpc`, `eml-core`, and older kernel/weave code. `scripts/build.sh
  check` is green; `scripts/build.sh clippy` is red. Pre-existing,
  documented in CHANGELOG.

### Open questions and known limitations

- **Duplicate ADR numbers**: two ADR-020s
  (chainloggable + kernel-phase-responsibilities) and two ADR-028s
  (post-quantum-dual-signing in `docs/adr/` vs weftos-kernel in
  `docs/architecture/`). Renumber and consolidate paths.
- **ADR-023 is "Proposed"** while mesh assessment transport already
  ships under it. Accept or mark dependent code provisional.
- **DEMOCRITUS "still stuck" log is expected**, not a bug, on empty
  causal graphs and idle conversations. v0.6.19 backoff keeps it
  quiet but operators will still see it; document.
- **`GraphFeatures::from_causal_graph` is O(n+m), not O(1)** as
  advertised — `connected_components()` walk. DEMOCRITUS fast path
  is therefore not constant-time on the EML side.
- **Identity / addressing gaps**: `chain_id: u32` (kernel-local, no
  cross-node uniqueness), `Pid: u64` (no `(NodeId, Pid)` composite),
  `PeerNode.address: Option<String>` (not a parsed `SocketAddr`),
  `PeerNode.capabilities` strings are unvalidated.
- **Cluster config gaps**: no `bind_address` / `listen_port` /
  `seed_peers`; `ClusterService::sync_to_membership()` is pull-based;
  `cluster_node_to_peer()` hardcodes `NodePlatform::CloudNative`.
- **Auth surface**: `add_peer()` accepts any `PeerNode` without
  signature verification (`add_peer_checked` only consults the
  revocation list). No cluster-join challenge-response. No
  per-message authentication on the wire beyond the Noise channel
  itself; `KernelMessage` is JSON on the wire.
- **No mTLS, no split-brain detection, no partition handling**.
- **Chain replication missing**: no `LocalChain::tail_from`, no
  merge/conflict resolution, no subscription/push, no chain anchoring
  beyond `MockAnchor`.
- **Tree limits**: `ResourceTree` uses non-async `Mutex`,
  `recompute_all()` is full-tree rehash, no incremental dirty-flag
  propagation, no Merkle proof generation, `MutationEvent.signature`
  is always `None`.
- **Inbox `mpsc` channel is in-process only** — remote delivery bridge
  missing.
- **Max-message-size enforcement** is in mesh_framing (16 MiB) but
  not on raw `KernelIpc::send`.
- **No DoS protection** on governance evaluation or `add_peer()`.
- **DiskANN backend is a brute-force HashMap stub**, not a real
  vector index.
- **`unsafe transmute_copy`** for ML-DSA-65 keys is upstream-fragile
  (security-audit L-3).
- **Test-suite hang** in `clawft-kernel --lib` aggregate run is
  unresolved (CHANGELOG 0.6.19 Known Issues).
- **`scripts/build.sh clippy` is red** on pre-existing workspace debt
  (~150 errors).

### Orphaned work

- **`mesh_runtime.rs:509-535` chain-sync stubs** — request builder +
  response parser exist, but no replay into `ChainManager`. K6.4
  design is documented, code is half-built.
- **`mesh_assess.rs AssessmentTransport`** — fully tested but never
  wired into the daemon mesh event loop.
- **`http_facade.rs`** — 13 routes + SSE + witness defined, but no
  axum handler in `clawft-services/src/api/` calls into it.
- **`mesh-time-sync.md`** — design only, no code, no owner, no sprint.
  Required for distributed ECC.
- **Quantum backends** (`quantum_pasqal.rs`, `quantum_braket.rs`,
  `quantum_register.rs`, `quantum_state.rs`, `quantum_backend.rs`)
  — interface-only, `NotImplemented`, no production caller.
- **`weftos-leaf-types` integration** (v0.6.17 leaf push CLI) —
  unclear if leaf pushes route through governance/chain. No
  workstream claims it.
- **`stream_anchor.rs::TopicAnchor`** — `topic_matches` exported but
  caller unverified; v0.6.19 may have shipped only the
  `StreamWindowAnchor` half.
- **`exo-resource-tree::permission`/`delegation`** — explicit K0
  stubs, no K1 owner named even though kernel K1 shipped.
  `exo-resource-tree::scoring::NodeScoring` (382 lines) — exposed
  but undocumented in planning notes.
- **`mesh_log` / `mesh_dedup` / `mesh_listener` / `mesh_bootstrap`**
  — implemented but no daemon RPC / CLI / observability surface.
  Verify integration path before 0.7.0.
- **`reliable_queue.rs` (669 lines) + `dead_letter.rs` (430 lines)**
  — `DeadLetterModel` EML wiring came in 0.6.19; verify production
  paths actually drain DLQ. No DLQ inspection CLI documented.
- **`cognitum-gate-tilezero`** — three-way Permit/Defer/Deny
  cryptographic-receipt path not exercised in documented tests.

## Task List

Tasks below are in rough priority order. None are 0.7.0 ship-gate filters
(per audit instructions); they capture all known kernel-and-governance
work surfaced in the docs and code.

> **Audit-row refresh — 2026-04-28**
>
> The previous rows 1, 2, 3, 5, 6, 7, 8, and 9 (in the original numbering
> below) were closed by commit `a0c54a47` "fix: close 7 ExoChain/governance
> certification failures" on Apr 14, 2026. Verified:
> `auth_service.rs:337` (rotate_credential) and `auth_service.rs:382`
> (request_token) carry `gate.check` calls; `chain_event.rs` exposes the
> tracing→ChainManager bridge; `tools_fs.rs`, `config_service.rs`,
> `cron.rs`, `environment.rs`, `hnsw_service.rs` all received the
> remediation. Per the docs/handoff.md "Refresh stale audit rows"
> instruction, the explicitly-named CRITICAL trio (the original rows 1, 2,
> 3) is stripped from this table; the others are also closed by the same
> commit but left in place pending a fuller audit pass — they should NOT
> be triaged into Plane.

| # | Task | Owner / Source | Severity | Effort |
|---|------|----------------|----------|--------|
| 1 | Add gate to `auth_service.rs:revoke_token` (DiD) | cert-critical PARTIAL #6 | HIGH | S |
| 2 | Populate `hnsw_service.rs:clear` chain payload with `entries_destroyed` + `epoch` *(closed in `a0c54a47` — verify and strip)* | cert-critical PARTIAL #10 | HIGH | XS |
| 3 | Add `EnvironmentError::GovernanceDenied` variant; replace `NotFound` reuse in `environment.rs:326` *(closed in `a0c54a47` — verify and strip)* | cert-critical PARTIAL #20 | MEDIUM | S |
| 4 | Add `EVENT_KIND_WASM_FS_*` constants and chain-log all `wasm_runner/tools_fs.rs` mutations *(closed in `a0c54a47` — verify and strip)* | cert-medium FAIL #32 | HIGH | M |
| 5 | Add `gate.check` to `config_service.rs:delete_typed` (line 380) *(closed in `a0c54a47` — verify and strip)* | gov-cert GAP-1 | MEDIUM | S |
| 6 | Add `gate.check` to `cron.rs:remove_job` *(closed in `a0c54a47` — verify and strip)* | gov-cert GAP-3 | LOW | XS |
| 10 | Re-enable auth middleware on `/api/*` and `/ws` | security-audit M-1 | HIGH | M |
| 11 | Replace `CorsLayer::permissive()` default with deny-by-default | security-audit M-2 | HIGH | XS |
| 12 | Add `tower::limit::RateLimitLayer` to `/api/*` (esp. token endpoints) | security-audit M-3 | HIGH | S |
| 13 | Add `TokenStore::revoke_token` + expired-token cleanup task | security-audit M-4 | MEDIUM | S |
| 14 | Add optional `idempotency_key` to `ChainEvent`; check duplicates before append | security-audit M-5 | MEDIUM | M |
| 15 | Add `cargo audit` to `scripts/build.sh gate` and CI | security-audit | MEDIUM | XS |
| 16 | Implement K6.4 chain replay: `LocalChain::tail_from(seq)` + apply remote events through `ChainManager::append_signed()` | k6-readiness-audit; mesh_runtime.rs:509-535 | HIGH | L |
| 17 | Implement K6.4 tree Merkle diff + remote mutation signing (`MutationEvent.signature`) | k6-readiness-audit | HIGH | L |
| 18 | Implement S10 key-rotation chain event + verifier per security panel design | k6-decision-coverage | HIGH | M |
| 19 | Implement `IpcScope::Restricted` browser default + `browser_policy` rules (S7) | k6-decision-coverage | MEDIUM | S |
| 20 | Decide Q1: chain merge (leader vs DAG); Q5: split-brain handling | k6-developer-readiness | HIGH | M |
| 21 | Decide Q2: KernelMessage wire format (JSON vs RVF) and freeze | k6-developer-readiness | HIGH | S |
| 22 | Decide Q4: full libp2p-kad vs lighter DHT | k6-developer-readiness | MEDIUM | M |
| 23 | Add InMemoryTransport / MockPeer / MockClock / FaultyTransport in `crates/clawft-kernel/src/mesh_test_support.rs` | k6-test-strategy-review P0 | HIGH | M |
| 24 | Define `Clock` trait, inject into all time-dependent mesh components | k6-test-strategy-review P0 | HIGH | M |
| 25 | Add `cargo check --target wasm32-unknown-unknown` (no mesh) to CI | k6-test-strategy-review P0 | MEDIUM | XS |
| 26 | Define missing K6 protocol struct types: `MeshStream`, `TransportListener`, `EncryptedPeer`, `WeftHandshake`, `JoinRequest/Response`, `TreeSyncRequest/Response`, `ServiceEndpoint`, `ProcessAdvertisement`, `ServiceAdvertisement`, `Frame`, full `msg_type` enumeration | k6-developer-readiness | HIGH | M |
| 27 | Resolve `mesh_adapter.rs` vs `mesh_ipc.rs` location discrepancy and `mesh/handshake.rs` subdir vs flat layout | k6-developer-readiness | LOW | XS |
| 28 | Wire `AssessmentTransport` into daemon mesh event loop; add `weft assess mesh-status` CLI; assessment diff propagation | mesh-k6-transport Next Steps | MEDIUM | M |
| 29 | Add QUIC transport (quinn + snow) alongside existing TCP/WS (ADR-026) | mesh-boot-integration Future Work | HIGH | L |
| 30 | Make Mesh a `SystemService` (proper start / stop / health_check) | mesh-boot-integration Future Work | MEDIUM | S |
| 31 | Wire ClusterService to mesh peer discovery | mesh-boot-integration Future Work | MEDIUM | M |
| 32 | Implement mesh time-sync per `mesh-time-sync.md` (authority election, offset smoothing, `mesh_time()`, TSF) | mesh-time-sync.md (design only) | HIGH | L |
| 33 | Wire axum handlers in `clawft-services/src/api/` to `http_facade` types; SSE loop using `poll_events()` | http-facade Next Steps | MEDIUM | M |
| 34 | Add integration tests for HTTP facade once `ProfilesConfig`/`PairingConfig` land | http-facade Next Steps | LOW | S |
| 35 | Wire `VectorBackend` into `DemocritusLoop` (currently raw `HnswService`) | vector-hybrid Next steps | MEDIUM | S |
| 36 | Add `ecc.vector-config` RPC endpoint to expose active backend | vector-hybrid Next steps | LOW | XS |
| 37 | Real DiskANN backend behind `diskann` feature flag once `ruvector-diskann` publishes | vector-hybrid Next steps | LOW | M |
| 38 | Persist HNSW tombstones across save/load (`vector-hardening` Tombstones note) | vector-hardening | MEDIUM | M |
| 39 | Land `ruvector-core` PR #352 and flip `LogQuantizedConfig`/`SimdDistanceConfig` `is_available()` | vector_quantization.rs:83,150 (KG-011/KG-012) | MEDIUM | XS once upstream |
| 40 | Ship a real Wasmtime backend for `quantum_register::spectral_embedding` (or move to deferred-feature appendix) | quantum_register.rs:9 TODO | LOW | M |
| 41 | Replace `permission.rs` always-`Allow` stub with K1 ACL engine + `EffectiveAclCache` + CapabilityChecker integration | exo-resource-tree K1 | HIGH | L |
| 42 | Implement `DelegationCert` lifecycle: grant/revoke with Ed25519, chain validation, expiry | exo-resource-tree K1 | HIGH | M |
| 43 | Implement `clawft-services/src/api/bridge.rs` TODOs: skill install/uninstall, memory delete, config persistence | bridge.rs:282,287,395,467 | MEDIUM | M |
| 44 | Add CSP middleware to API tower stack | handlers.rs:130 TODO | MEDIUM | XS |
| 45 | Resolve test-suite hang in `clawft-kernel --lib` aggregate run | CHANGELOG 0.6.19 Known Issues | HIGH | M |
| 46 | Clean workspace clippy debt (~150 errors) | CHANGELOG 0.6.19 Known Issues | MEDIUM | M |
| 47 | Persist AppManager state to disk (manifest store) | k0-k5-final-gap §6 | MEDIUM | S |
| 48 | Implement chain-anchored anchoring against external ledgers (`ChainAnchor` trait has only `MockAnchor`) | adr-041-chainanchor-trait | MEDIUM | M |
| 49 | Update `docs/weftos/k-phases.md` (K2.1/K3/K4/K5 currently mis-marked PENDING/STUBBED) | k0-k5-final §1, §3 | LOW | XS |
| 50 | Write `docs/guides/kernel.md` (deferred from K5) | k0-k5-final | MEDIUM | M |
| 51 | Renumber duplicate ADRs (two ADR-020s, two ADR-028s) and consolidate `docs/architecture/` vs `docs/adr/` paths | this audit | LOW | XS |
| 52 | Accept ADR-023 (assessment-as-kernel-service) — currently "Proposed" but mesh-assess code is shipped | adr-023 status | LOW | XS |
| 53 | Add `NodeId` composite for cross-node uniqueness (PID + node_id) and remote inbox bridge | k6-readiness-audit | HIGH | L |
| 54 | Add max-message-size enforcement on `KernelIpc::send` deserialization (16 MiB) | k6-readiness-audit Security RED | HIGH | S |
| 55 | Add `MutationEvent` Ed25519 signing for cross-node tree mutations | k6-readiness-audit Tree RED | HIGH | M |
| 56 | Incremental Merkle hash updates (`recompute_all` is full-tree today) | k6-readiness-audit Tree YELLOW | MEDIUM | M |
| 57 | Replace static `Vec<GovernanceRule>` with cluster-wide rule distribution + cross-node escalation | k6-readiness-audit Governance RED | HIGH | L |
| 58 | Capability-claim verification across nodes (signed advertisement) | k6-readiness-audit Security RED | HIGH | M |
| 59 | Rate-limit `add_peer()` and governance-evaluation requests | k6-readiness-audit Security RED | MEDIUM | S |
| 60 | Document the DEMOCRITUS "still stuck" log-line semantics in operator-facing docs | this audit | LOW | XS |
| 61 | Verify `weftos-leaf-types` push path goes through governance / chain (or document the bypass) | this audit | MEDIUM | S |
| 62 | Audit whether `mesh_log`, `mesh_dedup`, `mesh_listener`, `mesh_bootstrap` have an end-to-end caller; if not, schedule wiring or document orphan status | this audit | MEDIUM | S |
| 63 | Confirm `cognitum-gate-tilezero` (TileZeroGate) Permit/Defer/Deny path is exercised; add tests if missing | this audit | LOW | S |
| 64 | Add `EVENT_KIND_*` constants for `register_with_metadata`, `sandbox::check_tool|network|file_read|file_write`, `ingest::save_query_result` (minor non-kernel chain gaps) | exochain-certification-nonkernel Minor Gaps | LOW | S |

## Sources

### Code (file:line)

- DEMOCRITUS loop: `crates/clawft-kernel/src/cognitive_tick.rs:295-542`
  (warning at L472; bounded history + edge-trigger + exp-backoff
  L430-492). Impulse-pipeline DEMOCRITUS:
  `crates/clawft-kernel/src/democritus.rs:1-1112`.
- Boot: `boot.rs:160-1613`; DEMOCRITUS spawn `boot.rs:1497-1521`;
  vector backend selection `boot.rs:1379-1557`; "DiskANN (stub)" log
  `boot.rs:1415`.
- K6 mesh chain-sync stubs: `mesh_runtime.rs:509-535`.
- ChainAnchor trait + `MockAnchor`: `chain.rs:2180-2212`. ML-DSA-65
  `transmute_copy`: `chain.rs:1617-1624`.
- Quantum stubs: `quantum_braket.rs:74-160`, `quantum_pasqal.rs`,
  `quantum_register.rs:9`.
- Vector quantization stubs: `vector_quantization.rs:83` (KG-011),
  `vector_quantization.rs:150` (KG-012).
- WASM runner unsafe: `wasm_runner/registry.rs:318-326`.
- Tools-extended unsafe pointer: `tools_extended.rs:616-629`.
- exo-resource-tree K0 stubs: `permission.rs:23-31`,
  `delegation.rs:14`.
- Services TODOs: `clawft-services/src/api/handlers.rs:130,133`;
  `clawft-services/src/api/bridge.rs:282,287,395,467`;
  `clawft-services/src/api/mod.rs:295,309-315`;
  `clawft-services/src/api/auth.rs` (no `revoke_token`).
- Kernel feature flags: `crates/clawft-kernel/Cargo.toml` —
  `quantum-pasqal`, `quantum-braket`, `diskann`, `wasm-sandbox`,
  `os-patterns`, `treesitter`, `sensor`, `containers`, `http-api`.

### Planning notes & ADRs

All planning notes and ADRs are listed in the `related_plans` and
`related_adrs` frontmatter. Highest-leverage references for this
audit: sprint-16/{democritus-loop, chain-attestation, security-audit,
mesh-k6-transport, vector-hardening, vector-hybrid, http-facade}.md;
exochain-certification-{critical, medium, nonkernel}.md;
governance-certification.md; k0-k5-final-gap-analysis.md;
k6-{readiness-audit, decision-coverage, developer-readiness,
test-strategy-review}.md; mesh-{boot-integration, time-sync}.md;
CHANGELOG.md 0.6.19 entry. Two ADR-020 files
(chainloggable + kernel-phase-responsibilities) and two ADR-028 files
(post-quantum-dual-signing in `docs/adr/`, weftos-kernel in
`docs/architecture/`) share numbers — see Open Questions.

<!-- TRIAGED-STAMP:BEGIN -->
## Triaged into Plane — 2026-04-28

All open items in this audit have been filed as Plane work items in the WeftOS workspace under the `ws02-kernel` label.

- **Range**: WEFT-98 … WEFT-153 (56 items)
- **Per cycle**: 0.7.x: 13, 0.8.x: 39, 0.9.x: 4
- **Triage spec**: `.planning/reviews/0.7.0-release-gate/triage/`
- **WEFT-N → name map**: `.planning/reviews/0.7.0-release-gate/triage/weft-mapping.json`

Per the project rule (CLAUDE.md → "Plane is the authoritative work tracker"): future updates to these items happen in Plane, not in this audit doc. This doc remains the source-of-truth for the original survey.
<!-- TRIAGED-STAMP:END -->
