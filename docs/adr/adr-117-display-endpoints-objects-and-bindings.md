# ADR-117: Display endpoints: any app can target a leaf display with live objects bound to mesh data

- **Status**: Proposed (2026-10-10; the owner asked for the edge pad to be "a plain end point we can send any app we want at", driven "like vectors or objects on the screen, so it is less like a monitor and more like an interface with sensors", with pixel frames "for showing images or video only")
- **Deciders**: owner
- **Builds on**: `docs/design/vector-leaf-display.md` (retained scene graph, `SceneOp`,
  `InputRegion`, tweens, `DisplayId`; Proposed), the scene crates on `target-0.8.4`
  (`weftos-leaf-scene`, `weftos-leaf-renderer`, `weftos-leaf-sim`, `weftos-leaf-touch-gt911`),
  ADR-103 certified leaves, ADR-108 pairing and per-resource grants, ADR-114 `weftos://`
  names. The first device is the CrowPanel edge pad, which already renders scenes on hardware
  (2026-10-10).

## Context

On 2026-10-10 the edge pad came up on hardware: a stable display, a double-buffered flip with
no tearing, a GT911 touch controller that is probed and scanning, and a scene renderer in place
of the raster compositor.

The owner wants two things from it:
- it should be a **general endpoint**, so any app can be sent to it, not a device with one
  firmware per purpose;
- it should be driven as **objects and vectors**, so it behaves like an instrument panel wired
  to live data, not a remote monitor showing pixels.

A pixel stream would make the pad a slow monitor: every change costs bandwidth, and the device
knows nothing about what it shows. A retained object model makes it an interface: the pad
holds the objects, redraws only what changed, maps touch to the object that was touched, and
can update an object straight from a data stream without the app sending a frame.

## Decision

### 1. A display endpoint is a mesh service on a leaf

Every leaf with a screen advertises one service per display:
`weftos://<mesh>/nodes/<leaf node id>/services/display` (the `/display-<n>` form is used for
multi-display leaves, `DisplayId`). The service reports:
- size;
- capabilities: the renderer's `CapabilityMask`, plus frame support;
- input kinds: touch, and buttons later;
- the session currently holding the screen.

### 2. Objects first: apps send scenes, not pixels

An app opens a **session** on a display and sends `SceneEnvelope`s:
- create, update and remove nodes (shapes, text, images, groups);
- tweens.

The leaf keeps the scene, renders dirty regions, and sends back `InputEnvelope`s with **object
ids**: "node 42 tapped" and "node 7 dragged by (dx, dy)", not raw coordinates. Raw coordinates
stay available for canvases.

Sessions are bounded: at most 4,096 nodes, an image budget, and a per-session op rate.

### 3. Bindings: objects wired to live data on the leaf

An object property can be **bound** to a mesh data source instead of being pushed by the app:

```text
bind node 12 .text      ← weftos://<mesh>/sensors/<id>   field "temperature_c"  format "{:.1} °C"
bind node 13 .fill      ← weftos://<mesh>/agents/<id>     field "state"          map {running: green, blocked: amber}
bind node 14 .value     ← weftos://<mesh>/nodes/<id>      field "cpu"            range 0..100
```

- **How it updates:** the leaf subscribes to the source (sensor readings in the ADR-111 SenML
  profile, agent and node status from heartbeats and the agent bus) and updates the property
  itself, within declared rate and transform limits.
- **What it gives:** a gauge, a status light or a ticker stays live with no app traffic, and
  keeps working, showing stale state clearly, if the app goes away.
- **Allowed transforms:** formatting, linear range mapping, threshold and colour maps, and
  smoothing. Code is never shipped to the leaf.
- **Access:** every bound source is resolved through ADR-114 rules, authorized and recorded
  before the first value, under the session's grants (section 5).

### 4. The leaf is also a sensor

Touch, presence (where a sensor exists), ambient light and the leaf's own health (temperature,
Wi-Fi signal, frame rate) are published as readings from `weftos://<mesh>/sensors/<leaf>-*`.
Other apps and agents can react to the pad the same way they react to any sensor.

