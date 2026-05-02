---
title: Healthcheck contract — the generic shape every node and sensor satisfies
created: 2026-04-24
status: draft — post-Node/Actor-split ontology proposal
scope: one contract, two granularities (node-level, sensor-level), one Object Type
depends_on:
  - .planning/ontology/ADOPTION.md
  - .planning/sensors/JOURNALED-NODE-ESP32.md
  - .planning/sensors/JOURNALED-SENSOR-MIC.md
post_migration: true
---

# Healthcheck contract

## 0. Purpose

Every node and every sensor in the mesh must publish a health snapshot the
Explorer (and any automation) can read to answer two questions:

1. **Is this thing alive right now?** — staleness detection, offline
   chips, grey-out rules.
2. **Is this thing working correctly?** — observed-vs-configured rate,
   error counters, degradation reasons.

Without a shared contract, every producer invents its own "am I ok?"
field and every consumer writes bespoke parsing. With one, we ship one
classifier (`HealthReport` Object Type), one viewer, and one diff engine.

This document specifies the contract. The Object Type proposal is
§5. Concrete example emissions are in `JOURNALED-NODE-ESP32.md` §3.2
(node-level) and `JOURNALED-SENSOR-MIC.md` §4 (sensor-level).

## 1. Two granularities

Health is emitted at two path-granularities:

- **Node-level** at `substrate/<node-id>/health` — one per node.
- **Sensor-level** at `substrate/<node-id>/health/sensor/<sensor-name>`
  — one per sensor hosted by that node.

These share a shape (see §3) with a small number of granularity-specific
fields (§2.2 vs §3.2). The Explorer treats them uniformly; the author
decides which fields to populate based on granularity.

## 2. Node-level HealthReport

### 2.1 Shape

```json
{
  "status": "healthy",
  "uptime_s": 84210,
  "firmware_version": "0.7.0-phase2",
  "rssi_dbm": -56,
  "free_heap_bytes": 148320,
  "last_publish_ts": 1714000000000,
  "reboot_reason": "power-on",
  "boot_count": 17,
  "tick": 168420
}
```

### 2.2 Fields (node-level specifics)

| field | kind | required | semantics |
|---|---|---|---|
| `status` | string enum | yes | `"healthy" \| "degraded" \| "down"`. See §4 for transition rules. |
| `uptime_s` | u64 | yes | Seconds since last boot. |
| `firmware_version` | string | yes | Semver-ish. Used by Explorer to flag mismatched nodes. |
| `rssi_dbm` | i64 | optional | WiFi signal, negative dBm. Omit for wired nodes (daemon-host). |
| `free_heap_bytes` | u64 | optional | Smallest-free-block heap. Omit for nodes without exposed heap stats. |
| `last_publish_ts` | u64 | yes | Wall-clock millis of this node's last publish on *any* path. |
| `reboot_reason` | string enum | optional | `"power-on" \| "panic" \| "watchdog" \| "software-reset" \| "deep-sleep-wake" \| "unknown"`. |
| `boot_count` | u64 | optional | Monotonic counter persisted across boots. |
| `tick` | u64 | yes | Producer's monotonic tick counter. |

### 2.3 Publish cadence

**5 seconds** nominal. Acceptable drift: up to 15 s before Explorer
flags the node as stale. Nodes may publish faster during high-change
moments (e.g. right after boot) — consumers must not assume strict
periodicity.

## 3. Sensor-level HealthReport

### 3.1 Shape

```json
{
  "status": "healthy",
  "last_emit_ts": 1714000000000,
  "configured_rate_hz": 2.0,
  "observed_rate_hz": 1.98,
  "error_count": 0,
  "since_ms": 84210,
  "last_error": null,
  "notes": null,
  "tick": 168420
}
```

### 3.2 Fields (sensor-level specifics)

| field | kind | required | semantics |
|---|---|---|---|
| `status` | string enum | yes | `"healthy" \| "degraded" \| "stale" \| "down"`. Sensor-level adds `"stale"` (below). |
| `last_emit_ts` | u64 | yes | Wall-clock millis of this sensor's last successful payload publish (the `sensor/<name>/<whatever>` emission, not this health record). |
| `configured_rate_hz` | f64 | yes | Nominal emission rate the sensor is configured for. |
| `observed_rate_hz` | f64 | yes | Rolling-window measured rate. Computed by the health aggregator. |
| `error_count` | u64 | yes | Monotonic counter of errors this sensor has accumulated since boot. |
| `since_ms` | u64 | optional | Millis the sensor has been in its current `status` state. Lets Explorer render "down for 2m 14s". |
| `last_error` | string\|null | optional | Short human-readable last error message, if any. `null` when none. |
| `notes` | string\|null | optional | Free-text diagnostic, e.g. `"WARN: I2S DMA reported underrun"`. `null` when none. |
| `tick` | u64 | yes | Producer's monotonic tick counter. |

