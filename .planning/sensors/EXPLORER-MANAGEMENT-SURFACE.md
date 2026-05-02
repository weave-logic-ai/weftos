---
title: Explorer as management surface — what the user can DO from the panel
created: 2026-04-24
status: draft — post-Node/Actor-split ontology proposal
scope: concrete affordance list, reads-only vs Actions-needed split
depends_on:
  - .planning/ontology/ADOPTION.md (Workshop, Action Types as slots)
  - .planning/sensors/JOURNALED-NODE-ESP32.md
  - .planning/sensors/JOURNALED-SENSOR-MIC.md
  - .planning/sensors/HEALTHCHECK-CONTRACT.md
  - .planning/explorer/PROJECT-PLAN.md (the Explorer MVP)
post_migration: true
---

# Explorer management surface

## 0. Framing

The Explorer started as a read-only debug panel — one tree, one detail
viewer, no mutations. Phase 1 and 2 landed that. The question now is:
**what else does the Explorer do before it becomes a "management UI"?**

The answer is a list of affordances, each classified by:

- **Viewer / Object Type** that enables it.
- **Class**: `READS-ONLY` (ships today, no Actions pipeline), or
  `ACTIONS-REQUIRED` (slot-shaped, materializes when Actions pipeline
  lands).
- **Dependency**: other affordances or infrastructure it needs.

This is not a roadmap — it is a classification. Anything marked
`READS-ONLY` is shippable now as Explorer / Workshop / viewer work.
Anything marked `ACTIONS-REQUIRED` is a slot reserved in the
capability metadata of its Object Type so the eventual Actions
pipeline plugs in without reopening the type declarations.

## 1. Affordance list

Numbered for reference.

### 1. See node health

- **What**: viewing a node's current health snapshot (up/degraded/down,
  uptime, RSSI, free heap).
- **Viewer**: `HealthViewer` (proposed in `HEALTHCHECK-CONTRACT.md`).
- **Object Type**: `HealthReport` (proposed). Also surfaced as a chip
  by `NodeViewer`.
- **Class**: **READS-ONLY** — pure subscribe-and-render of
  `substrate/<node-id>/health`.
- **Needs**: `HealthReport` classifier + `HealthViewer`. That's it.

### 2. See sensor health + observed rate

- **What**: per-sensor status, observed vs configured Hz, error
  counter, last-emit age.
- **Viewer**: `HealthViewer` or `SensorViewer` header.
- **Object Type**: `HealthReport` + `Sensor`.
- **Class**: **READS-ONLY**.
- **Needs**: `Sensor` + `HealthReport` classifiers.

### 3. Drill into raw vs summary sensor emission

- **What**: toggle between "show me the meter-style summary" and "show
  me the raw payload / PCM chunk / frame."
- **Viewer**: `SensorViewer` with a child-pane switcher; raw view uses
  existing viewers (`JsonFallbackViewer` for PCM chunks, a future
  `FrameViewer` for camera).
- **Object Type**: `Sensor`.
- **Class**: **READS-ONLY**.
- **Needs**: `Sensor` classifier; SensorViewer's child-pane switcher.

### 4. Rename a node or sensor's friendly label

- **What**: user types a new label into a text field; the label writes
  to `substrate/<node-id>/meta/label` (node) or
  `substrate/<node-id>/sensor/<name>/meta/label` (sensor).
- **Viewer**: `NodeViewer` / `SensorViewer` — inline editable field.
- **Object Type**: `Node` / `Sensor`, with `applicable_actions`
  including `"node.rename"` / `"sensor.rename"`.
- **Class**: **ACTIONS-REQUIRED** — the user (an Actor) is not the node
  and cannot sign a publish into that node's namespace directly. The
  Actions pipeline is the mechanism by which the daemon accepts a
  user-initiated write to a node-scoped path (by wrapping it in an
  Action envelope signed by the user's Actor key, dispatched to the
  node, which re-publishes with its own signature).
- **Needs**: Action Type `rename`, actor-signing machinery, node-side
  action listener. All deferred.

### 5. Toggle a sensor on/off