### 5. Who may draw: grants and arbitration

- **Grants:** a session needs a **display grant** for that display, using the ADR-108 model
  (pairing plus a per-resource grant, checked before any op is accepted, revocable, chained).
  The grant also bounds which sources the session may bind.
- **Arbitration:** one session holds the screen at a time and others are queued. The pad keeps
  a small **system strip**, owned by the firmware and never by an app, that shows which app
  holds the screen and switches between queued sessions with a swipe. An app cannot hide the
  strip or fake another app.
- **Revocation:** revoking a grant ends the session on the next op; the leaf clears that
  session's objects and bindings.

### 6. Pixels only for images and video

Pixels are never a way to mirror an app's UI (owner, 2026-10-10). They appear only as
**media objects** inside a scene: an `Image` or `Video` node with a position, size and id,
like any other object.
- **Images:** a still image is sent once (raw, or QOI/PNG when the renderer supports it) and
  kept by the leaf until replaced.
- **Video:** a stream of frames into one media node, at a bounded rate and size, such as a
  camera preview or clip. It is clipped to the node's rectangle and never covers the system
  strip.
- **Touch:** touch on a media node reports the node id and the point within it.

Every UI, including existing egui apps, reaches a display as objects. egui apps get there
through an **egui-to-scene adapter** that translates egui's shapes and text into scene ops and
maps object events back to egui input. There is no full-screen frame-mirroring mode.

### 7. Client library

The client library, `weftos-display-client`, does three things for an app:
- discovers displays it is granted;
- opens sessions and sends scene diffs, with bindings declared as data;
- receives object events.

It runs against a real leaf over the certified-leaf channel, or against `weftos-leaf-sim` for
development and tests. The first app built on it is the **command center** (section 8).

### 8. First app: the command center

An app for the owner, which also runs as a normal window on the Mac:
- **project activity:** board, history and agent messages;
- **cards:** focus and reprioritising;
- **kicking off work:** start a team or lane on a card, approve steps, through the harness API;
- **notes:** recorded as project history, `kind: note`.

Built from scene objects: cards, lists and status lights bound to project, agent and node
sources.

## Consequences

- One firmware serves every app. New uses are new apps on the mesh, not new pad builds.
- The pad redraws only what changes and keeps live bindings without app traffic, so a busy
  dashboard costs little bandwidth or CPU, within the panel's 31 Hz refresh.
- Bindings put a small data client on the leaf: subscriptions, transforms and staleness. This
  is the main new firmware work, and it must stay bounded in memory and rate.
- Grants and the firmware-owned system strip keep a shared screen trustworthy.
- `vector-leaf-display.md` moves from Proposed toward its v1 scope. This ADR adds bindings,
  sessions, grants and the endpoint service on top of it.

## Phases

| Phase | Delivers | Done when |
|---|---|---|
| D1 | `display` service on the leaf; sessions with scene ops and object events; `weftos-display-client` against `weftos-leaf-sim` | a test app creates objects on the simulator and receives "node tapped" events |
| D2 | Display grants and arbitration, with the firmware system strip | a second app queues behind the first; a swipe switches between them; revoking a grant clears that session |
| D3 | Bindings with transforms and staleness; the leaf's own sensor readings | a gauge bound to a sensor stays live with the app stopped, and shows stale when the source stops |
| D4 | Command center v1 (Mac window and pad) | the owner sees project activity, kicks off a lane and records a note from the pad |
| D5 | Media objects (images, bounded-rate video); the egui-to-scene adapter | a camera preview plays in a media node beside live objects; the cog manager runs on the pad through the adapter, with no pixel mirroring |

## Open questions for the owner

1. **Which messages** does the command center show first? (a) agent and team messages,
   (b) board, history and notifications, (c) outside channels such as Slack or email.
   Proposed: (a) and (b) first.
2. **Where the command center runs:** on the Mac, or on PG so it's always up?
