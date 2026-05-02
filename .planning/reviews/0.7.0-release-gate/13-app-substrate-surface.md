---
title: App Layer / Substrate / Surface (M1.5)
slug: app-substrate-surface
workstream_id: "13"
release: "0.7.0"
last_updated: 2026-04-28
status: shipped-with-known-deferrals
crates:
  - clawft-app
  - clawft-substrate
  - clawft-surface
  - clawft-gui-egui (surface_host)
  - clawft-weave (substrate.* RPC verbs)
  - clawft-service-classify (substrate consumer)
related_adrs:
  - adr-006-primitive-head
  - adr-009-mission-console-seat-witness-anti-corruption
  - adr-012-capture-privacy-invariants
  - adr-013-canvas-primitive
  - adr-015-app-manifest
  - adr-016-surface-description
  - adr-017-ontology-adapter-contract
  - adr-018-ide-bridge-protocol
  - adr-019-input-modality-and-avatar
related_changelog:
  - "0.6.19 — M1.5 App Layer Trilogy + 21-item canon + sensor framework"
  - "0.6.19 (M1.5.1α) — built-in system components"
audit_type: comprehensive
---

# App Layer / Substrate / Surface (M1.5)

## General Description

Workstream 13 covers the M1.5 architectural payoff that landed in
0.6.19: the three-crate app-layer trilogy plus its supporting
substrate-RPC plumbing, sensor framework, and 21-item canon driven
from the surface IR. Concretely:

- **`clawft-app`** — ADR-015 manifest parser, JSON-backed
  `AppRegistry`, validation rules 1–9, lifecycle types, and a stub
  `Gate` trait (`NoopGate` / `StrictGate`).
- **`clawft-substrate`** — ADR-017 `OntologyAdapter` + `TopicDecl` +
  `StateDelta` + `Subscription`, the aggregate `Substrate` state tree,
  the `kernel` reference adapter (4 topics), the M1.5.1α system
  adapters (`network` / `bluetooth` / `mesh` / `chain`), and the
  M1.5.3 sensor framework (`physical::PhysicalSensorAdapter`,
  `MicrophoneAdapter` reference).
- **`clawft-surface`** — Surface description IR (`SurfaceTree`,
  `SurfaceNode`, `Binding`, `AffordanceDecl`), the 21+2 `IdentityIri`
  canon (ADR-001 21 + sensor-oriented `ui://heatmap` and
  `ui://waveform`), TOML parser, Rust builder, and binding-expression
  evaluator.
- **Substrate RPCs** — `substrate.read` / `subscribe` / `publish` /
  `notify` / `list` in `crates/clawft-weave/src/daemon.rs`.
- **WeftOS Admin app** — manifest + surface fixtures + end-to-end
  render test (`clawft-gui-egui/tests/admin_app_e2e.rs`).

M1.5 met `session-10 §7` 5/5; M1.5.1α closed the wasm `SystemTime`
panic, made affordances do real work via `ComposeOutcome.dispatches`,
and replaced five tray placeholders with `/sys/class`-backed adapters.
The composer still lives in `clawft-gui-egui::surface_host` rather
than in `clawft-surface` — the previously-cyclic dep is broken
(`f5e40c3`) but the proper fix (extract canon types to a shared crate
and move composer back) is unscheduled.

## Status & Timeline

- **2026-04-20**: M1.5 trilogy merged (`0e32e67`). 91 tests green
  across `clawft-app` (24), `clawft-surface` (36), `clawft-substrate`
  (22), plus the `clawft-gui-egui` admin e2e (9).
- **2026-04-21**: M1.5.1α follow-on (`22aed89` + 4 commits). Network
  / bluetooth / mesh / chain adapters added; DeFi vapor chip removed;
  affordance dispatch closed the loop. Substrate test count went to
  46/46.
- **2026-04-22**: 0.6.19 release rollup adds the M1.5.3 sensor
  framework slice — `physical::PhysicalSensorAdapter` trait,
  `MicrophoneAdapter` file-backed preview emitting
  `substrate/sensor/mic`, `ui://heatmap` + `ui://waveform` primitive
  IRIs, and ToF tray chip with native 8×8 heatmap.
