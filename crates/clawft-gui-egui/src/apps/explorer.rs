//! Explorer — substrate tree browser. Already shipped at
//! `crates/clawft-gui-egui/src/explorer/mod.rs`. DESIGN.md §9
//! sidebar 12. Phase 3 stub; WEFT-590 moves the existing module
//! here and renames it.

use eframe::egui;

use crate::live::Snapshot;

pub fn show(ui: &mut egui::Ui, rect: egui::Rect, snap: &Snapshot) {
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
