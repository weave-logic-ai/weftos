//! Scheduler — `substrate/scheduler/*`. DESIGN.md §9 sidebar 6,
//! archetype `app-window`. Phase 3 stub; the scheduler kernel
//! adapter itself is 0.9.x work, so this app perma-renders its
//! empty state until then. WEFT-584.

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
    super::paint_heading(ui, rect, "Scheduler");
    let body = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() + 64.0),
        rect.max,
    );
    super::state::render_if_needed(
        ui,
        body,
        snap,
        false,
        "Scheduler adapter not yet available (0.9.x)",
        Some("Add jobs with `weft schedule add` once the adapter ships."),
    );
}
