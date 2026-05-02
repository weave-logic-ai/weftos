//! Admin — reference app, already shipped via the surface composer
//! against `weftos-admin.toml`. DESIGN.md §9 sidebar 11. Phase 3
//! stub; WEFT-589 wires the existing composer render here and adds
//! the missing empty/loading/offline states (D-EM01 violations
//! flagged by `audit-surface.sh`).

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
    super::paint_heading(ui, rect, "WeftOS Admin");
    let body = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() + 64.0),
        rect.max,
    );
    super::state::render_if_needed(
        ui,
        body,
        snap,
        false,
        "Admin app pending graduation from legacy launcher",
        Some("Tracked under WEFT-589."),
    );
}
