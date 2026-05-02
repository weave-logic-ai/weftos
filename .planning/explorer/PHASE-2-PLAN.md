---
title: Explorer / Ontology — Phase 2 sequencing
created: 2026-04-23
status: plan — ordering approved in chat; work imminent
depends_on: Phase 1 Explorer merged into development-0.7.0
tracks: 5
execution: 2 waves, worktree-parallel within each wave
---

# Phase 2 — sequencing

## 0. Why this doc

Phase 1 lands the Explorer skeleton + shape-matched viewer registry. The architectural conversation in this session (see `.planning/ontology/ADOPTION.md`, `.planning/sensors/PIPELINE-PRIMITIVE-SPIKE.md`, `.planning/ontology/palantir-foundry-research.md`) expanded Phase 2 from "grow the viewer registry" into five tracks. This doc sequences them, identifies dependencies, and defines the worktree layout for parallel execution.

## 1. The five tracks

| # | Track | One-line purpose | Origin |
|---|---|---|---|
| 1 | **Viewer growth** | Add specialized viewers for paths currently rendered as JsonFallback | `.planning/explorer/PROJECT-PLAN.md` §4 |
| 2 | **Object Types** | First concrete Object Type primitive + capability metadata + registry | `.planning/ontology/ADOPTION.md` §6, Step 2 |
| 3 | **`ui://graph` primitive** | Vertex analog — graph-shaped viewer primitive serving Explorer + ⊃μBus | `.planning/ontology/ADOPTION.md` §9 |
| 4 | **Whisper pipeline spike** | Machinery + Functions probe — the real pipeline primitive driver | `.planning/sensors/PIPELINE-PRIMITIVE-SPIKE.md` |
| 5 | **Config-driven hot-reload composition** | Workshop-at-substrate-path, live GUI reconfig — the vector-synth unblocker | `.planning/ontology/ADOPTION.md` Step 3 |

## 2. Dependency graph

```
Phase 1 (merged)
   │
   ├── Track 1 (Viewers)         — independent, no deps above Phase 1
   │
   ├── Track 2 (Object Types)    — independent primitive
   │       │
   │       └── Track 5 (Workshop)   — wants ObjectType trait; can start with a
   │                                  minimal untyped Workshop shape that Track 2
   │                                  retrofits on landing
   │
   ├── Track 3 (ui://graph)      — independent viewer primitive; no deps
   │
   └── Track 4 (Whisper spike)   — independent; new crate; external build deps
```

- Tracks 1, 2, 3, 4 **have no internal dependencies** — all four can start immediately after Phase 1 merges.
- Track 5 wants Track 2's `ObjectType` trait; can start in parallel with a **temporary untyped Workshop shape** (plain JSON at `substrate/ui/workshop/<name>`) that gets promoted to an `ObjectType` when Track 2 lands. Slot-not-fill pattern applied to typing.
- No track depends on any other track landing first to compile.

## 3. File-conflict analysis for parallel execution

| Track | Primary files touched | Conflict risk with other tracks |
|---|---|---|
| 1 | `crates/clawft-gui-egui/src/explorer/viewers/{waveform,mesh_nodes,chain_tail,time_series,process_table}.rs` + `viewers/mod.rs` registrations | 3 (both modify `viewers/mod.rs`); 5 (may add viewer registrations) |
| 2 | new module `crates/clawft-gui-egui/src/ontology/` (or new crate `clawft-ontology`) + registration surface | low — new module |
| 3 | `crates/clawft-gui-egui/src/explorer/viewers/graph.rs` + `viewers/mod.rs` + new Cargo dep (`egui_node_graph` or `egui_graphs`) | 1 (both modify `viewers/mod.rs`) |
| 4 | new crate `clawft-service-whisper` + `Cargo.toml` workspace member add + possibly `clawft-substrate` for binary payload support | low — new crate; workspace root Cargo.toml add is tiny |
| 5 | new `clawft-gui-egui/src/explorer/workshop.rs` + `explorer/mod.rs` modifications + substrate subscribe wiring | medium — touches explorer infra |

**Mitigation: marker-comment pattern** proven in Phase 1 (`// [[VIEWERS_MODULES_INSERT]]`, `// [[VIEWERS_REGISTRATIONS_INSERT]]`). Each track's agent appends registrations into its own marker block; merge is mechanical.

## 4. Execution plan — two waves

Disk and CPU contention during Phase 1 (3 worktrees, ~4 hours of concurrent cargo builds) was real. Five parallel tracks would be worse. Split into two waves by resource profile, not by dependency.

