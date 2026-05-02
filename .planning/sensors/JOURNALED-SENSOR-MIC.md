---
title: Journaled sensor — INMP441 MEMS mic as the first fully-journaled sensor
created: 2026-04-24
status: draft — post-Node/Actor-split ontology proposal
scope: one sensor, two substrate emissions, healthcheck, explorer surface
depends_on:
  - .planning/ontology/ADOPTION.md (Mesh as root Object, shape-defines-interface)
  - .planning/sensors/PIPELINE-PRIMITIVE-JOURNAL.md (provisional SensorStage shape)
  - .planning/sensors/HEALTHCHECK-CONTRACT.md (generic health contract)
  - .planning/sensors/JOURNALED-NODE-ESP32.md (the node this sensor lives on)
post_migration: true  # all paths use substrate/<node-id>/..., no legacy flat form
---

# Journaled sensor — INMP441 MEMS mic

## 0. Purpose

This is the first sensor we journal end-to-end under the post-Node/Actor-split
substrate layout. The mic is picked because (a) it is already in production on
the ESP32-S3 edge node, (b) it already has two emissions (summary + raw PCM)
that exercise different axes, and (c) it is the input to the Whisper pipeline
spike — which means anything we say about it has to be concretely consistent
with `crates/clawft-service-whisper` and `crates/clawft-substrate/src/mic.rs`
as they exist today.

The journal covers:

1. Hardware profile and dB landmarks.
2. Substrate emissions and their shapes (post-migration paths).
3. Sensitivity tier, cadence, chunk sizing.
4. How this sensor ties into the ontology — existing types and proposed new ones.
5. Healthcheck contract for this sensor (references the shared contract doc).
6. Explorer affordances.
7. Open questions.

## 1. Hardware

- **Part**: InvenSense INMP441 I2S MEMS microphone.
- **Interface**: I2S slave, 24-bit left-justified, 16 kHz nominal sample rate.
- **Host**: ESP32-S3 (see `.planning/sensors/JOURNALED-NODE-ESP32.md`).
- **Noise floor** (observed): ~ -57 dBFS in a quiet room.
- **Speech peak** (observed at ~30 cm): ~ -10 dBFS.
- **Dynamic range of interest** for gauge/meter UIs: `-65 … -5 dBFS`
  (the range `AudioMeterViewer` already paints at
  `crates/clawft-gui-egui/src/explorer/viewers/audio_meter.rs`).
- **Characterization level** (per `clawft-substrate/src/physical.rs`):
  `Characterization::Rate` — scalar magnitudes only, no spectral structure.
  The upgrade path to `Characterization::Spectral` is an FFT stage in the
  daemon or on-device; same Object Type, sibling topic.

## 2. Substrate emissions (post-migration)

All paths are scoped under the ESP32's node-id. The ESP32 writes with its own
signing key; the daemon rejects unsigned writes to `substrate/<esp32-node-id>/*`.
(Write gate: §3 of `JOURNALED-NODE-ESP32.md`.)

Let `<esp32-node-id>` be the short fingerprint of the ESP32's ed25519 public
key, generated once at provisioning and burned into NVS (see
`.planning/sensors/JOURNALED-NODE-ESP32.md` §2). Concretely, something like
`n-6f3a9c` — readable, truncated, collision-probability negligible at mesh
scale.

### 2.1 Summary emission — scalar level snapshot

**Path:** `substrate/<esp32-node-id>/sensor/mic/summary`

**Shape** (already matches `AudioStream` Object Type at
`crates/clawft-gui-egui/src/ontology/types/audio_stream.rs`):

```json
{
  "rms_db": -41.2,
  "peak_db": -17.1,
  "available": true,
  "sample_rate": 16000,
  "characterization": "rate",
  "tick": 12847
}
```

Field-by-field:

- `rms_db` (f64): root-mean-square level over the window, dBFS.
- `peak_db` (f64): peak sample level over the window, dBFS.
- `available` (bool): whether the device is delivering audio this tick.
- `sample_rate` (i64): Hz. Informational — 16 000 today.
- `characterization` (string): `"rate"` today, `"spectral"` if FFT added.
- `tick` (u64): the publishing node's monotonic tick counter.

