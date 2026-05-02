# Desktop revision plan — 0.8.0 OOB stock desktop

**Owner**: ws08-weftos-gui.
**Plane cycle**: `0.8.x` (UUID `76a2e899-a3fd-4fdd-ab88-5310d458bb22`).
**Predecessors**: `docs/DESIGN.md`, `.claude/skills/weftos-design/`.
**Goal**: turn the demo-lab desktop into a working base OS — twelve stock surfaces, every one rendered through `surface_host::compose()` against the substrate, every one usable with **zero data and zero installed apps**.

---

## 1. The base view

A user opens WeftOS for the first time, with the kernel daemon running but no adapters yet attached. They see:

```text
┌──────────────────────────────────────────────────────────────────────────┐
│ ◉ WeftOS v0.8.0 · concierge-bot                                          │
│ ┌─────────────────┐                                                      │
│ │ ⌘  Files        │   ░ ░ ░  warped-grid wallpaper  ░ ░ ░                │
│ │ ⚙  Processes    │                                                      │
│ │ ⛁  Services     │                                                      │
│ │ ⌬  Network      │   (empty desktop area —                              │
│ │ ◎  Settings     │    open-app windows land here)                       │
│ │ ⏲  Scheduler    │                                                      │
│ │ ▥  Monitor      │                                                      │
│ │ ≡  Logs         │                                                      │
│ │ ▌_ Terminal     │                                                      │
│ │ ✱  Chat         │                                                      │
│ │ ⛨  Admin        │                                                      │
│ │ ⊞  Apps      ▾  │   ← group, expandable in place                       │
│ │   · Built-in    │                                                      │
│ │   · Installed   │                                                      │
│ │   · Developer   │                                                      │
│ │ ─── ◀ collapse  │                                                      │
│ └─────────────────┘   Demo mode — kernel daemon offline.                 │
│ ┌─ Tray ──────────────────────────────────────────────────────────────┐  │
│ │  Kernel ◯ disconnected   Mesh ─   ExoChain ─   Explorer    18:34   │  │
│ └─────────────────────────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────────────────────┘
```

**Three persistent layers**:

1. **Identity strip** — small monospace label, top-left above the sidebar. Reads `◉ <weftos-version> · <agent-id>`. Replaces the legacy window title.

2. **Sidebar** (`ui://sidebar`) — the new launcher. **A permanent screen region, not a floating window.** Occupies a full-height vertical strip flush against the left edge from y=0 to the bottom of the screen. No top margin, no bottom margin, no rounded corners, no drop shadow. The tray sits to its *right* (does not span behind it); the identity strip lives *inside* the sidebar's top area. The wallpaper does not paint behind the sidebar — its strip is the sidebar's own region. Replaces the centered tile-grid.

   Two collapse axes:
   - **Inline collapse**: width `220 → 48` px, icons-only rail. Toggle via the `◀` handle pinned at the bottom of the sidebar.
   - **Slide-out**: full hide off-screen-left. A 6-px edge handle remains pinned to the left screen edge to slide the sidebar back in.

   Entries are *leaves* (open the app — Files, Processes, Services, Network, Settings, Scheduler, Monitor, Logs, Terminal, Chat, Admin) or *groups* (expand in place — Apps → Built-in / Installed / Developer; the developer subgroup is where the existing `Blocks`/`Canon` demos move). Sidebar state (collapsed / hidden / per-group expanded) persists at `substrate/desktop/sidebar/*` so reboot restores layout.

3. **No tray.** Removed. The bottom bar is gone; system-wide signals (Kernel chip, clock) live in the sidebar header. The wallpaper region is reserved for app windows and the empty-state caption only. `shell/tray.rs` is deleted in 0.8.x; its retired chips (Mesh, ExoChain, Explorer) move into sidebar entries (Mesh under Network; ExoChain under Logs → Witness chain; Explorer becomes a top-level menu item).

**Demo-mode state.** Daemon offline → Kernel chip in the sidebar header is red with `disconnected`; the demo-mode caption sits in the wallpaper region. The desktop is *legible* with zero data.

---

## 2. The twelve stock surfaces

Numbering matches the launcher grid reading order. Each entry below names archetype, substrate roots, the composer primitives it uses, and what its three non-data states look like. Implementation effort estimates assume one swarm-agent per surface, parallelizable.

