//! Processes — `substrate/kernel/processes` table. DESIGN.md §9
//! sidebar 2, archetype `app-window`. Phase 3 stub; full table
//! ships under WEFT-580 (existing `explorer::viewers::process_table`
//! gets graduated).

use eframe::egui;

use crate::live::Snapshot;

pub fn show(ui: &mut egui::Ui, rect: egui::Rect, snap: &Snapshot) {
    super::paint_heading(ui, rect, "Processes");
    let body = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() + 64.0),
        rect.max,
    );
    let has_data = snap
        .processes
        .as_ref()
        .map(|v| !v.is_empty())
        .unwrap_or(false);
    super::state::render_if_needed(
        ui,
        body,
        snap,
        has_data,
        "No processes reported",
        Some("Substrate adapter `kernel.processes` is not yet publishing."),
    );
}
