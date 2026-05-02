---
title: Journaled node — ESP32-S3 as the first fully-journaled mesh node
created: 2026-04-24
status: draft — post-Node/Actor-split ontology proposal
scope: one node, its identity, its owned paths, its healthcheck, the Node Object Type
depends_on:
  - .planning/ontology/ADOPTION.md (Mesh as root Object, Node below Mesh)
  - .planning/sensors/JOURNALED-SENSOR-MIC.md (the sensor this node hosts)
  - .planning/sensors/HEALTHCHECK-CONTRACT.md (the health contract nodes satisfy)
post_migration: true
---

# Journaled node — ESP32-S3

## 0. Why this matters — Node ≠ Actor

The main-thread decision this plan reflects: **Node and Actor are separate
identities.** A Node is a physical thing that *emits* (sensor data,
heartbeats); it signs what it publishes. An Actor is an agent/user/program
that performs *Actions* (Foundry-style mutations) — it has its own separate
key. Sensing is not acting.

Practical consequences encoded throughout this doc:

- The ESP32 gets an **ed25519 keypair**. Its short fingerprint is its
  **Node ID**. That id appears as the first path segment of everything
  the ESP32 publishes.
- The daemon running on the WSL Linux host **also** is a Node (by the same
  rule: it publishes sensor-shaped and health-shaped emissions). It gets
  its **own** keypair, distinct from any Actor that runs on the same
  machine.
- **Write gate:** every substrate publish must hit a path starting with
  `substrate/<publisher-node-id>/`, and must be signed by that node's
  key. Unsigned publishes are rejected at the substrate boundary.
- Friendly labels like `"kitchen-mic"` are properties at
  `substrate/<node-id>/meta/label`, not identity. You can rename the
  label; you cannot rename the node-id.

The ESP32-S3 is the first node we journal end-to-end because it's the
simplest concrete case that forces us to pick a key-storage story, a
node-id format, and a meta/health contract. The daemon-as-node journal
is similar in shape but lives in a separate pass.

## 1. Hardware profile

- **SoC**: ESP32-S3, 2 cores @ 240 MHz, 512 KB SRAM, 8 MB PSRAM (typical
  module), 4–16 MB flash.
- **Radio**: 2.4 GHz WiFi (b/g/n) + BLE 5.
- **Crypto**: hardware HMAC / AES / SHA acceleration via esp-hal, optional
  eFuse block for key storage (one-time-programmable, readable only by
  the secure boot subsystem).
- **Attached sensor (today)**: INMP441 I2S MEMS microphone (see
  `JOURNALED-SENSOR-MIC.md`).
- **Future-attached sensors**: ToF, IMU, camera, environmental — all TBD.
  The node layout here explicitly scales to N sensors, not just 1.

### 1.1 Key-storage options (ranked)

| Option | Durability | Tamper-resistance | Cost to ship |
|---|---|---|---|
| **eFuse (secure)** | One-shot burn; survives reflash | High — not readable outside secure boot | Medium — needs secure-boot flow + doc |
| **NVS with encrypted partition** | Survives reflash; readable via firmware | Medium — encrypted at rest with flash-encryption key | Low — esp-idf NVS encrypted variant is builtin |
| **NVS plain** | Survives reflash; readable by any firmware | Low — anyone with flash access reads the key | Trivial |

**Proposal for first journaled node:** **NVS plain**, with the written
understanding that this is provisioning-grade only. Any production
deployment upgrades to NVS-encrypted or eFuse. The id derivation (pubkey
fingerprint) doesn't change with the storage mode, so upgrading is a
key-regen + rename-migration, not an ontology change.

## 2. Node identity

### 2.1 Keypair

- **Algorithm**: ed25519 (matches the rest of the WeftOS stack — `clawft-kernel`
  already signs chain entries with ed25519 per its ExoChain wiring).
- **Generation moment**: at provisioning (first-boot flash), via a host-side
  CLI that generates the keypair, burns the private half to the ESP32's
  NVS, and records the public half in the mesh's `cluster` section (also a
  signed write, from the daemon's node-id).
