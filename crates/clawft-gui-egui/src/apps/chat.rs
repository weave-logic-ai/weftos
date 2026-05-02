//! Chat — concierge agent stream. Already shipped at
//! `crates/clawft-gui-egui/src/explorer/chat.rs`. DESIGN.md §9
//! sidebar 10. Phase 3 stub; WEFT-588 graduates the real chat panel.

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
    super::paint_heading(ui, rect, "Chat · concierge-bot");
    let body = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() + 64.0),
        rect.max,
    );
    super::state::render_if_needed(
        ui,
        body,
        snap,
        false,
        "Chat panel pending graduation from `explorer/chat.rs`",
        Some("Tracked under WEFT-588."),
    );
}
