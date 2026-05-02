---
title: "WeftOS GUI (Explorer, egui, VSCode panel)"
slug: weftos-gui
workstream_id: "08"
release: 0.7.0
audit_type: comprehensive
last_updated: 2026-04-28
status: in-progress
owner: weftos-gui
crate_root: crates/clawft-gui-egui
---

# WeftOS GUI (Explorer, egui, VSCode panel)

## General Description

The WeftOS GUI is the OS-level interface for the substrate-as-ontology
system. It is implemented entirely in Rust + `egui` 0.34 / `eframe` and
ships in two targets from a single codebase:

- **Native `eframe`** — `weft-gui-egui` binary (boot splash, warped-grid
  desktop wallpaper, tray, app windows, Explorer panel, terminal, chat,
  canon demo lab).
- **`wasm32-unknown-unknown`** — same `ClawftApp` entry point compiled to
  a WASM bundle and hosted by the VSCode/Cursor extension
  `extensions/vscode-weft-panel/` inside a `WebviewPanel`. RPC is
  proxied through the extension host over `postMessage` to the daemon
  UDS (Unix domain socket).

This workstream is distinct from the standalone clawft AGENT React
dashboard (workstream 09) and from the docs site Next.js app
(workstream 11). ADR-007 (`zustand+tauri events`) and ADR-005
(`xterm.js`) describe a competing Tauri/React stack that the egui
shell explicitly replaces — they are not load-bearing for this stream
but their decisions are still documented for the older path. The egui
terminal pane is backed by `alacritty_terminal` (native) with a
WASM stub; xterm.js is not used here.

The crate is structured around five concerns:

- `canon/` — the **21-item primitive canon** (ADR-001 row 1–21:
  chip, pressable, field, toggle, select, slider, stack, grid, strip,
  dock, sheet, modal, table, tree, gauge, plot, media, stream-view,
  canvas, foreign — Tier B, tabs).
- `blocks/` — the legacy 12 demo blocks (Overview, Text, Button, Code,
  Status, Budget, Table, Tree, Tabs, Terminal, Layout, Oscilloscope) —
  the pre-canon spike, retained for `weft-demo-lab`.
- `surface_host/compose.rs` — ADR-016 surface-description IR composer:
  walks a `clawft_surface::SurfaceTree` against an `OntologySnapshot`
  and drives the canon primitives. WeftOS Admin app rendered through
  this path.
- `explorer/` — the Ontology Explorer (`PROJECT-PLAN.md` Phase 0/1/2,
  `PHASE-2-PLAN.md` Tracks 1/3/5): tree-left / detail-right substrate
  walker with shape-dispatched viewer registry, Workshop hot-reload,
  chat sentinel, terminal sentinel, control-toggle.
- `live/` — RPC transport: `native_live.rs` (tokio + UDS via
  `clawft_rpc::DaemonClient`, host-local adapter loop) and
  `wasm_live.rs` (`postMessage` bridge to extension host).

## Status & Timeline

