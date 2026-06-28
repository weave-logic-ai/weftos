# Brain · 03 — Architecture & ADRs

> System structure and the decisions behind it. Source-of-truth: `crates/`,
> `docs/adr/`, `docs/architecture/overview.md`, `docs/DESIGN.md`, `Cargo.toml`,
> `weave.toml`.

## 1. Crate map (44 crates by layer)

**Kernel / core**: `clawft-types` (zero-dep foundation: errors, config, messages,
15-provider registry) · `clawft-platform` (Platform trait: http/fs/env/spawn) ·
`clawft-llm` (provider abstraction, ProviderRouter, 11 providers) · `clawft-core`
(agent loop, MessageBus, 6-stage pipeline, SessionManager, ToolRegistry) ·
`clawft-kernel` (OS kernel: boot SM, ProcessTable, ServiceRegistry, KernelIpc/A2A,
RBAC, HealthSystem, GovernanceEngine, ContainerManager, AppManager) ·
`exo-resource-tree` (Merkle resource namespace, K0+K1) · `weftos` (product facade,
`WeftOs::boot_default()`).

**Services**: `clawft-services` (cron, heartbeat, pluggable MCP server/client,
delegation) · `clawft-service-agent` (daemon AgentLoop wrapper + cancellation) ·
`clawft-service-classify` (audio VAD stage) · `clawft-service-llm` (local
llama.cpp client) · `clawft-service-terminal` (PTY allocator) ·
`clawft-service-whisper` (STT pipeline).

**Channels**: `clawft-channels` (Channel trait + plugin host; Telegram/Slack/
Discord built-in, 8 more adapters).

**Security / chain**: `clawft-security` (50+ audit checks, `weft security scan`).

**Apps / surface (UI host)**: `clawft-app` (TOML manifest + JSON registry) ·
`clawft-substrate` (OntologyAdapter, StateDelta tree) · `clawft-surface` (surface
IR + binding evaluator + composer) · `clawft-gui-egui` (egui native + WASM shell)
· `clawft-weave` (`weaver` CLI lib, daemon, RVF wire bridge).

**Leaf / display (vector-first, out-of-workspace)**: `weftos-leaf-types` (no_std
CBOR wire) · `weftos-leaf-scene` (scene graph + damage + hit-test) ·
`weftos-leaf-renderer` (SceneSurface trait) · `weftos-scene-builder` (fluent
builder + diff) · `weftos-leaf-canvas` (Canvas2D WASM) · `weftos-leaf-sim`
(desktop sim) · `weftos-leaf-touch-gt911` (GT911 driver) · `weftos-leaf-display`
(deprecated raster, kept for IDF path-dep).

**Edge / firmware**: `clawft-edge-pad` (bare-metal ESP32-S3 embassy) ·
`clawft-edge-pad-idf` (ESP-IDF port) · `clawft-edge-bench` (benchmark harness).

**RVF / intelligence**: `eml-core` (EML universal fn approx `exp(x)−ln(y)`) ·
`clawft-treecalc` (Form triage Atom/Sequence/Branch) · `clawft-casestudy-gen-qsr`
(QSR synthetic corpus for ECC scenarios).

**Tooling / analysis**: `clawft-tools` (built-in tools, path containment) ·
`clawft-plugin` (6 extension traits, WasmHost, SkillLoader, hot-reload) ·
`clawft-plugin-treesitter` (AST analysis) · `clawft-rpc` (RPC types + DaemonClient)
· `clawft-graphify` (knowledge graph builder) · `clawft-lsp-extract` (LSP semantic
extraction) · `clawft-cli` (`weft` binary) · `clawft-wasm` (wasm32-wasip2 subset).

## 2. Kernel K-level model (ADR-048 = formal responsibility map)

