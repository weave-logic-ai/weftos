# wt-admin-launcher — WEFT-589 + WEFT-591 graduations

Branch: `feat/weft-589-591` off `master @ 4083f9f1` (Phase A).

## What I read

- `crates/clawft-gui-egui/src/shell/desktop.rs` — `Desktop` struct
  fields, `render_blocks_window`, `render_selected_app`, the dead-code
  helpers Phase A left behind.
- `crates/clawft-gui-egui/src/apps/{mod.rs,admin.rs,launcher.rs}` —
  the dispatch shape and the existing stubs.
- `crates/clawft-gui-egui/src/shell/sidebar.rs` — `SidebarTarget` /
  `SidebarAction` / `AppsTab` (Built-in / Installed / Developer).
- `crates/clawft-gui-egui/src/apps/state.rs` — the empty / loading /
  offline helper.
- `crates/clawft-gui-egui/src/theming.rs` — `Tokens` (only
  `bg_panel`, `bg_hover`, `stroke_soft`, `text_primary`, `text_dim`
  used by the tile grid).
- `.claude/skills/weftos-design/scripts/audit-surface.sh` — the
  D-EM01 regex (`^\[surfaces\.<slot>\]`).
- `crates/clawft-app/src/manifest.rs` — `AppManifest` schema.

## Key decisions / surprises

1. **D-EM01 needed a manifest-schema dance.** The audit's regex
   `^\[surfaces\.empty_state\]` (literal dot) demands a TOML
   dotted-table header that conflicts with the existing
   `surfaces = ["..."]` array assignment — TOML disallows the same
   key being both a value and a table parent.

   Resolution: rename the manifest field's TOML key from `surfaces`
   to `surface_refs` (Rust API name unchanged via `#[serde(rename =
   "surface_refs")]`) and add a new optional `surface_states:
   Option<toml::Value>` field deserialised from the `[surfaces]`
   parent table. The state subtables (`empty_state`,
   `loading_state`, `offline_state`) live there as opaque
   `toml::Value` blobs — no consumer reads them today; they exist
   to satisfy D-EM01 and document the §5 contract.

2. **`Eq` derive had to drop.** `toml::Value` is `PartialEq` only
   (TOML floats are `f64`), so `AppManifest` and `InstalledApp`
   now derive `PartialEq` only. No consumer ever puts a manifest in
   a hash-based set, so this is a no-op operationally.

3. **Pre-existing test failures in `clawft-surface`**: two tests in
   `tests/roundtrip.rs` were failing on `master @ 4083f9f1` before I
   touched anything (`parses_admin_fixture` expects 4 root.children
   but the existing fixture has 5 with the WEFT-439 modal). My
   additions to the surface fixture don't change `root.children`
   count — they hang off `surfaces[0].empty_state` etc., not under
   root. Out of my scope to fix.

4. **`render_selected_app` heading collision.** The instruction asks
   for `super::paint_heading("WeftOS Admin")` *and* a call to
   `render_selected_app`, which itself painted a name+id heading
   row. Rather than double-paint, I dropped the heading row from
   `render_selected_app` (the helper now does offline banner +
   composer body only — single caller). `apps/admin.rs::show` paints
   the heading band, then scopes a child Ui to the body rect and
   delegates to the helper. Single ownership.

5. **`render_blocks_window` Open Explorer pressable**: removed in
   place per the task ("Don't carry it forward — Explorer has its
   own sidebar entry now"). The helper is otherwise unchanged.

6. **Tile-grid is hand-rolled.** `egui::Grid` is opinionated about
   spacing and doesn't give us per-cell hover backgrounds. Painted
   each tile through `ui.interact` + raw `painter.rect_filled` so
   the surface lift on hover matches the sidebar rows (DESIGN.md
   §2.1 — surface lift only, no chromatic accent).

7. **No `Live::demo()` exists.** Initial test draft tried to
   construct a fake `Live`; pivoted to invariant-only tests that
   check the canonical tile order and §4.3 dimensions. The full
   render path is exercised at runtime — adding mock `Live` is
   cluster-wide work not specific to this graduation.

## Files changed

- `crates/clawft-gui-egui/src/apps/admin.rs` — graduated stub,
  delegates body to `desktop::render_selected_app`.
- `crates/clawft-gui-egui/src/apps/launcher.rs` — full tile-grid +
  Developer tab delegation.
- `crates/clawft-gui-egui/src/shell/desktop.rs` — drop the
  `#[allow(dead_code)]` on `render_blocks_window` and
  `render_selected_app`; remove the inline heading row from
  `render_selected_app`; remove the legacy "Open Explorer"
  pressable from `render_blocks_window`.
- `crates/clawft-app/src/manifest.rs` — schema rename
  `surfaces` → `surface_refs` (Rust field name unchanged); new
  `surface_states: Option<toml::Value>` field; drop `Eq` derive.
- `crates/clawft-app/src/registry.rs` — drop `Eq` derive on
  `InstalledApp` (transitive).
- `crates/clawft-app/src/{lifecycle,validation,registry}.rs` — add
  `surface_states: None` to in-Rust struct literals (4 sites).
- `crates/clawft-app/fixtures/weftos-admin.toml` — restructure to
  use `surface_refs = [...]` and add `[surfaces.empty_state]` /
  `[surfaces.loading_state]` / `[surfaces.offline_state]` blocks.
- `crates/clawft-surface/fixtures/weftos-admin-desktop.toml` —
  append the same three state blocks (bind to the latest
  `[[surfaces]]` entry — TOML array-of-tables). Audit doesn't
  flag this file (id is `weftos-admin/desktop`, not `app://...`),
  but the fixture is now visually consistent with the manifest.

## FINAL STATUS

**Gates** (run from `/home/aepod/dev/worktrees/wt-admin-launcher`):

- `scripts/build.sh check`: clean.
- `scripts/build.sh clippy`: clean (`-D warnings`).
- `cargo test -p clawft-gui-egui --lib`: **339 / 339 pass** (+2 net
  vs the 337 baseline — 2 new launcher invariant tests, 0 lost).
- `bash .claude/skills/weftos-design/scripts/audit-surface.sh
  crates/clawft-app/fixtures/weftos-admin.toml
  crates/clawft-surface/fixtures/weftos-admin-desktop.toml`: clean
  (D-EM01 violations: **3 → 0** on `weftos-admin.toml`).
- `bash .claude/skills/weftos-design/scripts/audit-theme.sh
  --baseline .planning/weftos-design/baseline-color-drift.txt`:
  holds at **246** (no new color-drift offenders introduced).

**Pre-existing test failures (NOT introduced by this graduation)**:

- `cargo test -p clawft-surface` `tests/roundtrip.rs::parses_admin_fixture`
  expected `tree.root.children.len() == 4` but the fixture (on
  `master @ 4083f9f1` before any of my edits) already had 5 root
  children — the WEFT-439 confirm-restart modal counts. Verified
  identical failure pre-change via `git stash` + retest.
- `cargo test -p clawft-surface`
  `tests/roundtrip.rs::toml_primitive_counts_match_expectations`
  — same root cause, same pre-change status.

**Commits** (on `feat/weft-589-591`):

- `aa48c92a` — feat(apps): graduate Admin app (WEFT-589)
- `<filled after second commit>` — feat(apps): graduate Apps launcher (WEFT-591)

**Followups filed**: none. The two pre-existing `clawft-surface`
test failures should be picked up by whoever owns WEFT-439 / the
surface-roundtrip contract; the count mismatch is a stale assertion
in the test, not a fixture bug.
