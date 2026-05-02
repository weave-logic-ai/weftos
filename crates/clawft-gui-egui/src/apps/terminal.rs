//! Terminal — first-class sidebar app (WEFT-587). Real PTY-backed
//! terminal emulator graduated from
//! `crates/clawft-gui-egui/src/explorer/terminal.rs`. DESIGN.md §9
//! sidebar 9.
//!
//! The renderer + RPC machinery (alacritty grid, vte parser, mouse
//! selection, scrollback wheel handler, etc.) live in
//! `explorer::terminal`. This module is a thin host: it owns nothing
//! itself, paints the canonical heading, and delegates the body rect
//! to the standalone [`Terminal`](crate::explorer::terminal::Terminal)
//! instance kept on the [`Desktop`].
//!
//! State-lifting note: `desk.terminal` is independent from
//! `desk.explorer.terminal_view` (which still backs the
//! substrate-sentinel dispatch path inside the Explorer detail pane).
//! Two panels by design — the sidebar app is the user's "open a shell"
//! affordance; the substrate-sentinel terminal is whatever the
//! substrate topology decides to expose.
//!
//! Wasm builds get the existing
//! `explorer::terminal::Terminal`'s "browser unavailable" placeholder
//! for free — that branch lives in the lifted module, not here.

use std::sync::Arc;

use eframe::egui;

use crate::live::{Live, Snapshot};
use crate::shell::desktop::Desktop;

/// Heading band height. Matches the convention shared with
/// `apps/explorer.rs` and `apps/chat.rs` — `paint_heading` writes its
/// glyphs at `rect.left() + 24, rect.top() + 24`, so the body must
/// start below that.
const HEADING_BAND_H: f32 = 64.0;

pub fn show(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    desk: &mut Desktop,
    live: &Arc<Live>,
    _snap: &Snapshot,
) {
    super::paint_heading(ui, rect, "Terminal");
    let body = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() + HEADING_BAND_H),
        rect.max,
    );
    // Confine the terminal body to the rect carved out below the
    // heading. `scope_builder` (vs. raw `new_child`) wraps the child
    // Ui so its widget entry is finalised against actual bounds —
    // important for the focus + click-and-drag plumbing inside
    // `Terminal::paint`.
    ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
        desk.terminal.paint(ui, live);
    });
}
