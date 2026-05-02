# Ontology Explorer — Project Plan

**Created:** 2026-04-23
**Status:** design approved in chat, awaiting Phase 0 kickoff
**Estimated total effort:** 1–2 engineering-days for MVP, then incremental per-viewer growth
**Target landing window:** current session (Phase 0 now) + next 1–2 sessions (Phase 1)

> The substrate **is** the ontology. This Explorer is the canonical tree view of it: left pane = hierarchical paths, right pane = a value viewer chosen by the value's shape. Every new viewer pattern lands here first and graduates into dedicated chip panels only when its shape is well understood.

---

## 0. Where we left off

**Observed 2026-04-23:** the VSCode webview panel shows widespread status-chip brokenness even though the daemon is serving correct substrate values (confirmed live via `substrate.read` on 127.0.0.1:9471 — `tick` climbs, `rms_db` moves in the −43…−16 dBFS range the bridge publishes).

User-visible symptoms in the webview:
| Chip | Inside the panel | Outside status icon | Real data |
|---|---|---|---|
| Kernel | green + live | green | ✓ working |
| Mesh | green + "connected" | **grey** | no total_nodes / healthy / shards shown inside either |
| ExoChain | green + "connected" | **grey** | panel body empty |
| Mic | green + "connected" | **grey** | rms_db never moves; no waveform |
| ToF | green + "connected" | **grey** | nothing shown (expected data off-chain too) |
| Wi-Fi / Bluetooth | broken | broken | leave alone for now |

**Root cause identified:** the VSCode extension's RPC proxy allowlist (`extensions/vscode-weft-panel/src/extension.ts:39–51`) permits only legacy per-chip methods (`kernel.status`, `cluster.status`, `chain.status`, `sensor.mic.status`, …) and does **not** include `substrate.read` / `substrate.subscribe`. The WASM `Live` loop that drives `Snapshot` — the same code the native GUI runs — uses substrate subscriptions to populate `snap.audio_mic`, `snap.tof_depth`, `snap.mesh_status`, `snap.chain_status`. Those are what the **chip icons** outside the panels read. Blocked at the proxy ⇒ chip icons never go green, mic gauge bound to `$substrate/sensor/mic.rms_db` never sees data.

Daemon RPCs available today (`crates/clawft-weave/src/daemon.rs:1911–1914`):
- `substrate.read(path)` — one-shot read
- `substrate.subscribe(path)` — stream updates
- `substrate.publish(path, value)` — write
- `substrate.notify(path, ...)` — fire-and-forget event
- **missing:** `substrate.list(prefix, depth)` — tree enumeration. Phase 1 adds this.

**RESUME →** Do Phase 0 first. It is a one-line allowlist change that probably fixes half the reported brokenness without writing any new GUI code. Phase 1 begins *after* we confirm what Phase 0 lit up.

---

## 1. Phases

```
┌───────────────────────────────────────────────────────────────┐
│ Phase 0   Unblock substrate proxy in VSCode     (5 min, 1 file)│
│ Phase 1   Explorer MVP + substrate.list RPC     (1–2 days)     │
│ Phase 2   Viewer pattern registry growth        (incremental)  │
└───────────────────────────────────────────────────────────────┘
```

---

## 2. Phase 0 — Unblock the substrate proxy

**Goal:** prove the allowlist is the only thing between the webview and live substrate data. Smallest possible change.

**Change:**
File: `extensions/vscode-weft-panel/src/extension.ts`, the `ALLOWED_METHODS` Set around line 39–51.
Add two entries:
```ts
"substrate.read",
"substrate.subscribe",
```
(Leave everything else alone. `substrate.publish` stays blocked — the webview is a viewer, not a writer.)

**Build + test:**
```bash
cd extensions/vscode-weft-panel && npm run compile   # or whatever script the extension uses
# reload the VSCode webview
```

**Expected outcome (post-reload, daemon still running):**
- Kernel chip: still green ✓
- Mesh chip: **icon turns green**, panel body populates `total_nodes`, `healthy_nodes`, shards
- ExoChain chip: **icon turns green**, panel body shows chain head / recent events
- Mic chip: **icon turns green**, gauge tracks −43…−16 dBFS, moves when you tap the piezo
- ToF chip: **icon turns green** when frame arrives (or amber if all depths 0xFFFF)
- Wi-Fi / Bluetooth: still broken (out of scope)