| Milestone | Date | Scope |
|---|---|---|
| Initial spike | 2026-03 (commit `f494db20`) | egui/eframe native GUI, 12 core blocks + oscilloscope |
| M1 wasm-compat | 2026-04 (`1204f7a5`, `894de997`, `6e2df4d9`) | `wasm32-unknown-unknown` build, VSCode panel hosts egui in Cursor |
| Canon foundation | 2026-04 (`eae8c170`, `853b9a85`, `54557f56`, `7ddc5ee2`) | `CanonWidget` trait + retrofit + 11 new primitives — full 21-item canon |
| M1.5 trilogy | 2026-04 (`b397330d`, `0fbc5449`, `ee347485`, `87e9591d`) | substrate adapter, app manifest, surface IR, WeftOS Admin app end-to-end |
| M1.5.1 chips | 2026-04 (`01d141fa`, `1f19162d`, `22aed898`, `8fe5be5d`) | Network / Bluetooth / Mesh / Chain / Microphone adapters, ToF heatmap chip |
| Explorer Phase 0 | 2026-04-23 (`00176c53`, `4ed33403`) | VSCode allowlist for `substrate.read/subscribe/list` |
| Explorer Phase 1 | 2026-04-23 (`4f80985a`, `aa7d0d82`, `71aa94fa`, `a095e880`, `441a97cf`) | Explorer panel skeleton + JSON fallback + AudioMeter / ConnectionBadge / DepthMap viewers + tray chip mount |
| Explorer Phase 2 (Wave A) | 2026-04 (`e4282d90` Object Types, `clawft-service-whisper`) | Object Type primitive + Mesh/AudioStream/ChainEvent ontology |
| Explorer Phase 2 (Wave B) | 2026-04 (`5af9b89a`, `31fb4a58`, `f882fb4e`, `23f08f19`, `3be63301`, `76eb45a1`, `cf8ae3e7`, `a75c1d2b`, `48bc2ac2`, `dc132499`) | Waveform/MeshNodes/ChainTail/TimeSeries/ProcessTable viewers, GraphViewer (`ui://graph`), Workshop primitive + TOML watcher |
| Late 0.7.0 | 2026-04-24…27 (`a509cd14`, `e23807fb`, `b96d413a`, `ced776bd`, `b068b063`) | terminal pane (PTY), chat-window panel, agent.chat concierge, soul promote |
| 0.6.19 release | 2026-04-22 | Rolled forward to 0.6.x (M1.5 + canon + chips + sensor framework) |

Branch state at audit time: `development-0.7.0` is 78 commits ahead of
origin; agent-core-v1 ships, last commit `8c08ce0a` adds worktree +
branch cleanup item to handoff. 69 commits in `git log` touch
`crates/clawft-gui-egui/`.

## Released Features

Already shipping in `development-0.7.0` (much of it rolled forward to
0.6.19):

### Canon primitive system (21 items, ADR-001)
- All 21 canon primitives present as modules under `src/canon/`:
  `canvas.rs`, `chip.rs`, `dock.rs`, `field.rs`, `gauge.rs`, `grid.rs`,
  `media.rs`, `modal.rs`, `plot.rs`, `pressable.rs`, `select.rs`,
  `sheet.rs`, `slider.rs`, `stack.rs`, `stream_view.rs`, `strip.rs`,
  `table.rs`, `tabs.rs`, `toggle.rs`, `tree.rs` (+ `response.rs`,
  `types.rs` for `CanonWidget` trait + `CanonResponse` head fields).
- Smoke tests in `canon/mod.rs` cover identity URIs, affordance
  toggle behaviour, modal modality axes, canvas defaults, field kind
  tags, grid affordances, tabs `switch-tab` exposure.
- Foreign primitive (Tier B, row 20) is declared in ADR-001 but the
  in-tree implementation surface for it lives indirectly through the
  composer's `render_todo` fallback — not a per-kind host.

### Surface description composer (ADR-016)
- `surface_host/compose.rs` (~810 LoC) walks `SurfaceTree` and renders
  Stack / Strip / Grid / Chip / Pressable / Gauge / Table /
  StreamView / Heatmap / Waveform.
- Affordance dispatch end-to-end: row click on `ui://table` →
  `kernel.kill-process { pid }`; gauge action button →
  `kernel.restart-service { name }`. `rpc.` prefix is stripped before
  dispatch so verbs match daemon-side handlers.
- WeftOS Admin app (manifest + desktop surface fixture) renders
  through the composer end-to-end; covered by
  `tests/admin_app_e2e.rs`.
- Per-tray-chip detail surfaces wired for kernel / mesh / exochain /
  wifi / bluetooth / audio / tof; `tests/chip_surfaces.rs` parse-
  smokes all seven; `tests/surface_headless_render.rs` exercises one
  pass through the composer.

### Explorer (Phase 0/1/2 complete)
- Two-pane substrate walker (left tree / right detail) keyed on
  `substrate.list` / `substrate.read`.
- Shape-dispatched viewer registry with marker-comment insert points
  (`[[VIEWERS_MODULES_INSERT]]`, `[[VIEWERS_REGISTRATIONS_INSERT]]`)
  for parallel-track work.
