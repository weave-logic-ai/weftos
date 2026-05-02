# wt-files-procs-svcs — graduation notes (WEFT-579 / 580 / 581)

Worktree: `/home/aepod/dev/worktrees/wt-files-procs-svcs`
Branch: `feat/weft-579-580-581`
Base: `4083f9f1` (Phase A — bottom tray + chips/launcher retired)

Each app graduated from `super::state::render_if_needed` stub to a
real first-class implementation, matching the archetype it advertises
in DESIGN.md §9.

## WEFT-580 — Processes

**Source-of-truth:** `snap.processes: Option<Vec<serde_json::Value>>`
projected from `kernel.ps`.

**Schema (canonical):** `pid`, `agent_id` (or `name`), `state`,
`memory_bytes`, `cpu_time_ms`. Loose variants accepted: `cpu` (f64
percent), `mem` (f64 MB).

**Approach:** call the existing
`crate::explorer::viewers::process_table::ProcessTableViewer`
(priority 12) inside the app body region. The viewer already does
sortable headers, state colouring, and byte/cpu formatting; lifting
it would have duplicated logic and forked the sort-state egui id.

The body is rendered into a child UI clipped to
`body.shrink2(vec2(24.0, 8.0))` so the panel matches the heading's
left inset. The empty-state helper short-circuits when
`snap.processes` is `None`/empty or the daemon is offline — same
contract as every other graduated app.

## WEFT-581 — Services

**Source-of-truth:** `snap.services: Option<Vec<serde_json::Value>>`
projected from `kernel.services`.

**Schema (observed via Admin surface and
`clawft_substrate::projection::project_service_rows`):**

```jsonc
{ "name": "weave",   "state": "running", "pid": 1234, "restarts": 0, "uptime_ms": 1_234_567 }
{ "name": "whisper", "state": "stopped", "pid": null,  "restarts": 2, "uptime_ms": 0 }
```

`field_*` helpers fall back to `-` for missing data so a partially
populated adapter still renders all rows. `state` falls back to
`status`. `uptime` accepts `uptime_ms` (u64), `uptime_s` (u64), or
`uptime` (string).

**Filter persistence:** `services_tab: ServicesTab` field added to
`Desktop` (chosen over a static `Mutex` — same crate, single owner,
keeps the Desktop "snapshot of UI state" contract from desktop.rs
intact and avoids hidden globals). Default `ServicesTab::All`.

**Tab predicate:** `Active` matches `running | active | ready`.
`Inactive` matches the complement *including unknown / `-` states* —
deliberately, so a malformed row surfaces in `Inactive` rather than
disappearing entirely.

**Restart affordance:** inline confirm pattern (no modal, per spec).
First click on `[restart]` arms the row — the label flips to
`confirm?` (warn-tone, strong), and the armed name is stored in
egui's per-context data store under
`Id::new("weft_services_armed_restart")`. Second click submits
`Command::Raw { method: "service.restart", params: { "name": <row.name> } }`
and disarms. Only one row can be armed at a time, which collapses
the "click another row's button" case to "re-arm that one" — exactly
the behaviour the user expects from a no-modal pattern.

`paint_restart_affordance` is a no-op for rows whose name resolved
to `-`, since `service.restart` with a missing name would be
ill-formed.

## WEFT-579 — Files

**Source-of-truth:** there isn't one yet. `snap.fs` is *not* a field
on `Snapshot` (read live.rs to confirm — fields are status,
processes, services, logs, network_*, bluetooth, mesh_status,
chain_status, audio_mic, tof_depth, last_error, tick, *, *). The
graduation has to render the *list-detail archetype* anyway so the
user sees the eventual shape.

**Approach:** three-region layout —
1. Top toolbar (`TOOLBAR_H = 32`): Up / Refresh / View ▾. Each
   button submits a no-op `Command::Raw { method: "files.noop", … }`
   so the wire-up is real even though the adapter isn't. View is a
   `menu_button` because egui has no native dropdown and this matches
   the toolbar's visual weight.
2. Left pane (`LEFT_PANE_W = 220`): single placeholder root
   `◇ /` so the tree archetype is recognisable. No expand chevron —
   there's nothing under it until the adapter ships.
3. Right pane: detail view. With `snap.fs` permanently `None`,
   `has_data = false` and `super::state::render_if_needed` paints
   the empty-state helper inside the right pane only. The empty-
   state hint matches the existing copy: "No filesystem adapter
   installed" / "Install one with `weft adapter install fs`."

Both panes get a 1px hair-stroke surface lift (`bg_surface` fill,
`stroke_hair` border) so the list-detail boundary reads even on a
fresh demo-mode boot.

When the adapter ships, only the empty-state branch in
`paint_right_pane` needs to switch on `has_data` and forward to a
real detail viewer — the layout doesn't change.

## Allowed-file deltas

- `apps/files.rs` — full rewrite (graduation).
- `apps/processes.rs` — full rewrite (graduation, calls
  `ProcessTableViewer::paint`).
- `apps/services.rs` — full rewrite (graduation; new
  `ServicesTab` enum exported for `Desktop`).
- `shell/desktop.rs` — added `services_tab: ServicesTab` field +
  default. No other changes.
- `blocks/table.rs` — **untouched.** The block already exists for
  the Blocks-demo path; the Services table hand-rolls a Grid
  because (a) it needs a per-row affordance column and (b)
  reusing `blocks::table` would couple this app to
  `Desktop::blocks_state`'s `selected_row` / `table_sort_col`
  and bleed selection state across surfaces.

## Tests added

19 new app-level tests across 3 files (panel-render smoke + per-app
helpers). Total lib test count: **352 pass** (was 337+).

- `apps::processes::tests` — 3 (default, kernel.ps fixture, empty-
  state).
- `apps::services::tests` — 9 (default, with-rows, empty-state,
  3× tab predicate, duration formatter, 2× field helper).
- `apps::files::tests` — 3 (default, connected, disconnected).

## Gates

| Gate                                     | Result          |
| ---------------------------------------- | --------------- |
| `scripts/build.sh check`                 | clean           |
| `scripts/build.sh clippy` (`-D warnings`)| clean           |
| `cargo test -p clawft-gui-egui --lib`    | 352 pass        |
| `audit-theme.sh --baseline …`            | 246 (== baseline) |

## FINAL STATUS

All three apps graduated. Gates green. Three commits queued on
`feat/weft-579-580-581`. Not pushed.