### 2.1 Files — `app://weftos.files`

- **Archetype**: `app-window` (left dock + content).
- **Substrate**: `substrate/fs/*`, `derived/*`.
- **Layout**:
  - Dock (left): `Files` / `Derived` / `Workspace` / `Trash`.
  - Content (right): `list-detail` — `ui://tree` of paths, right pane is a `ui://stack` with metadata header + `ui://media` (mime preview) or `ui://table` (directory listing).
- **Primitives**: `dock`, `tree`, `table`, `stack`, `media`, `pressable` (Open / Copy Path / Reveal in substrate), `field` (filter).
- **Empty**: *"No filesystem adapter installed. Install one with `weft adapter install fs`."* + remediation pressable.
- **Loading**: *"Listing substrate/fs/…"*.
- **Offline**: `crit` chip + *"Start with: `weaver kernel start`"*.
- **Effort**: M (~1.5 day).

### 2.2 Processes — `app://weftos.processes`

- **Archetype**: `app-window` (single content pane, no dock).
- **Substrate**: `substrate/kernel/processes`.
- **Layout**: `ui://stack` { filter strip, `ui://table` of processes, footer summary chips }.
- **Columns** (table): `pid` (number), `name` (text), `state` (chip, tone-coloured), `cpu_pct` (number), `mem_mb` (number), `uptime` (text).
- **Affordances**: per-row `kill` (dangerous=true → modal confirm), bulk `kill` from selection.
- **Reuses**: existing `explorer/viewers/process_table.rs` viewer logic; promote to a TOML-fed `ui://table` once the table primitive's row_action lands.
- **Empty**: *"No processes reported. The kernel is up but no adapter is publishing `kernel.processes` yet."*
- **Effort**: S (~0.5 day; data path exists).

### 2.3 Services — `app://weftos.services`

- **Archetype**: `app-window`.
- **Substrate**: `substrate/kernel/services`.
- **Layout**: `ui://tabs` { Running, Failed, All } each containing a `ui://table` + summary `ui://strip` of state chips.
- **Columns**: `name`, `state` (chip), `pid`, `uptime`, `restarts`.
- **Affordances**: per-row `start` / `stop` / `restart` (restart is dangerous on critical services). Tail logs button opens Logs filtered to the service.
- **Empty**: *"No services registered. WeftOS hosts services through `weft service register`."*
- **Effort**: S (~0.5 day; existing admin-app demo is the precedent).

### 2.4 Network — `app://weftos.network`

- **Archetype**: `app-window` (left dock).
- **Substrate**: `substrate/mesh/*`, `substrate/wifi/*`, `substrate/bluetooth/*`.
- **Dock**: `Mesh` / `Wi-Fi` / `Bluetooth` / `ExoChain`.
- **Sub-surfaces**: each section reuses an existing chip TOML (`weftos-chip-{mesh,wifi,bluetooth,exochain}.toml`) — already in the repo.
- **Common header per section**: connection chip + throughput gauge (rx/tx) + peer count chip.
- **Empty**: per-adapter — *"No mesh peers. Run `weft mesh join <key>`."* / *"Wi-Fi adapter not detected."*
- **Effort**: M (~1 day; fixtures exist, just need the wrapping app-window).

### 2.5 Settings — `app://weftos.settings`

- **Archetype**: `app-window` (left dock).
- **Substrate**: `substrate/config/*`.
- **Dock sections**: `Identity` / `Network` / `Voice` / `Channels` / `Plugins` / `Permissions` / `About`.
- **Layout**: each section is a `ui://stack` of typed editors:
  - bool → `ui://toggle`
  - bounded number → `ui://slider`
  - 1-of-N → `ui://select`
  - free text → `ui://field`
- **Schema source**: substrate publishes `substrate/config/<section>/schema` describing each key's kind and bounds; the surface binds against it. Existing daemon-side config types provide the schema today.
- **Affordance**: every change → `config.set { key, value }` verb; daemon validates + persists.
- **Empty**: *"Config schema not yet published. Run `weft init` to seed defaults."*
- **Effort**: L (~2 days; schema-driven, needs a tiny new RPC).

### 2.6 Scheduler — `app://weftos.scheduler`

