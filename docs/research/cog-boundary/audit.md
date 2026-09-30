# Cog-boundary audit of WeftOS

Status: advisory research, 2026-09-30, branch `0.8-metaharness`. Card cog-boundary-01. Nothing is removed because it is listed here. Read-only over code; this file is the only write.

Sources: ADR-099 (placement), ADR-100 (cog kind), ADR-101 (inference kind), ADR-103 ("Instances, federation and environments", "Core versus cog"), and the code at the paths cited. Line counts come from `wc -l` over `src/` on the audit date and are approximate. Where I read only a module header and not the body, the table says so.

## 1. Summary

The kernel is a 140,746-line crate (`crates/clawft-kernel`). By my tally roughly 20k lines are the placement layer (`workload_ctl`, `workload_runtime`, `workload_pkg`, `workload_governance`, `node_facts`, `revocation`), 16.7k are mesh, and about 35k are the ECC cognitive substrate plus its domain services (vectors, embeddings, causal graph, weaver, voice turn-taking, quantum stubs). The OS primitives that ADR-103 says every instance needs (identity, chains, governance, placement, IPC/mesh, supervision) all exist and are mostly sound. The cog layer on top of them does not really exist yet: the `cog` kind is hard-wired, cogs get no chain or capability API, and there is no code for instance-to-instance federation.

The practical conclusion: very little existing code needs to move now. The large win is adding six missing primitives (section 6) so that the staging gate, CI/CD and fusion behaviors can be written as cogs from the start, instead of being added to the kernel because there was nowhere else to put them.

## 2. The primitive set the kernel must own

These are the things a cog cannot provide for itself because they are the trust root or the shared substrate. File refs are `crates/clawft-kernel/src/` unless a crate is named.

| # | Primitive | Where it lives today | State |
|---|---|---|---|
| P1 | Identity and certification: node Ed25519 key, agent key registry, node registry, signed capability claims, key rotation, revocation | `cluster.rs:305` `NodeIdentity`; `agent_registry.rs:37`; `node_registry.rs:121`; `capability_claim.rs:140,157`; `key_rotation.rs`; `revocation.rs:39`, `revocation/subjects.rs` | Present. The machine to user to project to actor certification chain (ADR-103 D7) is not built. |
| P2 | Chains: append-only signed event chain, storage, external anchoring, tree-mutation chaining, window anchors, mesh sync | `chain.rs:139` `ChainEvent`, `:1087` `ChainManager`; `chain_storage.rs`; `chain_anchor.rs:100`; `stream_anchor.rs:76`; `tree_manager.rs:72`; `mesh_chain.rs:11-82` | Present, one chain per process. Project chains and `project.anchor` (D6) not built. |
| P3 | Governance: rules, effect vectors, three-way gate, rule distribution, workload gate | `governance.rs:60,343,937`; `gate.rs:82,99,304`; `rule_distribution.rs:29`; `workload_governance/gate.rs:55`; `capability.rs:112,284` | Present. `CapabilityGate` still permits unknown action categories (`gate.rs:139`); only `workload.*` is default-deny (`gate.rs:130-138`). |
| P4 | Placement: open capability vocabulary, node facts, pure placer, runtime adapter trait, signed packages, artifact transfer | `clawft-types/src/placement/` (6.7k lines); `node_facts/`; `workload_ctl/`; `workload_runtime/types.rs:278` `WorkloadRuntime`; `workload_pkg/{sign,verify,trust}.rs`; `artifact_store.rs:99`; `mesh_artifact*.rs` | Present for one kind (cog). |
| P5 | IPC and mesh: kernel messages, PID router, topic pub/sub, Noise mesh, discovery, service advertisement, SWIM liveness | `ipc.rs:47,202,414`; `a2a.rs:69`; `topic.rs:115`; `mesh_runtime.rs:50`; `mesh_noise.rs`; `mesh_discovery.rs`; `mesh_service_adv.rs`; `mesh_heartbeat.rs` | Present. |
| P6 | Supervision: process table, spawn backends, links and monitors, reconciler, workload restart/limits | `process.rs:147`; `supervisor.rs:301,390`; `monitor.rs:44,51`; `reconciler.rs:61`; `workload_runtime/supervise.rs:21,47` | Present. `SpawnBackend::Remote/Container/Wasm` remain stubs (ADR-099). |
| P7 | Token and credential authority | Outbound credentials: `auth_service.rs:188` (in-memory factotum). Inbound gateway tokens: `clawft-services/src/api/auth.rs` and `mcp/session_cap.rs` (two stores, per ADR-102) | Split across three places. ADR-102/103 put it in the user daemon; not done. |
| P8 | Config, resource tree, substrate paths with read ACLs | `config_service.rs:124`; `exo-resource-tree` crate; `tree_manager.rs:72`; `substrate_service.rs` (read/publish/subscribe/notify); `clawft-substrate/src/acl.rs:53` | Present. |
| P9 | Triggers: timers and cron | `timer.rs:20`; `cron.rs:71` | Present, but duplicated (section 4, row 15). No event-triggered start. |
| P10 | Service registry and health | `service.rs:108,162`; `health.rs:69`; `boot.rs:138` | Present. The daemon RPC surface is not extensible (section 6, M8). |

