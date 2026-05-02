//! Processes — `substrate/kernel/processes` table. DESIGN.md §9
//! sidebar 2, archetype `app-window`. Graduates the Phase 3 stub
//! to the existing `explorer::viewers::process_table::ProcessTableViewer`,
//! which already handles the canonical `kernel.ps` shape (sortable
//! columns, state colouring, byte/cpu formatting).
//!
//! WEFT-580.

use std::sync::Arc;

use eframe::egui;
use serde_json::Value;

use crate::explorer::viewers::{process_table::ProcessTableViewer, SubstrateViewer};
use crate::live::{Live, Snapshot};
use crate::shell::desktop::Desktop;

pub fn show(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    _desk: &mut Desktop,
    _live: &Arc<Live>,
    snap: &Snapshot,
) {
    super::paint_heading(ui, rect, "Processes");
    let body = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() + 64.0),
        rect.max,
    );
    let has_data = snap
        .processes
        .as_ref()
        .map(|v| !v.is_empty())
        .unwrap_or(false);
    if super::state::render_if_needed(
        ui,
        body,
        snap,
        has_data,
        "No processes reported",
        Some("Substrate adapter `kernel.processes` is not yet publishing."),
    ) {
        return;
    }

    // Connected, has data — paint the table inside the body rect.
    // We construct a child UI clipped to the body so the heading
    // stays out of the table's scroll area.
    let rows = snap.processes.as_ref().expect("has_data implies Some");
    let value = Value::Array(rows.clone());

    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(body.shrink2(egui::vec2(24.0, 8.0)))
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    ProcessTableViewer::paint(&mut child, "substrate/kernel/processes", &value);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::Connection;
    use serde_json::json;

    fn run_show(snap: Snapshot) {
        let ctx = egui::Context::default();
        let mut desk = Desktop::default();
        let live = Live::spawn();
        ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let rect = ui.max_rect();
                show(ui, rect, &mut desk, &live, &snap);
            });
        });
    }

    #[test]
    fn show_does_not_panic_with_default_snapshot() {
        run_show(Snapshot::default());
    }

    #[test]
    fn show_does_not_panic_with_kernel_ps_rows() {
        let mut snap = Snapshot::default();
        snap.connection = Connection::Connected;
        snap.processes = Some(vec![
            json!({
                "pid": 1,
                "agent_id": "kernel",
                "state": "running",
                "memory_bytes": 1_048_576_u64,
                "cpu_time_ms": 1200_u64,
            }),
            json!({
                "pid": 2,
                "agent_id": "mic-capture",
                "state": "sleeping",
                "memory_bytes": 524288_u64,
                "cpu_time_ms": 80_u64,
            }),
        ]);
        run_show(snap);
    }

    #[test]
    fn show_paints_empty_state_when_connected_with_no_rows() {
        let mut snap = Snapshot::default();
        snap.connection = Connection::Connected;
        snap.processes = Some(vec![]);
        run_show(snap);
    }
}
