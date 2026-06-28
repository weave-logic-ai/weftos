# wt-explorer-tty-chat — graduation notes

Worktree: `/home/aepod/dev/worktrees/wt-explorer-tty-chat`
Branch: `feat/weft-587-588-590`
Owner: WEFT-587, WEFT-588, WEFT-590
Date: 2026-05-02

## Goal

Graduate three sidebar apps (Terminal, Chat, Explorer) from Phase 3
empty stubs to first-class implementations. Source bodies live under
`crates/clawft-gui-egui/src/explorer/{mod,terminal,chat}.rs` — the
existing chrome is real, working code. Graduation was relocation +
signature adaptation, not a rewrite.

## State-lifting decisions

**No break of `Explorer`'s public API.** The Explorer struct itself
is untouched. Its existing `chat_view`, `terminal_view`, and
`close()` API all stay exactly as they were.

The sidebar Terminal and Chat apps got NEW standalone instances
(`desk.terminal: explorer::terminal::Terminal` and
`desk.chat: explorer::chat::ChatView`) on Desktop, separate from the
Explorer's internal sentinel-dispatch instances.

Why two instances, not one shared:

1. The Explorer's `paint_detail` chat/terminal sentinel-shape
   dispatch is a pre-existing feature that's not in scope for these
   graduations. Sharing state would couple them and break the
   Explorer when the sidebar app changes selection.
2. Two independent panels (one in the Chat sidebar app, one inside
   Explorer reading a chat sentinel) is reasonable UX — they're
   different conceptually:
   - The sidebar app is a permanent "concierge" / "open shell"
     surface.
   - The substrate-sentinel chat/terminal is whatever the substrate
     topology decides to expose under a `{kind:"chat"|"terminal"}`
     value at some path.
3. No break of `Explorer`'s public API. The merger doesn't need to
   audit any change to the Explorer struct.

This is a lift, not a refactor.

## Lifecycle hygiene (WEFT-590)

Single hygiene point added to `apps::dispatch` in
`crates/clawft-gui-egui/src/apps/mod.rs`:

- New `prev_active: SidebarTarget` field on `Desktop` (default
  `Files`, matching `Sidebar::default`'s active).
- At the start of `dispatch`, compare `desk.prev_active` to the
  current `target`.
- On a transition where prev was `SidebarTarget::Explorer`, call
  `desk.explorer.close(live)` so substrate polls don't fire against
  a hidden panel.
- Then update `desk.prev_active = target` for next frame.

Terminal/Chat sidebar apps intentionally do NOT close on nav-away —
the user might be mid-conversation or mid-shell-command and expects
to come back to a running session. Only Explorer's RPC polls leak
budget when nobody's watching, so only Explorer gets the wiring.
This is documented in `dispatch`'s docstring and in
`apps/explorer.rs`'s module doc.

## What changed (per commit)

### b18b3c5e — feat(apps): graduate Terminal app (WEFT-587)

- `crates/clawft-gui-egui/src/shell/desktop.rs`:
  - `use crate::explorer::{self, Explorer};` (added `self`).
  - Added `pub terminal: explorer::terminal::Terminal` field.
  - Added `terminal: explorer::terminal::Terminal::default()` in the
    `Default` impl.
- `crates/clawft-gui-egui/src/apps/terminal.rs` — rewrite.
  - Paints "Terminal" heading via `super::paint_heading`.
  - Carves body rect 64px below.
  - Calls `desk.terminal.paint(ui, live)` inside a `ui.scope_builder`
    confined to the body rect.
  - Wasm gets the existing browser-stub (`"Terminal is not available
    in the browser build."`) for free — that branch lives in the
    lifted `explorer::terminal::Terminal::paint`.

### 76679dc5 — feat(apps): graduate Chat app (WEFT-588)

- `crates/clawft-gui-egui/src/shell/desktop.rs`:
  - Added `pub chat: explorer::chat::ChatView` field.
  - Added `chat: explorer::chat::ChatView::default()` in the
    `Default` impl.
- `crates/clawft-gui-egui/src/apps/chat.rs` — rewrite.
  - Paints "Chat · concierge-bot" heading.
  - Synthesises a cosmetic substrate path (`ui://sidebar/chat`) and
    a `{kind:"chat",model:"local"}` value. Both args are
    cosmetic-only inside `chat::paint`: path is used solely for the
    muted footer hint; value is read only for the model display
    string. Neither drives any RPC.
  - Calls `crate::explorer::chat::paint(...)` against `desk.chat`.
  - Markdown rendering, system-prompt editor, heartbeat label,
    identity-drift warning chip — all preserved untouched. They live
    in `explorer::chat`.

### 7417f9f1 — feat(apps): graduate Explorer app (WEFT-590)

- `crates/clawft-gui-egui/src/shell/desktop.rs`:
  - Added `pub prev_active: sidebar::SidebarTarget` field on
    `Desktop` (default `Files`, kept in lockstep with
    `Sidebar::default`).
  - Dropped `#[allow(dead_code)]` from `render_explorer`.
  - Trimmed `render_explorer`'s inline header — heading is now
    painted by `apps/explorer.rs::show`, this helper just keeps the
    connection pill above the two-pane layout.
- `crates/clawft-gui-egui/src/apps/mod.rs`:
  - Added lifecycle hygiene block to `apps::dispatch` (see above).
  - Documented in `dispatch`'s docstring.
- `crates/clawft-gui-egui/src/apps/explorer.rs` — rewrite.
  - Paints "Explorer · substrate/" heading.
  - Body rect via `ui.scope_builder`.
  - Calls `desktop::render_explorer(ui, desk, live, snap)`.

## Gates run

After all three commits landed:

- `scripts/build.sh check` — clean (3s incremental).
- `scripts/build.sh clippy` — clean, `-D warnings` (5s incremental).
- `cargo test -p clawft-gui-egui --lib` — **337 passed, 0 failed**.

## FINAL STATUS

- **Worktree**: `/home/aepod/dev/worktrees/wt-explorer-tty-chat`
- **Branch**: `feat/weft-587-588-590`
- **Commits** (in graduation order):
  - `b18b3c5e` — feat(apps): graduate Terminal app (WEFT-587)
  - `76679dc5` — feat(apps): graduate Chat app (WEFT-588)
  - `7417f9f1` — feat(apps): graduate Explorer app (WEFT-590)
- **Gates**: all green (`check`, `clippy`, lib tests 337/337).
- **Files changed**:
  - `crates/clawft-gui-egui/src/apps/terminal.rs` (rewrite)
  - `crates/clawft-gui-egui/src/apps/chat.rs` (rewrite)
  - `crates/clawft-gui-egui/src/apps/explorer.rs` (rewrite)
  - `crates/clawft-gui-egui/src/apps/mod.rs` (added lifecycle
    hygiene to `dispatch`)
  - `crates/clawft-gui-egui/src/shell/desktop.rs` (added
    `terminal`, `chat`, `prev_active` fields; removed
    `#[allow(dead_code)]` from `render_explorer`; trimmed inline
    header)
- **Untouched** (per the allow-list):
  - `apps/{files,processes,services,network,settings,scheduler,monitor,logs,admin,launcher}.rs`
  - `shell/sidebar.rs`
- **Followups** (none blocking 0.7.0 release):
  - Terminal/Chat sidebar app PTY/conversation cleanup on nav-away
    is intentionally not wired (left running so the user can return
    to a session). If the merger wants tighter budget control, add a
    `close()` API to those panels and extend `apps::dispatch`'s
    lifecycle block. Decision documented in
    `apps/mod.rs::dispatch` and `apps/explorer.rs` module docs.
  - `desktop::render_explorer` could fold its connection pill into
    `apps/explorer.rs::show` directly and be retired entirely. Out
    of scope for graduation.
  - Chat sidebar app's synthesised sentinel could be replaced with a
    real substrate mount under `ui/concierge` once that topology
    surface is defined. Today it's cosmetic-only.
