//! Network — Mesh / Wi-Fi / Bluetooth tabs. DESIGN.md §9 sidebar 4,
//! archetype `app-window`, group expandable in sidebar. Phase 3
//! stub; full implementation wraps the existing chip TOML fixtures
//! under WEFT-582.

use eframe::egui;

use crate::live::Snapshot;
use crate::shell::sidebar::NetworkTab;

pub fn show(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    snap: &Snapshot,
    tab: NetworkTab,
) {
    let heading = match tab {
        NetworkTab::Mesh => "Network · Mesh",
        NetworkTab::WiFi => "Network · Wi-Fi",
        NetworkTab::Bluetooth => "Network · Bluetooth",
    };
    super::paint_heading(ui, rect, heading);
    let body = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() + 64.0),
        rect.max,
    );
    let has_data = match tab {
        NetworkTab::Mesh => snap.mesh_status.is_some(),
        NetworkTab::WiFi => snap.network_wifi.is_some(),
        NetworkTab::Bluetooth => snap.bluetooth.is_some(),
    };
    let (what, hint) = match tab {
        NetworkTab::Mesh => (
            "No mesh peers",
            "Run `weft mesh join <key>` to connect.",
        ),
        NetworkTab::WiFi => (
            "Wi-Fi adapter not detected",
            "Install with `weft adapter install wifi`.",
        ),
        NetworkTab::Bluetooth => (
            "Bluetooth adapter not detected",
            "Install with `weft adapter install bluetooth`.",
        ),
    };
    super::state::render_if_needed(ui, body, snap, has_data, what, Some(hint));
}