- 11 viewers shipped: `JsonFallbackViewer` (priority 1, fallback),
  `AudioMeterViewer` (10), `ConnectionBadgeViewer` (10),
  `DepthMapViewer` (10), `TimeSeriesViewer` (10), `MeshNodesViewer`
  (12), `ChainTailViewer` (12), `GraphViewer` (14, ADOPTION §9
  Vertex analog), `WaveformViewer` (15), `PcmChunkViewer` (inline
  waveform mini-plot, base64 i16le decode), `ProcessTableViewer`.
- Object Type registry (`ontology/`) — Mesh, AudioStream, ChainEvent
  shape-inferred and badged in tree.
- Workshop primitive (`explorer/workshop.rs`) — config-driven
  hot-reload composition with rows layout; subscribes per-panel,
  re-parses on each frame so a substrate publish replaces layout
  in next paint. TOML file-watcher example
  (`examples/workshop-watcher.rs`) signs publishes with ed25519 +
  Phase 3 node-identity gate.
- Control-intent toggle viewer fires `control.set_enabled`.
- Chat sentinel panel (`{ kind: "chat" }`) — calls `agent.chat`
  end-to-end against the WeftOS concierge.
- Terminal panel (`{ kind: "terminal" }`) — alacritty-backed
  ANSI grid renderer, native-only; WASM gets a stub.

### VSCode/Cursor panel (`extensions/vscode-weft-panel`)
- Sovereign-posture `WebviewPanel`, retains context when hidden,
  reload-survives via `WebviewPanelSerializer`.
- Hosts the egui WASM bundle at `webview/wasm/` (built via
  `scripts/build-wasm.sh`, ~4.2 MB unoptimized).
- `postMessage` JSON-RPC bridge to extension host → daemon UDS.
- Allowlist (`ALLOWED_METHODS` Set) covers 22 verbs:
  `kernel.{status,ps,services,logs,kill-process,restart-service}`,
  `cluster.{status,nodes}`, `chain.{status,tail}`,
  `sensor.mic.status`, `substrate.{read,subscribe,list}`,
  `control.{set_enabled,list}`, `llm.prompt`, `agent.chat`,
  `terminal.{spawn,write,resize,close}`. Per-method timeout policy
  (300s for `llm.prompt` / `agent.chat`, default for read-paths).
- WASM hot-reload watcher (`extension.ts:220`) toasts
  `WeftOS: reloaded wasm bundle` when bundle changes.

### Live transport (`live/`)
- Native: tokio runtime + `DaemonClient` UDS + per-topic
  `OntologyAdapter` subscribers (kernel, network, bluetooth, mesh,
  chain, mic). `Snapshot` populated at 250 ms cadence.
- WASM: `postMessage` request/response with pending-RPC registry,
  message listener installed on `window`, poll timer at 1000 ms.
- WSL/WASM `Instant`-subtraction guarded against time-origin panic
  (`eb4cd9d8`).

### Demo lab (`weft-demo-lab` bin)
- 20 canon demos in WeftOS panel (`7c0c523b`).
- Vendored `egui_demo_lib` Fractal Clock / HTTP / 3D / Color tabs
  with theme-toggle A/B (`12591309`).
- Standalone repro example for the `CanonResponse::from_egui`
  Windows RwLock-reentrancy deadlock (fixed in `b5ed97f4`).

### Boot/desktop shell
- Boot splash (`shell/boot.rs`) — gold-on-black logo, halo,
  fade-in/hold/fade-out timeline (~4.2 s).
- Warped-grid wallpaper (`shell/grid.rs`).
- Bottom tray with chips (`shell/tray.rs`) — Kernel / Mesh /
  ExoChain / Explorer.
- Audio shell (`shell/audio.rs`) — optional procedural boot sound
  via `rodio` (gated on `audio` feature).

## What's Left — Total Depth

### TODOs / FIXMEs in source