- **What**: a switch on the SensorViewer header that sends a
  `sensor.stop` or `sensor.start` Action to the hosting node.
- **Viewer**: `SensorViewer`.
- **Object Type**: `Sensor` with `applicable_actions: ["sensor.start",
  "sensor.stop"]`.
- **Class**: **ACTIONS-REQUIRED** — toggle is a mutation on the node's
  runtime state. Requires an Action pipeline and node-side action
  listener.
- **Needs**: Action Types + node-side listener. Deferred.

### 6. Calibrate a sensor

- **What**: per-sensor-kind: recompute mic dBFS baseline, ToF zero
  offset, IMU bias. Fires `sensor.calibrate` Action.
- **Viewer**: `SensorViewer`.
- **Object Type**: `Sensor` with applicable action `"sensor.calibrate"`.
- **Class**: **ACTIONS-REQUIRED**. Additionally, calibration often
  needs a short protected-mode period ("don't publish for 3 s while I
  measure"), implying the Action has a life-cycle, not just a one-shot
  fire. Schema implication: Action Types carry a `mode: "immediate" |
  "lifecycle"` hint.
- **Needs**: Action pipeline + lifecycle-aware Action schema. Deferred.

### 7. Reboot a node

- **What**: fire `node.reboot` to a target node; destructive-ish.
- **Viewer**: `NodeViewer` — confirmation required.
- **Object Type**: `Node` with applicable action `"node.reboot"`.
- **Class**: **ACTIONS-REQUIRED**. Plus UI guard (two-click / type-to-
  confirm).
- **Needs**: Action pipeline + node-side reboot listener.

### 8. Subscribe a Workshop composition to a specific node

- **What**: a pre-built Workshop (e.g. `sensor-card`, `node-card`)
  gets bound to a specific substrate path so a user can open "the
  sensor-card for `n-6f3a9c/sensor/mic`" and have the layout render
  against that node's live data.
- **Viewer**: Workshop (existing `ui://workshop` primitive).
- **Object Type**: `Workshop` (existing) + parameterization extension
  (panels accept a `substrate_path_template` with a parameter).
- **Class**: **READS-ONLY** (no mutation of nodes). But **Workshop
  parameterization** is a missing primitive feature that must ship
  first.
- **Needs**: Workshop panels accept parameters. New work in
  `crates/clawft-gui-egui/src/explorer/workshop.rs`.

### 9. Filter the tree by node / by sensor-kind / by status

- **What**: pseudo-query on the left tree: "show only sensors" /
  "show only degraded nodes" / "show only `n-6f3a9c`'s subtree."
- **Viewer**: upgrade to `explorer/tree.rs` + a filter chip row at
  the top of the panel.
- **Object Type**: consumes `Node`, `Sensor`, `HealthReport` — the
  filter predicates read shape-inferred types.
- **Class**: **READS-ONLY**. Pure client-side filtering over already-
  fetched substrate.list results.
- **Needs**: filter UI; Object Type inference already exists.

### 10. See historical sparkline of a sensor's observed rate or a
    node's RSSI / free-heap

- **What**: a small time-series graph below the header of a node or
  sensor, showing the last N minutes of a selected scalar.
- **Viewer**: existing `TimeSeriesViewer` repurposed as a child
  primitive of `NodeViewer` / `SensorViewer`.
- **Object Type**: consumes `HealthReport` repeatedly over time.
- **Class**: **READS-ONLY**. TimeSeriesViewer already maintains a
  bounded history in `Mutex<HashMap<String, Vec<f64>>>`
  (`crates/clawft-gui-egui/src/explorer/viewers/time_series.rs`).
- **Needs**: just wiring — host TimeSeriesViewer as a child of
  NodeViewer/SensorViewer.

### 11. Jump from a sensor to its producing node and vice versa

- **What**: breadcrumb-style navigation. Clicking the node-id chip on
  SensorViewer selects the parent node path in the tree; clicking a
  sensor row on NodeViewer selects that sensor's path.
- **Viewer**: `NodeViewer` + `SensorViewer`, both emit a "select path"
  intent that the Explorer's `on_select` handles.
- **Object Type**: `Node` + `Sensor`.
- **Class**: **READS-ONLY**.
- **Needs**: a small viewer → Explorer "select" callback surface.

### 12. Copy a path / copy a node's pubkey / export a snapshot

- **What**: clipboard-write affordances — useful for operators and
  for debugging.
- **Viewer**: tree row context menu + NodeViewer field buttons.
- **Object Type**: agnostic — works on any path.
- **Class**: **READS-ONLY**. Pure clipboard, no substrate write.
- **Needs**: egui clipboard API; no ontology work.

### 13. Flag a sensor as "watched" and get a desktop notification on
    status change

- **What**: user marks a sensor, a small watcher service diffs its
  HealthReport, desktop notification on `healthy → degraded/stale/down`.
- **Viewer**: `SensorViewer` adds a watch toggle.
- **Object Type**: `Sensor` + `HealthReport`.
- **Class**: **MIXED**. The watch-list itself is actor state (where
  does it live? `substrate/actor/<actor-id>/watchlist/...`). Reading
  HealthReport is READS-ONLY; writing the watchlist is an `Action` —
  but a low-risk one that's a candidate for an early Actions-pipeline
  tracer-bullet target.
- **Needs**: Actions pipeline *or* a shortcut where the watchlist
  lives entirely client-side in the Explorer (saved to local config,
  not substrate). Proposed: start client-side-only, migrate to
  substrate-backed when Actions lands.

### 14. View the lineage graph of a derived path

- **What**: given `substrate/<daemon-node-id>/derived/transcript/<esp32-node-id>/mic`,
  show the graph: Whisper Actor ← (reads) sensor/mic/pcm_chunk on
  `n-6f3a9c` → publishes derived/transcript under daemon. Uses
  `ui://graph` primitive.
- **Viewer**: `GraphViewer`.
- **Object Type**: a new `Lineage` type (not proposed in this pass —
  flagging as a follow-up).
- **Class**: **READS-ONLY** in principle, but requires the lineage to
  be recorded somewhere the Explorer can read. Actors and nodes need
  to emit lineage tags alongside their derived-path publishes.
- **Needs**: convention for lineage metadata (e.g. derived publishes
  carry `{ lineage: { source_paths: [...], via_actor: "..." } }`).
  Separate work.

### 15. Tap-to-listen on a mic sensor

- **What**: stream a short window of the raw PCM to the user's audio
  output. Dev-only affordance — helpful for quick "is this mic hearing
  me" debug.
- **Viewer**: `SensorViewer` for mic specifically.
- **Object Type**: `Sensor` (sensitivity-gated).
- **Class**: **ACTIONS-REQUIRED** (Capture-tier).
- **Needs**: Action pipeline + CapabilityGrant flow + audio-out path.
  Deferred.

## 2. Summary table

| # | Affordance | Class | Viewer / Object Type | Ships with |
|---|---|---|---|---|
| 1 | See node health | READS-ONLY | HealthViewer / Node+HealthReport | Migration pass |
| 2 | See sensor health + rate | READS-ONLY | HealthViewer / Sensor+HealthReport | Migration pass |
| 3 | Drill raw vs summary | READS-ONLY | SensorViewer / Sensor | Migration pass |
| 4 | Rename label | ACTIONS-REQUIRED | NodeViewer+SensorViewer / Node+Sensor | Actions pipeline |
| 5 | Toggle sensor on/off | ACTIONS-REQUIRED | SensorViewer / Sensor | Actions pipeline |
| 6 | Calibrate sensor | ACTIONS-REQUIRED (lifecycle) | SensorViewer / Sensor | Actions + lifecycle-aware schema |
| 7 | Reboot node | ACTIONS-REQUIRED | NodeViewer / Node | Actions pipeline |
| 8 | Subscribe Workshop to node | READS-ONLY (needs param) | Workshop | Workshop parameterization |
| 9 | Filter tree by type/status | READS-ONLY | tree / Node+Sensor+HealthReport | Small UI add |
| 10 | Historical sparkline | READS-ONLY | TimeSeriesViewer / HealthReport | Wiring only |
| 11 | Navigate sensor↔node | READS-ONLY | NodeViewer+SensorViewer | Wiring only |
| 12 | Copy path / pubkey / export | READS-ONLY | any | Clipboard API |
| 13 | Watch sensor, notify on state change | MIXED | SensorViewer | Client-side MVP |
| 14 | Lineage graph | READS-ONLY (needs metadata) | GraphViewer / new Lineage type | Lineage convention |
| 15 | Tap-to-listen mic | ACTIONS-REQUIRED (Capture) | SensorViewer / Sensor | Actions + CapabilityGrant |

## 3. What to ship first

The "migration pass" (the immediate code pass that lands post-Node/Actor
split) naturally bundles these READS-ONLY affordances:

- 1, 2, 3, 10, 11, 12 — everything that drops out of the
  `Node` / `Sensor` / `HealthReport` Object Types + paired viewers.
- 9 as a stretch if filter UI is cheap.

That's a meaningful, coherent slice of "Explorer becomes a management
UI" without touching Actions.

## 4. What comes next (ordered)

- **Workshop parameterization** → unlocks #8.
- **Watchlist client-side storage** → unlocks #13 MVP (no Actions
  needed yet).
