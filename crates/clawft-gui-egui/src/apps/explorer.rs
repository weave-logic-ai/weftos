//! Explorer — substrate tree browser. Already shipped at
//! `crates/clawft-gui-egui/src/explorer/mod.rs`. DESIGN.md §9
//! sidebar 12. Phase 3 stub; WEFT-590 moves the existing module
//! here and renames it.

use std::sync::Arc;

use eframe::egui;

use crate::live::{Live, Snapshot};
use crate::shell::desktop::Desktop;

pub fn show(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    _desk: &mut Desktop,
    _live: &Arc<Live>,
    snap: &Snapshot,
) {
    super::paint_heading(ui, rect, "Explorer");
    let body = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() + 64.0),
        rect.max,
    );
    super::state::render_if_needed(
        ui,
        body,
        snap,
        false,
        "Explorer pending graduation from `explorer/mod.rs`",
        Some("Tracked under WEFT-590."),
    );
}