### 3.3 Publish cadence

**Every emit, or at least every N=4 emits (≥ 0.5 Hz)**, whichever is
slower. Rationale: at 2 Hz emit cadence, a health publish every emit is
fine (cheap); at 100 Hz emit cadence (IMU), a health publish every emit
floods the tree — every 4 emits = 25 Hz is still fast enough for
Explorer's visual refresh and keeps traffic bounded.

Minimum publish cadence: **1 per second**, so staleness detection has
a clean signal.

## 4. Status transitions

### 4.1 Node-level

- `healthy` → `degraded` when any of: RSSI below `-85 dBm` sustained 10 s;
  heap below 10% of peak-free over any 30 s window; firmware mismatch
  detected by the mesh (other nodes on a newer version).
- `degraded` → `down` when `last_publish_ts` has not advanced for 30 s.
- `healthy` → `down` directly when `last_publish_ts` is older than 30 s
  (e.g. node vanished without a degraded phase).

### 4.2 Sensor-level

- `healthy` → `stale` when `observed_rate_hz < 0.5 * configured_rate_hz`
  sustained for 3 s.
- `stale` → `down` when `observed_rate_hz == 0` for 10 s.
- `healthy` → `degraded` when `error_count` increments in the current
  window (the most recent N emits).
- `degraded` → `healthy` when no new errors in 10 s AND rates back in
  range.

These thresholds are tunable — config-driven knobs on the health
aggregator. They are the defaults.

## 5. `HealthReport` — proposed Object Type

### 5.1 Proposal

- **`name()`**: `"health_report"`
- **`display_name()`**: `"Health Report"`
- **Priority**: `8` — below specialized leaf types like `AudioStream`
  (10), so when a path accidentally has both a `status` field AND RMS/
  peak fields the audio type still wins. Above catch-alls.
- **`matches(value)`**: returns `8` when the value is an object with all
  of:
  1. `status` field holding a string that's one of the declared enums
     (`healthy | degraded | stale | down`).
  2. At least ONE of: `uptime_s` (u64), `last_emit_ts` (u64),
     `observed_rate_hz` (f64) — i.e. at least one of the node-specific
     or sensor-specific required fields.

  This two-clause test lets the same type match both granularities while
  still rejecting random `{ status: "ok" }` blobs from unrelated code.

- **Properties** (union; granularity-specific fields are optional):

  | name | kind | doc |
  |---|---|---|
  | `status` | String | Enum: healthy \| degraded \| stale \| down |
  | `tick` | U64 | Producer's monotonic tick counter |
  | `uptime_s` | U64 | (Node) seconds since boot |
  | `firmware_version` | String | (Node) firmware semver |
  | `rssi_dbm` | I64 | (Node) WiFi signal strength |
  | `free_heap_bytes` | U64 | (Node) smallest-free-block heap |
  | `last_publish_ts` | U64 | (Node) last publish wall-clock ms |
  | `reboot_reason` | String | (Node) enum |
  | `boot_count` | U64 | (Node) persisted counter |
  | `last_emit_ts` | U64 | (Sensor) last sensor payload ms |
  | `configured_rate_hz` | F64 | (Sensor) nominal rate |
  | `observed_rate_hz` | F64 | (Sensor) measured rate |
  | `error_count` | U64 | (Sensor) error counter |
  | `since_ms` | U64 | (Sensor) time in current status |
  | `last_error` | String | (Sensor) last error message |
  | `notes` | String | Free-text diagnostic |

- **Paired viewer**: a new `HealthViewer` — renders a compact card:
  - **Status chip** (green / yellow / orange / red).
  - **One-line summary** — the most relevant numeric:
    - Node: "up 23h · RSSI -56 dBm · 145 KB free"
    - Sensor: "1.98/2.0 Hz · 0 errors · last 412 ms ago"
  - **Expand/collapse** into a simple field table for raw values.

- **`capabilities()`**:
  - `applicable_actions`: empty MVP; future: `"health.ack"`,
    `"health.mute-alert"`.
  - `events_emitted`: `["health.status-changed"]`.

### 5.2 Classifier notes

- The `Node` and `Sensor` Object Type matchers both look for a nested
  `health` child. They do *not* recurse into it to verify HealthReport
  shape — that's `HealthReport`'s job when the user drills into the
  health subtree specifically.
- Because health lives at a *sibling* path (`health/sensor/<name>`,
  not inside `sensor/<name>/health`), the classifier for `Sensor` has
  a choice: claim the parent AND the health child, or only the parent.
  **Recommended: only the parent.** The health subtree has its own
  dedicated shape and viewer; letting it stand on its own makes the
  healthcheck contract surfaces uniformly through the tree.

