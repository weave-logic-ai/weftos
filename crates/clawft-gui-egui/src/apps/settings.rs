//! Settings — `substrate/config/*` editor. DESIGN.md §9 sidebar 5,
//! archetype `app-window`. Phase 3 stub; schema-driven form
//! generation ships under WEFT-583.

use eframe::egui;

use crate::live::Snapshot;

pub fn show(ui: &mut egui::Ui, rect: egui::Rect, snap: &Snapshot) {
    super::paint_heading(ui, rect, "Settings");
    let body = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() + 64.0),
        rect.max,
    );
    super::state::render_if_needed(
        ui,
        body,
        snap,
        false,
        "Config schema not yet published",
        Some("Run `weaver init` to seed defaults."),
    );
}