- **Regeneration**: only via re-provisioning. There is no in-firmware rotate
  for MVP; rotation is a separate ADR.

### 2.2 Node ID derivation

- Compute BLAKE3 hash of the raw public key (32 bytes).
- Take the first 3 bytes → 6 hex chars.
- Prefix with `n-` for scannability in paths and logs.
- **Example**: `n-6f3a9c`.

Collision probability: with 3-byte truncation the birthday bound is ~4000
nodes for 1% collision probability. That's comfortably above any plausible
mesh size for the near horizon. If we ever hit mesh scale where that bound
squeezes, we extend to 4 or 5 bytes — the `n-` prefix makes the format
forward-extensible (just longer hex suffix).

Alternatives considered:

- Full pubkey (44 chars base58 or 64 hex): correct but noisy in paths.
  Every substrate path would start with `substrate/<64-hex-chars>/...`.
  UX tax is real.
- UUID-v4: zero collision risk, but doesn't bind to the key — you'd need
  a separate mapping from uuid → pubkey, adding an indirection layer the
  write-gate has to consult.
- DNS-like names (`mic-kitchen-1.local`): confuses label with identity.
  The whole point of separating these is to make the label mutable.

**Picked: `n-<6-hex>` truncated BLAKE3 of pubkey.** Compact, stable,
self-authenticating (anyone can verify a publish's signature against
the claimed node-id by hashing the key).

## 3. Paths this node owns

All paths below are prefixed with `substrate/<esp32-node-id>/`.
No path outside this prefix may be written by the ESP32.

### 3.1 Sensor subtree

`substrate/<esp32-node-id>/sensor/mic/summary`  — AudioStream-shaped snapshot
`substrate/<esp32-node-id>/sensor/mic/pcm_chunk` — raw PCM chunk

See `JOURNALED-SENSOR-MIC.md` §2 for the full shapes and cadences.

Future sensors extend this naturally:

`substrate/<esp32-node-id>/sensor/tof/summary`
`substrate/<esp32-node-id>/sensor/imu/samples`
`substrate/<esp32-node-id>/sensor/env/temperature`
(etc.)

### 3.2 Health subtree — node-level

`substrate/<esp32-node-id>/health` — the Node's own health emission.
Shape (HealthReport, see `HEALTHCHECK-CONTRACT.md`):

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

Field-by-field:

- `status`: `"healthy" | "degraded" | "down"` — derived from local
  checks (heap headroom, WiFi state, IMU DMA health, etc.).
- `uptime_s`: seconds since last boot.
- `firmware_version`: semver-ish string; e.g. `"0.7.0-phase2"`.
- `rssi_dbm`: WiFi signal strength. `-30` = excellent, `-90` = unusable.
- `free_heap_bytes`: smallest-free-block heap size in bytes. Useful for
  memory-leak detection.
- `last_publish_ts`: wall-clock millis of this node's last successful
  substrate publish (any path). Diverges from host time — the ESP32's
  clock is set via SNTP on boot.
- `reboot_reason`: one of `"power-on" | "panic" | "watchdog" |
  "software-reset" | "deep-sleep-wake"`. From ESP-IDF's reset-reason API.
- `boot_count`: monotonic counter persisted to NVS.
- `tick`: publisher's monotonic tick counter.

**Publish cadence:** every 5 s. This is less frequent than any sensor's
emissions because node-level health changes slowly; sensor-level health
is what caches in Explorer.

### 3.3 Health subtree — per-sensor

`substrate/<esp32-node-id>/health/sensor/mic`
`substrate/<esp32-node-id>/health/sensor/<name>` (per future sensor)

Shape + semantics in `HEALTHCHECK-CONTRACT.md` §3. Specific per-mic
details in `JOURNALED-SENSOR-MIC.md` §4.

### 3.4 Meta subtree

`substrate/<esp32-node-id>/meta`

Shape:

```json
{
  "node_id": "n-6f3a9c",
  "label": "kitchen-esp32",
  "hardware": {
    "soc": "ESP32-S3",
    "module": "ESP32-S3-WROOM-1-N16R8",
    "flash_mb": 16,
    "psram_mb": 8
  },
  "firmware_version": "0.7.0-phase2",
  "capabilities": {
    "sensors": ["mic"],
    "radios": ["wifi-2g4", "ble5"],
    "crypto": ["ed25519-sign", "blake3-hash"]
  },
  "provisioned_at": 1710000000000,
  "pubkey": "<64-hex-chars>"
}
```

Field-by-field:

- `node_id`: self-identifying, matches the path prefix. Redundant with
  the path but useful for snapshot exports where path context is lost.
- `label`: the mutable friendly name. Writeable via an `Action` (once
  the Actions pipeline exists); until then it is set at provisioning
  and persisted in NVS, refreshed on boot.
- `hardware`: static descriptor of the board.
- `firmware_version`: same value as appears in `health`; duplicated here
  so a meta-only subscriber doesn't need to read health too.
- `capabilities.sensors`: the list of sensor-names this node hosts. This
  is the schema hint that tells Explorer which subtrees to expect under
  `sensor/*`. Drives the Node Object Type matcher (§4).
- `capabilities.radios`: informational.
- `capabilities.crypto`: forward-looking — lets Actors check whether a
  node can satisfy a given signing request.
- `provisioned_at`: millis timestamp at first key-burn. Lets the mesh
  distinguish a freshly-provisioned node from a re-flashed one.
- `pubkey`: the node's public key in hex. Explorer can verify signed
  publishes against this.

**Publish cadence:** once at boot, then on any mutation (label rename).
Not polled; meta is semi-static.

### 3.5 Paths this node MUST NOT write

- Anything outside `substrate/<esp32-node-id>/`. The substrate enforces
  this at the write gate — any attempt is rejected and logged.
- `substrate/<esp32-node-id>/derived/*` — the `derived` segment is the
  convention for computed / transformed data produced by Actors or other
  nodes. The ESP32 only ever publishes *raw* sensor emissions. If a
  future firmware computes FFT bins on-device, they go under `sensor/mic/spectral`,
  not `derived/mic/spectral`.

## 4. Ontology tie — `Node` Object Type (proposed)

### 4.1 Proposal

- **`name()`**: `"node"`
- **`display_name()`**: `"Node"`
- **Priority**: `15` — below `Mesh` (20, root structural) and above
  specialized leaf types like `AudioStream` (10). This ordering matters
  because a Node subtree contains AudioStream-shaped children; when the
  user selects the Node path itself, we want `Node` to win, not some
  leaf type leaking up.
- **`matches(value)`**: returns `15` when the value is an object with
  *at least two* of:
  1. `meta` — object with a `node_id` field shaped like `^n-[0-9a-f]{6,}$`.
  2. `sensor` — object whose child keys are sensor-names (non-empty).
  3. `health` — object shaped like a HealthReport.

  Threshold of 2 rather than 3 so a freshly-booted node that hasn't yet
  populated its sensor subtree still classifies as a Node.

- **Properties:**

  | name | kind | doc |
  |---|---|---|
  | `meta` | Object | Static descriptor (hardware, firmware, capabilities, pubkey, label) |
  | `health` | Object (HealthReport) | Most-recent node health snapshot |
  | `sensor` | Object | Map of sensor-name → Sensor Object |
  | `derived` | Object | Optional — computed children produced by on-device transforms |

- **Paired viewer**: a new `NodeViewer` — renders a card with:
  - **Header**: `meta.label` (if set) + `node_id` chip + overall status
    chip (from `health.status`).
  - **Stats row**: uptime, RSSI, firmware version, free heap, boot count,
    last-publish-age.
  - **Sensors list**: one row per key in `sensor.*`, each showing the
    sensor's kind, status, and observed rate (drilling down opens the
    `SensorViewer` from `JOURNALED-SENSOR-MIC.md` §5).
  - **Pubkey**: expandable; with a copy-to-clipboard affordance.
  - **Label**: editable-looking field; disabled with an "Actions pipeline
    required" tooltip until that lands.