**Publish cadence:** 2 Hz (500 ms window), matches `TICK_MS` in
`crates/clawft-substrate/src/mic.rs:67` and the `WINDOW_SAMPLES = 8000`
constant at 16 kHz.

**Legacy shape note:** Today this lives at `substrate/sensor/mic` (a flat path).
Post-migration this moves to `substrate/<esp32-node-id>/sensor/mic/summary`.
The trailing `/summary` segment is new — see §2.3 for why summary+raw live at
siblings rather than fighting over the same path.

### 2.2 Raw PCM emission — the whisper-pipeline input

**Path:** `substrate/<esp32-node-id>/sensor/mic/pcm_chunk`

**Shape** (matches what `crates/clawft-service-whisper/src/service.rs`
consumes today):

```json
{
  "pcm_b64": "AAAA...AAAA",
  "sample_rate": 16000,
  "channels": 1,
  "encoding": "s16le",
  "seq": 2841,
  "chunk_ms": 500,
  "tick": 12847
}
```

Field-by-field:

- `pcm_b64` (string): base64-encoded s16le little-endian mono PCM.
- `sample_rate` (i64): Hz.
- `channels` (i64): 1 today (mono); 2 if a future I2S variant lands.
- `encoding` (string): `"s16le"`. Reserved slot so stages can branch if
  we ever publish f32 from a future codec.
- `seq` (u64): producer's chunk sequence id (monotonically increasing).
  The whisper service uses this as `seq` in the transcript output so
  downstream joiners can correlate without timing assumptions.
- `chunk_ms` (u64): nominal window length in ms; 500 today.
- `tick` (u64): the publishing node's monotonic tick counter.

**Publish cadence:** 2 Hz (one 500 ms chunk per window). Bandwidth is
~42.6 kB/s over the wire (journal §2 Q1 in `PIPELINE-PRIMITIVE-JOURNAL.md`
— 16 000 bytes raw per 500 ms × 1.33 b64 overhead × 2 Hz).

**Legacy shape note:** Today this lives at `substrate/sensor/mic/pcm_chunk`.
Post-migration: `substrate/<esp32-node-id>/sensor/mic/pcm_chunk`. The Whisper
service's `SUBSTRATE_PCM_INPUT_PATH` constant
(`crates/clawft-service-whisper/src/lib.rs`) is the concrete consumer that
will need updating.

### 2.3 Why summary + raw live at sibling paths, not one merged doc

Three reasons:

1. **Different subscribers care about different things.** The gauge UI wants
   `summary` only; Whisper wants `pcm_chunk` only. Forcing them into a single
   doc makes every subscriber pay the PCM deserialization cost.
2. **Different rates are plausible.** If we ever publish summary at 10 Hz and
   PCM at 2 Hz (to make the gauge feel lively while keeping bandwidth low),
   sibling paths handle that naturally; a merged doc does not.
3. **Different sensitivity tiers are possible.** Summary is `Capture` today
   (speech envelope is recoverable), but a coarser `summary_level`
   (only `{ loudness_bucket: "quiet|speech|loud" }`) could safely downgrade
   to `Ambient` and be exposed to less-privileged callers. Sibling paths let
   us do this without schema gymnastics.

### 2.4 Sensitivity tier

Both emissions are **Capture** (`Sensitivity::Capture` in
`clawft-substrate/src/adapter.rs`). Speech content is recoverable from either
the envelope or the raw PCM. ADR-012 requires a per-goal `CapabilityGrant`
for any consumer — the Capture label propagates through substrate metadata
and must not be silently lifted.

## 3. Ontology ties

### 3.1 Existing Object Types this sensor hits

- **`AudioStream`** (`crates/clawft-gui-egui/src/ontology/types/audio_stream.rs`)
  matches `substrate/<esp32-node-id>/sensor/mic/summary` shape with priority 10.
  Already paints as `AudioMeterViewer` via the viewer dispatch cascade.
  **No change required** to the existing type — the shape the mic emits
  today is what the type was built to match.

### 3.2 Proposed new Object Type — `Sensor` (generic)

This is load-bearing. Without it, the Explorer has no way to classify the
*parent directory* of a sensor's emissions. Clicking
`substrate/<esp32-node-id>/sensor/mic` today would fall through to the
JSON fallback; we want it to render a sensor-summary view.

**Proposal:**

