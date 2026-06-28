# Brain · 02 — Release History & Shipped Features

> What actually exists in the codebase now. Source-of-truth: `CHANGELOG.md`,
> `git log`, `docs/handoff.md`, `.planning/development_notes/`.

## 1. Release history (tagged on `master`)

CHANGELOG covers through **v0.6.19** (2026-04-22); 0.7.0/0.8.x are in flight,
not yet tagged.

| Version | Date | Headline |
|---|---|---|
| 0.1.0 | 2026-02-17 | 9-crate workspace; agent loop; 6-stage pipeline; Telegram/Slack/Discord; WASM stubs; 1,029 tests |
| 0.2.0 | 2026-03-31 | Block engine (later retired), theming, GEPA prompt evolution, local LLM provider, context compression |
| 0.3.0 | 2026-03-31 | GUI integration (KernelDataProvider), Paperclip patterns (Heartbeat, GoalTree, OrgChart), HTTP API, wasm32-wasip2 |
| 0.4.0 | 2026-04-03 | WASM sandbox, AssessmentService + 5 analyzers, daemon-first CLI (32 cmds), clawft-rpc, ADR-020..047 |
| 0.4.1–0.4.3 | 2026-04-03/04 | Pluggable AnalyzerRegistry; full boot in ExoChain; MeshCoordinator gossip; Docker 30→2 min, 50→15 MB |
| 0.5.0–0.5.5 | 2026-04-04 | wasmtime v33, ServiceApi, clawft-graphify (11,896 LoC), benchmark v3, ESP32 edge bench, VectorBackend (HNSW/DiskANN/Hybrid) |
| 0.6.0–0.6.5 | 2026-04-04 | Cognitum Seed gap sprint, tiered profiles T0–T4, EML coherence O(1) (~100ns vs ~500µs Lanczos), eml-core standalone, 66 ExoChain gaps closed |
| 0.6.6–0.6.11 | 2026-04-14/16 | 18 KG tasks (MCTS, causal tracing, RFF spectral, dedup), Quantum Cognitive Layer (Pasqal), EML Attention iters 0–2, vault cultivation |
| 0.6.12–0.6.17 | 2026-04-17 | Universal Topology Browser (Reingold-Tilford + Barnes-Hut, VOWL export), Adaptive HNSW, MeshClockSync, Noise XX end-to-end, leaf push protocol |
| 0.6.18 | 2026-04-19 | graphify ingest/query schema fix ("edges" vs "links") |
| 0.6.19 | 2026-04-22 | M1.5 App Layer Trilogy (clawft-app/substrate/surface), 21 canon UI primitives, Admin app end-to-end, StreamWindowCommit (BLAKE3), 8 EML-Swap wirings |

## 2. The 10 commit waves

0. **Foundation** (02-17→03-31, v0.1–0.3): agent loop, pipeline, 3 channels.
1. **Assessment + WASM + Graphify** (04-03/04, v0.4–0.5.1): WASM sandbox,
   daemon-first CLI, clawft-graphify.
2. **EML + Coherence** (04-04, v0.5.2–0.6.5): eml-core, DEMOCRITUS O(1)
   coherence, 66 ExoChain gaps.
3. **KG tasks + Quantum + Topology** (04-14/17, v0.6.6–0.6.17): 18 KG tasks,
   Pasqal backend, Topology Browser, Noise XX, MeshClockSync.
4. **M1.5 App Layer** (04-22, v0.6.19): app/substrate/surface trilogy, 21
   primitives, StreamWindowCommit.
5. **M4 multi-agent + GUI hardening** (late Apr): AgentRouter + per-agent
   runtime (WEFT-178/180/184), MCP allowlist, real Claude Code spawn.
6. **M5 voice security + browser tests** (late Apr): voice Level 0/1/2 gate
   (WEFT-557), mic privacy indicator, cost circuit-breaker (WEFT-322).
7. **M6 release-pipeline / 0.7.0-ready** (late Apr): Ed25519 skill keygen
   (WEFT-23), per-method capability gate + ipc_tcp auth (WEFT-479/481), sigstore.
8. **M7 sweeps + agent-core-v1** (04-27→05-01): HybridRouter v2.5, EmbeddingRouter
   v2, LlmClassifierRouter v1, soul promote; Cmd+K palette, Tauri 2.0, PWA.