## 6. How Explorer uses this

### 6.1 Tree decoration (live now, no Actions needed)

- **Staleness greying**: if the selected path's node OR nearest
  ancestor health report says `"stale"` or `"down"`, the row and all
  descendants render at 60% opacity.
- **Status dot**: the existing `ACTIVITY_WINDOW` dot in the tree
  (`crates/clawft-gui-egui/src/explorer/mod.rs:35`) already marks
  recently-updated paths. Upgrade: color it by health status when a
  HealthReport is in the inferred ancestor chain — green dot for
  healthy, yellow for degraded, red for down.
- **Tooltip**: hover a node or sensor row → show the summary line
  from the HealthViewer.

### 6.2 Detail-pane chip

- When the selected value is a Node-typed or Sensor-typed path,
  the header of `NodeViewer` / `SensorViewer` shows the status chip
  derived from the nearest HealthReport (same-level sibling, not a
  recursive lookup — nodes have `health` at their root, sensors have
  `health/sensor/<name>` at the node's root).

### 6.3 Automation (deferred to Actions pipeline)

- Stale-sensor alerts.
- Auto-reboot on degraded (only if operator opts in via `Action`).
- Firmware-mismatch swarm-upgrade proposals.

None of these need to ship in the first migration pass. The contract
reserves the slots by declaring `events_emitted` on HealthReport.

## 7. Health aggregator — who computes derived fields

A subtle design point: the sensor can tell you its own `configured_rate_hz`
and its own `error_count`. It **cannot** tell you its own `observed_rate_hz`
— that's a delivery-side measurement that depends on whether publishes
actually landed.

Proposal:

- **Raw counters** (`last_emit_ts`, `error_count`, `configured_rate_hz`,
  `last_error`) are emitted by the sensor/node itself at one path:
  `substrate/<node-id>/health/sensor/<name>/raw`
  (and `substrate/<node-id>/health/raw` for node-level).
- **Derived rollups** (`observed_rate_hz`, `status`, `since_ms`) are
  computed by a small daemon-side aggregator and published at:
  `substrate/<daemon-node-id>/derived/health/<source-node-id>/sensor/<name>`
  (and `.../derived/health/<source-node-id>` for node-level).
- **Merged read path** (what Explorer consumes) is a *projection*
  computed at read time by substrate projection rules: the daemon
  serves `substrate/<node-id>/health/sensor/<name>` as the merge of
  the raw path and the derived path. (This projection pattern already
  exists in `crates/clawft-substrate/src/projection.rs`.)

This cleanly preserves the write-gate rule: every path is written by
its owning node. The "merged" read is computed, not stored.

**Alternative** considered and rejected: have the aggregator write
directly into the source node's namespace (pretending to be the
source). Rejected because it violates the write-gate rule.

## 8. Migration notes

- **No legacy shape to carry.** Health is new in this pass; no prior
  structured health emission exists to migrate from. The ESP32
  firmware and daemon both emit HealthReport from first deploy of
  the post-migration code.
- **Backfill for existing sensor:** the INMP441 mic adapter today
  does not emit health. The migration pass adds it at both
  granularities (node-level from daemon, sensor-level from ESP32
  firmware).

## 9. Open questions

1. **Status-enum finality.** Are the four sensor-level states
   (`healthy | degraded | stale | down`) exhaustive? Considered:
   `"unknown"` for early-boot (<first-emit), `"inhibited"` for
   sensors intentionally paused by an Action. Proposal: add
   `"unknown"` now (it's trivially needed pre-first-emit); defer
   `"inhibited"` to Actions.
2. **Cadence policy for noisy sensors.** 1 Hz health minimum vs
   higher. IMU at 100 Hz emit → 1 Hz health is 100x cost
   reduction. Is this the right default or should we make cadence
   per-sensor configurable? Proposal: per-sensor config with 1 Hz
   default, documented in sensor's `meta.health_cadence_hz`.
3. **Aggregator ownership.** §7's "daemon-side aggregator" — is
   this a dedicated service (`clawft-service-health`) or does it
   live in `clawft-kernel`? Proposal: dedicated service, so we can
   swap implementations without kernel surgery. Sign-off needed.
4. **Projection vs union write.** Is the §7 projection approach
   cleaner than having the aggregator simply write to a parallel
   path that the Explorer joins client-side? Projection is server-
   side and cheaper for consumers; parallel-write is simpler
   server-side. Sign-off needed.
5. **Should `HealthReport` be split into two types** (`NodeHealth`
   and `SensorHealth`) rather than one union type? Pro-split:
   cleaner schema, rejects cross-granularity mistakes. Pro-union:
   one viewer, one match, aligns with the "shape defines interface"
   principle. Lean: **keep it one type** and rely on the optional-
   field pattern; revisit if we hit a concrete confusion case.