- **2026-04-22 → 2026-04-24**: Sensor planning track opens (see
  `.planning/sensors/`) — describes the post-Node/Actor-split
  substrate path layout (`substrate/<node-id>/...`) for the INMP441
  mic and ESP32-S3 node, plus a generic healthcheck contract. **None
  of that planning is implemented yet** — the in-tree mic adapter
  still emits the legacy flat `substrate/sensor/mic` path.

Branch state: `development-0.7.0`, clean. The trilogy lives entirely
on this branch; `master` does not yet have any M1.5 code.

## Released Features

### `clawft-app` (ADR-015 subset)

- `AppManifest` struct with all ADR-015 §Schema fields (`id`, `name`,
  semver `version`, `icon`, `supported_modes`, `supported_inputs`,
  `entry_points` Cli/VscodeCommand/WakeWord, `surfaces`,
  `subscriptions`, `influences`, `permissions`, `narration`); TOML
  round-trip; `Permission::parse` / `to_token` for the
  `camera|mic|screen|fs:<prefix>|net:<domain>` grammar.
  (`manifest.rs`.)
- Structural validation rules 1–5, 7, 8, 9 + narration-key-
  subscription cross-check. (`validation.rs`.)
- `AppRegistry` — JSON file at `$XDG_DATA_HOME/weftos/apps.json` or
  `~/.weftos/apps.json`, atomic tmp+rename writes, install /
  uninstall / enable / disable / list / get; `web-time` for wasm-safe
  timestamps. (`registry.rs`.)
- `SessionConfig`, `AppLaunchRequest`, `AppLaunchResult`,
  `LaunchError`, `check_launch_shape`; `governance::Gate` trait with
  `NoopGate` and `StrictGate` (denies `camera|mic|screen`).
  (`lifecycle.rs`.)
- WeftOS Admin manifest fixture; 24 unit tests green.

### `clawft-substrate` (ADR-017 subset)

- `OntologyAdapter` async trait + `TopicDecl` (`path`, `shape`,
  `refresh_hint`, `sensitivity`, `buffer_policy`, `max_len`) +
  `StateDelta::{Append, Replace, Remove}` + `Subscription`,
  `AdapterError`, `SubId`; `PermissionReq` re-exports
  `clawft_app::manifest::Permission` (M1.5-D unification).
  (`adapter.rs`.)
- `Substrate` state tree — flat `BTreeMap<String, Value>`, max-len
  auto-trim on `Append`, tracked subscriptions with `JoinHandle`s,
  async `close_all` honouring ADR-009 tombstones, `Drop` abort.
  (`snapshot.rs`.)
- Native adapters (cfg-gated): `kernel` (4 topics under
  `substrate/kernel/`), `network` (wifi/ethernet/battery), `bluetooth`,
  `mesh`, `chain`, `mic` (`substrate/sensor/mic`).
- Sensor framework: `physical::Characterization`
  (Presence/Rate/Enumerated/Spectral/Identifying — the spectrometer
  principle gate); `SensorInterface`
  (Gpio/I2c/Spi/Uart/I2s/Usb/HostAudio/FileBacked); `SensorCalibration`;
  `PhysicalSensorAdapter: OntologyAdapter` extension trait.
  (`physical.rs`.)
- Projection helpers (`project_process_rows`, `explode_services_by_name`)
  so the admin fixture resolves uniformly under native and wasm
  fallback. 46/46 tests green.

### `clawft-surface` (ADR-016 subset)

- 23-variant `IdentityIri` enum (canonical 21 + `Heatmap` / `Waveform`);
  `is_container()` predicate.
- `SurfaceTree` / `SurfaceNode` / `AttrValue` / `Binding` /
  `AffordanceDecl` IR re-using `clawft_app::manifest::{Mode, Input}`.
- TOML parser + Rust builder (`builder::Surface`, `chip`, `gauge`,
  `grid`, `stack`, `stream_view`, `table`, `strip`, `pressable`, …)
  proven structurally equal in `tests/weftos_admin_builder.rs`.
