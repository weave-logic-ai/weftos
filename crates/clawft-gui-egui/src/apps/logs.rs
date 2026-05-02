//! Logs — `derived/logs/*` stream + Witness chain mode. DESIGN.md §9
//! sidebar 8, archetype `stream`, group expandable in sidebar.
//! Phase 3 stub; filter strip + tail control ship under WEFT-586.

use std::sync::Arc;

use eframe::egui;

use crate::live::{Live, Snapshot};
use crate::shell::desktop::Desktop;
use crate::shell::sidebar::LogsTab;

pub fn show(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    _desk: &mut Desktop,
    _live: &Arc<Live>,
    snap: &Snapshot,
    tab: LogsTab,
) {
    let heading = match tab {
        LogsTab::System => "Logs · System",
        LogsTab::WitnessChain => "Logs · Witness chain",
    };
    super::paint_heading(ui, rect, heading);
    let body = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() + 64.0),
        rect.max,
    );
    let has_data = match tab {
        LogsTab::System => snap.logs.as_ref().map(|v| !v.is_empty()).unwrap_or(false),
        LogsTab::WitnessChain => snap.chain_status.is_some(),
    };
    let (what, hint) = match tab {
        LogsTab::System => (
            "No logs published yet",
            "Logs flow through `derived/logs/*` once a service writes.",
        ),
        LogsTab::WitnessChain => (
            "Witness chain not initialised",
            "Run `weaver chain init` to create the chain.",
        ),
    };
    super::state::render_if_needed(ui, body, snap, has_data, what, Some(hint));
}
