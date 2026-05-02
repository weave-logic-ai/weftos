# Worktree notes — wt-settings-sched-mon (WEFT-583/584/585)

Worktree: `/home/aepod/dev/worktrees/wt-settings-sched-mon`
Branch:   `feat/weft-583-584-585`
Owner:    Settings + Scheduler + Monitor app graduations.

## WEFT-583 — Settings

### Config schema decision

There is **no** `config` field on `crate::live::Snapshot` and no kernel
adapter publishes `substrate/config/*` in the 0.7.0 cut. The daemon
does maintain a per-workspace JSON file at `.clawft/config.json` and
exposes two RPCs (`crates/clawft-weave/src/daemon.rs`):

- `workspace.config.set` — `{"key":"<dotted.path>","value":"<string>"}`
  with primitive-coercion (`true`/`false`/int → JSON, otherwise
  string).
- `workspace.config.get` — `{"key":"<dotted.path>"}` returns the
  current value or null.

Decision: Settings reads from `live.substrate_snapshot()` for any
`substrate/config*` topics (forward-compat — when an adapter ships
that mirrors the file into the substrate, the form populates without
code changes), and falls back to the empty state with the
"`weaver init` to seed defaults" hint when nothing is published. The
list-detail archetype shape is *always* drawn so the user sees the
shape before adapters are wired.

Submission target: `workspace.config.set`. Naming matches the
existing CLI (`weft workspace config set <key> <value>`) so the
form's behaviour is consistent with the command line. The `params`
shape mirrors the daemon handler at line 4932 of `daemon.rs`.

Debounce: 500 ms per field (per-key buffer keyed by dotted path).
The buffer is dropped on submit so the next snapshot tick re-seeds
from the freshly-published value, avoiding ping-pong while the user
keeps typing. State buffer lives on `Desktop::settings_state`
(`SettingsState`).

### Things deferred

- No daemon-side `substrate/config` adapter — the form will be
  empty in the typical 0.7.0 deployment.
- `workspace.config.set` accepts only string-coerced values today;
  Bool/Int/Float editors send the stringified form which the daemon
  parses back into JSON primitives. Once an adapter publishes a
  schema with explicit JSON types we can switch the wire format to
  `serde_json::Value` directly.

## WEFT-584 — Scheduler

Acceptance: shell-only in 0.7.0.

The scheduler kernel adapter is 0.9.x work. There is no
`snap.scheduler` field today. The graduated app paints the
table-over-plot archetype shape (column headers + plot axes) and
drops the empty-state hint *inside* the table region so the user
sees both the schema shape and the remediation. When
`substrate/scheduler/jobs` starts publishing, the
`paint_jobs_table` / `paint_plot_region` scaffolds (currently
`#[allow(dead_code)]`) take over.

Probe path used: `live.substrate_snapshot().read("substrate/scheduler/jobs")`.

## WEFT-585 — Monitor

### Tile schema

| Title    | Source                  | KPI                        | Sub-label                |
| -------- | ----------------------- | -------------------------- | ------------------------ |
| Kernel   | `snap.status`           | `state` (running, …)       | `up <h>m · <p>p / <s>s` |
| Mesh     | `snap.mesh_status`      | `<healthy>/<total>`        | "healthy / total"       |
| Chain    | `snap.chain_status`     | `#<sequence \| event_count>` | "chain <chain_id>"     |
| Mic      | `snap.audio_mic`        | `<rms_db> dB`              | sample rate              |
| ToF      | `snap.tof_depth`        | `<w>×<h>`                  | "min <mm> mm"           |
| Battery  | `snap.network_battery`  | `<percent>%`               | charging / on battery    |

Tile layout: `220 × 140` px, `12 px` gap, `16 px` padding around the
grid, wrapping when the row width exceeds the body. Frame uses
`tokens.bg_panel` fill + `tokens.stroke_soft` stroke
(DESIGN.md §3 `tile-grid`).

Empty state: only when *every* listed source is `None` (which is the
typical 0.7.0 path). The moment any one source has data the full
grid renders and missing tiles show "—" so the user can see the
schema even before all adapters are wired.

Sparkline: hand-rolled `Painter::line_segment` polyline (no
`egui_plot` overhead per tile). Currently never drawn because no
adapter publishes a rolling-window series; the rendering path is
exercised in unit tests indirectly via the `Tile::spark` field.

## Gates

- `scripts/build.sh check`        — clean
- `scripts/build.sh clippy`       — clean
- `cargo test -p clawft-gui-egui --lib` — 344 passed (was 341; +3 graduation tests)
- `audit-theme.sh --baseline …`   — D-TK01 holds at baseline 246 (no new color drift)

## FINAL STATUS

Three apps graduated from Phase-3 stubs to first-class
implementations on `feat/weft-583-584-585`. All four gates green.
Color discipline preserved: every new visual goes through
`Tokens::default()` from `theming.rs`; the 246-offender baseline
is untouched. No new tokens were needed.

Three commits ready (one per WEFT ticket); not pushed.