- Hand-rolled recursive-descent expression parser with literal /
  `$path` / field-access / call / single-arg lambda / precedence
  climbing; static arity table for `count`/`filter`/`len`/`first`/
  `last`/`fmt_*`/`exists`; explicit `TernaryNotSupported` and
  `NestedLambda` error variants.
- Binding evaluator over `OntologySnapshot` with nested-JSON traversal,
  lambda binding, `fmt_*`, `exists`. Type-mismatch surfaced cleanly
  (regression-tested for "count on scalar topic").
- Composer runtime (lives in `clawft-gui-egui::surface_host::compose`,
  not yet in this crate) drives Stack/Strip/Grid/Chip/Pressable/Gauge/
  Table/StreamView/Heatmap/Waveform; other 13 leaves render TODO
  labels. WeftOS Admin desktop surface fixture (4-quadrant) +
  per-chip fixtures; 36 tests green.

### Substrate RPCs (in `clawft-weave`)

- `substrate.read` (sync value + tick + sensitivity tier),
  `substrate.subscribe` (streaming over IPC; cleanup on disconnect),
  `substrate.publish` (mandatory `node_id` + `node_signature`;
  unsigned rejected), `substrate.notify` (signal-only pulse),
  `substrate.list` (prefix enumeration with `depth`). Egress gating
  of capture-tier paths from anonymous callers.
  (`crates/clawft-weave/src/daemon.rs` ~lines 2210–2920;
  `tests/substrate_rpc.rs` covers publish/read/subscribe/notify +
  signature verification + capture-tier path-name hiding.)

### Canon primitive system (21 + 2 sensor leaves)

- 21 ADR-001 primitives wired through `CanonWidget` trait +
  `CanonResponse` (topology / doppler / range / bearing) +
  `Pressable` reference impl in `clawft-gui-egui::canon::*`.
- 20-primitive demo lab in the WeftOS panel
  (`canon_demos.rs` + `Blocks | Canon` toggle).
- Two new sensor-oriented IRIs (`ui://heatmap`, `ui://waveform`) with
  composer renderers in `surface_host::compose::render_heatmap` /
  `render_waveform` and an explorer-side `WaveformViewer` /
  `AudioMeterViewer` for the substrate-explorer panel.

### WeftOS Admin app

- Manifest at `clawft-app/fixtures/weftos-admin.toml`.
- Surface description at
  `clawft-surface/fixtures/weftos-admin-desktop.toml`.
- Auto-installed on boot from a bundled fixture so the Apps tab
  always has content.
- End-to-end test wires `CannedKernelAdapter` →
  `Substrate::subscribe_adapter` → parsed admin surface →
  `surface_host::render_headless` → asserts non-empty responses +
  presence of `ui://gauge` / `ui://table`.
  (`clawft-gui-egui/tests/admin_app_e2e.rs`.)
- Composer dispatches `kernel.kill-process` + `kernel.restart-service`
  via `ComposeOutcome.dispatches`.

## What's Left — Total Depth

### TODOs / FIXMEs (in code, ripgrep)

- `crates/clawft-app/src/lib.rs:13` — Permission ↔ adapter
  consistency check (ADR-015 rule 6) is TODO'd until ADR-017 /
  `clawft-adapter` lands. (No such crate exists yet.)
- `crates/clawft-app/src/validation.rs:172–178` — Rule 6 deliberately
  deferred for M1.5; the `let _ = Permission::Camera;` line is a
  placeholder kept "in scope for TODO readers." Governance at install
  time is the backstop.
- `crates/clawft-app/src/lifecycle.rs:11,72` — Real ADR-012
  governance gate is M1.6+; `NoopGate` / `StrictGate` are
  placeholders.
- `crates/clawft-substrate/src/lib.rs:19` — App-manifest layer
  governance hook into `OntologyAdapter::open` is TODO'd
  (`permissions()` is advisory only).
- `crates/clawft-substrate/src/lib.rs:21` — Dynamic-lib adapter
  registration (ADR-017 §3 path 2) deferred.