| Location | Item |
|---|---|
| `src/canon/field.rs:61` | `// TODO: Date — egui_extras::DatePickerButton + chrono::NaiveDate state` (canon row 3 incomplete; first pass ships Text/Number/Choice only) |
| `src/canon/field.rs:62` | `// TODO: Code — egui::TextEdit::multiline + egui_extras::syntax_highlighting` (CodeEditor variant; ADR-003 codemirror does NOT apply here) |
| `src/canon/field.rs:6` | doc comment: "`Date` (DatePickerButton) and `Code` (CodeEditor) are explicit TODOs" |
| `src/canon/select.rs:6` | doc: "`TableBuilder`-based large-set form is a future extension kept inside `ui://table`" — Select large-N variant not implemented |
| `src/canon/stream_view.rs:216` | comment: ScrollArea body marks no chosen affordance this frame; future affordance dispatch deferred |
| `src/surface_host/compose.rs:144` | `other => render_todo(other, &node.path, ui)` — fallthrough for any canon IRI not in the 10-arm match (Pressable + Strip + 8 others wired; **the remaining 11 of 21 canon IRIs render a yellow `"TODO: <iri> not wired in M1.5"` label**: chip is wired, but field/toggle/select/slider/sheet/modal/dock/tabs/tree/plot/media/canvas/foreign all fall through) |
| `src/surface_host/compose.rs:683` | `fn render_todo` body — visible TODO label kept so surface still renders without panic |
| `src/surface_host/compose.rs:803-805` | `pub fn honest_affordances` is identity passthrough — "ADR-006 rule 2 TODO". GEPA-gated governance intersection deferred to M2 active-radar loop |
| `src/explorer/viewers/audio_meter.rs:14` | "viewer is stateless; no scrolling history is kept. A future `WaveformViewer` will cover that niche" — partial; Waveform shipped but inline scroll history on the meter itself is not |
| `src/explorer/chat.rs:23-32` | scope cuts: no streaming (sync only; needs `agent.chat_stream` v1.1+), no on-disk persistence (in-memory across selection), no system-prompt UI, no model picker, no markdown rendering |
| `src/explorer/terminal.rs:27-37` | deferred: no selection / clipboard, no bold/italic glyph variants (italic ignored), no scrollback view (only viewport), wasm stub only |

### Deferred items captured in plans / ADRs / handoff

#### Phase 2 acceptance criteria — partially landed
Per `.planning/explorer/PHASE-2-PLAN.md` §6:
- Track 1 (viewers) — 5 of 5 acceptance viewers shipped.
- Track 2 (Object Types) — `ObjectType` trait + Mesh/AudioStream/
  ChainEvent shipped; capability metadata hooks (`applicable_actions`,
  `events_emitted`) declared but **all values empty** for MVP.
- Track 3 (`ui://graph`) — read-only MVP shipped; **editable graph
  is explicit stretch goal NOT in 0.7.0**, deferred to "Phase 3+
  patch UI" (rolled-our-own painter; `egui_node_graph` adapter is
  the migration seam).
- Track 4 (Whisper pipeline) — new crate `clawft-service-whisper`
  exists but is daemon-side, not GUI; flagged as in-flight.
- Track 5 (Workshop hot-reload) — Rows layout shipped; Grid and
  Tabs layouts fall through to Rows (with debug hint); `Unknown`
  layout preserved on round-trip but unrendered.