- **`name()`**: `"sensor"`
- **`display_name()`**: `"Sensor"`
- **Priority**: `5` (below specialized `audio_stream` at 10, above `Mesh` at 20
  — `Mesh` is structural/root, `Sensor` is per-emission-family).
  Wait — priority 5 only wins if no other type claims. Mesh's 20 beats it,
  but Mesh only matches at the root. This is fine.
- **`matches(value)`**: returns `5` when the value is an object carrying
  *all three* of:
  1. a child object at key `summary` OR a child object at any key shaped
     like an `AudioStream` / depth grid / IMU sample (i.e. the parent
     "has at least one child that looks sensor-payload-shaped");
  2. a child object at key `health` (the sensor's healthcheck subtree —
     see `HEALTHCHECK-CONTRACT.md`);
  3. a child object at key `meta` with a `kind` field holding a sensor-
     identifier string (e.g. `"inmp441-mic"`, `"vl53l1x-tof"`).

  An alternative simpler heuristic: the value is an object with exactly the
  subtree shape `{ summary|<payload_subtrees>..., health, meta }`. If at
  least two of `{summary|payload, health, meta}` are present as objects,
  `Sensor` matches at priority 5.

- **Properties:**

  | name | kind | doc |
  |---|---|---|
  | `summary` | Object (AudioStream / DepthGrid / ...) | Most-recent summary emission |
  | `pcm_chunk` or `frame` or `sample` | Object | Most-recent raw emission (optional; family-dependent) |
  | `health` | Object (HealthReport) | Last health snapshot for this sensor |
  | `meta` | Object | Static metadata (kind, model, sample_rate, configured_rate_hz) |

- **Paired viewer**: a new `SensorViewer` (not built) — renders a card with:
  - top bar: sensor kind + status chip (from health)
  - body: inline render of `summary` using the existing shape dispatch
    (so a mic-sensor shows the meter, a ToF shows depth map, etc.)
  - footer: last-seen timestamp, configured rate, observed rate, error count
  - affordances placeholder for start/stop/calibrate (see §5)

- **`capabilities()`**: initially empty. Reserved for:
  - `applicable_actions: ["sensor.start", "sensor.stop", "sensor.calibrate", "sensor.rename"]`
    — all deferred until the Actions pipeline lands.
  - `events_emitted: ["sensor.degraded", "sensor.recovered"]` — emitted by the
    health diff engine, not the sensor itself.

### 3.3 `HealthReport` — also proposed, shared

See `HEALTHCHECK-CONTRACT.md`. Both nodes and sensors emit HealthReport-shaped
values; the Object Type classifier is the same.

### 3.4 What the ontology does NOT need for this sensor

- No new type for the raw `pcm_chunk` emission — it's a transient binary
  carrier shape, not an Object Type. Wrapping it in `Sensor` at the parent
  is enough.
- No Link Type between `Node` and `Sensor` in this pass. That's the next
  layer up (Node hosts Sensors) and `JOURNALED-NODE-ESP32.md` owns that
  proposal.
- No Action Types yet — all affordances in §5 that imply writes are
  deferred to the Actions pipeline.

## 4. Healthcheck contract for the mic

**Path:** `substrate/<esp32-node-id>/health/sensor/mic`

**Shape** (HealthReport, see `HEALTHCHECK-CONTRACT.md` §2):

```json
{
  "status": "healthy",
  "last_emit_ts": 1714000000000,
  "configured_rate_hz": 2.0,
  "observed_rate_hz": 1.98,
  "error_count": 0,
  "since_ms": 84210,
  "last_error": null,
  "notes": null
}
```

Specific notes for the mic:

- `status` enum: `"healthy" | "degraded" | "stale" | "down"`.
  Transitions:
  - `healthy` → `stale` when `observed_rate_hz < 0.5 * configured_rate_hz`
    for > 3 s.
  - `stale` → `down` when `observed_rate_hz == 0` for > 10 s.
  - `healthy` → `degraded` when `error_count` has grown in the last window.
- `last_emit_ts` is the wall-clock millis when the last `pcm_chunk` or
  `summary` publish landed (whichever is later, since they share fate).
- `configured_rate_hz`: 2.0 nominal; taken from the `chunk_ms` config on
  the ESP32 side.
- `observed_rate_hz`: rolling 3-s window, computed by the daemon-side
  health aggregator (not the ESP32 — it doesn't know whether its publishes
  landed).
