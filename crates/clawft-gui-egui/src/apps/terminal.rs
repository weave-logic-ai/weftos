//! Terminal — already shipped at `crates/clawft-gui-egui/src/explorer/
//! terminal.rs`. DESIGN.md §9 sidebar 9. Phase 3 stub re-exports
//! placeholder text; WEFT-587 graduates the real terminal here.

use eframe::egui;

use crate::live::Snapshot;

pub fn show(ui: &mut egui::Ui, rect: egui::Rect, snap: &Snapshot) {
    super::paint_heading(ui, rect, "Terminal");
    let body = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() + 64.0),
        rect.max,
    );
    super::state::render_if_needed(
        ui,
        body,
        snap,
        false,
        "Terminal panel pending graduation from `explorer/terminal.rs`",
        Some("Tracked under WEFT-587."),
    );
}