#### Explorer management surface (`.planning/sensors/EXPLORER-MANAGEMENT-SURFACE.md`)
15 affordances classified READS-ONLY vs ACTIONS-REQUIRED. NOT in
0.7.0:
1-3, 10, 11 (READS-ONLY but require new `HealthViewer` /
`SensorViewer` + Node/Sensor/HealthReport object types — "migration
pass" post Node/Actor split).
4-7, 15 (ACTIONS-REQUIRED — full Actions pipeline + actor signing
+ node-side action listeners; **Action Types declared as slots only,
all empty**).
8 (Workshop parameterization — schema unsigned-off, not wired).
9 (tree filter UI — small UI add, not done).
12 (clipboard / pubkey export — egui clipboard API, not wired).
13 (watch + notify — split; client-side watchlist storage TBD).
14 (lineage graph — needs lineage-metadata convention; not started).

#### VSCode panel — scope cuts in `SMOKE.md` and `README.md`
- No voice input (`microsoft/vscode#303293`: webviews can't expose
  `allow="microphone"`). Capture sidecar deferred to M2.
- No typed active-radar return schema; webview posts only plain
  RPC-request / RPC-response messages (no `variant-id` echo).
- No `ThreadDock` primitive for per-agent parallel output.
- Panel does not yet speak WSP-0.1 verbs (`protocol-spec.md`); raw
  `kernel.*` / `agent.chat` RPC only. WSP verbs queued for M3.

#### Handoff open loops (2026-04-26 / 04-27)
- Live smoke verify against running llama-server still pending
  (chat-agent timeout patched with 300s + `AGENT_CHAT_PER_TURN_MAX_TOKENS = 256`
  in `e6f8c816`; user has not yet exercised the round-trip).
- Apr 25 user brief items: inline-streaming (`agent.chat_stream`),
  provider switcher in chip strip, multi-conversation sidebar (also
  no multi-tab terminal — `HashMap<SessionId, Terminal>` structure
  foreshadowed only).
- chat-agent-v1.1 backlog (`chat-agent-v1.md` §14 + handoff §57-65):
  streaming, sidebar, real interactive defer, per-user agent_ids,
  heartbeat label, identity-drift surface, markdown rendering,
  system-prompt UI affordance.

### Open questions

#### Explorer planning open questions (`PROJECT-PLAN.md` §6)
1. Tray entry mechanism — Explorer chip vs. launcher menu vs. both.
   Default is launcher-menu (chip shipped instead — diverges).
2. Should clicking a tray chip open the Explorer focused on the
   chip's substrate path? Deferred until MVP lands; **MVP landed,
   convergence not done**.
3. Should `substrate.list` include paths that have never carried a
   Replace value but are referenced by subscribers? Probably no —
   sign-off needed.
4. Wildcard-root subscribe is out of scope (back-pressure risk on
   WASM).

#### Explorer management-surface open questions (`EXPLORER-MANAGEMENT-SURFACE.md` §6)
1. Watchlist storage location — proposal: client-side TOML at
   `~/.config/weftos/explorer-watchlist.toml`; migrate to
   `substrate/actor/<actor-id>/watchlist/` once Actors exist.
2. Workshop parameterization schema —
   `{ "substrate_path_template": "...", "params": { ... } }` shape
   not signed off.
3. Lineage metadata placement — inline field vs sibling
   `<derived-path>/meta/lineage` path. Sign-off needed.
4. "Open in Workshop" button on every viewer? Deferred.
5. Notification channel — native vs WSL vs WASM are three different
   mechanisms; start native only.

#### Phase 2 ontology open questions
- ObjectType `applicable_actions` slot is reserved but always empty
  in MVP — what shape exactly does an `applicable_actions` entry
  carry once Actions land? Per-type schema not designed.
- Lineage Object Type proposed but not in current registry.

#### Composer governance question
- `honest_affordances` (`compose.rs:805`) is identity passthrough.
  ADR-006 rule 2 says affordances must be intersected with
  governance pre-render. The hook exists; the policy doesn't.

### Orphaned work / parallel paths

- **`blocks/` directory (12 legacy blocks)** runs in parallel to
  `canon/` (21 primitives). The 0.6.19 changelog said "Retrofit pass:
  7–8 existing blocks wrapped in the trait". The blocks are still
  driven by `weft-demo-lab` and the `Desktop` panel `BlockKind`
  variant (`shell/desktop.rs:88-118`). Retirement plan for `blocks/`
  not documented.
- **`bin/demo_lab_vendored/`** — `fractal_clock.rs`, `http_app.rs`,
  `custom3d_glow.rs` vendored from `egui_demo_lib`; `serde` feature
  gate kept just to satisfy upstream `#[cfg_attr]` lines. Drift from
  upstream egui demo.
- **Top-level ADRs 005 / 007 / 038 / 013** describe the Tauri+React
  stack the egui canon replaces; none have been marked superseded.
  Old guidance still indexed in `docs/adr/`.
- **CHANGELOG `[0.5.x] – legacy "Lego Block Engine"`** — Zustand
  block engine references a React `gui/` source tree no longer
  present in `crates/clawft-gui-egui` (workstream 09's `ui/`).
- **WASM time-origin guards** — `checked_sub` pattern duplicated at
  `Explorer::default` and `SubscriptionHandle::new`, not factored.
- **Locked agent-core/* worktrees (12)** — retained as rollback
  hatch under `.claude/worktrees/agent-*` per handoff; cleanup
  follow-up is the GUI panel commit (#9, ~300 LoC) and 11 sibling
  agent-core branches.

### Identified bugs / risk surfaces

- `wasm-pack` profile sets `wasm-opt = false` because cached
  `wasm-opt` rejects modern bulk-memory output (`Cargo.toml:9-12`);
  GUI takes "modest size hit". Bundle reported ~4.2 MB unoptimized;
  affects WASM panel cold-load time in Cursor.
- `image` crate pinned to `default-features = false` + `["png"]`
  features explicitly because feature unification doesn't reliably
  pick PNG on the wasm target — boot logo silently fails to decode
  otherwise. Brittle: any future workspace dep that flips
  `default-features = true` reintroduces the bug.
- The composer's per-frame `RefCell<Vec<...>>` accumulators
  (`compose.rs:96-105`) walk the tree using interior mutability;
  any future re-entrancy from a primitive's `show()` panics. The
  `canon_slider_windows_deadlock_repro.rs` example exists because of
  a related Windows-only RwLock reentrancy that was patched
  (`b5ed97f4`); class of bug is not eliminated.
- Activity-dot `HashMap` in `Explorer` grows unbounded — paths added
  on every value change with no eviction. Long-running session leaks
  one entry per visited path with mutation history.
- VSCode allowlist drift: every new substrate consumer needs an
  allowlist edit + WASM rebuild + extension recompile + Cursor
  webview reload. Captured in user memory
  `feedback_rebuild_webview_wasm.md` as recurring failure mode.

## Task List

The list below is comprehensive (audit, not release-gate triage).
0.7.0-blocker / nice-to-have separation is not asserted here.

### Immediate-term (smoke / hygiene)
- [ ] T08-01: Smoke-test panel in Cursor against post-patch daemon
  (chat ask "what is this project about?", expect concierge to
  read `CLAUDE.md`). Open loop from 2026-04-27 handoff.
- [ ] T08-02: Document `weft-gui-egui` native binary path in
  `scripts/build.sh native` (handoff note: "one-line addition" —
  defer until non-Cursor user reports a need).
- [ ] T08-03: Cleanup 12 `agent-core/*` locked worktrees once smoke
  green (`docs/handoff.md` follow-up; not GUI-only but tangled).
- [ ] T08-04: Confirm whether `npm run package` + `.vsix`
  install/uninstall path is current; documented in
  `extensions/vscode-weft-panel/README.md` but unexercised this cycle.

### Canon (composer wiring gaps)
- [ ] T08-10: Wire `ui://field` in composer (`render_todo` →
  `render_field`). Field primitive exists; surface IR can declare it
  but composer falls through.
- [ ] T08-11: Same for `ui://toggle`, `ui://select`, `ui://slider`,
  `ui://sheet`, `ui://modal`, `ui://dock`, `ui://tabs`, `ui://tree`,
  `ui://plot`, `ui://media`, `ui://canvas`, `ui://foreign`. Each
  primitive type has an in-crate widget but isn't dispatched from a
  surface description today (M1.6+ work per `compose.rs:11`).
- [ ] T08-12: Implement `Field::Date` (`egui_extras::DatePickerButton`
  + `chrono::NaiveDate` state). Add `FieldValue::Date` enum variant.
- [ ] T08-13: Implement `Field::Code` (`TextEdit::multiline` +
  syntax highlighting). Add `FieldValue::Code` variant.
- [ ] T08-14: `Select` `TableBuilder`-based large-set form (per
  ADR-001 row 5).

### Explorer management-surface (READS-ONLY tier — ship without Actions)
- [ ] T08-20: `HealthViewer` for `substrate/<node>/health` paths
  (affordance #1, #2). Needs new `HealthReport` Object Type.
- [ ] T08-21: `SensorViewer` with raw-vs-summary child-pane switcher
  (#3); needs `Sensor` classifier.
- [ ] T08-22: Tree filter UI — chip row at top of Explorer for
  type / status filters (#9).
- [ ] T08-23: Sparkline embed for `HealthReport.rssi`,
  `free_heap`, observed-rate scalars under Node/Sensor viewers (#10).
  Reuses existing `TimeSeriesViewer`, just wiring.
- [ ] T08-24: Sensor↔Node breadcrumb navigation — viewer-emitted
  "select path" intent that Explorer's `on_select` consumes (#11).
- [ ] T08-25: Copy-path / copy-pubkey / export-snapshot
  affordances via egui clipboard API (#12).

### Explorer Phase 3 (ACTIONS-REQUIRED) — slots only today
- [ ] T08-30: Workshop parameterization schema sign-off + impl —
  `substrate_path_template` + params (#8). Open question.
- [ ] T08-31: Lineage Object Type + viewer — needs metadata
  convention sign-off first (#14).
- [ ] T08-32: `ObjectType::applicable_actions` populated for at
  least Mesh / Sensor / Node when the per-type schema lands.
- [ ] T08-33: `honest_affordances()` real GEPA / governance
  intersection (`compose.rs:805` ADR-006 rule 2 TODO).

### Workshop primitive
- [ ] T08-40: Implement `Grid` layout (today degrades to Rows).
- [ ] T08-41: Implement `Tabs` layout (today degrades to Rows).
- [ ] T08-42: Wire `viewer_hint` overrides (today: `"auto"` only).

### Graph viewer (`ui://graph`)
- [ ] T08-50: Editable Phase 3+ patch UI — migration to
  `egui_node_graph` along the JSON `Value` → node/edge adapter
  seam.

### Chat / agent panel (chat-agent-v1.1)
- [ ] T08-60: Inline streaming via `agent.chat_stream` daemon RPC.
- [ ] T08-61: Multi-conversation sidebar UI.
- [ ] T08-62: Markdown rendering in chat bubbles (`chat.rs:32`).
- [ ] T08-63: System-prompt UI (struct has `system` field unwired, `chat.rs:29`).
- [ ] T08-64: Model / provider switcher in chip strip (Apr 25 user brief).
- [ ] T08-65: Heartbeat label (spinner currently occludes `derived/chat/<conv>/status`).
- [ ] T08-66: Real interactive defer — panel prompt-and-resume on `{ deferred: true, reason }`.
- [ ] T08-67: Identity-drift / binding-thread mismatch warning surface.

### Terminal panel
- [ ] T08-70: Mouse selection + clipboard.
- [ ] T08-71: Bold / italic glyph variants (italic ignored today).
- [ ] T08-72: Scrollback view + wheel handler.
- [ ] T08-73: Multi-tab terminal (`HashMap<SessionId, Terminal>`).
- [ ] T08-74: Real WASM terminal renderer (alacritty native-only today).

### VSCode/Cursor panel
- [ ] T08-80: Capture sidecar (mic/camera) for `microsoft/vscode#303293`. M2.
- [ ] T08-81: Typed active-radar return schema (`variant-id` echo).
- [ ] T08-82: `ThreadDock` primitive for per-agent parallel output.
- [ ] T08-83: WSP-0.1 verb support (raw RPC only today). M3 scope.
- [ ] T08-84: Reconcile / supersede ADRs 005 / 007 / 038 (legacy Tauri+React path).
- [ ] T08-85: `wasm-opt` upgrade path (disabled per `Cargo.toml:9-12`).

### Hygiene / refactor
- [ ] T08-90: Factor WASM `Instant::checked_sub` time-origin guard helper (used twice).
- [ ] T08-91: Bound `Explorer::activity` HashMap (LRU/TTL — unbounded today).
- [ ] T08-92: Document or retire the legacy `blocks/` 12-block set vs `canon/` 21-primitive duality.
- [ ] T08-93: Document or eliminate the `image = ["png"]` feature pin (brittle, `Cargo.toml:48-51`).
- [ ] T08-94: Decide vendored vs upstream path for `egui_demo_lib` Fractal/HTTP/3D/Color demos.

## Sources

### Source files (absolute paths)
- `/home/aepod/dev/clawft/crates/clawft-gui-egui/Cargo.toml`,
  `src/lib.rs`, `src/canon/{mod,field,select}.rs`,
  `src/surface_host/{mod,compose}.rs`,
  `src/explorer/{mod,workshop,chat,terminal}.rs`,
  `src/explorer/viewers/{mod,audio_meter,graph}.rs`,
  `src/live/{native_live,wasm_live}.rs`,
  `src/shell/{mod,desktop}.rs`
- `tests/{admin_app_e2e,chip_surfaces,surface_headless_render,workshop_integration}.rs`
- `examples/{canon_slider_windows_deadlock_repro,workshop-watcher}.rs`
- `/home/aepod/dev/clawft/extensions/vscode-weft-panel/`
  (`src/extension.ts`, `src/rpc.ts`, `scripts/build-wasm.sh`,
  `SMOKE.md`, `README.md`)

### Planning / design
- `/home/aepod/dev/clawft/.planning/explorer/PROJECT-PLAN.md` (Phase 0/1)
- `/home/aepod/dev/clawft/.planning/explorer/PHASE-2-PLAN.md` (Phase 2 tracks)
- `/home/aepod/dev/clawft/.planning/sensors/EXPLORER-MANAGEMENT-SURFACE.md` (15-affordance classification)
- `/home/aepod/dev/clawft/.planning/ontology/ADOPTION.md`
- `/home/aepod/dev/clawft/.planning/symposiums/compositional-ui/AGENDA.md`
- `/home/aepod/dev/clawft/.planning/symposiums/compositional-ui/session-7-dev-panel-embedding.md`
- `/home/aepod/dev/clawft/.planning/symposiums/compositional-ui/protocol-spec.md`
- `/home/aepod/dev/clawft/.planning/symposiums/compositional-ui/foundations.md`

### ADRs (top-level — `/home/aepod/dev/clawft/docs/adr/`)
- adr-003-codemirror, adr-005-xterm-js, adr-007-zustand-tauri-events,
  adr-013-json-block-descriptor — legacy React/Tauri path, not load-bearing
  for egui canon (terminal uses alacritty; shell uses egui directly).
- adr-016-multi-target-theming, adr-038-tauri-desktop-shell (parallel/competing).

### Symposium ADRs — load-bearing (`/home/aepod/dev/clawft/.planning/symposiums/compositional-ui/adrs/`)
- adr-001-primitive-canon (the 21-item canon), adr-006-primitive-head,
  adr-011-dev-panel-embedding-hybrid, adr-013-canvas-primitive,
  adr-014-modal-modality-split, adr-015-app-manifest,
  adr-016-surface-description, adr-017-ontology-adapter-contract.

### Memory artifacts
- `/home/aepod/.claude/projects/-home-aepod-dev-clawft/memory/feedback_rebuild_webview_wasm.md` (rebuild WASM bundle after GUI/RPC-allowlist edits)
- `/home/aepod/.claude/projects/-home-aepod-dev-clawft/memory/feedback_extension_rpc_allowlist.md`
- `/home/aepod/.claude/projects/-home-aepod-dev-clawft/memory/project_substrate_list_leaf_self.md`

### Handoff / session state
- `/home/aepod/dev/clawft/docs/handoff.md` (latest session, agent-core-v1 ships, chat-panel commit (9) ~300 LoC)
- `/home/aepod/dev/clawft/CHANGELOG.md` §0.6.19 (canon + chips + Admin app rolled to release line)

### Git log scope
- Branch `development-0.7.0`; 69 commits touch `crates/clawft-gui-egui/`
  from `f494db20` (initial spike) through `b068b063` (`feat(weaver):
  soul promote command` — peripheral).
- Tip: `8c08ce0a docs(handoff): add worktree + branch cleanup item`.

<!-- TRIAGED-STAMP:BEGIN -->
## Triaged into Plane — 2026-04-28

All open items in this audit have been filed as Plane work items in the WeftOS workspace under the `ws08-weftos-gui` label.

- **Range**: WEFT-242 … WEFT-291 (50 items)
- **Per cycle**: 0.7.x: 14, 0.8.x: 30, 0.9.x: 6
- **Triage spec**: `.planning/reviews/0.7.0-release-gate/triage/`
- **WEFT-N → name map**: `.planning/reviews/0.7.0-release-gate/triage/weft-mapping.json`

Per the project rule (CLAUDE.md → "Plane is the authoritative work tracker"): future updates to these items happen in Plane, not in this audit doc. This doc remains the source-of-truth for the original survey.
<!-- TRIAGED-STAMP:END -->