- **`capabilities()`**:
  - `applicable_actions`: `["node.rename", "node.reboot", "node.reprovision"]`
    — all deferred, but the slot is declared.
  - `events_emitted`: `["node.degraded", "node.recovered", "node.rebooted"]`
    — emitted by the health-diff engine, not the node.

### 4.2 Relationship to `Mesh`

- **`Mesh`** is the root Object; its shape is "the whole substrate root,"
  i.e. the value returned by `substrate.read("")` — with `kernel`,
  `cluster`, `chain`, etc. as children.
- **`Node`** is at depth 1: a child of the Mesh, keyed by node-id.
- A Link Type `hosts` between `Mesh → Node` is reserved but not
  materialized in this pass. In Foundry terms, the Mesh-Node
  relationship is conceptually the "Mesh contains Nodes" containment
  link; making it a Link Type hardens it for typed traversal.

Crucially, **Mesh is the whole network; Node is one machine**. This
separation is not ceremonial — it's what makes federation (multiple
Meshes linked across a boundary) cleanly modeled later as Link Types
between Meshes, not as flat identity union.

### 4.3 Relationship to `Sensor`

- Each child of `substrate/<node-id>/sensor/` is a `Sensor`-typed
  subtree. The matcher for `Sensor` lives in its own type file; the
  Node type does not need to know about concrete sensor shapes.
- A Link Type `emits-from` between `Sensor → Node` is the inverse of
  Mesh→Node-hosts. Also reserved, also deferred.

## 5. Explorer affordances

When the user clicks `substrate/<esp32-node-id>` in the tree, they should
see — in order of shippability:

### 5.1 Shippable now

1. **Header** with label, node-id, overall status chip.
2. **Stats panel** — live-updating uptime, RSSI, firmware, free heap.
3. **Sensor list** — one row per sensor under this node, each clickable
   into the `SensorViewer` for that path.
4. **Meta expand/collapse** — `hardware`, `capabilities`, `pubkey`,
   `provisioned_at` (relative: "6 days ago").
5. **Health history sparkline** (optional, stretch) — 60-sample scroll of
   `free_heap_bytes` or `rssi_dbm`, using the existing `TimeSeriesViewer`
   as a child primitive.

### 5.2 Needs Actions pipeline (deferred)

