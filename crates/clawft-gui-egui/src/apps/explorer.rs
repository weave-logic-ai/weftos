//! Explorer — first-class sidebar app (WEFT-590). Substrate tree
//! browser graduated from `crates/clawft-gui-egui/src/explorer/mod.rs`.
//! DESIGN.md §9 sidebar 12.
//!
//! All the real work — left-tree navigation, right-detail viewer
//! cascade, viewer dispatch, `substrate.list` / `substrate.read`
//! polling, activity tracking, copy-actions — lives in
//! `crate::explorer::Explorer`, owned by the `Desktop`
//! (`desk.explorer`). This module paints the canonical heading and
//! delegates the body to `desktop::render_explorer`, the helper that
//! puts a connection pill above the two-pane Explorer layout.
//!
//! Lifecycle: subscription cleanup on nav-AWAY is handled centrally
//! by [`crate::apps::dispatch`], which compares the previous active
//! sidebar target against the current one and calls
//! `desk.explorer.close(live)` on the leave-Explorer transition. See
//! `apps::dispatch` for the rationale (single hygiene point covering
//! every app that needs it; Terminal/Chat sidebar apps intentionally
//! don't close on nav-away — the user expects to come back to a
//! running session).

use std::sync::Arc;

use eframe::egui;

use crate::live::{Live, Snapshot};
use crate::shell::desktop::{self, Desktop};

const HEADING_BAND_H: f32 = 64.0;

pub fn show(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    desk: &mut Desktop,
    live: &Arc<Live>,
    snap: &Snapshot,
) {
    super::paint_heading(ui, rect, "Explorer · substrate/");
    let body = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() + HEADING_BAND_H),
        rect.max,
    );
    // Confine the Explorer's two-pane layout to the body rect.
    // `desktop::render_explorer` paints a connection pill above the
    // left-tree / right-detail split.
    ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
        desktop::render_explorer(ui, desk, live, snap);
    });
}