Not in the list on purpose: ECC (causal graph, HNSW, DEMOCRITUS, weaver), voice, sensors, assessment, apps. ECC is the product's differentiator and every cog may want it, so I treat it as a shared system service (row 3 below), not as an OS primitive and not as a cog.

## 3. Crates and purpose

Sizes are lines under `src/`. Grouped by role. Crates with no kernel dependency are marked (no-kernel).

| Group | Crates (lines) | Purpose |
|---|---|---|
| Kernel and daemon | `clawft-kernel` (140.7k), `clawft-weave` (31.0k; `weaver` CLI and daemon, `daemon.rs` alone is 9.6k), `clawft-rpc` (1.3k), `weftos` (2.2k facade), `exo-core` (0.4k), `exo-dag` (0.6k), `exo-resource-tree` (5.2k) | Boot, chains, governance, mesh, placement, daemon RPC |
| Agent layer | `clawft-core` (71.9k: agent loop 31.6k, pipeline 15.4k, embeddings 5.7k, agent_bus 1.2k), `clawft-service-agent` (11.8k), `clawft-llm` (6.5k), `clawft-service-llm` (1.9k), `clawft-tools` (9.7k), `clawft-cli` (25.0k; `weft`) | Agent runtime, LLM routing, tools |
| Plugin and I/O | `clawft-plugin` (12.3k, of which voice 7.9k), `clawft-plugin-treesitter`, `clawft-channels` (32.6k; 11 adapters plus voice 13.5k), `clawft-wasm-host` (4.6k), `clawft-wasm` (2.2k) | Trait crates, channel adapters, WASM sandbox host |
| Services | `clawft-services` (22.7k: mcp 11.5k, api 5.7k, delegation 1.5k, cron 1.2k, clawhub 1.0k, heartbeat 0.5k), `clawft-service-whisper`, `-classify`, `-terminal` | Gateway, MCP, scheduled jobs, skill registry |
| Voice | `clawft-voice-aec`, `-onnx`, `-tts`, `-talk` (9.8k total), `clawft-bench-voice`; plus voice inside channels, plugin, kernel, weave | Full-duplex voice pipeline |
| Knowledge | `clawft-graphify` (35.8k; optional `kernel-bridge` feature), `clawft-treecalc` (0.2k, no-kernel), `clawft-cow-memory` (1.7k, no-kernel), `eml-core` (4.2k), `clawft-lsp-extract`, `clawft-bvh` (1.9k, no-kernel) | Graph extraction, branchable memory, spatial index |
| Sensing and world model | `weftos-sensor-pipeline` (+`-wire`), `weftos-worldmodel` (+`-core`, `-impls`), `clawft-worldmodel-service`, `clawft-delegation` (all no-kernel), `clawft-splat-pipeline`, `clawft-splatd`, `clawft-sonobuoy-ranging` (no-kernel) | Sensor fusion, latent world model, splat jobs, ranging scaffold |
| Substrate and apps | `clawft-substrate` (10.3k), `clawft-app` (3.6k), `clawft-surface`, `clawft-canon`, `clawft-gui-egui` (30.1k), `clawft-security` (1.1k), `clawft-types` (24.1k) | State tree, app manifests, UI, shared types |
| Leaf and firmware | `weftos-leaf-*` (7 crates), `weftos-scene-builder`, `lgfx-bus-rgb-rs`, `clawft-edge-pad`, `-idf`, `clawft-android-edge`, `clawft-edge-bench` | Leaf role (ADR-103): display, touch, ESP32 |
| Harness | `clawft-casestudy-gen-qsr`, `clawft-bench-voice`, `clawft-edge-bench` | Test and benchmark corpora |

Structural observation: the sensing and world-model stack, splat jobs and sonobuoy ranging already have no kernel dependency. They are cog-shaped today. The domain code that is not is inside `clawft-kernel`, `clawft-weave` and `clawft-channels`.

## 4. Inventory of candidates

Verdicts: **keep-core** (correctly core; fix in place if noted), **move-later** (should become a cog or separate crate, no urgency), **move-now** (cheap and safe enough to do soon, still advisory). "Missing" names the primitive from section 6 that keeps the item core today.

