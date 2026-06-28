//! Scheduler — `cron.list` driven jobs table + add/remove. DESIGN.md
//! §9 sidebar 6, archetype `app-window` (table-over-plot). Graduated
//! under WEFT-584 and wired up post-graduation when the user surfaced
//! that "Add scheduled job" was a no-op.
//!
//! Data path: `Live` polls `cron.list` once a second on both native
//! and wasm and stores the array on `snap.cron_jobs`. The Scheduler
//! reads from there, submits `cron.add` / `cron.remove` via raw RPC
//! commands, and refreshes on the next tick.
//!
//! The plot region is reserved but unfilled — `substrate/scheduler/
//! run_history` is 0.9.x work. Today's table is the actionable surface.
//! WEFT-584 follow-up.

use std::sync::Arc;

use eframe::egui;
use serde_json::{Value, json};

use crate::live::{Command, Live, Snapshot};
use crate::shell::desktop::Desktop;
use crate::theming::Tokens;

const COLUMNS: &[(&str, f32)] = &[
    ("Job", 200.0),
    ("Every", 90.0),
    ("Command", 260.0),
    ("Last run", 160.0),
    ("Fires", 60.0),
    ("", 80.0),
];

const HEADER_H: f32 = 28.0;
const ROW_H: f32 = 24.0;
const TOOLBAR_H: f32 = 36.0;

/// Scheduler dialog + add-job form state. Persisted on `Desktop` so an
/// in-flight edit isn't clobbered by the next snapshot tick.
#[derive(Clone, Debug, Default)]
pub struct SchedulerState {
    /// Whether the add-job inline form is expanded.
    pub adding: bool,
    /// Form: job name (free text, must be non-empty).
    pub form_name: String,
    /// Form: interval in seconds (parsed from a string so the user can
    /// type freely; validated on submit).
    pub form_interval: String,
    /// Form: command for the daemon to fire.
    pub form_command: String,
    /// Last error from a failed submit, surfaced inline in the form.
    pub last_error: Option<String>,
}

impl SchedulerState {
    /// Reset the form fields to default — called after a successful
    /// add so the next click starts clean.
    fn reset_form(&mut self) {
        self.form_name.clear();
        self.form_interval = "60".to_string();
        self.form_command.clear();
        self.last_error = None;
    }
}

pub fn show(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    desk: &mut Desktop,
    live: &Arc<Live>,
    snap: &Snapshot,
) {
    super::paint_heading(ui, rect, "Scheduler");
    let body = egui::Rect::from_min_max(egui::pos2(rect.left(), rect.top() + 64.0), rect.max);

    let tokens = Tokens::default();
    let jobs = snap.cron_jobs.as_deref().unwrap_or(&[]);
    let has_data = !jobs.is_empty();

    // Carve the body: toolbar (top) + table region. The plot is
    // reserved for `substrate/scheduler/run_history` (0.9.x); not
    // drawn until that adapter ships so it doesn't read as a bug.
    let toolbar_rect =
        egui::Rect::from_min_max(body.min, egui::pos2(body.right(), body.top() + TOOLBAR_H));
    let table_rect = egui::Rect::from_min_max(
        egui::pos2(body.left(), toolbar_rect.bottom() + 6.0),
        body.max,
    );

    paint_toolbar(ui, toolbar_rect, desk, live, jobs.len());

    // Inline add-job form expands above the table when active. Carving
    // a strip from the table region keeps the layout self-contained
    // without modal overlay machinery.
    let table_rect = if desk.scheduler.adding {
        let form_h = 140.0;
        let form_rect = egui::Rect::from_min_max(
            table_rect.min,
            egui::pos2(table_rect.right(), table_rect.top() + form_h),
        );
        paint_add_form(ui, form_rect, desk, live);
        egui::Rect::from_min_max(
            egui::pos2(table_rect.left(), form_rect.bottom() + 8.0),
            table_rect.max,
        )
    } else {
        table_rect
    };

    paint_table_header(ui, table_rect, &tokens);

    if !has_data {
        let hint_rect = egui::Rect::from_min_max(
            egui::pos2(table_rect.left(), table_rect.top() + HEADER_H),
            egui::pos2(table_rect.right(), table_rect.bottom() - 6.0),
        );
        super::state::render_if_needed(
            ui,
            hint_rect,
            snap,
            false,
            "No scheduled jobs",
            Some("Click `+ Add job` above, or run `weft cron add` from the CLI."),
        );
        return;
    }

    paint_jobs_table(ui, table_rect, &tokens, jobs, live);
}

