//! Empty / loading / offline state renderer — DESIGN.md §5.
//!
//! Every WeftOS app calls one of these helpers when its bound substrate
//! paths produce no data. The three states are visually distinct so the
//! user always knows *why* the panel is empty:
//!
//! - **Loading**: italic dim text, no spinner.
//! - **Empty**: italic dim text describing what would appear, plus an
//!   optional remediation pressable.
//! - **Offline**: tone=`crit` chip with monospace remediation hint.
//!
//! Color and glyph in lockstep. Surface lift only — no chromatic
//! emphasis other than the `crit` tone on the offline chip.

use eframe::egui;

use crate::live::{Connection, Snapshot};
use crate::theming::Tokens;

/// Pick the right state to render. Returns `true` if a non-data state
/// was painted (i.e. the caller should bail out of its body render).
pub fn render_if_needed(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    snap: &Snapshot,
    has_data: bool,
    what: &str,
    remediation: Option<&str>,
) -> bool {
    let tokens = Tokens::default();
    match snap.connection {
        Connection::Disconnected => {
            render_offline(ui, rect, &tokens, remediation);
            true
        }
        Connection::Connecting => {
            render_loading(ui, rect, &tokens, what);
            true
        }
        Connection::Connected if !has_data => {
            render_empty(ui, rect, &tokens, what, remediation);
            true
        }
        _ => false,
    }
}

/// Render the offline state — daemon link is `Disconnected`.
pub fn render_offline(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    tokens: &Tokens,
    remediation: Option<&str>,
) {
    let painter = ui.painter_at(rect);
    let cy = rect.center().y - 12.0;

    // Crit chip
    let chip_w = 280.0;
    let chip_h = 28.0;
    let chip_rect =
        egui::Rect::from_center_size(egui::pos2(rect.center().x, cy), egui::vec2(chip_w, chip_h));
    painter.rect_stroke(
        chip_rect,
        egui::CornerRadius::same(4),
        egui::Stroke::new(1.0, tokens.crit),
        egui::epaint::StrokeKind::Inside,
    );
    painter.circle_filled(
        egui::pos2(chip_rect.left() + 14.0, chip_rect.center().y),
        4.0,
        tokens.crit,
    );
    painter.text(
        egui::pos2(chip_rect.left() + 26.0, chip_rect.center().y),
        egui::Align2::LEFT_CENTER,
        "Demo mode — kernel daemon offline",
        egui::FontId::proportional(12.0),
        tokens.crit,
    );

    if let Some(hint) = remediation {
        painter.text(
            egui::pos2(rect.center().x, cy + 30.0),
            egui::Align2::CENTER_TOP,
            hint,
            egui::FontId::monospace(12.0),
            tokens.text_dim,
        );
    } else {
        painter.text(
            egui::pos2(rect.center().x, cy + 30.0),
            egui::Align2::CENTER_TOP,
            "Start with: weaver kernel start",
            egui::FontId::monospace(12.0),
            tokens.text_dim,
        );
    }
}

/// Render the loading state — first poll in flight.
pub fn render_loading(ui: &mut egui::Ui, rect: egui::Rect, tokens: &Tokens, what: &str) {
    let painter = ui.painter_at(rect);
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        format!("Waiting for {what}…"),
        egui::FontId::proportional(13.0),
        tokens.text_dim,
    );
}

/// Render the empty state — connected but bound paths return null.
pub fn render_empty(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    tokens: &Tokens,
    what: &str,
    remediation: Option<&str>,
) {
    let painter = ui.painter_at(rect);
    let cy = rect.center().y - 8.0;

    painter.text(
        egui::pos2(rect.center().x, cy),
        egui::Align2::CENTER_CENTER,
        what,
        egui::FontId::proportional(13.0),
        tokens.text_dim,
    );
    if let Some(hint) = remediation {
        painter.text(
            egui::pos2(rect.center().x, cy + 22.0),
            egui::Align2::CENTER_CENTER,
            hint,
            egui::FontId::monospace(12.0),
            tokens.text_dim,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_if_needed_short_circuits_on_disconnected() {
        let mut snap = Snapshot::default();
        snap.connection = Connection::Disconnected;
        // Sanity: the helper signals "rendered" without panicking.
        // Actual painting is exercised in the snapshot tests added
        // alongside the per-app modules in Phase 3.
        let ctx = egui::Context::default();
        ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let rect = ui.max_rect();
                let painted = render_if_needed(ui, rect, &snap, false, "anything", None);
                assert!(painted);
            });
        });
    }

    #[test]
    fn render_if_needed_short_circuits_on_connecting() {
        let mut snap = Snapshot::default();
        snap.connection = Connection::Connecting;
        let ctx = egui::Context::default();
        ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let rect = ui.max_rect();
                let painted = render_if_needed(ui, rect, &snap, false, "data", None);
                assert!(painted);
            });
        });
    }

    #[test]
    fn render_if_needed_paints_empty_when_connected_no_data() {
        let mut snap = Snapshot::default();
        snap.connection = Connection::Connected;
        let ctx = egui::Context::default();
        ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let rect = ui.max_rect();
                let painted =
                    render_if_needed(ui, rect, &snap, false, "files", Some("install fs adapter"));
                assert!(painted);
            });
        });
    }

    #[test]
    fn render_if_needed_returns_false_when_data_present() {
        let mut snap = Snapshot::default();
        snap.connection = Connection::Connected;
        let ctx = egui::Context::default();
        ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let rect = ui.max_rect();
                let painted = render_if_needed(ui, rect, &snap, true, "files", None);
                assert!(!painted);
            });
        });
    }
}
