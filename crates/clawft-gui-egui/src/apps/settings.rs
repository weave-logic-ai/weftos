//! Settings — `substrate/config/*` editor. DESIGN.md §9 sidebar 5,
//! archetype `app-window`. Phase 3 stub; schema-driven form
//! generation ships under WEFT-583.

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
