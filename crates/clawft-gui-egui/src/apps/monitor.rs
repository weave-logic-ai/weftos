//! Monitor — system dashboard. DESIGN.md §9 sidebar 7, archetype
//! `tile-grid` + plots. Phase 3 stub; rolling-window plot wiring
//! ships under WEFT-585.

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
    super::paint_heading(ui, rect, "Monitor");
    let body = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() + 64.0),
        rect.max,
    );
    let has_data = snap.status.is_some();
    super::state::render_if_needed(
        ui,
        body,
        snap,
        has_data,
        "No sensor adapters publishing",
        Some("Install one with `weft adapter install sensors-host`."),
    );
}
