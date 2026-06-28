use eframe::egui;
use egui_extras::{Column, TableBuilder};

use super::DemoState;
use crate::live::Snapshot;

pub fn show(ui: &mut egui::Ui, state: &mut DemoState, snap: &Snapshot) {
    ui.heading("Table — Processes");
    ui.label("Sortable process table from `kernel.ps` (click a header to sort).");
    ui.separator();

    let Some(raw) = &snap.processes else {
        ui.label("daemon offline — no process list");
        return;
    };

    let mut rows: Vec<ProcRow> = raw.iter().filter_map(ProcRow::from_json).collect();
    if rows.is_empty() {
        ui.label("no processes running");
        return;
    }

    if let Some(col) = state.table_sort_col {
        rows.sort_by(|a, b| {
            let ord = match col {
                0 => a.pid.cmp(&b.pid),
                1 => a.agent_id.cmp(&b.agent_id),
                2 => a.state.cmp(&b.state),
                3 => a.memory_bytes.cmp(&b.memory_bytes),
                _ => a.cpu_time_ms.cmp(&b.cpu_time_ms),
            };
            if state.table_sort_asc {
                ord
            } else {
                ord.reverse()
            }
        });
    }

    TableBuilder::new(ui)
        .striped(true)
        .column(Column::auto().at_least(60.0))
        .column(Column::auto().at_least(160.0))
        .column(Column::auto().at_least(80.0))
        .column(Column::auto().at_least(100.0))
        .column(Column::remainder())
        .header(24.0, |mut h| {
            for (i, label) in ["PID", "Agent", "State", "Memory", "CPU ms"]
                .iter()
                .enumerate()
            {
                h.col(|ui| {
                    if ui.strong(*label).clicked() {
                        toggle_sort(state, i);
                    }
                });
            }
        })
        .body(|mut body| {
            for (idx, r) in rows.iter().enumerate() {
                let selected = state.selected_row == Some(idx);
                body.row(22.0, |mut row| {
                    row.col(|ui| {
                        if ui.selectable_label(selected, r.pid.to_string()).clicked() {
                            state.selected_row = Some(idx);
                        }
                    });
                    row.col(|ui| {
                        ui.monospace(&r.agent_id);
                    });
                    row.col(|ui| {
                        ui.label(&r.state);
                    });
                    row.col(|ui| {
                        ui.monospace(fmt_bytes(r.memory_bytes));
                    });
                    row.col(|ui| {
                        ui.monospace(r.cpu_time_ms.to_string());
                    });
                });
            }
        });
}

struct ProcRow {
    pid: u64,
    agent_id: String,
    state: String,
    memory_bytes: u64,
    cpu_time_ms: u64,
}

impl ProcRow {
    fn from_json(v: &serde_json::Value) -> Option<Self> {
        Some(Self {
            pid: v.get("pid")?.as_u64()?,
            agent_id: v.get("agent_id")?.as_str()?.to_string(),
            state: v.get("state")?.as_str()?.to_string(),
            memory_bytes: v.get("memory_bytes").and_then(|x| x.as_u64()).unwrap_or(0),
            cpu_time_ms: v.get("cpu_time_ms").and_then(|x| x.as_u64()).unwrap_or(0),
        })
    }
}

fn fmt_bytes(b: u64) -> String {
    if b >= 1 << 30 {
        format!("{:.1} GiB", b as f64 / (1u64 << 30) as f64)
    } else if b >= 1 << 20 {
        format!("{:.1} MiB", b as f64 / (1u64 << 20) as f64)
    } else if b >= 1 << 10 {
        format!("{:.1} KiB", b as f64 / (1u64 << 10) as f64)
    } else {
        format!("{b} B")
    }
}

fn toggle_sort(state: &mut DemoState, col: usize) {
    if state.table_sort_col == Some(col) {
        state.table_sort_asc = !state.table_sort_asc;
    } else {
        state.table_sort_col = Some(col);
        state.table_sort_asc = true;
    }
}
