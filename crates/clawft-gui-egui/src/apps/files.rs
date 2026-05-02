//! Files — substrate browser. DESIGN.md §9 sidebar 1, archetype
//! `app-window` (DESIGN.md §4.1). Phase 3 stub; full list-detail
//! implementation lands under WEFT-579.

use eframe::egui;

use crate::live::Snapshot;

pub fn show(ui: &mut egui::Ui, rect: egui::Rect, snap: &Snapshot) {
    super::paint_heading(ui, rect, "Files");
    let body = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() + 64.0),
        rect.max,
    );
    super::state::render_if_needed(
        ui,
        body,
        snap,
        false,
        "No filesystem adapter installed",
        Some("Install one with `weft adapter install fs`."),
    );
}
