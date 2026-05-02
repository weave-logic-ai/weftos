//! Services — `substrate/kernel/services` table + tabs. DESIGN.md
//! §9 sidebar 3, archetype `app-window`. Phase 3 stub; tabs/start/
//! stop/restart ship under WEFT-581.

use eframe::egui;

use crate::live::Snapshot;

pub fn show(ui: &mut egui::Ui, rect: egui::Rect, snap: &Snapshot) {
    super::paint_heading(ui, rect, "Services");
    let body = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() + 64.0),
        rect.max,
    );
    let has_data = snap.services.as_ref().map(|v| !v.is_empty()).unwrap_or(false);
    super::state::render_if_needed(
        ui,
        body,
        snap,
        has_data,
        "No services registered",
        Some("Register one with `weft service register <name>`."),
    );
}
