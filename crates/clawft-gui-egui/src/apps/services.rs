//! Services — `substrate/kernel/services` table + tab filter.
//! DESIGN.md §9 sidebar 3, archetype `app-window`. WEFT-581.
//!
//! Schema (canonical `kernel.services` shape, observed via the Admin
//! surface and `clawft_substrate::projection::project_service_rows`):
//!
//! ```jsonc
//! [
//!   { "name": "weave", "state": "running", "pid": 1234,
//!     "restarts": 0, "uptime_ms": 1_234_567 },
//!   { "name": "whisper", "state": "stopped", "pid": null,
//!     "restarts": 2, "uptime_ms": 0 },
//! ]
//! ```
//!
//! Field names that aren't present fall back to "-" — the row still
//! renders so a half-populated adapter doesn't yield a blank column.
//!
//! Per-row affordances are contextual on the row's `state`:
//!
//! - **Running** rows show `[stop]` and `[restart]`.
//! - **Inactive** rows (stopped / failed / unknown) show `[start]`.
//!
//! Each click submits the matching verb (`service.start`,
//! `service.stop`, `service.restart`) through the [`Live`] bridge.
//! No modal — the daemon is the source of truth, and the next
//! snapshot tick reflects whatever happened. A failed RPC stays
//! visible because the row's state simply doesn't change.

use std::sync::Arc;

use eframe::egui;
use serde_json::{Value, json};

use crate::live::{Command, Live, Snapshot};
use crate::shell::desktop::Desktop;
use crate::theming::Tokens;

/// Filter tab on the Services panel. Drives the row predicate and is
/// persisted on [`Desktop::services_tab`] so the choice survives
/// sidebar navigation.
#[derive(Copy, Clone, Default, PartialEq, Eq)]
pub enum ServicesTab {
    #[default]
    All,
    Active,
    Inactive,
}

impl ServicesTab {
    fn label(self) -> &'static str {
        match self {
            ServicesTab::All => "All",
            ServicesTab::Active => "Active",
            ServicesTab::Inactive => "Inactive",
        }
    }

    /// Predicate over a row's `state` field. `Active` matches any state
    /// in the running family; `Inactive` matches everything that isn't
    /// `Active` (including unknown / missing states), so a malformed row
    /// shows up in `Inactive` rather than disappearing.
    fn includes(self, state: &str) -> bool {
        match self {
            ServicesTab::All => true,
            ServicesTab::Active => matches!(state, "running" | "active" | "ready"),
            ServicesTab::Inactive => !matches!(state, "running" | "active" | "ready"),
        }
    }
}

pub fn show(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    desk: &mut Desktop,
    live: &Arc<Live>,
    snap: &Snapshot,
) {
    super::paint_heading(ui, rect, "Services");
    let body = egui::Rect::from_min_max(egui::pos2(rect.left(), rect.top() + 64.0), rect.max);

    let has_data = snap
        .services
        .as_ref()
        .map(|v| !v.is_empty())
        .unwrap_or(false);
    if super::state::render_if_needed(
        ui,
        body,
        snap,
        has_data,
        "No services registered",
        Some("Register one with `weft service register <name>`."),
    ) {
        return;
    }

    let rows = snap.services.as_ref().expect("has_data implies Some");
    let inset = body.shrink2(egui::vec2(24.0, 8.0));
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inset)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    paint_body(&mut child, desk, live, rows);
}

fn paint_body(ui: &mut egui::Ui, desk: &mut Desktop, live: &Arc<Live>, rows: &[Value]) {
    paint_tab_bar(ui, &mut desk.services_tab);
    ui.add_space(6.0);

    let tab = desk.services_tab;
    let filtered: Vec<&Value> = rows
        .iter()
        .filter(|r| tab.includes(field_state(r)))
        .collect();

    let count_lbl = format!("{} of {} services", filtered.len(), rows.len(),);
    ui.label(
        egui::RichText::new(count_lbl)
            .small()
            .color(Tokens::default().text_dim),
    );
    ui.add_space(4.0);

    egui::ScrollArea::vertical()
        .auto_shrink([false, true])
        .show(ui, |ui| {
            egui::Grid::new("weft_services_grid")
                .num_columns(6)
                .spacing([12.0, 4.0])
                .striped(true)
                .show(ui, |ui| {
                    paint_header_row(ui);
                    for (idx, row) in filtered.iter().enumerate() {
                        paint_row(ui, idx, row, live);
                    }
                });
        });
}