9. **0.8.0 desktop wave + graduation** (05-01/02): design system v0.1, canonical
   sidebar, tray retired, 13 app graduations (WEFT-579..591).
10. **Vector-first leaf-display pivot (UNCOMMITTED)** (05-14→05-17): see §5.

## 3. Implemented feature inventory (by subsystem)

- **Kernel**: phased boot in ExoChain, tiered profiles T0–T4, cluster peer
  persistence, KernelIpc 16 MiB cap + idempotency replay protection, TokenStore +
  revoke gate, optional ipc_tcp TCP relay, DEMOCRITUS cycle detection, treecalc,
  weave.toml + config.json deep merge, workspace overlay.
- **EML / learnable models**: eml-core (depth 2–5, multi-head, coordinate
  descent, zero-dep); 12+ production models (governance scorer, restart strategy,
  health threshold, dead-letter, gossip timing, complexity, tick interval, edge
  decay, HNSW 4-model suite); ToyEmlAttention SafeTree (7.3% MSE gate pass).
- **LLM / agent core**: clawft-service-llm (OpenRouter, tool-call wire format),
  clawft-service-agent (AgentLoop, EffectGate, per-tool EffectVector check),
  AgentRouter + delegation depth guard, EmbeddingRouter v2 / LlmClassifierRouter
  v1 / HybridRouter v2.5, cost circuit-breaker, Ed25519 skill signing + sigstore,
  `.clawft/SOUL.md` + `weaver soul promote`.
- **Apps / desktop shell**: canonical sidebar + 12-module dispatch, design system
  v0.1 (`bg_sidebar` token, DESIGN.md contract test, audit ratchet CI), 13
  graduated apps (Files/Processes/Services/Network/Logs/Settings/Scheduler/
  Monitor/Terminal/Chat/Admin/Explorer/Apps launcher), tray retired, Tauri 2.0
  scaffold, `weaver` multi-subcommand binary.
- **clawft-ui (web)**: Cmd+K palette (WEFT-308), PWA + offline shell (WEFT-311),
  single-use URL token auth, Playwright E2E (WEFT-314), jsx-a11y + bundle-size CI
  gates, multi-stage Dockerfile, WS heartbeat + dead-conn eviction, ADR-055
  BackendAdapter.
- **Mesh / chain**: Noise XX 25519 end-to-end, MeshClockSync (GPS>TSF>NTP>Mesh>
  Local, <1µs target), K6 mesh transport, StreamWindowCommit (BLAKE3 rolling
  window), ExoChain mesh audit trail, MessagePayload::Binary, agent.register
  signed envelopes, ipc.subscribe_stream.
- **Channels (real I/O)**: Email (IMAP+SMTP, WEFT-154), WhatsApp Cloud API
  (157), Google Chat Pub/Sub (155), Teams Bot Framework (156), Signal signal-cli
  (158), IRC TCP/TLS (160), voice substrate STT+TTS (164), Discord/Slack/Telegram
  fixes. **Note**: the 0.7.0 audit found 7 of 11 adapters were still stubs at
  audit time — see [`04`](04-bugs-gaps-and-current-state.md).
- **Voice / substrate / sensors**: clawft-service-whisper (integrity + audit),
  voice Level 0/1/2 gate (WEFT-557), mic privacy indicator (WEFT-207),
  AudioClassifier VAD stage, substrate RPCs (read/subscribe/publish/notify),
  MicrophoneAdapter + healthcheck contract, PhysicalSensorAdapter trait,
  Network/Bluetooth adapters, ui://heatmap/waveform/media/canvas.
- **egui GUI**: compiles to wasm32-unknown-unknown, VSCode `weft-panel`, 21 canon
  primitives, Explorer (shape-dispatched viewers, sparklines), Chat (markdown,
  heartbeat, identity drift), Terminal (alacritty-backed), Workshop, canon demo
  lab.
- **Graphify / topology**: Universal Topology Browser, `weaver topology`
  subcommands, clawft-lsp-extract (rust-analyzer/tsserver/gopls/pylsp), 18 KG
  tasks, 7R disposition enum, `weaver vault` cultivation.
- **Security**: MAESTRO prompt-injection defense (`sanitize_llm_input` at 7
  paths), MCP allowed-tools allowlist, per-method capability gate + ipc_tcp auth,
  CORS deny + CSP middleware, cargo audit CI gate, sigstore default-on.