| K | Name | Subsystems → crates |
|---|---|---|
| K0 | Boot + lifecycle | state machine, config, health → `clawft-kernel` boot.rs, `exo-resource-tree` |
| K1 | Process + supervision | PID, ProcessTable, Supervisor, RBAC → `clawft-kernel` process/supervisor/capability.rs |
| K2 | IPC + comms | MessageBus, A2A, cron, service registry → `clawft-kernel` ipc/topic.rs, `clawft-services`, `clawft-core` bus.rs |
| ExoChain | Crypto audit | SHAKE-256 chain, Merkle namespace, dual sign → `exo-resource-tree`, kernel exochain feature |
| K3 | WASM sandbox | wasmtime, fuel, epoch, host fns → `clawft-kernel` wasm_runner.rs, `clawft-plugin` WasmHost |
| K3c | ECC cognitive | CausalGraph, HNSW, ImpulseQueue, DEMOCRITUS, CrossRef → kernel ECC modules, `clawft-treecalc`, `eml-core` |
| K4 | Containers | Docker/Podman lifecycle (bollard) → `clawft-kernel` container.rs |
| K5 | App framework | manifest parse/validate, lifecycle, fleet spawn → `clawft-kernel` app.rs, `clawft-app` |
| K6 | Mesh | Noise/QUIC/Kademlia/SWIM/mDNS/LWW-CRDT → kernel mesh, `clawft-weave`, leaf crates |
| K7 | Cognitive sync | cross-node structure sync → planned (ADR-026) |
| K8 | GUI | egui native+WASM, composer, theming → `clawft-gui-egui`, `clawft-surface`, `clawft-substrate` |

## 3. ADR index (001–057)

| ADR | Decision · rationale |
|---|---|
| 001 | Lockstep semver for all crates · avoids cascade-bump overhead |
| 002 | cargo-dist for releases · 5+ targets + Homebrew + installers |
| 003 | CodeMirror 6 not Monaco · 150 KB vs 2.5 MB |
| 004 | CSS Grid + custom engine, no Dockview · avoids competing layout systems |
| 005 | ~~xterm.js console~~ **superseded** by egui shell (WEFT-242) |
| 006 | Custom block renderer · kernel + governance integration needs full control |
| 007 | ~~Zustand + Tauri events~~ **superseded** by substrate RPCs (WEFT-242) |
| 008 | WeftOS cloud-side for Mentra · BES2700 8 MB PSRAM too small for 50–200 MB runtime |
| 009 | Sparse Lanczos for ECC spectral · dense O(k·n²) breaks tick budget at 10K nodes |
| 010 | Keep Tokio, not Asupersync · rewrite cost unjustified |
| 011 | No FrankenSearch, raw HNSW · <10K entries, BM25 doubles memory |
| 012 | Inline sha3, drop rvf-crypto path dep · path dep broke standalone build |
| 013 | ~~JSON block descriptor~~ **superseded** by surface IR (WEFT-242) |
| 014 | Fumadocs as single docs source · 38+87 files were drifting |
| 015 | Three-property web (buyers/devs/clients) · one site dilutes audiences |
| 016 | Multi-target theming (token IR, 8+ targets) · React/terminal/HUD/voice/MCP/PDF |
| 017 | GEPA prompt evolution in learner.rs · self-improvement flywheel |
| 018 | Hermes models as provider, not framework · only open weights useful |
| 019 | `Registry` trait in clawft-types · 15 registries, GUI needs generic browse |
| 020 | `ChainLoggable` trait · all state changes auditable |
| 021 | All CLI routes through kernel daemon RPC · thin client (ADR-048) |
| 022 | All state changes log to ExoChain · single source of truth |
| 023 | Assessment as kernel SystemService · governed, mesh-visible |
| 024 | Noise Protocol (snow 0.9) for inter-node · forward secrecy + mutual auth |
| 025 | Ed25519 pubkey as node identity (SHAKE-256 hash) · ties mesh+chain+capability |
| 026 | QUIC (quinn) primary, WS fallback · multiplexed, 0-RTT, hole-punch |
| 027 | Selective libp2p (kad+mdns only), no Swarm · avoids framework runtime |
| 028 | Dual sign Ed25519 + ML-DSA-65 (FIPS 204) · harvest-now-decrypt-later |
| 029 | weftos-rvf-crypto as crates.io fork · removes path dep |
| 030 | CBOR (ciborium) canonical for ExoChain · deterministic reproducible hashes |
| 031 | rvf-wire zero-copy segments as mesh wire format |
| 032 | DashMap for registries; Mutex only for Chain/Tree managers · deadlock-free reads |
| 033 | Three-branch governance (Legislative/Executive/Judicial) |
| 034 | Five-dim `EffectVector` scoring · composable gate decisions |
| 035 | Layered `ServiceApi` trait · transport-decoupled service logic |
| 036 | Hierarchical ToolRegistry (kernel base + per-agent overlay) |
| 037 | Rust Edition 2024 + MSRV 1.93 |
| 038 | ~~Tauri 2.0 desktop shell~~ **superseded** by egui shell (WEFT-242) |
| 039 | SWIM (HeartbeatTracker) for failure detection |
| 040 | LWW-CRDT for distributed process table |
| 041 | `ChainAnchor` trait · chain-agnostic anchoring |
| 042 | Three ECC modes: Act / Analyze / Generate · shared infra |
| 043 | BLAKE3 for new ECC; SHAKE-256 for ExoChain until K6 |
| 044 | wasm32-wasip2 build target · component model |
| 045 | `TieredRouter` = permission + complexity + cost |
| 046 | CMVG as forest of domain structures, not one graph |
| 047 | Self-calibrating cognitive tick · adapts to node capacity |
| 048 | Formal K0–K6 responsibility map |
| 049 | `clawft-kernel` composes primitives into `Kernel<P>` · additive |
| 050 | CONS-003 escalation resolved (FIX-04 + FIX-06) |
| 051 | Archive 8 orphaned clawft-plugin-* crates → crates/archive/ |
| 052 | Two distinct ToolRegistry types (core vs kernel) · don't unify |
| 053 | whisper + classify as canonical voice STT path |
| 054 | claude-flow stays user-installed, not first-party |
| 055 | `BackendAdapter` trait · single dashboard integration seam |
| 056 | BVH-on-RVF spatial-temporal index over ECC graph (`clawft-bvh`, planned) |
| 057 | Substrate per-path read ACLs · MUST-HAVE gate for 0.8.x |

