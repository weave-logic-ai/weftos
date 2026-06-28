# Brain · 04 — Bugs, Gaps & Current State

> Known defects and live working-tree state. Source-of-truth: `.planning/reviews/`,
> `.planning/sonobuoy/GAPS.md`, `docs/handoff.md`, `git status`. Compiled 2026-06-28.

## 1. Known bugs / defects

| ID | Severity | Summary | Status |
|---|---|---|---|
| BUG-1 | HIGH | Leaf-display: gutter coords land in wire (Q24.8 x=12800=50px confirmed, drawn=9) but panel doesn't move. Prime suspect: `lgfx-bus-rgb-rs` v0.2.1 double-buffer swap presents wrong buffer. | Open — cheap fix (disable `double-buffer` feature) NOT yet run |
| BUG-2 | MEDIUM | Tofu sidebar icons | Believed fixed at HEAD (`912e9394`, Dingbats), not re-verified on dirty tree |
| BUG-3 | HIGH | ExoChain tracing→ChainManager bridge missing in daemon (`clawft-weave/main.rs`): 12 chain events from non-kernel crates hit stdout, never reach ExoChain | NOT fixed (RSS + chain.tail mapping fixed separately) |
| BUG-4 | LOW | clawft-rpc: 2 env-bound test flakes when a weaver daemon is already running | Pre-existing |
| BUG-5 | LOW | 2 `embedding_onnx` wordpiece tests flake under parallelism (1937/1939 pass) | Pre-existing |
| BUG-6 | LOW | DEMOCRITUS "Stuck { net_change: 0.0 }" spam on idle/empty causal graph — expected but undocumented | Hardened v0.6.19; doc bug |
| **BUG-7** | **CRITICAL** | `auth_service.rs` `rotate_credential` (L325) + `request_token` (L354) chain-log but **do not gate on governance approval** — only CRITICAL FAILs in the 21-item audit | **Open** since 2026-04-28, no fix on this branch |
| BUG-8 | MEDIUM | `.env` shadows `[kernel.llm]`: dotenvy loads LLM_SERVICE_URL/MODEL before weave.toml; stale values silently win; boot log doesn't say which layer won | Not fixed |
| BUG-9 | MEDIUM | EML `from_causal_graph` is O(n+m) (calls connected_components), not advertised O(1) | Open |

## 2. Audit & review findings

**Phase 3 review cycle (3d–3i, consensus.md)** — all 7 P0s resolved 2026-02-17
(ruvector Sprint-0 validation PASS with 4 plan corrections; pluggable ToolProvider
MCP architecture; 3-level discovery chain; WASM cdylib baseline 57.9 KB/24.3 KB
gzip; 3I re-baselined 12→8 P0s; 3D deferred to Phase 4). Open P1 items remained
in opt-level mismatch, YAML skill security, normalize_keys before merge, MCP
allowed_tools access control, SSE streaming.

**0.7.0 release-gate audit (2026-04-28, 17 workstreams)** — key CRITICAL/HIGH
findings, all open at audit time:
1. 7 of 11 channel adapters were stubs (email/google_chat/teams/whatsapp/signal/
   matrix/irc returned synthetic IDs) — *partially closed by WEFT-154..164 since*.
2. W-BROWSER pipeline not wired (browser_entry bypasses the 6-stage pipeline).
3. Multi-agent WS-07: 14/14 "done" but type-level scaffolding only; FlowDelegator
   never created; AgentRouter stored but not consulted; no recursive-delegation
   guard — *partially closed by WEFT-178/180/184 since*.
4. Voice WS-10: 5 P0 security controls unimplemented; no mic→TTS path — *closed by
   M5 / WEFT-555..557 since*.
5. Internal-dep drift: workspace 0.6.19 but path-deps pinned 0.6.6; stale
   `ghcr.io/clawft/clawft` paths.
6. KG-011/012 blocked on ruvllm-wasm 11-pattern HNSW cap; DiskANN backend is a
   HashMap linear-scan stub.
7. JEPA/LeWM world-model: 7 crates don't exist on master; ADRs 048–058 greenfield.
8. Auth middleware exists but NOT wired into the Axum router (D-7).