| # | Candidate and location | What it is | Would sit on | Verdict | Cost and risk | Missing |
|---|---|---|---|---|---|---|
| 1 | Voice turn-taking in kernel: `talk_loop.rs`, `talk_loop_service.rs`, `floor.rs`, `duplex.rs`, `thin_edge.rs`, `coherence.rs`, `view_resolver.rs` (~3.7k lines, plus tests) | ECC graph-walk conversation loop (ADR-062/068), full-duplex floor actuator, thin-edge wire vocabulary. Hosted at daemon boot when `[kernel.agent].talk_loop` is on (`clawft-weave/src/daemon.rs:497`) | substrate subscribe, chain append, service registration, a 50 ms tick | move-later | High. Live-mic confirmation (WEFT-615) is still open per repo notes; latency-sensitive; talk_loop shares `SessionView` with `context_graft`. | M1, M2, M8, M11 |
| 2 | Voice, everything else. `clawft-channels/src/voice` (13.5k), `clawft-plugin/src/voice` (7.9k), `clawft-voice-{aec,onnx,tts,talk}` (9.8k), `clawft-service-{whisper,classify}` (4.0k), `clawft-weave/src/{voice_loop,voice_router,voice_trace}.rs` (1.8k) | The largest single domain: about 40k lines in six places, of which two (`channels`, `plugin`) are trait crates that should not hold a domain | Same as row 1, plus placement for accelerator/mic locality | move-later, consolidate location first | Very high. First step is only to gather the code under one owner, which costs little and changes no behavior. | M1, M2, M3 |
| 3 | ECC substrate: `causal*.rs`, `cognitive_tick.rs`, `democritus.rs`, `crossref.rs`, `calibration.rs`, `ecc_segment.rs`, `impulse.rs` (~9.8k); vectors and embeddings `hnsw*.rs`, `vector_*.rs`, `embedding*.rs`, `eml_*.rs` (~12.8k); `spatial_*.rs` (0.8k). Wired at `boot.rs:1836-2087` behind default feature `ecc` | The cognitive layer the product is built around. `graphify`, `weaver` and voice all consume it | It is the shared knowledge service that cogs call | keep-core as a system service; later split into its own crate (`clawft-ecc`) without changing behavior | Medium for the crate split (feature `ecc` already fences it). Do not turn it into a cog: every cog would then depend on another workload for memory. | none |
| 4 | Weaver: `weaver.rs` (5.1k) and `weave` ecc.* RPCs and `commands/ecc_cmd.rs` (0.8k) | "ECC-powered codebase modeling": ingests git logs, file trees, CI pipelines, docs into the causal graph | ECC service, chain subscribe | move-later | Medium-high. Largest single kernel file. Daemon and CLI call it directly. | M1, M8 |
| 5 | Quantum: `quantum_backend.rs`, `quantum_register.rs`, `quantum_state.rs`, `quantum_pasqal.rs`, `quantum_braket.rs` (2.7k) | Marked EXPERIMENTAL in its own header; backends are stubs returning NotImplemented. Compiled by default because the first three sit under `ecc` (`lib.rs:144-152`) | A future `accelerator-job` kind (ADR-099 defers these) | move-now | Low. I found no consumer outside `clawft-kernel/src/lib.rs` and `tests/pasqal_live.rs` (the hit in `clawhub/search.rs` is a test string). Cheapest is gating it off `default` or moving it to its own crate. | M3 |
| 6 | Sensor topics: `mesh_sensor.rs` (`mesh.sensor.v1.{encoded,consensus,control}`, chain index), `sensor_graph/` (1.7k) | Kernel registers the LeWM sensor topics and indexes frames. `sensor_graph` sits behind feature `sensor`, which no crate in the workspace enables (I grepped Cargo.toml files of weave, weftos, cli) | topic router, chain append, topic ACL | `mesh_sensor`: move-later to the fusion cog. `sensor_graph`: move-now (dead by default) | Low for `sensor_graph`. `mesh_sensor` has no in-tree consumer beyond a doc comment in `weftos-sensor-pipeline/src/lib.rs:330`, so moving it is cheap too. | M1, M2 |
| 7 | Assessment: `assessment/` (1.8k, analyzers for complexity, dependency, security, terraform, rabbitmq, network, topology, data-source), `mesh_assess.rs` (0.8k); boot registers the service (`boot.rs:343`); daemon `assess.run`/`assess.status` (`daemon.rs:7341,7379`); `clawft-cli/.../assess_cmd.rs` (1.6k); `graphify::bridge` implements the `Analyzer` trait | Code and infrastructure analysis with cross-project gossip | job cog plus chain append for findings | move-later. This is the natural first "check" cog for staging. | Medium. `Analyzer` trait is the seam and already exists. Terraform and RabbitMQ analyzers are client-specific domain logic in the OS. | M3, M4 |
| 8 | Cognitum Seed adapter: `workload_runtime/seed*.rs`, `host_seed.rs`, `workload_pkg/cognitum.rs`, seed tests (~3.5k) | The `remote.api.cognitum-seed` runtime adapter (ADR-100 s5) | `WorkloadRuntime` trait, already a trait (`types.rs:278`) | move-later as an optional adapter crate | Low-medium. The trait seam exists; the daemon reads `workload-seeds.json` (`workload_place_policy.rs:52`). | none |
| 9 | Cognitum compatibility surface: `http_facade.rs` (1.0k, SSE deltas, `/custody/witness` injection), `profile_store.rs` (0.7k, per-user vector namespaces) | Closes "Cognitum Seed gaps" 6-8 and 14 | gateway, chain append | move-later. `http_facade` already has its axum binding in `clawft-services/src/api/http_facade_api.rs`; only the types stay in the kernel. `profile_store` overlaps D6 per-project scoping and should be judged against it. | Low-medium | M1 (SSE feed is really a chain subscription) |
| 10 | Apps: `app.rs` (2.3k), `reconciler.rs`, `agency.rs`, `container.rs` (1.5k), `clawft-app` (3.6k), `weave/src/app_rpc*.rs` | A second packaging and lifecycle system (`AppManifest`: agents, tools, services) beside `workload_pkg`. ADR-099 s1 says a workload is not an app | workload kind `app` | keep-core now; converge later so apps become a workload kind | Medium. Two lifecycles will diverge if both keep growing. `container.rs` is documented as superseded by `workload_runtime/container.rs` (`container.rs:280`); retire it in place. | M3 |
| 11 | `wasm_runner/` (5.7k) beside `clawft-wasm-host` (4.6k) | Two WASM sandboxes, two permission stores | `clawft-wasm-host` | keep-core, fix in place: ADR-099 decision 4 already says unify | Medium. This is convergence, not extraction. | none |
| 12 | `tools_extended.rs` (1.4k) | Ten WASM catalog tools (`fs.analyze`, `git.log`, `doc.parse` ...). It is **not declared in `lib.rs`** and nothing references it, so it is not compiled | tools/cog | move-now: confirm with the owner, then delete or wire | Very low | none |
| 13 | `http_api.rs` (0.5k, feature `http-api`) | Endpoint for the Paperclip orchestrator to submit tasks | service registration, gateway | move-later | Low | M8 |
| 14 | `environment.rs` (0.7k): `EnvironmentManager`, dev/staging/prod classes with risk thresholds 0.9/0.6/0.3 | Governance scopes switchable inside one instance (`set_active`, `environment.rs:312`). Referenced only from `lib.rs` and itself; `boot.rs` never constructs it | governance | keep-core, fix in place. See finding 3. | Low. Keep the presets as genesis-time profiles; drop the switch. | none |
| 15 | Scheduling: `cron.rs` (0.4k), `timer.rs` (0.5k), `heartbeat.rs` (0.4k) in the kernel, versus `clawft-services/src/cron_service` (1.2k) and `heartbeat` (0.5k) | Two cron implementations: the kernel one dispatches IPC to agents, the services one posts `InboundMessage` on the bus and persists JSONL. `HeartbeatScheduler` (agent wake cycles) is a third thing named heartbeat, next to SWIM in `mesh_heartbeat.rs` | P9 trigger primitive | cron/timer: keep-core and consolidate to one. services `heartbeat` (posts a prompt on an interval): move-later, agent-layer. | Low-medium | M9 |
| 16 | `clawft-channels` adapters (discord 4.4k, slack 2.3k, email 2.1k, telegram 1.5k, irc, matrix, google_chat, signal, teams, whatsapp, web; host 0.6k, plugin_host 0.7k) | Channel adapters behind `ChannelAdapter` (`clawft-plugin/src/traits.rs:116`), run in-process | workload kind `channel`, credential broker | move-later. The trait seam already isolates them. The gain is credential and network isolation per adapter. | High for little urgency. Needs per-cog credentials (M2, M10). | M2, M3, M10 |
| 17 | `clawft-services/src/delegation` (1.5k) | Routes a task to Local or Claude by regex rules and complexity. Agent-layer routing, not OS delegation | agent layer | move-later (agent skill or cog) | Low-medium. Used by `clawft-tools/src/delegate_tool.rs` and CLI. | none |
| 18 | `clawft-services/src/clawhub` (1.0k) | Skill registry with REST stubs, stars, versions, signing. Overlaps `workload_pkg` signing and trust | registry cog on placement + package trust | move-later | Low. Mostly stubs. | M3 |
| 19 | `clawft-delegation` (0.85k) | `WorkDelegationCert`: Ed25519 grant/TTL/revoke, scoped to `worldmodel.lattice` methods. Same lifecycle as `exo_resource_tree::DelegationCert` | P1 certification | keep-core pattern, fix in place: generalize the cert so it scopes any service/kind (this is the primary-to-subordinate grant in ADR-103). The world-model server stays a cog. | Low. The crate has no kernel deps. | M6 |
| 20 | `clawft-graphify` (35.8k) with `clawft-weave/src/commands/graphify_cmd.rs` (1.1k) | Knowledge-graph extraction, layout, export, vault. `kernel-bridge` feature (off by default) imports `clawft_kernel::{causal, crossref, hnsw_service, assessment}` (`bridge.rs:18-22,606`) | ECC service via its public API, chain subscribe | move-later. Already a library outside the kernel; the bridge should call public APIs rather than kernel types. | Medium. Non-bridge builds are unaffected. | M1, M8 |
| 21 | Sensor pipeline, world model, `clawft-worldmodel-service` (single, hot-standby, peer-to-peer topologies), splat pipeline and `clawft-splatd` (HTTP job server), sonobuoy ranging | Already separate crates with no kernel dependency | placement (`service` and `job` kinds), topics/chain | keep as is; this is the model for the rest | Cost is on the kernel side: nothing places these because only `cog` is a kind (M3). | M3, M4 |
| 22 | `SwarmCoordinator` in `clawft-core/src/agent_bus/coordinator.rs:79` | In-process flat agent fan-out. Distributes tasks with no reference to ADR-099 placement | placement client for cross-node fan-out | keep-core, fix in place: leave in-process fan-out alone; route cross-node distribution through placement rather than adding a second mechanism | Low | M4 |
| 23 | `clawft-substrate` hardware adapters (`mic.rs` 1.1k, `sensor_paths.rs`, `rfkill.rs`, `network.rs`) | Host-hardware adapters behind the `OntologyAdapter` contract (ADR-017) | substrate publish | contract keep-core; adapters move-later | Low | M2 |
| 24 | `lewm_invariant.rs` (ADR-090) | Kernel-side predicates that the world model stays a non-authoritative consumer of ECC | ECC boundary | move-later with the fusion cog, or keep as an ECC test module | Low | none |
| 25 | `clawft-cow-memory` (1.7k, no-kernel) | Branchable, checkpointable RVF store (agenticow port) | chain, artifact store | keep separate; it is the building block for speculative deploy and rollback (section 7) | None | M14 |
| 26 | `clawft-treecalc` (0.2k), `clawft-bvh`, `eml-core` | Pure libraries | none | keep (libraries, not OS or cog) | None | none |
| 27 | Daemon-embedded singletons: `DAEMON_LLM`, `DAEMON_TERMINAL`, `DAEMON_AGENT`, `DAEMON_ATOM_REGISTRY` ... (`daemon.rs:36-202`); `llm_service.rs`, `turn_ledger.rs`, `conv_postmortem.rs` in `clawft-weave/src` | Domain services wired into the daemon as process globals and served from the `dispatch` match (`daemon.rs:5487`, fallthrough at `:8404`) | service registry, service RPC | move-later, once M8 exists | Medium. Touching `daemon.rs` (9.6k lines) is the risk, and there is a 500-line file rule in `CLAUDE.md`. | M8 |

