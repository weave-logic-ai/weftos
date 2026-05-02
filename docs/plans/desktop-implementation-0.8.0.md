# Desktop implementation — 0.8.0 base OS

**Owner**: ws08-weftos-gui swarm.
**Plane cycle**: `0.8.x` (UUID `76a2e899-a3fd-4fdd-ab88-5310d458bb22`).
**Predecessors**:
- `docs/DESIGN.md` — design contract (v0.1).
- `docs/plans/desktop-revision-0.8.0.md` — desktop revision plan.
- `docs/design/mockups/desktop-0.8.0.png` — base view mockup.
- `docs/design/mockups/apps/*.png` — 12 app mockups (Files, Processes, Services, Network, Settings, Scheduler, Monitor, Logs, Terminal, Chat, Admin, Explorer, Apps).
- `.claude/skills/weftos-design/` — design discipline skill + audit scripts.

Goal: take the design system from PNGs to a working egui base — uniform sidebar, 12 stock apps wired against the substrate, audit gates in CI. Ship the 0.8.0 OOB desktop.

---

## Phase 0 — land the design contract (1 commit)

**Scope**: docs-only commit, no code changes.

**Work**:
1. Branch off `m7-08-sweep` → `weftos-design-0.8.0`.
2. Stage: `docs/DESIGN.md`, `docs/plans/desktop-revision-0.8.0.md`, `docs/plans/desktop-implementation-0.8.0.md`, `docs/design/mockups/`, `.claude/skills/weftos-design/`.
3. Update `docs/handoff.md` with the design-system landing entry.
4. Run `scripts/build.sh check` (no code changed → expect clean).
5. Commit `docs(design): WeftOS design system v0.1 + 0.8.0 desktop plan + skill`.

**Gate**: build clean, no untracked beyond `node_modules/` + `ui/`.

**Owner**: human. **Effort**: 30 min.

---

## Phase 1 — token sync (1 PR)

**Scope**: align `crates/clawft-gui-egui/src/theming.rs` with DESIGN.md §2 tokens, establish drift baseline.

**Work**:
1. Add `bg_sidebar: Color32::from_rgb(0x2A, 0x2A, 0x30)` field to `Tokens` struct in `theming.rs`.
2. Wire `bg_sidebar` into `visuals()` if/when sidebar lands (Phase 2a will use it).
3. Run `bash .claude/skills/weftos-design/scripts/audit-theme.sh` — record the current count of `Color32::from_rgb` outside `theming.rs` as the baseline (~22 known offenders in `shell/desktop.rs:413,428,432,436,494,502,509,512,657,680,686`, `shell/tray.rs:66-68,96,100,122,130,158,160,174`, `shell/grid.rs:69,86,90,94,127`).
4. Save baseline to `.planning/weftos-design/baseline-color-drift.txt` for the CI ratchet.
5. Add a unit test to `theming.rs` asserting every `Tokens` field matches the table in `.claude/skills/weftos-design/references/tokens.md`. Test parses the markdown, fails on drift either direction.
6. Run `scripts/build.sh check + clippy + test`.

**Gate**: build clean, new test passes, audit-theme.sh reports the recorded baseline (no fewer, no more — any change = bug).

**Owner**: human or single agent. **Effort**: 1 hour.

---

## Phase 2 — new shell (3 PRs, parallel after Phase 1)

### Phase 2a — sidebar module

**File**: `crates/clawft-gui-egui/src/shell/sidebar.rs` (new).

**Spec**: DESIGN.md §5 "Canonical sidebar layout (frozen)".

**Public API**:
```rust
pub struct Sidebar {
    pub collapsed: bool,
    pub hidden: bool,
    pub expanded: HashSet<&'static str>,  // group ids that are expanded
    pub active: SidebarTarget,
}

pub enum SidebarTarget {
    Files, Processes, Services,
    Network(NetworkTab),
    Settings, Scheduler, Monitor,
    Logs(LogsTab),
    Terminal, Chat, Admin, Explorer,
    Apps(AppsTab),
}

pub enum NetworkTab { Mesh, WiFi, Bluetooth }
pub enum LogsTab { System, WitnessChain }
pub enum AppsTab { BuiltIn, Installed, Developer }

pub enum SidebarAction {
    Open(SidebarTarget),
    ToggleGroup(&'static str),
    ToggleCollapsed,
    ToggleHidden,
}

pub fn paint(ui: &mut egui::Ui, state: &mut Sidebar, snap: &Snapshot, live: &Arc<Live>) -> Option<SidebarAction>;
```

**Behaviour**:
- Reserves 220px width on the left (or 48px when `collapsed=true`).
- Renders identity strip from `snap.config.identity.instance_name` (fallback: cwd basename via `std::env::current_dir()`).
- Renders Kernel chip with color keyed off `snap.connection`:
  - `Connected` → green dot `tokens.ok`, label + state text in `tokens.text_secondary`.
  - `Connecting` → amber dot `tokens.warn`, dim text.
  - `Disconnected` → red dot `tokens.crit`, label + state text in `tokens.crit`.
