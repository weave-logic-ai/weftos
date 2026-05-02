# WEFT-579..591 graduation wave + tray removal

Branch: `feat/weftos-579-591-graduations` (off `master @ b6c6e46f`).

## User intent

> complete WEFT-579-591 we cannot release 0.7.0 until this is done. The
> bottom bar is also still there, that should have been removed.

So this wave moves WEFT-579..591 from the 0.8.x cycle into 0.7.0
release-blockers and additionally retires the bottom tray (`shell/tray.rs`
paint call from `desktop.rs:324`).

## Phase A — foundational (on graduation branch, before fan-out)

Single commit. Lays the groundwork for parallel worktrees.

1. **Remove tray paint** from `desktop.rs::show`. Drop the `tray::paint(...)`
   call. Module file stays compiled (no churn) but `chip_subtree`,
   `ChipId`, etc. are still referenced by chip-detail rendering and by
   the `apply_sidebar_action` chip pathway, so we keep the module.
2. **Kill the dual-render side-effects** in `Desktop::apply_sidebar_action`:
   - `SidebarTarget::Admin` no longer flips `launcher_open=true` /
     `section=Apps`. Admin is rendered via the new app stub.
   - `SidebarTarget::Apps(_)` no longer flips `launcher_open=true`. The
     graduated `apps/launcher.rs` owns the entire panel.
   - `SidebarTarget::Network(_)` and `Logs(WitnessChain)` STOP returning
     `Some(ChipId::Mesh)`. Network/Logs apps own their own bodies.
   - `SidebarTarget::Explorer` STOPS returning `Some(ChipId::Explorer)`.
     The graduated Explorer app owns `&mut Explorer`.
3. **Broaden `apps::dispatch` signature** to take `&mut Desktop` and
   `&Arc<Live>`. Each graduating app needs to read mutable state
   (Explorer expansion set, Terminal PTY buf, Chat draft, etc.) and
   submit RPC commands. Update the call site in `desktop.rs::show`.
4. **Drop the floating Blocks window** from `desktop.rs::show`. The
   legacy Blocks/Canon/Apps demos relocate under WEFT-591's Apps ·
   Developer tab, but Phase A leaves them rendered nowhere — the
   parallel agents put them back where they belong. Until WEFT-591
   merges, the Developer demos are temporarily inaccessible. That's
   acceptable for the graduation wave because the user is shipping
   the desktop-as-product, not the dev tooling.
5. **Drop the `open_chip` floating window** from `desktop.rs::show`.
   The chip-detail view code (`render_chip_detail`, `render_explorer`,
   etc.) is moved into the apps that own those substrates: Network app
   renders the chip-surface composer paths; Explorer app owns Explorer.
   Phase A stops painting the chip windows so we don't double-render.

After Phase A, every sidebar click produces exactly one paint, every
app stub still works (just empty until graduated), and the desktop is
visually clean.

## Worktree fan-out

Worktrees branch off the Phase A commit. Each cluster is self-contained:
agents only write to their own `apps/<name>.rs` files and (where needed)
new TOML fixtures under `crates/clawft-surface/fixtures/`. Conflicts are
limited to `apps/mod.rs` (the dispatch table is already correct after
Phase A) and occasionally `Desktop` struct fields when an app needs a
new `&mut` state member.

| Worktree | Branch | Items | Existing source to graduate from |
|---|---|---|---|
| wt-admin-launcher | `feat/weft-589-591` | WEFT-589 (Admin) + WEFT-591 (Apps launcher) | desktop.rs::render_selected_app + render_blocks_window |
| wt-explorer-tty-chat | `feat/weft-587-588-590` | WEFT-587 (Terminal) + WEFT-588 (Chat) + WEFT-590 (Explorer) | explorer/terminal.rs + explorer/chat.rs + explorer/mod.rs |
| wt-network-logs | `feat/weft-582-586` | WEFT-582 (Network) + WEFT-586 (Logs) | desktop.rs::render_chip_detail + chip-surface fixtures |
| wt-files-procs-svcs | `feat/weft-579-580-581` | WEFT-579 (Files) + WEFT-580 (Processes) + WEFT-581 (Services) | blocks/table.rs + substrate snapshot |
| wt-settings-sched-mon | `feat/weft-583-584-585` | WEFT-583 (Settings) + WEFT-584 (Scheduler) + WEFT-585 (Monitor) | blocks/* + composer + new TOML fixtures |

Each agent writes notes to
`.planning/weftos-579-591-graduations/notes/<worktree>.md`.

## Merge order back into the graduation branch

1. wt-admin-launcher (most desktop.rs surface area; merge first)
2. wt-explorer-tty-chat (touches Desktop struct for state lifts)
3. wt-network-logs
4. wt-files-procs-svcs
5. wt-settings-sched-mon
6. Final phase: build/clippy/test/audit, rebuild wasm-panel

## Audit + ratchet

- `audit-theme.sh --baseline` — must hold ≤ 246 (the recorded floor).
- `audit-surface.sh` — D-EM01 violations should drop to zero on
  `weftos-admin.toml` (allowance ratchets from 1 → 0).
- New fixtures must pass `audit-surface.sh` clean.

## Plane state

After landing, transition Plane:
- WEFT-578 → Done (sidebar already canonical)
- WEFT-579..591 → Done with commit SHAs
- Move from 0.8.x cycle → 0.7.x cycle (per user: 0.7.0 release-blockers)
- 0.7.0 close gate flips green once all 14 are Done.