Correctly core and fine as they are (no action): P1-P10 in section 2, `clawft-types/src/placement`, `node_facts`, `workload_ctl`, `workload_pkg`, `workload_governance`, `artifact_store`, all `mesh_*` transport, `exo-*`, `clawft-security`, and the leaf crates (the leaf role in ADR-103, not the kernel's concern).

## 5. Domain logic inside `clawft-kernel`, called out

Kernel modules that are domain logic and not OS, ranked by how clearly they are domain:

1. `quantum_*` (2.7k): experimental stubs; no consumers found (row 5).
2. `tools_extended.rs` (1.4k): orphan, not compiled (row 12).
3. `sensor_graph/` (1.7k): behind a feature nothing enables (row 6).
4. `assessment/` and `mesh_assess.rs` (2.7k): includes Terraform and RabbitMQ analyzers (row 7).
5. `weaver.rs` (5.1k): codebase modeling (row 4).
6. Voice turn-taking (~3.7k): `talk_loop*`, `floor`, `duplex`, `thin_edge`, `coherence`, `view_resolver` (row 1).
7. Cognitum-shaped code (~5.2k): Seed adapter, `http_facade.rs`, `profile_store.rs`, `workload_pkg/cognitum.rs` (rows 8, 9). The host contract in `workload_runtime/host_contract.rs` is also Cognitum-shaped: `COG_CSI_BIND`, the ESP32 UDP port 5006, `COGNITUM_COG_TOKEN`.
8. `mesh_sensor.rs`, `lewm_invariant.rs` (rows 6, 24).
9. `app.rs` and `container.rs` (row 10).
10. `http_api.rs` (Paperclip, row 13).

Not domain logic, though it looks like it: `context_graft.rs` and `context_promote.rs` (1.3k) are agent memory over ECC (ADR-058), `impulse.rs` is the ECC inter-structure queue (its header says so; `floor.rs` merely reads it), `democritus.rs` and `cognitive_tick.rs` are the ECC loop. These stay with ECC.

Two shapes of coupling matter more than the line counts:

- **The `cog` kind is not an extension point.** `workload_pkg/manifest.rs:143` rejects any envelope whose kind is not `"cog"`, and `workload_ctl/plane_place.rs:183` always calls `cog_workload_spec`. ADR-099 s1 says kinds are open strings, but the code has one kind. Every new behavior (a staging gate, a deploy job, a `project` kind from ADR-103 D9, an inference server from ADR-101) would currently have to be added by editing kernel files. The inference kind is not implemented at all (`"inference"` appears only in tests and a fixture).
- **The cog host contract is a Cognitum sensor contract.** A running cog receives env vars and an ingest bridge that accepts vectors (`host_contract.rs:14-24`). It has no way to read a chain, subscribe to a topic, call a service or place a workload. A staging-gate cog cannot be written against this contract.

## 6. Missing primitives

M-numbers are referenced in the tables above.

| ID | Missing primitive | Evidence it is missing | What a cog needs from it |
|---|---|---|---|
| M1 | **Chain subscription and scoped append.** Push subscription to a chain (own chain and, per D6, `chain/<project-id>/`) filtered by kind prefix and from-sequence, with ADR-057 ACLs. Append scoped to the caller's principal with a kind namespace it owns (`cog/<id>/...`). | Readers poll: `ChainManager::tail_from` (`chain.rs:1334`), daemon `chain.tail` (`daemon.rs:6027`); `chain.rs` has no listener or channel. Writers use the tracing bridge (`clawft-weave/src/chain_bridge.rs`) or `chain.append` (`daemon.rs:6217`), which accepts any non-empty kind. | React to `candidate.ready`, `check.passed`, `promote`; record its own verdicts under its own kind. |
| M2 | **Per-instance capability grants.** A grant set issued on-chain to a workload instance and checked by the gate: subscribe topic X, read chain Y, call service Z, append kind prefix K, place kind W. | `AgentCapabilities` is four booleans plus `IpcScope` (`capability.rs:112-136`). Cog token is for the ingest bridge only (`host_contract.rs:22`). Substrate ACLs match on caller identity (`clawft-substrate/src/acl.rs:53`) but a workload instance has no such identity. | Least-privilege cogs; the gate can deny a fusion cog write access to a staging chain. |
| M3 | **Workload kind registry.** A `WorkloadKind` trait: manifest schema, spec-to-requirements, health meaning, allowed adapters; registered at boot or from a signed package. Kinds `service`, `job`, `project`, `inference`, `app` as the first users. | `manifest.rs:143`, `plane_place.rs:183` (section 5). | Add a behavior without editing the kernel. |
| M4 | **Cog-callable workload control.** A governed client for `workload.place|status|stop|logs` usable from inside a cog, with delegation limits, plus run-to-completion outputs (evidence to artifact plus chain). | The control plane exists for operators (`workload_ctl/`, `clawft-weave/src/workload_place_rpc.rs`); `RunEvidence` exists (`workload_runtime/evidence.rs`). Nothing exposes them to a running workload. `SwarmCoordinator` is in-process only. | Primary distributes to subordinates; deploy cog places a canary; check-runner returns evidence. |
| M5 | **Cross-instance link.** Admit a peer instance that has its own genesis, under a scoped signed claim, with read-only chain subscription and anchoring, tighten-only. | `GenesisMismatch` is defined (`mesh.rs:50`) but I found it raised nowhere except a display test (`mesh.rs:213`); `governance_genesis_hash` travels in the handshake (`mesh.rs:129`) but is not compared. Chain sync assumes a shared genesis (`mesh_runtime.rs:1463`). `coordination.link` (ADR-022, ADR-103) exists only in those two ADRs, not in code. | Staging reads dev's candidate events; production accepts only staging promotions; fusion consumes edge chains. |
| M6 | **Generic scoped delegation cert.** One certificate type scoping methods, workload kinds and node sets, with TTL and revocation, reusable by placement. | Two separate types: `exo_resource_tree::DelegationCert` and `clawft-delegation::WorkDelegationCert`, the latter typed to `weftos_worldmodel` methods (`clawft-delegation/src/lib.rs:99`). | "Primary distributes tasks to subordinates" as a signed, revocable grant. |
| M7 | **Chain-recorded attestations as governance preconditions.** A check result (subject hash, verdict, evidence hash, attester key) that a governance rule can require before permitting `promote`/`workload.place` into an instance. | Gate has Permit/Deny/Defer and `human_approval` (`workload_governance/gate.rs:55-67`, `environment.rs` prod preset). `AttestationRef` in a package manifest (`manifest.rs:216`) is a release-record file pointer, not a chain-time attestation. No `promote` action or chain kind; only `workload.migrate`. | The staging-gate cog produces the attestation; governance, not the cog, decides. |
| M8 | **Dynamic service RPC.** A service or cog registers method names with per-method gate action and the gateway/daemon routes to it (over mesh `ServiceAdvertisement` for remote ones). | Daemon RPC is a fixed `match` (`daemon.rs:5487` to `:8404`, default arm `unknown method`). Workload RPCs and `assess.*` were each added by editing that file. | A cog exposes `staging.status` or `fusion.query` without a kernel change. |
| M9 | **Event-triggered start.** "When chain event kind K or topic T fires, start workload W," next to cron. | Cron/timer only (`cron.rs`, `timer.rs`). | Deploy-on-promotion, check-on-candidate, without a polling cog. |
| M10 | **Token authority in the user role.** | Three stores (row P7); ADR-102 and ADR-103 direct this and it is not yet done. | Per-cog and per-project scoped tokens (`project_id` in `GatePrincipal`, ADR-103 P2). |
| M11 | **Stream QoS for cogs.** A declared rate, backpressure and latency class on topics used by voice (50 ms tick) and sensor frames. | `topic.rs:115` is a router; I did not find a QoS contract. Marked unverified in section 9. | Voice and fusion cogs meeting a deadline. |
| M12 | **`project_id` and instance id in `GatePrincipal`** (ADR-103 P2, D6, D8). | Not built (ADR-103 phases table). | Scope every grant and chain read to a project and instance. |
| M13 | **Governance default for unknown actions.** `CapabilityGate` still returns Permit for any action prefix it does not know (`gate.rs:139`). | Only `workload.*` was made default-deny. | New cog-facing actions (`promote`, `chain.subscribe`) must not be silently permitted; add explicit prefixes as they are introduced. |
| M14 | **Instance-state branch and rollback** as a first-class primitive. | `clawft-cow-memory` exists but is not wired to the kernel; the workload layer has no rollback beyond restart. | Speculative deploy, canary compare, rollback (section 7). |

M1, M2, M3, M5 are the load-bearing four. M4, M7 and M8 follow directly. The rest are smaller.

## 7. Mapping the owner's scenarios onto primitives and cogs

Shorthand: "core" means a primitive from section 2; a cog is a signed workload package (ADR-100) of some kind.

### Instance model

An organization has a **primary** instance holding the governance root. ADR-103 gives three software instances: dev (distributed), staging (a gate), production (deploy target). Each has its own genesis and chain. Promotion is a chain-recorded hand-off between instances. Mechanically this needs P1 (each instance a certified root), P2 (each chain), P3 (each instance's tighten-only rules), M5 (the link), M6 (the delegation grant) and M7 (attestations). Everything below is a cog on top.

### Development (distributed, many developers on one mesh)

| Cog | Kind | Job | Depends on |
|---|---|---|---|
| `dev-dispatch` | service | On the primary, watches ready work (board items, merged branches), decides where builds and tests run, places them | P4, M4, M9 |
| `build-runner` | job | Runs `scripts/build.sh` checks on the node the placer picks (ADR-099 phase A/B); returns `RunEvidence` and an artifact | P4, M3, M4 |
| `candidate-publisher` | job | On merge, packs and signs the artifact (`workload_pkg` pack/sign), appends `candidate.ready {artifact_hash, commit}` to the dev chain | P2, P4, M1 |

Kernel role: mesh admission, placement fan-out, project chains anchored into user chain (D6). Nothing here needs kernel code beyond M1-M4.

### Staging (a gate; small, deliberately undistributed)

| Cog | Kind | Job | Depends on |
|---|---|---|---|
| `staging-gate` | service | Subscribes to dev's `candidate.ready` (read-only cross-instance), verifies package signature and signer trust (`workload_pkg/verify.rs`), orders the checks, collects evidence, appends the attestation set, proposes promote or reject | M5, M1, M7, M2 |
| `check-runner` | job | The checks the user's global rule requires before a commit: type-check, lint, build, container build. Pinned to the staging node; evidence to artifact store | M3, M4 |
| `assess` | job | Today's `assessment/` analyzers (row 7) run as a check | M3, M4 |
| `intel` | service | ECC and graphify over the history of candidates: risk score, flaky-check detection, change blast radius; publishes a score the gate may cite | ECC service, M1, M8 |

"Undistributed" is a policy, not code: staging's governance overlay restricts `workload.place` to nodes of one class and one trust tier (existing permit rules in `workload-permits.json`). The decision is made by governance (P3) requiring the attestation (M7) and, per `environment.rs`, human approval for high risk. The gate cog only proposes. **No kernel change beyond M5 and M7 makes staging a gate.**

### Production (fed only by staging promotions)

| Cog | Kind | Job | Depends on |
|---|---|---|---|
| `deploy` | job | Accepts a promotion only when it carries staging's attestation under a `DelegationCert` from the primary (M6); places the package with `workload.place`; canary, health watch, rollback on failure | M5, M6, M7, M4, M14 |
| `prod-watch` | service | Health and drift; appends incident events; can request rollback | M1, M2 |

Production's overlay is tighter than staging's (D8, tighten-only): denies any `workload.place` not accompanied by a valid promotion attestation, and requires human approval (the prod preset in `environment.rs` already encodes risk 0.3 and `human_approval_required`).

### Sensor fusion

| Cog | Kind | Job | Depends on |
|---|---|---|---|
| `edge-publish` (leaf/edge) | leaf | Signed `mesh.sensor.v1.*` frames from ESP32 and edge nodes (`weftos-sensor-pipeline-wire`) | leaf provisioning (ADR-103 leaf phase) |
| `fusion` | service | `weftos-sensor-pipeline` (collect, aggregate, encode) plus `clawft-worldmodel-service`; placed by ADR-099 with feed locality (`feed.*`); consumes edge chains or topics; publishes fused state to its own chain | M1, M2, M3, M5, M11 |
| `fusion-standby` | service | The existing hot-standby and peer-to-peer topologies of the world-model service; offload under `WorkDelegationCert` | M6 |
| Downstream consumers | any | Subscribe to `chain/<fusion-id>/` under ADR-057 ACLs | M1 |

This is the scenario closest to done: the crates exist and have no kernel dependency. What is missing is a kind for a long-running service (M3), grants so `fusion` can read edge topics but nothing else (M2), and a way to consume another instance's chain (M5). The kernel-side `mesh_sensor.rs` topics have no consumer yet; the fusion cog would own them.

## 8. Top findings

1. The cog layer is one kind deep: `cog` is hard-coded (`workload_pkg/manifest.rs:143`, `workload_ctl/plane_place.rs:183`), so no new behavior can be a cog without editing the kernel.
2. Cogs cannot read or subscribe to chains. Readers poll `chain.tail`; there is no push, and writers are unscoped (`chain.rs:1334`, `daemon.rs:6027,6217`).
3. `environment.rs` models dev/staging/prod as switchable scopes inside one instance (`set_active`, `:312`), which is the opposite of ADR-103's instance-per-environment; it is also never constructed by `boot.rs`. Keep the presets, drop the switch.
4. Cross-instance federation has no code: `GenesisMismatch` is never raised (`mesh.rs:50`), chain sync assumes shared genesis (`mesh_runtime.rs:1463`), and `coordination.link` exists only in ADRs.
5. The cog host contract is a Cognitum sensor contract (UDP 5006, vector ingest), so a staging-gate cog cannot be written against it (`host_contract.rs:14-24`).
6. About 20k lines of the kernel are domain logic, led by weaver (5.1k), assessment (2.7k), voice turn-taking (3.7k) and Cognitum-shaped code (5.2k). Most is safe to leave until M1-M3 exist.
7. Safe and cheap now: `tools_extended.rs` (1.4k) is not declared in `lib.rs` and is not compiled; `quantum_*` (2.7k) has no consumers; `sensor_graph/` sits behind a feature nothing enables.
8. Voice is about 40k lines in six places, two of them trait crates (`clawft-channels`, `clawft-plugin`). Gathering it under one owner is a no-behavior-change first step.
9. The sensor, world-model, splat and sonobuoy crates already have no kernel dependency: they are the model for the rest, and what blocks them from being placed cogs is finding 1, not their code.
10. Two duplicated systems will diverge if both grow: cron (`cron.rs` and `clawft-services/src/cron_service`) and app-versus-workload packaging (`app.rs`, `clawft-app` versus `workload_pkg`). Also a second task distributor, `SwarmCoordinator`, is unaware of ADR-099 placement.

## 9. Could not verify

- I did not run any build, test or feature-matrix check. All "not compiled", "no consumer" and "no in-tree consumer" claims come from `grep` over `crates/`, `lib.rs` module lists and `Cargo.toml` feature tables. A dependency through a path I did not grep (examples, benches, `archive/`, non-`.rs` build scripts) would change a verdict. `crates/archive` had no Cargo.toml and was not inspected.
- Line counts are `wc -l` sums including test files in the same module; they overstate production code for the placement layer, which has many `tests_*.rs`.
- I read module headers and signatures, not full bodies, for most kernel modules. Verdicts on `weaver.rs`, `assessment/`, `http_facade.rs`, `profile_store.rs`, `talk_loop*.rs` rest on headers, `boot.rs` wiring and call sites, not on line-by-line reads.
- Whether `EnvironmentManager` is unused: I searched for the type name across `crates/` and found it only in `environment.rs` and `lib.rs`. Something could construct it through the `weftos` facade re-exports under another path.
- M11: I did not find a QoS or backpressure contract on `TopicRouter`, but did not read `topic.rs` beyond its header and type list. It may exist.
- I did not verify that the gateway or MCP tokens and `auth_service.rs` are unrelated at runtime, only that they are three separate stores in code.
- ADR-101 (inference) I read to line 40 only. I did not check whether any inference adapter exists beyond the fixtures noted.
- `clawft-core` (72k lines) I classified by directory names and `agent_bus/coordinator.rs`; I did not audit its 31.6k-line agent module for domain logic that could be a cog (skills, learning, soul journal are candidates).
- `clawft-gui-egui`, `clawft-surface`, `clawft-canon`, the leaf crates and firmware crates were classified from Cargo descriptions only.
- Repo state notes (WEFT-615 open, the exact `talk_loop` toggle default) come from memory notes and one code reference, not from a fresh run.
- I did not check other agents' work in this session (daemon-topology, install-review, p1-planner); the daemon-topology analysis (`docs/research/daemon-topology/analysis.md`) was not read, so where it already answers M5 or M12 this audit may duplicate it.