- Menu rows in the canonical order from DESIGN.md §5.
- Active row highlighted via surface lift (`bg_active`) + 2px left edge stripe in `text_dim` (no chromatic).
- Group expansion state persisted via `Live::submit("config.set", { key: "desktop.sidebar.expanded.<id>", value: bool })`.
- Footer: `◀ collapse` row, dispatches `ToggleCollapsed`.

**Tests**:
- Snapshot test (`egui_kittest` or render-to-buffer) comparing painted output against `docs/design/mockups/desktop-0.8.0.png` reference frame.
- Unit tests for `Sidebar::default()` + `apply()` state transitions.

**Gate**: build + clippy + test clean. Snapshot test against base mockup matches within tolerance.

**Effort**: 1 day.

### Phase 2b — desktop.rs rewrite

**File**: `crates/clawft-gui-egui/src/shell/desktop.rs` (rewrite), `crates/clawft-gui-egui/src/shell/wallpaper.rs` (extract).

**Work**:
1. Extract warped-grid wallpaper to `shell/wallpaper.rs`. `paint(ui, rect, t)`.
2. Delete `shell/tray.rs` (chips are now sidebar entries).
3. Rewrite `desktop::show()`:
   ```rust
   pub fn show(ui, desk, live, snap) {
       let total = ui.max_rect();
       let sidebar_width = if desk.sidebar.collapsed { 48.0 } else { 220.0 };
       let sidebar_rect = total.with_max_x(total.left() + sidebar_width);
       let wallpaper_rect = total.with_min_x(sidebar_rect.right());

       // Sidebar
       let action = ui.allocate_ui_at_rect(sidebar_rect, |ui| {
           sidebar::paint(ui, &mut desk.sidebar, snap, live)
       }).inner;
       if let Some(action) = action { desk.apply(action); }

       // Wallpaper + active app
       wallpaper::paint(ui, wallpaper_rect, t);
       apps::dispatch(ui, wallpaper_rect, &desk.sidebar.active, &mut desk.apps, live, snap);
   }
   ```
4. Delete the old `Blocks` floating launcher window. The Blocks demo gallery moves to `apps/launcher.rs` Developer category.
5. Delete the chip-detail floating window code (now apps).

**Gate**: build clean. Old behaviour gone, new behaviour matches base mockup.

**Effort**: 1 day.

### Phase 2c — empty/loading/offline contract helper

**File**: `crates/clawft-gui-egui/src/apps/state.rs` (new).

**Public API**:
```rust
pub fn render_offline(ui: &mut egui::Ui, rect: Rect);
pub fn render_loading(ui: &mut egui::Ui, rect: Rect, what: &str);
pub fn render_empty(ui: &mut egui::Ui, rect: Rect, what: &str, remediation: Option<(&str, Box<dyn Fn() + Send + Sync>)>);
```

Each app's `show()` calls one of these when the substrate snapshot has nothing for the app's bound paths. Matches DESIGN.md §5.

**Gate**: build clean. Snapshot test for each empty state matches the base mockup's caption styling.

**Effort**: 0.5 day.

---

## Phase 3 — 12 app shells (12 PRs, swarm-parallel)

For each app, scaffold a module under `crates/clawft-gui-egui/src/apps/<id>.rs` matching its mockup at `docs/design/mockups/apps/<id>.png`.

**Workstream lookup**:

| App | Module | Reuses | Plane | Effort |
|---|---|---|---|---|
| Files | `apps/files.rs` | `blocks::tree`, `blocks::table`, `blocks::layout` | WEFT-579 | 1.5d |
| Processes | `apps/processes.rs` | `blocks::table`, `blocks::strip`, `explorer/viewers/process_table.rs` | WEFT-580 | 0.5d |
| Services | `apps/services.rs` | `blocks::tabs`, `blocks::table`, `blocks::strip` | WEFT-581 | 0.5d |
| Network | `apps/network.rs` | `surface_host::compose` + chip TOMLs | WEFT-582 | 1d |
| Settings | `apps/settings.rs` | `blocks::layout`, new field/toggle/slider/select primitives | WEFT-583 | 2d |
| Scheduler | `apps/scheduler.rs` | `blocks::table`, `blocks::tabs`, `blocks::plot` | WEFT-584 | 0.5d (stub w/ empty state) |
| Monitor | `apps/monitor.rs` | `blocks::layout`, new gauge/plot blocks | WEFT-585 | 1.5d |
| Logs | `apps/logs.rs` | `blocks::strip`, `blocks::stream` (rename from `terminal.rs`) | WEFT-586 | 0.5d |
| Terminal | `apps/terminal.rs` | move from `explorer/terminal.rs` | WEFT-587 | 2h |
| Chat | `apps/chat.rs` | move from `explorer/chat.rs` | WEFT-588 | 0.5d |
| Admin | `apps/admin.rs` | `surface_host::compose` + `weftos-admin.toml` | WEFT-589 | 0.5d |
| Explorer | `apps/explorer.rs` | move from `explorer/mod.rs` (rename module) | WEFT-590 | 0.5d |
| Apps launcher | `apps/launcher.rs` | `blocks::layout`, tile pressables | WEFT-591 | 1d |