- **Archetype**: `app-window` (top tabs).
- **Substrate**: `substrate/scheduler/*` (new — needs a kernel-side adapter first; tracked separately).
- **Tabs**: `Jobs` / `History` / `Triggers`.
- **Jobs**: `ui://table` { name, schedule (cron or interval), enabled toggle, last run, next run, status chip }.
- **History**: `ui://table` of executions + filter strip (success/failure/cancelled).
- **Triggers**: `ui://table` of registered triggers + `ui://pressable` "Run now".
- **Empty**: *"No scheduled jobs. Add one with `weft schedule add …`."*
- **Effort**: L (~2.5 day; depends on scheduler adapter — likely 0.9.x not 0.8.x).

### 2.7 Monitor — `app://weftos.monitor`

- **Archetype**: `tile-grid` of plot cards (dashboard).
- **Substrate**: `substrate/kernel/*`, `substrate/sensors/*`.
- **Tiles** (6, 2-row × 3-col):
  - CPU (per-core): `ui://plot` rolling 60s.
  - Memory: `ui://gauge` + `ui://plot` 60s.
  - Disk I/O: `ui://plot` rx/tx.
  - Network: `ui://plot` rx/tx.
  - Process count: `ui://gauge` (0..max).
  - Service health: `ui://strip` of chips.
- **Empty**: *"No sensor adapters publishing. Install one with `weft adapter install sensors-host`."*
- **Effort**: M (~1.5 day; primitives all exist, plumbing rolling-window data through `ui://plot` is the work).

### 2.8 Logs — `app://weftos.logs`

- **Archetype**: `stream`.
- **Substrate**: `derived/logs/*`.
- **Layout**: filter strip top (severity chips: info/warn/error/debug; service chips drawn from substrate; text filter `ui://field`); `ui://stream` body.
- **Affordances**: pause / resume tail; export visible buffer (`logs.export` verb).
- **Empty**: *"No logs published yet. Logs flow through `derived/logs/*` once a service writes to the witness chain."*
- **Effort**: S (~0.5 day; `ui://stream` exists, just needs the filter strip wiring).

### 2.9 Terminal — `app://weftos.terminal`

- **Status**: ✅ shipped (`explorer/terminal.rs`).
- **0.8.x work**: graduate from a chip-detail panel to a first-class app-window in the launcher grid. Move the existing impl behind a `ui://foreign` shim with `WEFTOS-DESIGN: TODO graduate to ui://terminal` so the audit script tracks the debt.
- **Effort**: XS (~2 hours).

### 2.10 Chat — `app://weftos.chat`

- **Status**: ✅ shipped (`explorer/chat.rs`, calls `agent.chat`).
- **0.8.x work**: same graduation as Terminal. Promote to launcher grid; wrap with `ui://foreign` + TODO. Add a `ui://strip` of model + provider chips at top so the user can see which LLM is answering.
- **Effort**: S (~0.5 day).

### 2.11 Admin — `app://weftos.admin`

- **Status**: ✅ shipped (`weftos-admin.toml` reference app).
- **0.8.x work**: refactor against §5 of DESIGN.md — currently the empty/loading/offline states are bespoke at `desktop.rs:render_selected_app:676-691`. Move them into the manifest's `[surfaces.empty_state]` etc.
- **Effort**: XS (~2 hours).

### 2.12 Apps (Launcher) — `app://weftos.launcher`

- **Archetype**: `tile-grid`.
- **Substrate**: app registry (`AppRegistry` already exists at `crates/clawft-app/`).
- **Layout**: `ui://strip` (search field + filter chips: All / Built-in / Installed / Developer); `ui://grid` of tiles. Each tile = `ui://pressable` containing icon + label + version chip.
- **Affordances**: tap → open app (`ui.app.open` verb); right-click / long-press → context menu (info, uninstall — for non-built-in).
- **Developer category**: surfaces the existing 12 `Blocks` demos and `Canon` demos so they're not lost.
- **Empty**: *"No apps installed beyond the built-ins. Install with `weft app install <id>`."*
- **Effort**: M (~1 day).

---

## 3. Built-in surfaces (already in tray)

The four tray chips (Kernel, Mesh, ExoChain, Explorer) carry over unchanged. Two cleanups in scope:

- **Refactor empty hint**: today `render_empty_hint` at `desktop.rs:451` is bespoke. Promote to the canonical `ui://stack` empty pattern from DESIGN.md §5. (XS, ~1 hour).
- **Tray status cluster**: add right-aligned connection pill + UTC clock + "Demo" indicator when offline. (XS, ~1 hour).

---

## 4. Implementation phases

### Phase D1 — Foundation (1 day, serial)

1. Land `docs/DESIGN.md` + `.claude/skills/weftos-design/` (this PR).
2. Run `audit-theme.sh` against the GUI crate; record current offender count as the baseline (WEFTOS-DESIGN-1).
3. Add launcher-grid scaffold to `shell/desktop.rs` — replaces the current "Blocks" launcher window as the default home view. Demo gallery moves to `app://weftos.launcher` Developer category.
4. Add identity strip + tray status cluster.

**Gate**: build + clippy + tests clean. Mockup matches.

### Phase D2 — Quick-wins, parallel (2 days, 5 agents)

| Agent | Surfaces | Notes |
|---|---|---|
| 1 | Processes (2.2), Services (2.3) | Data paths exist. |
| 2 | Logs (2.8) | Stream primitive exists. |
| 3 | Network (2.4) | Wraps existing chip fixtures. |
| 4 | Terminal (2.9) graduation, Chat (2.10) graduation, Admin (2.11) refactor | Foreign shim. |
| 5 | Apps launcher (2.12) | Drives the developer-category move. |

**Gate**: each surface passes `audit-surface.sh`. Build + tests clean.

### Phase D3 — Heavy hitters, parallel (3 days, 3 agents)

| Agent | Surfaces |
|---|---|
| 1 | Files (2.1) |
| 2 | Settings (2.5) — schema RPC + form generation |
| 3 | Monitor (2.7) — rolling-window plot wiring |

**Gate**: every surface in §2 except Scheduler is in the launcher and rendering. `audit-theme.sh` reports zero new offenders since baseline.

### Phase D4 — Scheduler (deferred to 0.9.x)

The scheduler adapter doesn't exist yet. Track as a 0.9.x prerequisite. The Scheduler tile (2.6) is present in the launcher but renders an "install scheduler adapter" empty state until then.

---

## 5. New RPC verbs introduced

| verb | params | purpose | manifest |
|---|---|---|---|
| `ui.app.open` | `{ id: string }` | Open an app from the launcher | launcher |
| `kernel.kill-process` | `{ pid: u32 }` | Kill a process | processes |
| `kernel.start-service` | `{ name: string }` | Start a service | services |
| `kernel.stop-service` | `{ name: string }` | Stop a service | services |
| `kernel.restart-service` | `{ name: string }` | Restart (dangerous) | services |
| `config.set` | `{ key, value }` | Persist a config change | settings |
| `logs.export` | `{ filter, format }` | Export visible buffer | logs |

Each must land with a governance gate entry per ADR-053 (substrate-side write-gate). Tracked under WEFT-NNN range to be filed.

---

## 6. Risks

- **Composer primitive gaps.** `ui://table.row_action` and `ui://field.kind="secret"` may not be implemented yet in `surface_host::compose.rs`. Each gap surfaces as a `ui://foreign` with a graduation TODO; the audit script tracks them.
- **Substrate schema for Settings.** Today config schemas live in Rust types, not on the substrate. A small adapter exposing `substrate/config/<section>/schema` is on D3's critical path.
- **Demo-mode emptiness must look intentional.** The biggest UX risk is twelve tiles all in offline state looking broken. The fix is a single, clearly-worded "demo mode" banner just below the identity strip when `Connection == Disconnected`, and the empty-state copy in §2 above being remediation-shaped, not apology-shaped.

---

## 7. Out-of-scope (defer)

- Window snapping / tiling.
- Multi-monitor.
- Workspace switching (Ctrl+Tab between desktops).
- Theme switcher.
- Localization.
- Keyboard-driven app launcher (Cmd+K palette already exists in dashboard but not in egui shell — separate work).

These are 0.9.x+ items; not blockers for an OOB working desktop.

---

## 8. Mockup

A high-fidelity mockup of the base view (§1) lives at
`docs/design/mockups/desktop-0.8.0.png` (generated by `media-pipeline:image-generation`). Iteration on the visual happens against that file before any egui code is written.