**ADR hygiene debt**: 003/005/007/013/038 not marked superseded; two ADR-020s
and two ADR-028s share numbers (collisions).

## 4. Key architectural patterns

- **Three-branch governance** (ADR-033/034): every action scored by an
  `EffectVector` (5 dims); Legislative allows, Judicial reviews, Executive gates.
  `weave.toml` sets env=development, risk_threshold=0.9.
- **ExoChain** (ADR-022/028/030): append-only, CBOR canonical, dual-signed
  (Ed25519 + ML-DSA-65), SHAKE-256→BLAKE3 migration; `exo-resource-tree` Merkle
  namespace underneath.
- **Mesh** (ADR-024–027/039/040): QUIC primary + WS fallback over Noise XX/NK;
  Kademlia + mDNS discovery (no libp2p Swarm); SWIM failure detection; LWW-CRDT
  process table; node-id = SHAKE-256(ed25519 pubkey)[0..16].
- **ECC cognitive substrate** (ADR-042/046/047/056): forest of domain structures
  (CausalGraph DAG, HNSW, CrossRef, ImpulseQueue); DEMOCRITUS loop on a
  self-calibrating tick; eml-core O(1) learned functions.
- **Leaf-display vector scene graph** (Phase A–E): kernel publishes CBOR
  SceneEnvelope to `mesh.leaf.<pk>.push`; touch returns InputEnvelope; crates
  excluded from main workspace for embedded toolchain isolation.
- **Substrate / surface IR** (ADR-016/017): OntologyAdapter producers publish
  StateDeltas; surface IR binding mini-language evaluates against snapshots; 23
  canon `ui://` primitives; affordances → PendingDispatch → `Live::submit`; ADR-057
  per-path read ACLs.
- **6-stage pipeline**: TaskClassifier → ModelRouter → ContextAssembler →
  LlmTransport → QualityScorer → LearningBackend; tool loop ≤ max_tool_iterations.

## 5. Config surface (`weave.toml`)

`[kernel]` max_processes=64, health interval=30s · `[tick]` interval_ms=50,
budget_ratio=0.3, adaptive=true (DEMOCRITUS) · `[embedding]` mock-sha256, 384-dim
· `[governance]` env=development, risk_threshold=0.9 · `[kernel.mesh]` tcp,
0.0.0.0:9470, noise=false (ESP32 bring-up) · `[kernel.ipc_tcp]` 127.0.0.1:9471
(loopback-only without bearer) · `[kernel.llm]` local llama.cpp http://127.0.0.1:8111,
gemma-iq2m (overrides OpenRouter) · `[kernel.agent]` anchor_chain/hnsw/causal=true
(each chat turn → witness tick + HNSW entry + causal node).