6. **Rename** — writes `meta.label`.
7. **Reboot** — fires `node.reboot` to the ESP32, which must be listening
   on a control channel (doesn't exist yet).
8. **Reprovision** — regenerate keypair + wipe NVS. Destructive; needs
   an `Action` with confirmation flow.

### 5.3 Where this renders

Same two-option question as Sensor:

- **Option A**: `NodeViewer` in the viewer registry.
- **Option B**: a `node-card` Workshop parameterized by node-id.

Same lean: **Option A first**, Option B when Workshop gains panel-
parameterization.

## 6. Cross-references to code

- **ESP32 firmware**: `crates/clawft-edge-bench/src/main.rs`. Today it
  has hard-coded `KERNEL_HOST` / `KERNEL_PORT` constants and publishes
  to a flat path. The refactor:
  1. Load / generate ed25519 keypair from NVS at boot.
  2. Compute node-id from pubkey (BLAKE3 → truncate → hex-prefix).
  3. Derive all substrate paths by prefixing with
     `substrate/<node-id>/...`.
  4. Sign every publish with the private key; carry the signature in
     the RPC envelope (not inside the value).
  5. Emit node-level `health` every 5 s alongside existing benchmark
     logic.
  6. Emit `meta` once on boot, then on label changes only.
- **Daemon write gate**: `crates/clawft-substrate/src/kernel.rs` and
  `crates/clawft-weave/src/daemon.rs`. The write path today accepts
  any path; post-migration it must:
  1. Extract the node-id from the first path segment.
  2. Look up the node's pubkey in `substrate/<mesh-id>/cluster/nodes/<node-id>`.
  3. Verify the publish signature.
  4. Reject unsigned or wrong-signature publishes.
- **Ontology types**: add `crates/clawft-gui-egui/src/ontology/types/node.rs`
  and `crates/clawft-gui-egui/src/ontology/types/health_report.rs`;
  register at the `[[OBJECT_TYPES_REGISTRATIONS_INSERT]]` marker in
  `crates/clawft-gui-egui/src/ontology/mod.rs`.
- **Viewer**: `crates/clawft-gui-egui/src/explorer/viewers/node_viewer.rs`
  (new), registered at the viewers' marker comment.

## 7. Migration (hard cut, no compat window)

The main-thread decision is explicit: no compat shim. The old flat
`substrate/sensor/mic` is abandoned. On the ESP32 side:

1. Flash new firmware with provisioned keypair + node-id-scoped paths.
2. Daemon rebuild rejects the old unscoped paths at the write gate
   (logs the rejection so stray firmware is visible).
3. Explorer starts showing the new tree shape
   (`substrate/<node-id>/sensor/mic/summary` etc.).

Anything subscribing to the old flat path breaks. Known consumers:

- `clawft-service-whisper` — hardcoded `SUBSTRATE_PCM_INPUT_PATH`;
  needs to subscribe to the new path
  (`substrate/<esp32-node-id>/sensor/mic/pcm_chunk`) and publish its
  output under the daemon's node-id.
- Any Workshop TOMLs referencing old paths (there is a fixture at
  `crates/clawft-gui-egui/examples/example-workshop.toml`; needs an
  update pass).

Nothing else in the tree currently consumes the flat mic path in a
baked-in way.

## 8. Open questions

1. **Node-id length and format final sign-off.** `n-<6-hex>` truncated
   BLAKE3 of pubkey — confirm or adjust. Sign-off needed.
2. **Key-storage tier for first journaled node.** Plain NVS vs NVS-
   encrypted vs eFuse for provisioning-grade. Proposal: plain NVS,
   upgrade later. Sign-off needed.
3. **Daemon-as-node path.** The daemon is ALSO a node. Does it get the
   same `substrate/<daemon-node-id>/...` prefix treatment? (Yes, per
   main-thread decision.) But the kernel's own state (`kernel`,
   `cluster`, `chain`) lives at the Mesh root, not under a node. So the
   Mesh root has a mix: `substrate/{kernel, cluster, chain}` (Mesh-
   owned, not any single node's) AND `substrate/<node-id>/...` (each
   node's scoped subtree). This needs a clean statement in ADOPTION.md
   or a sibling ADR. **Explicit sign-off needed — is `kernel`/`cluster`
   /`chain` mesh-owned (no node-id scope) or do they move under the
   daemon's node-id?**
4. **Where do Actors store their state?** Actors aren't nodes; they
   don't emit. But they do need a place to record their own identity
   and state (registered actions, recent invocations, etc.).
   Proposal: `substrate/actor/<actor-id>/...`. Out of scope for this
   doc but needs alignment before Actions pipeline lands.
5. **Pubkey-directory authority.** The daemon needs a trusted directory
   of node pubkeys to enforce the write gate. Where does it live?
   Proposal: `substrate/<mesh-id>/cluster/nodes/<node-id>` — but this
   is a chicken-and-egg: that path is in the Mesh namespace, which
   means *someone* has to be authoritative for writes to it. Likely:
   the daemon's own node, bootstrap-trusted. Needs an ADR.
6. **Mesh-id format.** We've been assuming a Mesh exists and is
   identifiable, but ADOPTION.md doesn't commit to a specific id
   scheme for Meshes. Proposal: Meshes also get an `m-<6-hex>` id
   derived from a mesh-wide key (the cluster's genesis keypair).
   **Needs sign-off or deferral.**
