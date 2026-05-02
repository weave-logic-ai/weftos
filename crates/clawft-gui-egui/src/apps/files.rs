//! Files — substrate browser. DESIGN.md §9 sidebar 1, archetype
//! `app-window` (DESIGN.md §4.1 list-detail). WEFT-579.
//!
//! There is no native filesystem adapter shipping in 0.7.0 — that's
//! intentional, the app is a placeholder for the substrate filesystem
//! mount point. The graduation here renders the *list-detail archetype*
//! so the user sees the eventual shape, even without an adapter:
//!
//! - top toolbar: Up / Refresh / View ▾ (no-ops in 0.7.0)
//! - left pane: tree of paths (placeholder root only — adapter not
//!   installed)
//! - right pane: detail view for the selected node, OR the empty-state
//!   helper from `apps::state` when there's no data
//!
//! `snap.fs` doesn't exist on [`Snapshot`] today. The layout still
//! renders so that the moment the adapter ships, only the data-source
//! lookup needs to change.

use std::sync::Arc;

use eframe::egui;

use crate::live::{Command, Live, Snapshot};
use crate::shell::desktop::Desktop;
use crate::theming::Tokens;

const TOOLBAR_H: f32 = 32.0;
const LEFT_PANE_W: f32 = 220.0;
const HEADER_H: f32 = 64.0;

pub fn show(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    _desk: &mut Desktop,
    live: &Arc<Live>,
    snap: &Snapshot,
) {
    super::paint_heading(ui, rect, "Files");

    let body = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() + HEADER_H),
        rect.max,
    );
    let inset = body.shrink2(egui::vec2(24.0, 8.0));

    // Toolbar — empty buttons in 0.7.0. They submit no-op `Command::Raw`
    // calls so the wire-up is real even though the adapter isn't.
    let toolbar_rect = egui::Rect::from_min_max(
        inset.min,
        egui::pos2(inset.right(), inset.top() + TOOLBAR_H),
    );
    paint_toolbar(ui, toolbar_rect, live);

    // List-detail panes below the toolbar.
    let panes_top = toolbar_rect.bottom() + 8.0;
    if panes_top >= inset.bottom() {
        return; // safety: panel too short to host the panes
    }
    let left_rect = egui::Rect::from_min_max(
        egui::pos2(inset.left(), panes_top),
        egui::pos2(inset.left() + LEFT_PANE_W, inset.bottom()),
    );
    let right_rect = egui::Rect::from_min_max(
        egui::pos2(left_rect.right() + 8.0, panes_top),
        egui::pos2(inset.right(), inset.bottom()),
    );

    paint_left_pane(ui, left_rect);
    paint_right_pane(ui, right_rect, snap);
}

fn paint_toolbar(ui: &mut egui::Ui, rect: egui::Rect, live: &Arc<Live>) {
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    // Empty-but-real buttons: the verb is a "files.*" no-op so the
    // RPC bridge has something to drop on the floor today, and the
    // moment the adapter ships these become live with no UI change.
    if child.button("Up").clicked() {
        let _ = live.submit(Command::Raw {
            method: "files.noop".to_string(),
            params: serde_json::json!({ "action": "up" }),
            reply: None,
        });
    }
    if child.button("Refresh").clicked() {
        let _ = live.submit(Command::Raw {
            method: "files.noop".to_string(),
            params: serde_json::json!({ "action": "refresh" }),
            reply: None,
        });
    }
    // egui has no built-in dropdown; a `menu_button` is the
    // canonical idiom and matches the toolbar's visual weight.
    child.menu_button("View ▾", |ui| {
        ui.label(
            egui::RichText::new("List | Tree | Details")
                .small()
                .color(Tokens::default().text_dim),
        );
    });
}

fn paint_left_pane(ui: &mut egui::Ui, rect: egui::Rect) {
    let tokens = Tokens::default();
    let painter = ui.painter_at(rect);
    // Pane fill — surface lift only, no chromatic emphasis.
    painter.rect_filled(rect, egui::CornerRadius::same(4), tokens.bg_surface);
    painter.rect_stroke(
        rect,
        egui::CornerRadius::same(4),
        egui::Stroke::new(1.0, tokens.stroke_hair),
        egui::epaint::StrokeKind::Inside,
    );

    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(8.0, 8.0)))
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.label(
        egui::RichText::new("PATHS")
            .small()
            .color(tokens.text_dim),
    );
    child.add_space(4.0);
    // Single placeholder root so the tree archetype is recognisable.
    // No expand chevron — there's nothing under it until the adapter
    // ships.
    let _ = child.selectable_label(false, egui::RichText::new("◇  /").monospace());
}

fn paint_right_pane(ui: &mut egui::Ui, rect: egui::Rect, snap: &Snapshot) {
    let tokens = Tokens::default();
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, egui::CornerRadius::same(4), tokens.bg_surface);
    painter.rect_stroke(
        rect,
        egui::CornerRadius::same(4),
        egui::Stroke::new(1.0, tokens.stroke_hair),
        egui::epaint::StrokeKind::Inside,
    );

    let inner = rect.shrink2(egui::vec2(8.0, 8.0));
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    // `snap.fs` doesn't exist yet — `has_data` is permanently false
    // until the adapter ships. The empty-state helper handles the
    // connection-state matrix (offline / connecting / connected-no-data).
    super::state::render_if_needed(
        &mut child,
        inner,
        snap,
        false,
        "No filesystem adapter installed",
        Some("Install one with `weft adapter install fs`."),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::Connection;

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
    fn show_does_not_panic_when_connected() {
        let mut snap = Snapshot::default();
        snap.connection = Connection::Connected;
        run_show(snap);
    }

    #[test]
    fn show_does_not_panic_when_disconnected() {
        let mut snap = Snapshot::default();
        snap.connection = Connection::Disconnected;
        run_show(snap);
    }
}