**cargo audit (2026-04-28)** — 21 finding rows / 18 advisory IDs, all transitive:
Wasmtime 33.0.2 → 15 advisories incl. aarch64 Cranelift sandbox escape
(RUSTSEC-2026-0096) and Winch sandbox escape (-0095) [WEFT-551]; rustls-webpki
name-constraint bypass + CRL panic [WEFT-552]; unmaintained bincode/instant/paste/
rustls-pemfile/serial + unsound rand [WEFT-553]. All in `--ignore` list; gate stays
green; deferred to 0.8.x by design.

## 3. Gaps

**Sonobuoy GAPS.md** — all 5 research gaps (G1–G5) CLOSED by 2026-04-15
(sensor-position uncertainty → OWTT ranging ADR-078; Helmholtz-PINN → XPINN/PINO
ADR-079; FNO thermocline → ThermoFno ADR-080; multistatic SAS → VAE-prior ADR-079b
parked; federated learning → 5-layer codec ~210 B/buoy ADR-080b/090). G6 (Perch 2.0
weights) + G7 (Closure-SDK AGPL) tracking. No Rust crate scaffolded yet.

**Kernel phase completion** (k0-k5-final-gap-analysis, 2026-03-25): K4 86.7%
(2 criteria), K5 94.1% (1 criterion), K6 133 tests but K6.4/K6.5 sub-phases
(chain replay, tree Merkle diff, SWIM semantics, CRDT gossip) designed but not
exercised end-to-end; no two-node integration harness.

**3I 8 true P0s**: GAP-11 SSE streaming (hardest, trait change cascade risk),
GAP-03 web search wiring, GAP-14 JSON repair, GAP-18 retry/failover, GAP-12
`weft onboard`, GAP-15 memory tool search, GAP-17 tool-call parsing.

## 4. ⚠️ Current working-tree state — CRITICAL hazard

**Branch**: `feat/weftos-579-591-graduations`, ~27 commits ahead of master + HEAD
`7475ef99`. **Nothing committed across the entire 05-14→05-17 session chain.**

`git diff --stat HEAD` ≈ **588 files / +19,498 / −14,389**.

**Untracked new crates (zero git history)**: `lgfx-bus-rgb-rs` (production-proven
bus driver), `weftos-leaf-scene` (112 tests), `weftos-leaf-renderer` +
`weftos-leaf-sim` (74 tests), `weftos-leaf-canvas` (~1135 LoC), `weftos-scene-builder`
(19 tests), `weftos-leaf-touch-gt911` (6 tests), `clawft-edge-pad-idf`. Plus
`docs/design/vector-leaf-display.md` (~4800 words), `docs/adr/adr-056`, `adr-057`,
`docs/leaf-push-protocol.md`.

**Risk rating: CRITICAL.** Handoff says "one accidental `git checkout` from
disappearing." The whole vector-display subsystem (5+ crates, ~300 tests, 2 ADRs,
1 design doc) has no history. The `~/.claude/agents/esp32-s3-rgb-touch-display/`
learnings are outside the repo with no backup. Partial mitigation: the "Fallout
glitch" snapshot at `.planning/actors/inkpad-snapshots/2026-05-15-fallout-glitch/`.
**Recommended immediate action: commit a focused diff of the new crates + design
doc before touching anything else.**

## 5. Operational gotchas

- **Daemon binary swap**: `cp weaver ~/.cargo/bin/` while daemon runs → "Text file
  busy". Use atomic `mv` + restart. Running daemon keeps old inode; new features
  invisible until restart. Bit the user 3+ times in the 05-17 session.
- **`.env` shadows weave.toml** (BUG-8).
- **scripts/build.sh is mandatory** for build/test/check/lint/gate per CLAUDE.md —
  not raw cargo.

## 6. TODO / FIXME density

`git grep -iE "TODO|FIXME|HACK|XXX"` over `crates/**/*.rs` = **74 lines**; most are
in detection/test infrastructure. Substantive hotspots: `plugins_cmd.rs` (6, all
plugin scaffolding), `surface_host/compose.rs` (6, unwired Surface IRIs +
governance deferred to M1.6), `vector_quantization.rs` (2, KG-011/012 blocked on
ruvector-core PR #352), `service.rs` (2, cancellation token Phase D2). **Zero
substantive FIXME/HACK in production logic.** Deceptive: the voice workstream
encodes deferred work in module doc-comments, not inline markers (~40 stub modules,
0 grep hits).