fn paint_tab_bar(ui: &mut egui::Ui, tab: &mut ServicesTab) {
    ui.horizontal(|ui| {
        for option in [ServicesTab::All, ServicesTab::Active, ServicesTab::Inactive] {
            let selected = *tab == option;
            if ui.selectable_label(selected, option.label()).clicked() {
                *tab = option;
            }
        }
    });
}

fn paint_header_row(ui: &mut egui::Ui) {
    let style = |s: &str| egui::RichText::new(s).small().weak();
    ui.label(style("name"));
    ui.label(style("state"));
    ui.label(style("pid"));
    ui.label(style("restarts"));
    ui.label(style("uptime"));
    ui.label(style(""));
    ui.end_row();
}

fn paint_row(ui: &mut egui::Ui, _idx: usize, row: &Value, live: &Arc<Live>) {
    let name = field_name(row);
    let state = field_state(row);
    let pid = field_pid(row);
    let restarts = field_restarts(row);
    let uptime = field_uptime(row);

    ui.monospace(name);
    ui.label(egui::RichText::new(state).color(state_color(state)));
    ui.monospace(pid);
    ui.monospace(restarts);
    ui.monospace(uptime);

    paint_action_affordances(ui, name, state, live);
    ui.end_row();
}

/// Render the row's contextual action buttons. Running rows expose
/// stop/restart; non-running rows expose start. Each click submits
/// the matching `service.*` verb directly — no inline-confirm step.
/// Failed RPCs stay visible because the daemon's next snapshot tick
/// is the source of truth on whether the action took.
fn paint_action_affordances(ui: &mut egui::Ui, name: &str, state: &str, live: &Arc<Live>) {
    if name == "-" {
        ui.label("");
        return;
    }
    let is_running = matches!(state, "running" | "active" | "ready");
    ui.horizontal(|ui| {
        if is_running {
            paint_action_btn(ui, name, "stop", live);
            paint_action_btn(ui, name, "restart", live);
        } else {
            paint_action_btn(ui, name, "start", live);
        }
    });
}

fn paint_action_btn(ui: &mut egui::Ui, name: &str, verb: &str, live: &Arc<Live>) {
    let label = format!("[{verb}]");
    let resp = ui
        .selectable_label(false, egui::RichText::new(label).small().monospace())
        .on_hover_text(format!("service.{verb} {{name: \"{name}\"}}"));
    if resp.clicked() {
        let _ = live.submit(Command::Raw {
            method: format!("service.{verb}"),
            params: json!({ "name": name }),
            reply: None,
        });
    }
}

fn field_name(row: &Value) -> &str {
    row.get("name").and_then(Value::as_str).unwrap_or("-")
}

fn field_state(row: &Value) -> &str {
    row.get("state")
        .and_then(Value::as_str)
        .or_else(|| row.get("status").and_then(Value::as_str))
        .unwrap_or("-")
}

fn field_pid(row: &Value) -> String {
    match row.get("pid") {
        Some(Value::Number(n)) if n.is_u64() => n.as_u64().unwrap().to_string(),
        Some(Value::Number(n)) if n.is_i64() => n.as_i64().unwrap().to_string(),
        Some(Value::String(s)) => s.clone(),
        _ => "-".to_string(),
    }
}

