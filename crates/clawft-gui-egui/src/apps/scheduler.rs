//! Scheduler — `substrate/scheduler/*`. DESIGN.md §9 sidebar 6,
//! archetype `app-window` (table-over-plot). Graduated under
//! WEFT-584.
//!
//! 0.7.0 acceptance: the scheduler kernel adapter itself is 0.9.x
//! work — `snap.scheduler` does not exist on the Snapshot type and
//! no adapter publishes `substrate/scheduler/*`. This graduation
//! ships the **shell** of the app (header, table column headers,
//! plot region with axes) so the archetype shape is visible and
//! the empty-state hint points the user at the right CLI command.
//!
//! When the adapter ships, replace `has_data` with a real check
//! against the substrate snapshot and fill in `paint_jobs_table`
//! and `paint_plot_region` with bound rows / series.

use std::sync::Arc;

use eframe::egui;

use crate::live::{Live, Snapshot};
use crate::shell::desktop::Desktop;
use crate::theming::Tokens;

const COLUMNS: &[(&str, f32)] = &[
    ("Job ID", 200.0),
    ("Schedule", 160.0),
    ("Last run", 160.0),
    ("Next run", 160.0),
    ("Status", 100.0),
];

const HEADER_H: f32 = 28.0;
const TABLE_BOTTOM_GAP: f32 = 12.0;

pub fn show(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    _desk: &mut Desktop,
    live: &Arc<Live>,
    snap: &Snapshot,
) {
    super::paint_heading(ui, rect, "Scheduler");
    let body = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() + 64.0),
        rect.max,
    );

    // Probe the substrate for forward-compat: the moment a scheduler
    // adapter starts publishing `substrate/scheduler/jobs` we'll show
    // it. 0.7.0: this is always None.
    let snapshot = live.substrate_snapshot();
    let jobs = snapshot.read("substrate/scheduler/jobs");
    let has_data = jobs.is_some();

    let tokens = Tokens::default();

    // Always paint the archetype shape — table header + plot axes —
    // so the user sees what the form will look like once data lands.
    // The table body and plot series are the only things hidden when
    // empty.
    let (table_rect, plot_rect) = split_body(body);
    paint_table_header(ui, table_rect, &tokens);
    paint_plot_axes(ui, plot_rect, &tokens);

    if !has_data {
        // Drop the empty-state hint *centered in the table region* so
        // it sits where the rows would appear, not floating between
        // the table and the plot.
        let hint_rect = egui::Rect::from_min_max(
            egui::pos2(table_rect.left(), table_rect.top() + HEADER_H),
            egui::pos2(table_rect.right(), table_rect.bottom() - TABLE_BOTTOM_GAP),
        );
        super::state::render_if_needed(
            ui,
            hint_rect,
            snap,
            false,
            "No scheduled jobs",
            Some("Add jobs with `weft schedule add` once the adapter ships."),
        );
        return;
    }

    // Forward-compat: real rendering. Currently unreachable in 0.7.0
    // because no adapter publishes scheduler topics.
    paint_jobs_table(ui, table_rect, &tokens, jobs.as_ref());
    paint_plot_region(ui, plot_rect, &tokens);
}

/// Split the body into (table region, plot region). The table claims
/// the upper half, the plot the lower; a 12 px gap separates them.
fn split_body(body: egui::Rect) -> (egui::Rect, egui::Rect) {
    let split_y = body.top() + body.height() * 0.55;
    let table = egui::Rect::from_min_max(
        body.min,
        egui::pos2(body.right(), split_y - 6.0),
    );
    let plot = egui::Rect::from_min_max(
        egui::pos2(body.left(), split_y + 6.0),
        body.max,
    );
    (table, plot)
}

fn paint_table_header(ui: &egui::Ui, rect: egui::Rect, tokens: &Tokens) {
    let painter = ui.painter_at(rect);
    // Header row background.
    let header = egui::Rect::from_min_max(
        rect.min,
        egui::pos2(rect.right(), rect.top() + HEADER_H),
    );
    painter.rect_filled(header, 0.0, tokens.bg_panel);
    // Hairline under the header.
    painter.line_segment(
        [
            egui::pos2(rect.left(), header.bottom()),
            egui::pos2(rect.right(), header.bottom()),
        ],
        egui::Stroke::new(1.0, tokens.stroke_soft),
    );

    // Column labels.
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
    // Outer table stroke.
    painter.rect_stroke(
        rect,
        egui::CornerRadius::same(4),
        egui::Stroke::new(1.0, tokens.stroke_hair),
        egui::epaint::StrokeKind::Inside,
    );
}

fn paint_plot_axes(ui: &egui::Ui, rect: egui::Rect, tokens: &Tokens) {
    let painter = ui.painter_at(rect);
    // Frame.
    painter.rect_stroke(
        rect,
        egui::CornerRadius::same(4),
        egui::Stroke::new(1.0, tokens.stroke_hair),
        egui::epaint::StrokeKind::Inside,
    );
    // Plot region with 32 px margin for axes.
    let plot_margin = 32.0;
    let plot_x = rect.left() + plot_margin;
    let plot_y = rect.bottom() - plot_margin;

    // Y-axis.
    painter.line_segment(
        [
            egui::pos2(plot_x, rect.top() + 12.0),
            egui::pos2(plot_x, plot_y),
        ],
        egui::Stroke::new(1.0, tokens.stroke_soft),
    );
    // X-axis.
    painter.line_segment(
        [
            egui::pos2(plot_x, plot_y),
            egui::pos2(rect.right() - 12.0, plot_y),
        ],
        egui::Stroke::new(1.0, tokens.stroke_soft),
    );
    // Axis labels.
    painter.text(
        egui::pos2(plot_x - 6.0, rect.top() + 12.0),
        egui::Align2::RIGHT_TOP,
        "runs",
        egui::FontId::proportional(11.0),
        tokens.text_dim,
    );
    painter.text(
        egui::pos2(rect.right() - 12.0, plot_y + 4.0),
        egui::Align2::RIGHT_TOP,
        "time",
        egui::FontId::proportional(11.0),
        tokens.text_dim,
    );
}

#[allow(dead_code)] // unreachable in 0.7.0 — wired up when the scheduler adapter ships
fn paint_jobs_table(
    _ui: &egui::Ui,
    _rect: egui::Rect,
    _tokens: &Tokens,
    _jobs: Option<&serde_json::Value>,
) {
    // Forward-compat scaffold. Iterates `_jobs` (a JSON array) and
    // paints one row per job using `COLUMNS` widths.
}

#[allow(dead_code)] // unreachable in 0.7.0 — wired up when the scheduler adapter ships
fn paint_plot_region(_ui: &egui::Ui, _rect: egui::Rect, _tokens: &Tokens) {
    // Forward-compat scaffold. Real implementation will read
    // `substrate/scheduler/run_history` and paint a per-job stripe
    // chart inside the axes.
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
    fn split_body_carves_table_then_plot() {
        let body = egui::Rect::from_min_max(
            egui::pos2(0.0, 0.0),
            egui::pos2(800.0, 600.0),
        );
        let (t, p) = split_body(body);
        assert!(t.bottom() < p.top(), "table sits above plot, with a gap");
        assert!(t.height() > 100.0, "table region has usable height");
        assert!(p.height() > 100.0, "plot region has usable height");
    }
}