**Dispatch shape**:
```rust
// crates/clawft-gui-egui/src/apps/mod.rs
pub fn dispatch(ui, rect, target: &SidebarTarget, state: &mut AppState, live, snap) {
    match target {
        SidebarTarget::Files => files::show(ui, rect, &mut state.files, live, snap),
        SidebarTarget::Processes => processes::show(ui, rect, &mut state.processes, live, snap),
        ...
    }
}
```

**Per-app contract**:
1. Match the mockup's layout exactly.
2. Use only `blocks::*` primitives or `surface_host::compose`. No new ad-hoc UI.
3. Render empty/loading/offline states from `apps::state` per DESIGN.md §5.
4. No `Color32::from_rgb` in app code — use `Tokens` only.
5. Include a snapshot test against the corresponding mockup PNG.

**Swarm orchestration**:
```bash
npx @claude-flow/cli@latest swarm init --topology hierarchical --max-agents 8 --strategy specialized
# Then spawn 12 Task tool agents in ONE message, one per app, with full instructions.
```

**Gate per app**: `audit-surface.sh` clean (if TOML), `audit-theme.sh` no new offenders, snapshot test passes, `scripts/build.sh check + clippy + test` clean.

---

## Phase 4 — CI gates (1 PR)

**File**: `.github/workflows/pr-gates.yml` (extend existing).

**Add**:
```yaml
- name: weftos-design audit
  run: |
    bash .claude/skills/weftos-design/scripts/audit-theme.sh \
      --baseline .planning/weftos-design/baseline-color-drift.txt
    for f in crates/clawft-surface/fixtures/*.toml crates/clawft-app/fixtures/*.toml; do
      bash .claude/skills/weftos-design/scripts/audit-surface.sh "$f"
    done
```

The `--baseline` flag (to add to `audit-theme.sh`) makes the existing offender count a ratchet — count can decrease, not increase.

**Effort**: 0.5 day (split between CI yaml + script flag).

---

## Phase 5 — Plane filing (one-shot)

```bash
for slug in sidebar files processes services network settings scheduler monitor logs terminal chat admin explorer apps-launcher; do
  bash scripts/plane.sh create-issue \
    --title "ws08: implement $slug per DESIGN.md §9" \
    --cycle 0.8.x \
    --labels ws08-weftos-gui,audit-finding-0.8.0 \
    --body "Implement $slug to match docs/design/mockups/apps/$slug.png. Spec: docs/DESIGN.md §5 (sidebar) + §9 (OOB manifest). Audit before commit: scripts/audit-{theme,surface}.sh."
done
```

14 items filed (sidebar + 12 apps + CI gate).

---

## Sequencing — recommended

| Day | Phase | Status check |
|---|---|---|
| Day 1 AM | Phase 0 + Phase 1 | Design contract in tree, theme baseline recorded |
| Day 1 PM | Phase 2a sidebar | Sidebar renders, snapshot test green |
| Day 1 EOD | Phase 2b desktop rewrite + 2c state helper | Empty desktop matches base mockup |
| Day 2 | Phase 4 CI gates | Drift ratchet locked |
| Day 2-4 | Phase 3 — 12-app swarm | All apps shipped |
| Day 4 EOD | Phase 5 Plane filing + final `gate` | 0.8.0 desktop base ready to ship |

**Critical path**: Phase 2a blocks every app. Build sidebar first, snapshot it, then unleash the swarm.

**Risk** — both native (`scripts/build.sh native`) and webview (`scripts/build.sh wasm-panel`) targets must build green at every gate. Test both before any commit.

---

## What does NOT happen this wave

- Window snapping / tiling (0.9.x).
- Multi-monitor (0.9.x+).
- Workspace switching / Cmd+K palette in egui shell (separate work).
- Light theme (defines `bg_sidebar` differently — 0.9.x).
- Localization (0.9.x).
- Voice surface conventions (gated on WEFT-205).

These are explicitly out of scope; do not pull forward.

---

## Failure modes

- **Sidebar drift**: snapshot test catches. Fix the implementation, not the test, unless DESIGN.md §5 changed (must be a separate commit).
- **App body floats as a card**: violates DESIGN.md §4 archetypes. Audit script (when extended) flags. For now, code review responsibility.
- **New `Color32::from_rgb` outside theming**: `audit-theme.sh` ratchet fails. Fix by adding a token to `Tokens` struct + DESIGN.md §2 + `references/tokens.md`.
- **TOML surface missing empty/loading/offline**: `audit-surface.sh` D-EM01. Fix the surface.
- **Substrate adapter not yet shipped** (e.g. Scheduler): app renders its empty state with install hint. Don't block the launcher landing.
