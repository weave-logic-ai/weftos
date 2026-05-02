# wt-network-logs — WEFT-582 + WEFT-586 graduation notes

Worktree: `/home/aepod/dev/worktrees/wt-network-logs`
Branch: `feat/weft-582-586` (off Phase A retire-tray commit `4083f9f1`).

## Scope

Two sidebar apps from Phase 3 stubs to first-class:

- **WEFT-582 — Network app** (`Mesh | WiFi | Bluetooth` tabs).
- **WEFT-586 — Logs app** (`System | WitnessChain` tabs).

## Decisions

### WEFT-582: composer for Mesh, JSON dump for Wi-Fi/Bluetooth

The task allowed either authoring fresh `weftos-net-{wifi,bluetooth}.toml`
fixtures or rendering raw JSON dumps. I went with:

- **Mesh**: composer path through `desktop::render_chip_detail` against
  the existing `chip_surfaces[&ChipId::Mesh]` (the `weftos-chip-mesh.toml`
  fixture). This is the canonical surface and was already written for
  the retired tray chip-detail floating window. Reusing it keeps the
  composer path load-bearing for substrate/mesh/status — exactly what
  the comment in `desktop.rs` envisaged when it wrote
  `// wired up by apps/network.rs (WEFT-582) graduation`.

- **Wi-Fi / Bluetooth**: substrate-path label + scrollable monospace
  pretty-printed JSON via `serde_json::to_string_pretty`. The empty/
  loading/offline case is handled by `super::state::render_if_needed`
  before we get to the JSON dump.

  Rationale for *not* writing `weftos-net-{wifi,bluetooth}.toml`:
  - Existing `weftos-chip-{wifi,bluetooth}.toml` already cover those
    subsystems (M1.5.1b/M1.5.1c). Authoring near-identical fixtures
    under a new `weftos-net-*` prefix would create two near-clones of
    the same surface to maintain.
  - The `audit-surface.sh` D-EM01 rule fires for surfaces under
    `crates/clawft-app/fixtures/` or with `id = "app://..."`. Fixtures
    under `crates/clawft-surface/fixtures/` with `id = "weftos-chip/..."`
    do NOT trigger D-EM01, so the existing chip fixtures pass clean
    *without* `[surfaces.empty_state]` / `loading_state` / `offline_state`.
    Writing new fixtures with those sections would either need to copy
    the no-state pattern (then they're indistinguishable from the
    existing chip-* files) or invent a new state-sections shape that
    the composer doesn't currently consume.
  - The empty/loading/offline contract (DESIGN.md §5) is satisfied by
    `super::state::render_if_needed` running first — the visual
    behaviour the user sees on a no-data Wi-Fi tab is identical to what
    a fully-fixturised path would render.

  Trade-off: when the daemon *does* publish wifi/bluetooth state, the
  user sees raw JSON instead of two chip primitives. This is honest
  ("here's what the adapter wrote") and informative — the values are
  flat enough (state/iface, present/enabled/controller) that the JSON
  dump is readable. Follow-up tickets can graduate to composer
  surfaces with richer renderings once the substrate-over-postMessage
  bridge lands (M1.6+).

### WEFT-586: stream view + filter chips, witness via composer

- **System tab**: top-of-pane filter strip (`All / Info / Warn /
  Error`) using `egui::SelectableLabel`, then a scrollable monospace
  stream view with newest-first ordering (`rows.iter().rev()`).
- **Witness chain tab**: re-uses `desktop::render_chip_detail` against
  `tray::ChipId::ExoChain` — same fixture (`weftos-chip-exochain.toml`)
  the ExoChain chip used. The Logs · Witness chain header replaces
  the chip's own header.

#### Filter strip / `LogLevelFilter`

Defined as a public enum in `apps/logs.rs`:

```rust
pub enum LogLevelFilter { All, Info, Warn, Error }
```

`Default` is `All`. Stored on `Desktop` as `pub log_filter:
LogLevelFilter`, so the chosen filter survives across paints and
between tab switches without leaking into the wider sidebar state.

`accepts(&row_level)` is the predicate. Notable choices:
- Unknown / missing levels (`""`) and `debug`/`trace` count as Info.
  This means a service that forgets to set `level: "info"` doesn't
  silently disappear when the user picks Info — the most common
  fallback bucket should be the most permissive.
- `Error` accepts the aliases `err / crit / fatal` to match what
  Rust's `log` crate, syslog severity, and weaver-internal terms
  produce.

#### Log row schema (assumed)

The substrate snapshot exposes `snap.logs: Option<Vec<Value>>` but no
existing daemon writes `derived/logs/*` yet. I assumed the following
shape for each row, picking the most common conventions:

```jsonc
{
  "level": "info" | "warn" | "error" | "debug" | ...,  // also accepts "severity"
  "msg":   "human-readable text",                       // also "message" or "text"
  ...                                                    // anything else passes through
}
```

`row_level` resolves the level field with case-insensitive matching;
`format_row` falls back to a compact JSON dump (`row.to_string()`)
when no recognised message field is present, so nothing is silently
dropped.

#### Color rule

Every row colour comes from `Tokens` (no raw `Color32::from_rgb`
literals — `audit-theme.sh` would catch new offenders against the
246-baseline):

| Level                    | Token            |
|--------------------------|------------------|
| `warn` / `warning`       | `tokens.warn`    |
| `error` / `err` / `crit` | `tokens.crit`    |
| info / unknown / debug   | `tokens.text_dim`|

#### Stream UX

- Newest-first via `iter().rev()`.
- 12px monospace via `RichText::new(line).monospace().size(12.0)`.
- Filter strip clicks update `*filter = variant` on the
  `&mut Desktop`-owned `log_filter`.

## Files touched

- `crates/clawft-gui-egui/src/apps/network.rs` — graduate to composer
  + JSON-dump body.
- `crates/clawft-gui-egui/src/apps/logs.rs` — graduate to filter
  strip + stream view.
- `crates/clawft-gui-egui/src/shell/desktop.rs`:
  - Removed `#[allow(dead_code)]` from `render_chip_detail`,
    `render_empty_hint`, `connection_pill` (now wired through
    Network · Mesh and Logs · Witness chain).
  - Added `pub log_filter: LogLevelFilter` field + default init.

No new TOML fixtures.

## Gates

- `scripts/build.sh check` — clean.
- `scripts/build.sh clippy` — clean.
- `cargo test -p clawft-gui-egui --lib` — 344 pass (337 baseline +
  7 new tests in `apps::logs::tests`).
- `audit-surface.sh` — n/a (no new fixtures authored).
- `audit-theme.sh --baseline .planning/weftos-design/baseline-color-drift.txt`
  — holds at 246. No new color literals introduced.

## Commits

- `feat(apps): graduate Network app (WEFT-582)` — d65bc2ea
- `feat(apps): graduate Logs app (WEFT-586)` — (pending second commit)

## FINAL STATUS

- WEFT-582 (Network app graduation): **DONE** — Mesh tab on composer
  path, Wi-Fi / Bluetooth on substrate-path + JSON dump, all wired
  through the canonical `super::state::render_if_needed` empty/
  loading/offline gate.
- WEFT-586 (Logs app graduation): **DONE** — System tab with
  filter-chip strip + newest-first monospace stream, Witness chain
  tab re-uses the ExoChain chip-detail composer surface.
- Gates: green (check / clippy / lib tests / audit-theme baseline).
- No fixtures authored; rationale documented above.
- No push performed (per task instruction).