**If Phase 0 does NOT fix Mesh/Chain/Mic icons:** the chip-icon path diverges from substrate beyond just the allowlist — fall through to `native_live.rs:280+` in the WASM build and verify the `Live` loop actually calls `substrate.read`/`subscribe` on webview target (vs. only on native target). Add temporary `tracing::debug!` at the call site and inspect the webview console.

**Out of scope for Phase 0:** any GUI code, any daemon code. One-line proxy fix only.

**RESUME →** if Phase 0 lights things up, commit the allowlist change with message `fix(vscode): allow substrate.read/subscribe through webview proxy`, then start Phase 1. If it does not, debug the WASM `Live` loop before writing the Explorer on top of a broken foundation.

---

## 3. Phase 1 — Explorer MVP

**Goal:** a tree-left, detail-right panel that lets you walk the substrate live and see the actual value at every node, with type-aware renderers for the shapes we already have.

### 3.1 Daemon: add `substrate.list`

New RPC in `crates/clawft-weave/src/daemon.rs` alongside the other `substrate.*` handlers.

Signature:
```rust
// Request
{ "prefix": "substrate/sensor", "depth": 1 }   // depth = how many levels below prefix

// Response
{
  "children": [
    { "path": "substrate/sensor/mic",  "has_value": true,  "child_count": 0 },
    { "path": "substrate/sensor/tof",  "has_value": true,  "child_count": 0 }
  ]
}
```

- Backed by `crates/clawft-kernel/src/substrate_service.rs` — enumerate keys matching the prefix, group by next path segment, count children at that segment.
- `has_value: true` when the path itself has a Replace value (vs. pure internal node).
- Keep default `depth = 1` (single level) so the tree is lazy; Explorer expands on click.

Allowlist in `extension.ts`: add `"substrate.list"`.

### 3.2 GUI: the Explorer panel

Crate: `crates/clawft-gui-egui` (lives alongside existing chip panels, not replacing them).

Layout:
```
┌───────────────── Explorer ────────────────────────────────────┐
│ ▸ substrate               │  substrate/sensor/mic              │
│   ▸ kernel                │  ─────────────────────────────     │
│   ▸ mesh                  │  [AudioMeterViewer]                │
│   ▾ sensor                │    rms_db:  −41.2 ████▌·········   │
│       • mic  (live) ●     │    peak_db: −17.1 ██████▌·······   │
│       • tof           ○   │    available: true                 │
│   ▸ chain                 │    sample_rate: 16000 Hz           │
│   ▸ network               │    tick: 214                       │
│                           │                                    │
└───────────────────────────┴────────────────────────────────────┘
```

- **Left tree:** root `substrate/`. Each click on a `▸` arrow calls `substrate.list` with `prefix = that path, depth = 1` and fills children. Refresh on a slow tick (1 Hz) so newly appearing paths show up. Live-activity dot (●) next to any path currently emitting deltas.
- **Right detail:** the currently selected path. Subscribed via `substrate.subscribe` so the viewer updates in real time. When selection changes, the previous subscription drops.

### 3.3 Viewer registry

Trait:
```rust
pub trait SubstrateViewer {
    /// Return a priority > 0 if this viewer can render `value`.
    /// Higher priority wins. The JSON fallback returns 1.
    fn matches(value: &serde_json::Value) -> u32;

    fn paint(ui: &mut egui::Ui, path: &str, value: &serde_json::Value);
}
```

Registry: a `Vec<Box<dyn ...>>` or a compile-time match cascade. Bias towards the latter so we avoid dyn overhead in the draw loop.

**Phase 1 viewers (ship 4):**

| Viewer | matches when | Where it's useful today |
|---|---|---|
| `AudioMeterViewer` | `rms_db` AND `peak_db` present, both numeric | `substrate/sensor/mic` — dB bars + numeric readout |
| `ConnectionBadgeViewer` | value is `{ state: "connected" \| "disconnected" \| … }` | `substrate/network/wifi`, `substrate/bluetooth`, and anything else shaped like a link |
| `DepthMapViewer` | `depths_mm` array AND `width` AND `height` numeric | `substrate/sensor/tof` — reuses the `ui://heatmap` primitive from commit `613b58a` |
| `JsonFallbackViewer` | always matches, priority 1 | everything else — pretty-printed JSON with type badges, expand/collapse for nested objects |