### Wave A — least-conflict, highest-resource-heterogeneity

**Tracks 2 (Object Types) + 4 (Whisper spike)**

- Track 2 is clawft-gui-egui-side and introduces a new module (light new deps).
- Track 4 is a new crate with FFI to whisper.cpp (heavy C++ build, but isolated — doesn't touch shared crates much).
- Zero file overlap between the two.
- Different dependency graphs → different compile hotspots → less mutual contention.

Run **in parallel** in two worktrees.

### Wave B — after Wave A lands

**Tracks 1 (Viewers) + 3 (ui://graph) + 5 (Workshop)**

- Tracks 1 and 3 both register new viewers; both modify `explorer/viewers/mod.rs`. Use marker-comment pattern.
- Track 5 may register a `WorkshopViewer` too + adds the hot-reload subscription infra.
- All three are clawft-gui-egui-side with more overlap than Wave A.

Run **in parallel** in three worktrees using marker-comment coordination. If this proves too conflict-heavy on merge, fall back to serial within Wave B.

## 5. Worktree layout

After Phase 1 merges, prune the phase1 worktrees and create phase2 worktrees:

```
/home/aepod/dev/clawft                          (main,       development-0.7.0)

Wave A worktrees:
/home/aepod/dev/clawft-wt/phase2-object-types   (phase2-object-types branch)
/home/aepod/dev/clawft-wt/phase2-whisper-spike  (phase2-whisper-spike branch)

Wave B worktrees (created after Wave A merges):
/home/aepod/dev/clawft-wt/phase2-viewers        (phase2-viewers branch)
/home/aepod/dev/clawft-wt/phase2-ui-graph       (phase2-ui-graph branch)
/home/aepod/dev/clawft-wt/phase2-workshop       (phase2-workshop branch)
```

All branches base off `development-0.7.0` after Phase 1 merges.

## 6. Acceptance criteria per track

### Track 1 — Viewer growth

- At minimum three of: `WaveformViewer`, `MeshNodesViewer`, `ChainTailViewer`, `TimeSeriesViewer`, `ProcessTableViewer`.
- Each conforms to `SubstrateViewer` trait; priority 10; registered via marker-comment insert.
- `matches()` positive + negative unit tests per viewer.
- At least one existing chip panel becomes eligible for replacement by its substrate-path Explorer view (user-facing verification: the Explorer renders the chip's data as well or better than the chip panel).
- `scripts/build.sh check/clippy/test` clean; WASM rebuilt.

### Track 2 — Object Types

- `ObjectType` trait with `matches(value) -> u32`, `name()`, `properties() -> &[PropertyDecl]`, `default_viewer_priority()`, and capability metadata hooks (applicable_actions, events_emitted — may be empty for MVP).
- Registry with shape-based dispatch (same priority cascade as viewers; JsonFallback equivalent for untyped paths).
- **Mesh** as root Object Type with one or two declared properties; two other concrete types promoted from existing substrate shapes (`Mic` and one of `ChainEvent` / `MeshNode`).
- Tree view (from Phase 1) shows an Object Type badge next to paths whose shape matches a registered type.
- Unit tests per type's `matches()`; integration test verifying badge renders.
- `scripts/build.sh check/clippy/test` clean.

### Track 3 — `ui://graph` primitive

- Library decision recorded: `egui_node_graph` vs `egui_graphs` vs roll-own, with rationale.
- `GraphViewer` matches `{ nodes: [...], edges: [...] }` shape (schema TBD within the track).
- Read-only MVP is acceptable; editable is explicit stretch goal.
- At least one substrate path rendered by the viewer — the Explorer tree's own shape could serve as the first real input, or a synthetic fixture.
- `matches()` + `paint()` tests + at least one integration that actually paints a small graph and doesn't panic.
- `scripts/build.sh check/clippy/test` clean; WASM rebuilt.

### Track 4 — Whisper pipeline spike

- New crate `clawft-service-whisper` wrapping `whisper-rs`. Model load at init; subscribe loop running.
- Subscribes to `substrate/sensor/mic/pcm_chunk`; publishes to `substrate/derived/transcript/mic`.
- Binary payload handling: decide between b64-in-JSON vs native binary substrate support; document the decision in `PIPELINE-PRIMITIVE-JOURNAL.md` and implement.
- If the ESP32 firmware isn't yet publishing PCM, ship a test harness that publishes synthetic PCM from a WAV file to verify the service end-to-end.
- Journal doc `PIPELINE-PRIMITIVE-JOURNAL.md` captures every "ugh should be general" moment and answers the six concrete questions in `PIPELINE-PRIMITIVE-SPIKE.md` §4.3.
- `scripts/build.sh check/clippy/test` clean.

### Track 5 — Config-driven hot-reload composition

- `Workshop` struct (untyped shape OK for MVP, typed via Track 2 later): `{ title, layout, panels: [{ substrate_path, viewer_hint, position }] }`.
- GUI subscribes to `substrate/ui/workshop/<name>`; publishes there cause live GUI reconfigure with no reload.
- At least one non-GUI writer shipped: a TOML-file watcher under `scripts/` or a small binary that reads a local `.toml` and pushes to substrate.
- Demonstrated end-to-end: edit TOML → file watcher publishes → Workshop shape changes in the GUI visibly within ~1 s.
- Unit tests for subscribe → reconfigure lifecycle; integration test with an in-process substrate client.
- `scripts/build.sh check/clippy/test` clean; WASM rebuilt.

## 7. Shared conventions across all Phase 2 tracks

- **`scripts/build.sh` for all build/test/check/lint** — no raw cargo unless debugging a script-uncovered case.
- **Commit on own branch only; never to master** (project CLAUDE.md + user's global rule).
- **Never `git add -A`** — stage named files.
- **Commit messages**: conventional `type(scope): subject` + body + `Co-Authored-By: claude-flow <ruv@ruv.net>` footer.
- **Rebuild the WASM bundle** (`extensions/vscode-weft-panel/scripts/build-wasm.sh`) whenever GUI changes affect the webview path — per the Explorer-phase rule.
- **Marker-comment insertion pattern** for coordinating edits to shared files like `viewers/mod.rs`.
- **Do NOT merge from your branch** — merge decisions come back to the main checkout.

## 8. Merge discipline

After each Wave, merge order for the wave's branches:

1. Merge the Wave's smallest-surface branch first (least likely to need rebase).
2. Rebase (or merge) the next branch onto the just-updated `development-0.7.0`.
3. Resolve marker-comment conflicts manually — they are designed to be mechanical.
4. Run `scripts/build.sh gate` (all 11 checks) on the merged tip before the next wave starts.

Phase 1's merge precedent: backend → panel → viewers. Viewers/mod.rs conflict resolved by taking panel's trait + JsonFallback infra and splicing viewers' three module decls + registrations at the marker comments.

## 9. What Phase 2 explicitly does NOT do

- Does NOT build the Ontology Manager governance workflow (slot-shape only, per ADOPTION §7).
- Does NOT build Action Types with real pre-commit policy — slots reserved, empty.
- Does NOT build Interfaces (polymorphism-over-Actions tradeoff is inherited from Foundry; deferred).
- Does NOT build the LLM composer (Step 7 of the ADOPTION staircase).
- Does NOT build cross-mesh federation (Meshes as Link-connected Objects is a future ADR).
- Does NOT build writebacks / external-system propagation.

These are the explicit non-goals already recorded in ADOPTION §13 and carried forward here.

## 10. Post-Phase-2 state

When all five tracks land, the system has:

- **Substrate** with binary-payload support (from Track 4).
- **Object Types** as a first-class typed layer with capability metadata (Track 2).
- **Five+ specialized viewers** covering the majority of current substrate shapes (Track 1).
- **Vertex-style graph viewer** for patches, dependencies, any graph-shaped value (Track 3).
- **Workshop composition primitive** with substrate-resident config and live hot-reload (Track 5).
- **Whisper service** running as the first ingestion pipeline, producing journal artifacts that inform the pipeline primitive proposal (Track 4).

That state unblocks the next round of work: first real `ui://workshop` user-facing surfaces (hand-composed dashboards), the ⊃μBus patch UI (via `ui://graph`), the second ingestion pipeline (camera or ToF — validates the pipeline primitive axes), and the beginning of manual drag-drop composition (Step 4 of the ADOPTION staircase).

## 11. References

- `.planning/explorer/PROJECT-PLAN.md` — Phase 0/1 plan, Phase 2 original scope (viewer growth only)
- `.planning/ontology/ADOPTION.md` — architectural direction informing Tracks 2, 3, 5
- `.planning/ontology/palantir-foundry-research.md` — research base for ontology model
- `.planning/sensors/PIPELINE-PRIMITIVE-SPIKE.md` — Track 4 brief
- `docs/handoff.md` — session state