- `error_count`: counter of I2S-side DMA dropouts reported by the ESP32
  firmware, carried alongside normal publishes and snapshotted here.

**Publish cadence for the health emission:** every emit, or at least every
N=4 emits (i.e. ≥ 0.5 Hz). A sensor that publishes but whose health sibling
is silent is itself degraded — Explorer can flag this.

**Who writes this path:**

- The **ESP32 node** writes fields it owns: `last_emit_ts`, `error_count`,
  `configured_rate_hz`, `last_error`.
- The **daemon-side health aggregator** writes the derived fields:
  `observed_rate_hz`, `status`, `since_ms`. This is a separate write that
  must *also* be signed — since the daemon is a node (see
  `JOURNALED-NODE-ESP32.md` §0), it owns its own write path
  `substrate/<daemon-node-id>/derived/health/<esp32-node-id>/sensor/mic`,
  OR there is one canonical health path written by whichever party is
  authoritative for that field. The cleaner answer is the latter: the
  ESP32 writes the raw counters to `.../health/sensor/mic/raw`, and the
  daemon writes derived rollups to `.../health/sensor/mic` under its own
  signature. This is an **open question** — see §7.

## 5. Explorer affordances

When the user clicks
`substrate/<esp32-node-id>/sensor/mic` in the Explorer tree, they should
see — in order of shippability:

### 5.1 Shippable now (reads only — no Actions pipeline required)

1. **Status chip** at the top — green/yellow/red derived from
   `HealthReport.status` at `.../health/sensor/mic`.
2. **Live meter** rendering `summary` via existing `AudioMeterViewer`.
3. **Spec card** showing `configured_rate_hz`, `observed_rate_hz`,
   `sample_rate`, `characterization`, `last_emit_ts` (relative:
   "2.3 s ago"), `error_count`.
4. **Sensitivity badge** — `Capture`, styled amber, with a tooltip
   explaining speech recoverability.
5. **"View raw PCM"** toggle — expands a second viewer pane that mounts
   the `pcm_chunk` path and renders latest seq + size + age. No tap-to-
   listen; just presence-of-data + a copy-path button for devs.
6. **Lineage breadcrumb** — "emits from `<esp32-node-id>` (`Label`, if set)",
   linking back to the node view.

### 5.2 Needs Actions pipeline (deferred — slot-shaped only)

7. **Start/stop toggle** — writes `sensor.stop` / `sensor.start` Action
   Type. No-op today; rendered disabled with a tooltip "requires Actions
   pipeline." `applicable_actions` metadata on the Sensor type is the
   slot.
8. **Calibrate** — a button that would invoke `sensor.calibrate`
   (mic-specific: recompute the dBFS reference against a known-silence
   window). Deferred.
9. **Rename label** — writes to `meta/label` on the sensor. This is
   shippable *if* we scope "rename a label" as just-a-write with no
   validation. The write-gate question is: the user (an Actor, not a
   Node) wants to mutate a substrate path inside the ESP32's namespace.
   That requires the Actions pipeline to exist, because the Actor is
   *not* the ESP32 and cannot sign writes into the ESP32's subtree.
   **Deferred.**
10. **Tap-to-listen** — a privileged, Capture-tier Action that streams
    a short PCM window to the user's audio output. Definitely behind
    the Actions pipeline and an explicit CapabilityGrant.

### 5.3 Open question: where does the Sensor-type summary panel actually render?

Two options:

- **Option A (simpler):** When the user selects a sensor path, the detail
  pane dispatches to `SensorViewer` which paints the card described in
  §5.1. Implementation: add `SensorViewer` to the viewer registry at
  `crates/clawft-gui-egui/src/explorer/viewers/`.
- **Option B (more Workshop-native):** Ship a pre-built `sensor-card`
  Workshop (at a known path like `substrate/ui/workshop/sensor-card`)
  that composes existing viewers (`AudioMeterViewer`, a small JSON-field
  viewer for the spec card, a badge viewer for the status chip). The
  Explorer, when selection lands on a Sensor-type path, renders that
  Workshop with the sensor path bound into its panels.