- **Lineage metadata convention** → unlocks #14.
- **Actions pipeline** (large, separate work) → unlocks #4, #5, #6,
  #7, #13-full, #15.

## 5. Governance-slot observations

Per ADOPTION.md §7, every Action Type carries a pre-commit hook slot
(initially `allow_all()`), an edit-visibility tag slot (empty), and an
audit sink slot (`/dev/null`). For each ACTIONS-REQUIRED affordance
above, the slots that matter first:

- `node.rename`, `sensor.rename` — low-risk, pre-commit can stay
  `allow_all()`; audit sink is probably worth piping to the chain
  even in MVP, because rename is a cheap way to exercise "can we
  actually record an Action in the chain?"
- `sensor.start`, `sensor.stop`, `sensor.calibrate`, `node.reboot` —
  operational, need a non-trivial pre-commit (at minimum: the invoker
  must be an Actor with a role that allows this; which means the
  permission system has to exist in at least stub form).
- `tap-to-listen` — Capture-tier, demands the CapabilityGrant flow
  that ADR-012 gestured at. The pre-commit hook for this one is *not*
  optional.

Explicitly: the migration pass does NOT commit to governance fills.
It only reserves the slots on the Object Type `capabilities()` entries,
so when the Actions pipeline does land, the per-type applicable-action
list already exists and viewers don't need type surgery.