Each viewer is its own file in `crates/clawft-gui-egui/src/explorer/viewers/`. Adding a pattern = one file + one line in the registry.

### 3.4 Success criteria for Phase 1

- Open the Explorer panel (new menu entry or a tray chip)
- Tree loads `substrate/` → click to expand → `sensor/` → click `mic`
- Right pane shows live audio meter with bars moving when you tap the piezo
- Click `substrate/sensor/tof` → right pane shows depth heatmap (once a frame is published)
- Click any path with no specialized viewer → JSON fallback renders it readably
- Tree refreshes when a new path appears in substrate
- Unsubscribes cleanly when Explorer closes (no leaked subscriptions on the daemon side)

### 3.5 Test plan

- Unit: each viewer's `matches` against fixture JSON (positive + negative cases)
- Integration: spawn a daemon in test mode, publish to a synthetic path, verify `substrate.list` returns it and the Explorer tree updates
- Manual: open VSCode panel, walk the tree, tap the piezo, confirm the mic meter moves

---

## 4. Phase 2 — Viewer pattern registry growth (ongoing)

Every time we find a path rendering as JSON-fallback where a better view is possible, we add a viewer. No ADR needed — just one file + one registry line + a shape test.

**Known future viewers queued from today's session:**

| Viewer | Trigger shape | Motivator |
|---|---|---|
| `WaveformViewer` | `{ samples: [f32; N], sample_rate }` | mic real-time waveform, beyond the dB bars |
| `MeshNodesViewer` | `{ total_nodes, healthy_nodes, nodes: [...] }` | today's Mesh chip panel wants to show this |
| `ChainTailViewer` | `[{ seq, ts, kind, payload }, …]` | ExoChain event tail |
| `TimeSeriesViewer` | numeric scalar under a path that ticks | any metric that changes over time — ring-buffer + sparkline |
| `ProcessTableViewer` | `kernel.ps` output shape | replace the current kernel panel's table if the shape matches |

**The discipline:** every chip panel we already have is eligible to be *replaced* by the Explorer viewing that chip's substrate path — if the Explorer's viewer is as good as or better than the bespoke panel. This is how we avoid maintaining two UIs for the same data.

---

## 5. Files likely to change

Phase 0:
- `extensions/vscode-weft-panel/src/extension.ts` — allowlist

Phase 1:
- `crates/clawft-weave/src/daemon.rs` — `substrate.list` handler
- `crates/clawft-kernel/src/substrate_service.rs` — enumerate-by-prefix helper
- `crates/clawft-substrate/src/*` — possibly a `list_paths(prefix, depth)` method on the substrate store
- `crates/clawft-gui-egui/src/explorer/mod.rs` — new panel
- `crates/clawft-gui-egui/src/explorer/tree.rs` — left tree
- `crates/clawft-gui-egui/src/explorer/viewers/{mod,audio_meter,connection_badge,depth_map,json_fallback}.rs`
- `crates/clawft-gui-egui/src/shell/tray.rs` or wherever panels register — mount point for the Explorer
- `extensions/vscode-weft-panel/src/extension.ts` — allow `substrate.list`

---

## 6. Open questions

1. **How should the Explorer be reached from the tray?** Options: (a) add an "Explorer" chip next to Kernel/Mesh/…, (b) a launcher-menu entry, (c) both. Default: (b) — the Explorer is a tool, not a status surface.
2. **Should clicking a tray chip *also* open the Explorer focused on that chip's substrate path?** Could be a nice convergence — every chip is just a shortcut to its Explorer subtree. Defer until MVP lands.
3. **Does `substrate.list` also include paths that have never carried a Replace value but are referenced by subscribers?** Probably no — only paths with values. Confirm with user before implementation.
4. **Back-pressure:** if the Explorer subscribes to the root with a wildcard, the firehose could overwhelm the WASM side. MVP avoids this by only subscribing to the currently-selected detail path. Wildcard-root subscribe stays out of scope.

---

## 7. Non-goals

- Writing to substrate from the Explorer (publish stays blocked at the proxy for the webview build)
- Replacing the existing chip panels wholesale — they stay; Explorer grows alongside them
- Schema validation / type inference beyond "does this shape match a known viewer"
- Persistence of expanded-tree state across sessions (MVP is ephemeral; revisit if it matters)