- `crates/clawft-substrate/src/lib.rs:23` —
  `substrate/meta/adapter/<id>/health` topic stub in kernel adapter,
  surfaces as TODO. (Same item flagged in ROADMAP "Review-deferred
  follow-ups".)
- `crates/clawft-substrate/src/adapter.rs:133` —
  `AdapterError::PermissionDenied` is never emitted in M1.5 (gate
  not wired).
- `crates/clawft-substrate/src/kernel.rs:13` — processes/services
  topics emit a whole-list `Replace` per tick; per-pid / per-name
  deltas deferred to M1.6+.
- `crates/clawft-substrate/src/kernel.rs:319–321` — log poller is a
  periodic poll fallback because the daemon RPC has no streaming log
  endpoint yet (despite the topic declaring `RefreshHint::EventDriven`
  — declared intent ≠ runtime today).
- `crates/clawft-substrate/src/kernel.rs:412,437` — `diff_tail`
  Finding-1 fix is option-2 (capped-tail + capped-returns); option-1
  (monotonic `seq: u64` per entry) deferred to M1.6+ pending a
  daemon-side RPC change.
- `crates/clawft-substrate/src/mic.rs:33,84,192–202` — file-backed
  preview only (no CPAL/ALSA/CoreAudio/WASAPI backing); capture
  sensitivity declared but per-goal `CapabilityGrant` (ADR-012) not
  wired (`open` proceeds unconditionally); `model()` returns literal
  "preview stub", `interface()` always `FileBacked`.
- `crates/clawft-substrate/src/network.rs:24–27` — SSID / signal /
  connection management deferred to an nmcli-iwd variant in M1.6+.
- `crates/clawft-substrate/src/bluetooth.rs:7,16` — device
  enumeration deferred (user-content sensitivity).
- `crates/clawft-substrate/src/physical.rs:41–45` — no central device
  registry, no calibration database (ADR-020+).
- `crates/clawft-surface/src/tree.rs:119,276` — 13 of the 21 canon
  leaves ship as "M1.5 stub" `TODO:` labels (Field, Toggle, Select,
  Slider, Plot, Media, Canvas, Foreign + the unlisted Modal/Tabs/
  Sheet/Dock/Tree); `AffordanceDecl` passes through composer
  unfiltered (ADR-006 rule 2 intersection lives in
  `surface_host/compose.rs:804` `honest_affordances`, currently
  identity).
- `crates/clawft-gui-egui/src/surface_host/compose.rs:11,681,686` —
  any non-switched IR node renders "TODO: \<iri\> not wired in M1.5".

### Deferred items (ADR-anchored, ROADMAP-tracked)

ROADMAP §"Review-deferred follow-ups" tracks these as M1.5-tail
beads (each estimated < 30 LoC; all non-blocking for 0.7.0):

#### `clawft-app`

- `ValidationError::UnknownMode` is dead code — serde rejects
  out-of-set `supported_modes` values at parse time. **Decision
  open**: wire a Rust-constructed-manifest check, or delete the
  variant.
- Registry corruption recovery is "return JSON error to caller"; a
  quarantine / backup / repair path is M1.6+ polish.
- `uninstall` while enabled does NOT yet run the ADR-015 §Lifecycle
  teardown (surfaces / subscriptions / affordances) — those hooks
  don't exist at the compositor level yet.
- `[narration]` parsing is implemented but the speakable-template
  *evaluator* (ADR-019 narration rule language) is not — there's no
  surface-level wiring that emits TTS from a narration-key/template
  pair on a substrate change.

#### `clawft-surface`

- `.first` / `.last` as field-access shorthand is not supported (only
  function-call form). Documented in `lib.rs` header.
- `sort(list, key)` ordering combinator from ADR-016 §5 is not
  implemented.
- Scientific (`1e5`) / hex (`0xff`) number literals not accepted.
- User-defined compositions (`[compositions.*]`) not parsed.
- Ternary `?:` and nested lambdas are *explicitly* rejected with
  typed parse errors (`TernaryNotSupported` / `NestedLambda`); these
  are scope reductions, not bugs.
- Composer runtime location: lives in `clawft-gui-egui::surface_host`
  rather than in `clawft-surface`. Ticket (informal): extract canon
  types to a shared crate so the composer can move back, breaking
  `surface → gui-egui → surface` for real.

#### `clawft-substrate`

- `substrate/meta/adapter/<id>/health` topic (ADR-017 §7) not yet
  emitted. Adapter health is invisible to surfaces today.
- Log event-driven ingest is a periodic-poll fallback (kernel RPC
  has no streaming log endpoint).
- `processes` / `services` topics emit whole-list `Replace`; per-row
  deltas deferred to M1.6+ (requires daemon-side row-id contract).
- Adapter health: no liveness ping, no auto-reconnect signal — the
  drain task silently exits when the sender closes.
- Cross-platform stubs: `network` / `bluetooth` adapters return
  `absent` on non-Linux; macOS / Windows variants unscheduled.
- `sysfs` permissions / read errors degrade silently to `absent`
  without surfacing the cause to a UI.

#### Cross-cutting

- `variant_id` stamping is identity-mapped; ADR-006 head-metadata
  plumbing lives but doesn't drive rendering.
- Honest governance-gated affordance intersection
  (`compose.rs::honest_affordances`) is identity. Real wiring lands
  with M2's active-radar loop.
- The `surface → gui-egui` cycle was *broken* (per CHANGELOG 0.6.19)
  but the composer still lives in `gui-egui` — the cycle is broken
  by the composer not being in `clawft-surface`, not by extracting
  canon types. The "real" fix (canon-types-crate extraction) is
  unscheduled.

### Sensor-framework deferred items

The `.planning/sensors/` track proposes an upgraded substrate path
layout that is not yet in code:

- `JOURNALED-SENSOR-MIC.md` calls for
  `substrate/<node-id>/sensor/mic/{summary,pcm}` with ESP32-signed
  publishes; in-tree the `MicrophoneAdapter` still emits the legacy
  flat `substrate/sensor/mic` summary only (no `pcm`, no node-scoped
  path).
- `JOURNALED-NODE-ESP32.md` requires the per-node-prefix write gate
  (only the owning node may write `substrate/<node-id>/*`); the
  `substrate.publish` handler verifies signatures but the typed
  prefix rule is not yet expressed.
- `HEALTHCHECK-CONTRACT.md` describes a per-sensor health shape;
  nothing emits this yet.
- `EXPLORER-MANAGEMENT-SURFACE.md` management affordances
  (claim-node / unclaim-sensor / re-key / reset-calibration) are
  unscheduled.
- `PIPELINE-PRIMITIVE-{JOURNAL,SPIKE}.md` `SensorStage` shape has no
  in-tree representation.
- Only `Characterization::Rate` is exercised (mic). No `Presence` /
  `Enumerated` / `Spectral` / `Identifying` adapter ships.
- Sensor RPC catalogue (start / stop / recalibrate / re-key /
  publish-window) is not enumerated; only the mic's allowlist
  extension landed.

### Open questions (carried from ROADMAP / ADRs)

- **App distribution / sandboxing / localisation / multi-avatar** —
  ROADMAP §Round 3 holds these for after the first third-party app.
- **Ontology shape URIs.** `TopicDecl::shape` is a `&'static str`
  placeholder; no formal schema registry yet (ADR-020+, paired with
  SHACL-vs-TopologySchema).
- **Permissions-as-affordances vs permissions-as-filters.** Partly
  resolved (ADR-015 install-consent + ADR-018 per-invocation); full
  resolution deferred. Affects tray chip surface.
- **ADR-006 rule 2 intersection** — blocked on goal-aggregate
  (ADR-008) in M2.
- **Variant reconciliation across participants** (session-3 §6) —
  `variant-id` stamping is the M1.5-side surface.
- **Confidence-visual uniformity** (sessions 5/7) — renderer
  treatment varies; pick one language.
- **Goal vs Task vs Milestone naming** (session-8); affects the
  `goal_id` thread through the substrate RPC `actor_id` chain.

### Orphaned / ambiguous work

- `BlockKind` legacy demo enum vs the new Apps tab — duplicate code
  path; ROADMAP §Handoff defers the delete to M1.7+.
- `clawft-surface/src/substrate.rs` is a 14-line shim re-exporting
  `OntologySnapshot` from `clawft_substrate::snapshot`. Decide: keep
  indirection or fold into `lib.rs`.
- `clawft-surface/Cargo.toml` keeps `egui = "0.34"` with a comment
  "kept for now; can drop once eval.rs is fully UI-free." `eval.rs`
  no longer references egui; dep can be dropped, unblocking the
  surface crate for non-egui consumers (browser canvas, terminal).
- Mic adapter implicit invariant: `mic.rs::poll_level` short-circuits
  emission when the source file is missing so it doesn't clobber
  external `substrate.publish` writes (ESP32). Correct behaviour but
  unspecified — `JOURNALED-SENSOR-MIC.md` should formalise the
  external-publisher / internal-adapter-quiesce contract.
- `ManifestParseError::Serialize` wasm32 path unverified (the
  `web-time` switch handled `SystemTime`, not `toml::ser::Error`).

## Task List

Candidate M1.5-tail beads, grouped (not prioritised); each <30 LoC
unless flagged.

`clawft-app`:

1. Decide `UnknownMode` validation variant — wire Rust-side check or
   delete.
2. Registry corruption quarantine path (rename → `apps.json.corrupt-<ts>`).
3. Lifecycle teardown on `uninstall` while enabled — emit a
   tombstone the compositor catches when M1.6+ hooks land.
4. Wire ADR-015 rule 6 once `clawft-adapter` exists.
5. Cover wasm `to_toml_string` failure path with a negative test.

`clawft-substrate`:

6. Emit `substrate/meta/adapter/<id>/health` from each adapter.
7. Per-id `Replace` + `Remove` deltas on `processes` / `services`
   once the daemon contract grows row ids.
8. Surface `Subscription closed` on adapter teardown via the
   adapter-health topic (today the drain task silently exits).
9. Migrate mic adapter to `substrate/<node-id>/sensor/mic/{summary,pcm}`;
   ship the `pcm` windowed-`Append` topic.
10. Second `Characterization` exemplar (`Enumerated` rfkill or
    `Spectral` FFT-mic).
11. Cross-platform `network` / `bluetooth` impls (macOS / Windows) —
    or document Linux-only.

`clawft-surface`:

12. Wire the 13 stub-leaf canon primitives in the composer
    (Field/Toggle/Select/Slider, Plot, Media, Canvas, Foreign,
    Modal/Tabs/Sheet/Dock/Tree). ~80 LoC each.
13. `.first` / `.last` field-access shorthand support.
14. `sort(list, key)` evaluator function + parser arity.
15. Scientific / hex number literals in the expression parser.
16. `[compositions.*]` parser path + composer expansion.
17. Drop unused `egui` dep from `clawft-surface/Cargo.toml`.
18. Extract canon types to a shared crate; move composer back into
    `clawft-surface`.
19. Replace 14-line `src/substrate.rs` shim with a direct re-export.

Integration:

20. Real `governance::Gate` backed by ADR-012; plumb through
    `Substrate::subscribe_adapter`.
21. `affordance ∩ permit` honest intersection in
    `surface_host::compose::honest_affordances`.
22. `variant_id` stamping in `CanonResponse` driven by surface binding.
23. Per-sensor healthcheck contract emitter (cross-cuts 6 + 9).

Substrate RPCs:

24. Per-node-prefix write gate on `substrate.publish` (typed rule).
25. Streaming log endpoint so kernel adapter drops the poll fallback.
26. Test `substrate.notify` consumer wakeup semantics in the
    integration suite.

Sensor framework + admin app:

27. Ship a `Presence` exemplar adapter.
28. Implement `HEALTHCHECK-CONTRACT.md` as a
    `clawft-substrate::healthcheck` module.
29. Resolve legacy-flat-path vs node-scoped-path naming
    (`ADOPTION.md` flags the decision, no migration PR exists).
30. Add a wired Modal to the admin surface ("confirm restart") to
    exercise the container outside the demo lab.
31. Migrate the auto-install-from-fixture flow off the `web-time`
    workaround once a real install pipeline exists.

## Sources

Crate code:

- `/home/aepod/dev/clawft/crates/clawft-app/src/{lib,manifest,validation,registry,lifecycle}.rs`
- `/home/aepod/dev/clawft/crates/clawft-substrate/src/{lib,adapter,delta,snapshot,projection,kernel,network,bluetooth,mesh,chain,physical,mic}.rs`
- `/home/aepod/dev/clawft/crates/clawft-surface/src/{lib,tree,builder,eval,substrate,parse/mod,parse/expr,parse/toml}.rs`
- `/home/aepod/dev/clawft/crates/clawft-gui-egui/src/surface_host/{mod,compose,test_harness}.rs`
- Substrate RPCs: `/home/aepod/dev/clawft/crates/clawft-weave/src/daemon.rs` (handlers ~2210–2920) +
  `/home/aepod/dev/clawft/crates/clawft-weave/tests/substrate_rpc.rs`

Tests + fixtures:

- `/home/aepod/dev/clawft/crates/clawft-substrate/tests/mock_adapter.rs`
- `/home/aepod/dev/clawft/crates/clawft-surface/tests/{eval_bindings,roundtrip,weftos_admin_builder}.rs`
- `/home/aepod/dev/clawft/crates/clawft-gui-egui/tests/admin_app_e2e.rs`
- `/home/aepod/dev/clawft/crates/clawft-app/fixtures/weftos-admin.toml`
- `/home/aepod/dev/clawft/crates/clawft-surface/fixtures/weftos-admin-desktop.toml` + `weftos-chip-{audio,bluetooth,exochain,kernel,mesh,tof,wifi}.toml`

ADRs (`/home/aepod/dev/clawft/.planning/symposiums/compositional-ui/adrs/`):
`adr-001-primitive-canon.md`, `adr-006-primitive-head.md`,
`adr-009-mission-console-seat-witness-anti-corruption.md`,
`adr-012-capture-privacy-invariants.md`, `adr-013-canvas-primitive.md`,
`adr-015-app-manifest.md`, `adr-016-surface-description.md`,
`adr-017-ontology-adapter-contract.md`,
`adr-018-ide-bridge-protocol.md`, `adr-019-input-modality-and-avatar.md`.

Planning docs:

- `/home/aepod/dev/clawft/.planning/symposiums/compositional-ui/ROADMAP.md` (M1.5 §, "Review-deferred follow-ups", "Round 3 open questions")
- `/home/aepod/dev/clawft/.planning/symposiums/compositional-ui/session-10-app-layer.md`
- `/home/aepod/dev/clawft/.planning/sensors/{JOURNALED-SENSOR-MIC,JOURNALED-NODE-ESP32,HEALTHCHECK-CONTRACT,EXPLORER-MANAGEMENT-SURFACE,PIPELINE-PRIMITIVE-JOURNAL,PIPELINE-PRIMITIVE-SPIKE}.md`
- `/home/aepod/dev/clawft/.planning/ontology/ADOPTION.md`
- `/home/aepod/dev/clawft/CHANGELOG.md` (0.6.19 entry).

<!-- TRIAGED-STAMP:BEGIN -->
## Triaged into Plane — 2026-04-28

All open items in this audit have been filed as Plane work items in the WeftOS workspace under the `ws13-app-substrate` label.

- **Range**: WEFT-410 … WEFT-440 (31 items)
- **Per cycle**: 0.8.x: 31
- **Triage spec**: `.planning/reviews/0.7.0-release-gate/triage/`
- **WEFT-N → name map**: `.planning/reviews/0.7.0-release-gate/triage/weft-mapping.json`

Per the project rule (CLAUDE.md → "Plane is the authoritative work tracker"): future updates to these items happen in Plane, not in this audit doc. This doc remains the source-of-truth for the original survey.
<!-- TRIAGED-STAMP:END -->
