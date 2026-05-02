//! Apps launcher — Built-in / Installed / Developer tile-grid.
//! DESIGN.md §9 sidebar 13. Phase 3 stub; full tile-grid + search
//! field + category filter ship under WEFT-591. The Developer
//! sub-tab is where the existing Blocks / Canon demos relocate.

use eframe::egui;

use crate::live::Snapshot;
use crate::shell::sidebar::AppsTab;

pub fn show(ui: &mut egui::Ui, rect: egui::Rect, snap: &Snapshot, tab: AppsTab) {
    let heading = match tab {
        AppsTab::BuiltIn => "Apps · Built-in",
        AppsTab::Installed => "Apps · Installed",
        AppsTab::Developer => "Apps · Developer",
    };
    super::paint_heading(ui, rect, heading);
    let body = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() + 64.0),
        rect.max,
    );
    let what = match tab {
        AppsTab::BuiltIn => "Tile-grid pending implementation",
        AppsTab::Installed => "No apps installed",
        AppsTab::Developer => "Blocks / Canon demos relocate here in WEFT-591",
    };
    let hint = match tab {
        AppsTab::Installed => Some("Install with `weft app install <id>`."),
        _ => Some("Tracked under WEFT-591."),
    };
    super::state::render_if_needed(ui, body, snap, false, what, hint);
}