fn paint_toolbar(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    desk: &mut Desktop,
    _live: &Arc<Live>,
    job_count: usize,
) {
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(8.0, 4.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.label(
        egui::RichText::new(format!(
            "{job_count} job{}",
            if job_count == 1 { "" } else { "s" }
        ))
        .small()
        .monospace(),
    );
    child.add_space(12.0);
    let btn_label = if desk.scheduler.adding {
        "Cancel"
    } else {
        "+ Add job"
    };
    if child.button(btn_label).clicked() {
        if desk.scheduler.adding {
            desk.scheduler.adding = false;
            desk.scheduler.last_error = None;
        } else {
            desk.scheduler.reset_form();
            desk.scheduler.form_interval = "60".to_string();
            desk.scheduler.adding = true;
        }
    }
}

fn paint_add_form(ui: &mut egui::Ui, rect: egui::Rect, desk: &mut Desktop, live: &Arc<Live>) {
    let painter = ui.painter_at(rect);
    let tokens = Tokens::default();
    painter.rect_filled(rect, tokens.rounding, tokens.bg_panel);
    painter.rect_stroke(
        rect,
        egui::CornerRadius::same(tokens.rounding.round() as u8),
        egui::Stroke::new(1.0, tokens.stroke_soft),
        egui::epaint::StrokeKind::Inside,
    );

    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(12.0, 10.0)))
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.label(egui::RichText::new("Add scheduled job").strong());
    child.add_space(6.0);

    egui::Grid::new("scheduler_add_form")
        .num_columns(2)
        .spacing([8.0, 4.0])
        .show(&mut child, |ui| {
            ui.label("Name");
            ui.text_edit_singleline(&mut desk.scheduler.form_name);
            ui.end_row();
            ui.label("Every (s)");
            ui.add(
                egui::TextEdit::singleline(&mut desk.scheduler.form_interval).desired_width(80.0),
            );
            ui.end_row();
            ui.label("Command");
            ui.text_edit_singleline(&mut desk.scheduler.form_command);
            ui.end_row();
        });

    child.add_space(6.0);
    child.horizontal(|ui| {
        if ui.button("Add").clicked() {
            match build_cron_add_params(&desk.scheduler) {
                Ok(params) => {
                    live.submit(Command::Raw {
                        method: "cron.add".into(),
                        params,
                        reply: None,
                    });
                    desk.scheduler.adding = false;
                    desk.scheduler.reset_form();
                }
                Err(e) => {
                    desk.scheduler.last_error = Some(e);
                }
            }
        }
        if ui.button("Cancel").clicked() {
            desk.scheduler.adding = false;
            desk.scheduler.last_error = None;
        }
        if let Some(err) = &desk.scheduler.last_error {
            ui.add_space(8.0);
            ui.colored_label(tokens.crit, format!("error: {err}"));
        }
    });
}

/// Validate the form fields and assemble a `cron.add` params object.
/// Pulled out into a free fn so it's unit-testable without the egui
/// scaffolding.
fn build_cron_add_params(state: &SchedulerState) -> Result<Value, String> {
    let name = state.form_name.trim();
    if name.is_empty() {
        return Err("name is required".into());
    }
    let interval: u64 = state
        .form_interval
        .trim()
        .parse()
        .map_err(|_| "interval must be a positive integer".to_string())?;
    if interval == 0 {
        return Err("interval must be > 0".into());
    }
    let command = state.form_command.trim();
    if command.is_empty() {
        return Err("command is required".into());
    }
    Ok(json!({
        "name": name,
        "interval_secs": interval,
        "command": command,
        "target_pid": Value::Null,
    }))
}

fn paint_table_header(ui: &egui::Ui, rect: egui::Rect, tokens: &Tokens) {
    let painter = ui.painter_at(rect);
    let header =
        egui::Rect::from_min_max(rect.min, egui::pos2(rect.right(), rect.top() + HEADER_H));
    painter.rect_filled(header, 0.0, tokens.bg_panel);
    painter.line_segment(
        [
            egui::pos2(rect.left(), header.bottom()),
            egui::pos2(rect.right(), header.bottom()),
        ],
        egui::Stroke::new(1.0, tokens.stroke_soft),
    );
    let mut x = rect.left() + 16.0;
    for (label, w) in COLUMNS {
        painter.text(
            egui::pos2(x, header.center().y),
            egui::Align2::LEFT_CENTER,
            *label,
            egui::FontId::proportional(12.0),
            tokens.text_secondary,
        );
        x += w;
    }
    painter.rect_stroke(
        rect,
        egui::CornerRadius::same(4),
        egui::Stroke::new(1.0, tokens.stroke_hair),
        egui::epaint::StrokeKind::Inside,
    );
}

fn paint_jobs_table(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    tokens: &Tokens,
    jobs: &[Value],
    live: &Arc<Live>,
) {
    // Carve the row region below the header; scroll the rows so long
    // lists don't overflow the body rect.
    let rows_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() + HEADER_H + 1.0),
        rect.max,
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rows_rect.shrink2(egui::vec2(8.0, 4.0)))
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(&mut child, |ui| {
            for job in jobs {
                paint_job_row(ui, tokens, job, live);
            }
        });
}