## 6. Open questions

1. **Where does client-side watchlist state live?** Proposal: a tiny
   TOML/JSON file in the Explorer's user-config directory
   (`~/.config/weftos/explorer-watchlist.toml`). Explicitly *not*
   substrate, because it's per-user-per-client state, not mesh truth.
   Migrates to `substrate/actor/<actor-id>/watchlist/` once Actors
   and Actions exist.
2. **Workshop parameterization schema.** What does
   `{ "substrate_path_template": "substrate/${node}/sensor/mic", "params": { "node": "n-6f3a9c" } }`
   look like in the Workshop JSON? Alternative: parameters are
   injected at mount-time, not baked in. Sign-off needed on the
   template syntax.
3. **Lineage metadata placement.** Should derived-path publishes
   carry `lineage` as an inline field, or at a sibling
   `<derived-path>/meta/lineage`? Proposal: sibling path, so the
   main publish envelope stays small. Sign-off needed.
4. **"Open in Workshop" button on every viewer?** When a user is
   looking at a `NodeViewer`, should there be a "convert to
   Workshop" button that materializes an equivalent composition at
   `substrate/ui/workshop/<auto-name>` they can edit? Interesting
   as a democratization story (#8 + #4 combined). Deferred.
5. **Notification channel for #13.** Desktop notifications on WSL
   / native / WASM are three different mechanisms. Start with native
   only; WSL and WASM get follow-up tickets.
