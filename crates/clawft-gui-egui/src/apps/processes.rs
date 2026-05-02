//! Processes — `substrate/kernel/processes` table. DESIGN.md §9
//! sidebar 2, archetype `app-window`. Phase 3 stub; full table
//! ships under WEFT-580 (existing `explorer::viewers::process_table`
//! gets graduated).

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