Option B is more ontology-consistent (Workshop is already the composition
primitive per ADOPTION.md §8 Step 3) but requires Workshop panels to
accept *parameters* (a bound substrate path). Today Workshop panels
carry a fixed `substrate_path` per panel — parameterization is an
unblocked-but-unlanded extension. **Lean: Option A for first-ship,
Option B as the target once Workshop parameterization exists.**

## 6. Cross-references to code

Touched modules and files, for the implementation pass:

- **Publisher (ESP32 firmware):** `crates/clawft-edge-bench/src/main.rs`
  publishes to a hard-coded path today. Needs refactor to take a node-id
  at boot (from NVS) and publish to the scoped paths.
- **Host-side adapter (daemon):** `crates/clawft-substrate/src/mic.rs`
  `MicrophoneAdapter` hardcodes `substrate/sensor/mic` in `TOPICS`
  (line 70–81). On migration, `TOPICS` becomes dynamic on the daemon's
  own node-id, and the path moves to
  `substrate/<daemon-node-id>/sensor/mic/summary`. (The host's own
  microphone — when CPAL lands — is a separate sensor from the ESP32's.)
- **Whisper pipeline consumer:**
  `crates/clawft-service-whisper/src/lib.rs` `SUBSTRATE_PCM_INPUT_PATH`
  and `SUBSTRATE_TRANSCRIPT_OUTPUT_PATH`. Both paths move under the
  producing node's and producing actor's scopes respectively. Transcript
  is an interesting case — the Whisper service is an *actor*, not a node.
  It does not sign emissions; it performs Actions. Where does its output
  land? Answer: under the daemon's node-id at
  `substrate/<daemon-node-id>/derived/transcript/<esp32-node-id>/mic`.
  The fact that it is "derived from" the ESP32's mic is encoded in the
  path structure — see §7 Q4.
- **Object Type trait + registry:**
  `crates/clawft-gui-egui/src/ontology/mod.rs` and
  `crates/clawft-gui-egui/src/ontology/types/`. New modules:
  `sensor.rs`, `health_report.rs`. New dispatch branches at the
  `[[OBJECT_TYPES_REGISTRATIONS_INSERT]]` marker.
- **Viewer registry:**
  `crates/clawft-gui-egui/src/explorer/viewers/`. New:
  `sensor_viewer.rs` (Option A above). No change to existing viewers.
- **Explorer panel:** no structural change — the detail pane already
  dispatches by shape.

## 7. Open questions

1. **Exact format of the node-id fingerprint.** Short hash length,
   separator, printable-safety. Proposal: `n-` prefix + first 6 hex
   chars of the pubkey's BLAKE3 hash, i.e. `n-6f3a9c`. Collision
   probability at mesh scale (~100s of nodes) is negligible;
   user-scannable. **Needs user sign-off.**
2. **Health authority split.** Does the ESP32 write raw counters to
   `.../health/sensor/mic/raw` and the daemon write derived rollups to
   `.../health/sensor/mic`, or is there one merged path with mixed
   authorship? The write-gate rule says paths must start with
   `substrate/<node-id>/`; if the derived rollup is written by the
   daemon, its path is under the daemon's node-id, not the ESP32's.
   **Needs user sign-off.** Proposal: derived rollups live under
   `substrate/<daemon-node-id>/derived/health/<source-node-id>/sensor/<name>`.
3. **Sensitivity downgrade path.** Is a `summary_level` emission
   (coarse bucket, no envelope info) worth shipping as a true
   `Ambient`-tier sibling? Would let us expose a mic-presence chip to
   less-privileged callers without a CapabilityGrant. Probably yes,
   but not in this pass.
4. **Binary payload path.** The PCM b64 tax is fine on loopback, but
   moving it over the ESP32's WiFi link at production scale burns
   airtime. The journal flags this as "a later problem" but the
   migration to scoped paths is the natural moment to also introduce a
   native binary substrate variant. **Deferred but flagged.**
5. **Multi-mic cardinality under one node.** The INMP441 is the only
   mic today, but an ESP32-S3 with stereo MEMS inputs is a real
   hardware variant. Does the path become
   `substrate/<node-id>/sensor/mic/left` + `.../mic/right` + a
   `.../mic/summary` rollup? Or `.../mic0`, `.../mic1`? Proposal:
   `.../mic` stays singular for the sole-mic case;
   `.../mic/channels/<n>` for multi-channel cases when they land.
   **Low-priority — sensor-2 problem.**