- **Edge / sensors**: ESP32-S3 RGB DPI panel bring-up (CrowPanel 7" 800×480),
  GT911 touch driver, `weftos-leaf-types` no_std CBOR schema.

## 4. WEFT ticket map (selected)

WEFT-16 strict mcp tool-name validation · WEFT-23 Ed25519 skill keygen · WEFT-98/
102 token revoke gate · WEFT-103 idempotency replay protection · WEFT-104 cargo
audit CI · WEFT-130 K1 ACL engine scaffold · WEFT-143 KernelIpc 16 MiB cap ·
WEFT-154–164 real channel I/O · WEFT-178/180/184 AgentRouter + delegation guard ·
WEFT-207 mic privacy indicator · WEFT-242 supersede Tauri/React/xterm ADRs ·
WEFT-260–262 terminal selection/glyphs/scrollback · WEFT-268–276 Explorer
viewers · WEFT-300 WS heartbeat eviction · WEFT-306–315 clawft-ui wave (Cmd+K,
PWA, Tauri, Playwright) · WEFT-322 cost circuit-breaker · WEFT-432–439 sensor
healthcheck + adapters · WEFT-479/480/481/487 capability gate + MCP filter +
ipc_tcp auth · WEFT-555/556/557 voice STT consumer + gate + Level flags ·
WEFT-579–591 13 app graduations.

## 5. Recent session narrative (the "chat history")

**Session A — WEFT-579..591 graduation + 0.8.0 desktop (05-01/02, COMMITTED)**:
0.8.0 desktop wave shipped Phases 0–5 from a worktree, merged at `b6c6e46f`
(design system, `bg_sidebar` token, canonical sidebar + 12 stubs, audit ratchet,
tray retired). On 05-02, 13 app-graduation commits (`d65bc2ea`→`1f5c05a5`) merged
in 4 batches. Post-graduation defect fixes: tofu icons → Dingbats, `service.*`
panel proxy whitelist, DejaVuSans fallback, real RSS in kernel.ps, ServiceInfo
metadata. HEAD = `7475ef99`.

**Session B — Inkpad LCD + touch bring-up (05-14, UNCOMMITTED)**: CrowPanel 7"
ESP32-S3 hardware spike. LCD RGB DPI brought up (gotchas: `next_frame_en=true`
required; `core::mem::forget(transfer)` blocks espflash auto-reset → 3s grace
window). GT911 touch fully reverse-engineered (addr 0x5D not 0x14, PCA9557 I/O
expander routes RST, factory config version `0xFF` is valid, spurious
`commit_config()` was the bug). Multi-point touch + drag confirmed.

**Session C — Vector-first leaf-display pivot (05-17, UNCOMMITTED)**: The pivotal
decision. After 11 iterations patching a hand-rolled raster DPI driver, the
raster compositor was thrown out. Trigger: factory `.bin` rendered clean, so the
integrated compose-during-scan path (full 800×480 framebuffer rewrite per push)
was the tearing cause. New architecture = retained-mode vector scene graph + CBOR
wire + damage-rect rendering, shipped build-clean in 5 phases:
- **Phase A** `weftos-leaf-scene` (no_std, 112 tests): Scene/Node/Primitive,
  SceneStore, DamageSet (8-rect budget, 50% → full repaint), codec (CBOR).
- **Phase B** `weftos-leaf-renderer` + `weftos-leaf-sim` (74 tests): SceneSurface
  trait, render_damage(), LRU GlyphCache, rgb565_be.
- **Phase C** `clawft-edge-pad` DpiSurface over proven `lgfx-bus-rgb-rs` v0.2.1.
- **Phase D** `weftos-leaf-canvas` (wasm): Canvas2D backend, full capabilities.
- **Phase E** `weftos-scene-builder` (19 tests) + `weftos-leaf-touch-gt911` (6
  tests); `weaver leaf scene` CLI; `clawft-edge-pad/mesh.rs` rewritten to decode
  SceneEnvelope → store.apply → render_damage → present.

Hardware confirmed end-to-end (`[mesh] APPLY display=0 ops=1 ... drawn=9`).
**Residual gap**: gutter coords don't land visibly despite correct wire format;
prime suspect is `lgfx-bus-rgb-rs` v0.2.1 double-buffer swap presenting the wrong
buffer. Cheap disambiguation (disable `double-buffer` feature) **not yet run**.
The entire subsystem is uncommitted — see [`04`](04-bugs-gaps-and-current-state.md)
§4 for the recoverability hazard.
