//! Legacy demo blocks — the 12-item demo gallery shown by the
//! `weft-demo-lab` binary and the [`Desktop`](crate::shell::desktop)
//! showroom panel.
//!
//! # Relationship to [`crate::canon`]  (WEFT-286)
//!
//! There are intentionally two parallel widget paths in this crate:
//!
//! | Path        | Purpose                                  | Surface           |
//! |-------------|------------------------------------------|-------------------|
//! | `canon/`    | The frozen 21-primitive vocabulary       | Production renderer |
//! | `blocks/`   | Demo gallery + theming spike             | `weft-demo-lab` + Desktop showroom |
//!
//! `canon/` is the system-of-record. ADR-001 froze the 21-primitive
//! vocabulary; every renderer (egui, ratatui, web) is required to
//! speak it, and a [`canon::CanonResponse`] is what flows back through
//! the kernel boundary. New product-facing UI is built out of canon
//! primitives.
//!
//! `blocks/` is the original 12-block demo gallery — predates ADR-001
//! and exists for two non-trivial reasons:
//!
//! 1. **Theming proof.** `weft-demo-lab` paints both the upstream
//!    `egui_demo_lib::DemoWindows` and the WeftOS-themed blocks
//!    side-by-side so a theming change can be visually A/B'd against
//!    a known control.  The blocks intentionally cover 12 *different*
//!    layout primitives (text, table, tree, tabs, oscilloscope, …)
//!    to exercise the theme broadly.
//! 2. **Showroom continuity.** The Desktop's
//!    [`BlockKind`](crate::shell::desktop::BlockKind) panel still
//!    reaches into `blocks::*::show` for its built-in showcase tab.
//!    Replacing those call sites with canon primitives is the long-
//!    arc retirement plan; the wrappers don't exist yet.
//!
//! The 0.6.19 changelog mentioned a "retrofit pass" that rewrote
//! several blocks in terms of canon primitives. That work was
//! partial — most blocks still hold their own egui code. The
//! retirement plan, captured here so future readers don't re-discover
//! it from changelogs:
//!
//! 1. **Wrap, then retire.** Each `blocks::<name>::show` becomes a
//!    thin call into the equivalent canon primitive (e.g.
//!    `blocks::table` → `canon::Table`, `blocks::tabs` →
//!    `canon::Tabs`).
//! 2. **Drop the BlockKind variant** from `shell::desktop` once the
//!    showroom can reach the wrapped block via the canon registry.
//! 3. **Retire `weft-demo-lab` blocks tab** when the canon gallery
//!    covers enough primitives for the theming proof.
//!
//! Until that retirement is complete this module remains live; it is
//! **not** dead code.

pub mod budget;
pub mod button;
pub mod code;
pub mod layout;
pub mod oscilloscope;
pub mod overview;
pub mod status;
pub mod table;
pub mod tabs;
pub mod terminal;
pub mod text;
pub mod tree;

/// Demo-wide state so interactive blocks (counter, tabs selection, table
/// sorting, terminal history, oscilloscope time) persist across frames.
pub struct DemoState {
    pub counter: u32,
    pub tab_idx: usize,
    pub table_sort_col: Option<usize>,
    pub table_sort_asc: bool,
    pub selected_row: Option<usize>,
    pub tree_open: std::collections::HashSet<&'static str>,
    pub terminal_input: String,
    pub terminal_history: Vec<(TerminalLineKind, String)>,
    pub pending_rpcs: Vec<terminal::PendingRpc>,
    pub scope_t: f32,
    pub scope_samples: std::collections::VecDeque<(f64, f64)>,
}

#[derive(Copy, Clone)]
pub enum TerminalLineKind {
    Input,
    Output,
    Error,
}

impl Default for DemoState {
    fn default() -> Self {
        let mut tree_open = std::collections::HashSet::new();
        tree_open.insert("kernel");
        tree_open.insert("kernel/services");

        Self {
            counter: 0,
            tab_idx: 0,
            table_sort_col: None,
            table_sort_asc: true,
            selected_row: None,
            tree_open,
            terminal_input: String::new(),
            terminal_history: vec![
                (
                    TerminalLineKind::Output,
                    "weft v0.6.17 — egui RPC console".into(),
                ),
                (
                    TerminalLineKind::Output,
                    "Type `help` for commands. Tries kernel daemon on localhost.".into(),
                ),
            ],
            pending_rpcs: Vec::new(),
            scope_t: 0.0,
            scope_samples: std::collections::VecDeque::with_capacity(512),
        }
    }
}