fn paint_job_row(ui: &mut egui::Ui, tokens: &Tokens, job: &Value, live: &Arc<Live>) {
    let name = job.get("name").and_then(|v| v.as_str()).unwrap_or("?");
    let interval = job
        .get("interval_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let command = job.get("command").and_then(|v| v.as_str()).unwrap_or("");
    let last_fired = job
        .get("last_fired")
        .and_then(|v| v.as_str())
        .unwrap_or("—");
    let fires = job.get("fire_count").and_then(|v| v.as_u64()).unwrap_or(0);
    let id = job.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let enabled = job.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);

    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), ROW_H),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            // Row striping using the panel-soft hairline so rows don't
            // bleed into each other on dense lists.
            let row_color = if enabled {
                tokens.text_primary
            } else {
                tokens.text_dim
            };
            ui.add_sized(
                egui::vec2(COLUMNS[0].1, ROW_H),
                egui::Label::new(egui::RichText::new(name).color(row_color)),
            );
            ui.add_sized(
                egui::vec2(COLUMNS[1].1, ROW_H),
                egui::Label::new(egui::RichText::new(format!("{interval}s")).monospace()),
            );
            ui.add_sized(
                egui::vec2(COLUMNS[2].1, ROW_H),
                egui::Label::new(egui::RichText::new(command).monospace().small()),
            );
            ui.add_sized(
                egui::vec2(COLUMNS[3].1, ROW_H),
                egui::Label::new(egui::RichText::new(last_fired).small()),
            );
            ui.add_sized(
                egui::vec2(COLUMNS[4].1, ROW_H),
                egui::Label::new(egui::RichText::new(fires.to_string()).monospace()),
            );
            // Last column: remove button. Disabled when we can't
            // identify the job.
            ui.add_enabled_ui(!id.is_empty(), |ui| {
                if ui
                    .add_sized(
                        egui::vec2(COLUMNS[5].1 - 8.0, ROW_H - 6.0),
                        egui::Button::new("remove"),
                    )
                    .clicked()
                {
                    live.submit(Command::Raw {
                        method: "cron.remove".into(),
                        params: json!({ "id": id }),
                        reply: None,
                    });
                }
            });
        },
    );
    ui.separator();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::Connection;

    #[test]
    fn renders_default_desktop_without_panic() {
        let ctx = egui::Context::default();
        let live = Live::spawn();
        let mut desk = Desktop::default();
        let mut snap = Snapshot::default();
        snap.connection = Connection::Connected;
        ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let rect = ui.max_rect();
                show(ui, rect, &mut desk, &live, &snap);
            });
        });
    }

    #[test]
    fn renders_with_one_job_without_panic() {
        let ctx = egui::Context::default();
        let live = Live::spawn();
        let mut desk = Desktop::default();
        let mut snap = Snapshot::default();
        snap.connection = Connection::Connected;
        snap.cron_jobs = Some(vec![json!({
            "id": "job-1",
            "name": "heartbeat",
            "interval_secs": 30,
            "command": "echo hi",
            "fire_count": 12,
            "last_fired": "2026-05-02T18:00:00Z",
            "enabled": true,
        })]);
        ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let rect = ui.max_rect();
                show(ui, rect, &mut desk, &live, &snap);
            });
        });
    }

    #[test]
    fn build_cron_add_params_validates_required_fields() {
        let mut s = SchedulerState::default();
        assert!(build_cron_add_params(&s).is_err(), "blank name should fail");
        s.form_name = "ping".into();
        s.form_interval = "0".into();
        s.form_command = "echo".into();
        assert!(build_cron_add_params(&s).is_err(), "interval=0 should fail");
        s.form_interval = "abc".into();
        assert!(build_cron_add_params(&s).is_err(), "non-numeric interval");
        s.form_interval = "60".into();
        s.form_command = "".into();
        assert!(
            build_cron_add_params(&s).is_err(),
            "blank command should fail"
        );
        s.form_command = "echo hi".into();
        let params = build_cron_add_params(&s).expect("valid form should pass");
        assert_eq!(params.get("name").and_then(|v| v.as_str()), Some("ping"));
        assert_eq!(
            params.get("interval_secs").and_then(|v| v.as_u64()),
            Some(60)
        );
        assert_eq!(
            params.get("command").and_then(|v| v.as_str()),
            Some("echo hi")
        );
    }

    #[test]
    fn reset_form_clears_user_input() {
        let mut s = SchedulerState {
            adding: true,
            form_name: "x".into(),
            form_interval: "300".into(),
            form_command: "y".into(),
            last_error: Some("e".into()),
        };
        s.reset_form();
        assert!(s.form_name.is_empty());
        assert!(s.form_command.is_empty());
        assert!(s.last_error.is_none());
        // adding stays — caller toggles it explicitly.
    }
}