fn field_restarts(row: &Value) -> String {
    row.get("restarts")
        .and_then(Value::as_u64)
        .map(|n| n.to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn field_uptime(row: &Value) -> String {
    if let Some(ms) = row.get("uptime_ms").and_then(Value::as_u64) {
        return fmt_duration_ms(ms);
    }
    if let Some(s) = row.get("uptime_s").and_then(Value::as_u64) {
        return fmt_duration_ms(s.saturating_mul(1000));
    }
    if let Some(s) = row.get("uptime").and_then(Value::as_str) {
        return s.to_string();
    }
    "-".to_string()
}

fn fmt_duration_ms(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 86_400 {
        format!("{}d{}h", s / 86_400, (s % 86_400) / 3_600)
    } else if s >= 3_600 {
        format!("{}h{}m", s / 3_600, (s % 3_600) / 60)
    } else if s >= 60 {
        format!("{}m{}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}

fn state_color(state: &str) -> egui::Color32 {
    let t = Tokens::default();
    match state {
        "running" | "active" | "ready" => t.ok,
        "stopped" | "paused" | "idle" => t.text_dim,
        "failed" | "error" | "crashed" => t.crit,
        _ => t.text_secondary,
    }
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
    fn show_does_not_panic_with_service_rows() {
        let mut snap = Snapshot::default();
        snap.connection = Connection::Connected;
        snap.services = Some(vec![
            json!({
                "name": "weave",
                "state": "running",
                "pid": 1234,
                "restarts": 0,
                "uptime_ms": 1_234_567_u64,
            }),
            json!({
                "name": "whisper",
                "state": "stopped",
                "pid": null,
                "restarts": 2,
                "uptime_ms": 0,
            }),
        ]);
        run_show(snap);
    }

    #[test]
    fn show_paints_empty_state_when_connected_with_no_rows() {
        let mut snap = Snapshot::default();
        snap.connection = Connection::Connected;
        snap.services = Some(vec![]);
        run_show(snap);
    }

    #[test]
    fn services_tab_active_filter_includes_running_states() {
        assert!(ServicesTab::Active.includes("running"));
        assert!(ServicesTab::Active.includes("active"));
        assert!(ServicesTab::Active.includes("ready"));
        assert!(!ServicesTab::Active.includes("stopped"));
        assert!(!ServicesTab::Active.includes("failed"));
        assert!(!ServicesTab::Active.includes("-"));
    }

    #[test]
    fn services_tab_inactive_includes_unknown_states() {
        // Malformed-row safety: `Inactive` catches anything that isn't
        // running, so a half-populated row still surfaces somewhere.
        assert!(ServicesTab::Inactive.includes("stopped"));
        assert!(ServicesTab::Inactive.includes("failed"));
        assert!(ServicesTab::Inactive.includes("-"));
        assert!(ServicesTab::Inactive.includes(""));
        assert!(!ServicesTab::Inactive.includes("running"));
    }

    #[test]
    fn services_tab_all_includes_everything() {
        for s in ["running", "stopped", "failed", "-", ""] {
            assert!(ServicesTab::All.includes(s));
        }
    }

    #[test]
    fn fmt_duration_ms_thresholds() {
        assert_eq!(fmt_duration_ms(0), "0s");
        assert_eq!(fmt_duration_ms(999), "0s");
        assert_eq!(fmt_duration_ms(1_000), "1s");
        assert_eq!(fmt_duration_ms(60_000), "1m0s");
        assert_eq!(fmt_duration_ms(3_600_000), "1h0m");
        assert_eq!(fmt_duration_ms(86_400_000), "1d0h");
    }

    #[test]
    fn field_helpers_handle_missing_data() {
        let row = json!({});
        assert_eq!(field_name(&row), "-");
        assert_eq!(field_state(&row), "-");
        assert_eq!(field_pid(&row), "-");
        assert_eq!(field_restarts(&row), "-");
        assert_eq!(field_uptime(&row), "-");
    }

    #[test]
    fn field_helpers_handle_string_pid_and_status_fallback() {
        let row = json!({
            "name": "x",
            "status": "running",
            "pid": "n/a",
        });
        assert_eq!(field_state(&row), "running");
        assert_eq!(field_pid(&row), "n/a");
    }
}
