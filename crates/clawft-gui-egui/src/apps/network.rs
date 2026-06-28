//! Network — Mesh / Wi-Fi / Bluetooth tabs. DESIGN.md §9 sidebar 4,
//! archetype `app-window`, group expandable in sidebar.
//!
//! Graduated under WEFT-582 from the Phase 3 stub. The Mesh tab now
//! renders the existing chip-detail surface (the same one the Mesh
//! tray chip used before tray retirement) through the surface
//! composer; Wi-Fi and Bluetooth render a heading + scrollable
//! pretty-printed JSON dump of the bound substrate value, falling
//! back to the standard empty/loading/offline state via
//! [`super::state::render_if_needed`] when the snapshot has nothing.
//!
//! Why JSON for Wi-Fi/Bluetooth (vs new TOML fixtures)? The existing
//! `weftos-chip-{wifi,bluetooth}.toml` fixtures already cover those
//! subsystems for the chip-detail path; duplicating them here as
//! `weftos-net-*.toml` would just create a new audit liability without
//! changing what the user sees. The composer path stays canonical for
//! Mesh, and the JSON dump for the other two is honest about "the
//! adapter wrote this; here's what landed" — useful while the WEFT
//! follow-up tickets that ship richer Wi-Fi/Bluetooth surfaces are
//! still in flight. See `wt-network-logs.md` notes for the full
//! decision rationale.

use std::sync::Arc;

use eframe::egui;

use crate::live::{Live, Snapshot};
use crate::shell::desktop::{self, Desktop};
use crate::shell::sidebar::NetworkTab;
use crate::shell::tray;

pub fn show(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    desk: &mut Desktop,
    live: &Arc<Live>,
    snap: &Snapshot,
    tab: NetworkTab,
) {
    let tab_name = match tab {
        NetworkTab::Mesh => "Mesh",
        NetworkTab::WiFi => "Wi-Fi",
        NetworkTab::Bluetooth => "Bluetooth",
    };
    super::paint_heading(ui, rect, &format!("Network · {}", tab_name));

    let body = egui::Rect::from_min_max(egui::pos2(rect.left(), rect.top() + 64.0), rect.max);

    // Whether the bound substrate path has data. Drives the
    // empty/loading/offline branch decision below.
    let has_data = match tab {
        NetworkTab::Mesh => snap.mesh_status.is_some(),
        NetworkTab::WiFi => snap.network_wifi.is_some(),
        NetworkTab::Bluetooth => snap.bluetooth.is_some(),
    };

    let (what, hint) = match tab {
        NetworkTab::Mesh => ("No mesh peers", "Run `weft mesh join <key>` to connect."),
        NetworkTab::WiFi => (
            "Wi-Fi adapter not detected",
            "Install with `weft adapter install wifi`.",
        ),
        NetworkTab::Bluetooth => (
            "Bluetooth adapter not detected",
            "Install with `weft adapter install bluetooth`.",
        ),
    };

    // Empty/loading/offline — short-circuit if any of those apply.
    if super::state::render_if_needed(ui, body, snap, has_data, what, Some(hint)) {
        return;
    }

    // Connected with data — render the per-tab body inside `body`.
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(body)
            .layout(egui::Layout::top_down(egui::Align::LEFT)),
    );

    match tab {
        NetworkTab::Mesh => {
            // Composer path — the canonical chip-detail surface for
            // `substrate/mesh/status`, lifted out of the retired tray
            // chip-detail floating window.
            desktop::render_chip_detail(&mut child, desk, tray::ChipId::Mesh, live, snap);
        }
        NetworkTab::WiFi => {
            render_json_dump(
                &mut child,
                "substrate/network/wifi",
                snap.network_wifi.as_ref(),
            );
        }
        NetworkTab::Bluetooth => {
            render_json_dump(&mut child, "substrate/bluetooth", snap.bluetooth.as_ref());
        }
    }
}

/// Render a substrate path label + pretty-printed JSON dump in a
/// scrollable monospace area. Keeps the body honest while the
/// WEFT-followup tickets graduate Wi-Fi / Bluetooth into composer
/// surfaces of their own.
fn render_json_dump(ui: &mut egui::Ui, path: &str, value: Option<&serde_json::Value>) {
    ui.horizontal(|ui| {
        ui.monospace(path);
    });
    ui.separator();

    // `render_if_needed` already short-circuited the `None` case above,
    // but stay defensive: if some race somehow hands us None here, fall
    // through to a one-line message rather than panicking.
    let pretty = match value {
        Some(v) => serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string()),
        None => "(no data)".to_string(),
    };

    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut pretty.as_str())
                    .font(egui::TextStyle::Monospace)
                    .desired_width(f32::INFINITY)
                    .code_editor()
                    .interactive(false),
            );
        });
}
