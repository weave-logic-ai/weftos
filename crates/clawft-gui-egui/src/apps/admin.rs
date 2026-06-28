//! Admin — the surface-composer-driven reference panel. DESIGN.md §9
//! sidebar 11, archetype `app-window` (DESIGN.md §4.1).
//!
//! WEFT-589 graduation: lifts what used to live in
//! `desktop::render_selected_app` (driven from the retired floating
//! Blocks · Apps tab) into the canonical sidebar app slot. The body
//! still composes against the cached `app_surfaces` SurfaceTree built
//! at boot from `weftos-admin-desktop.toml`, draining any pending
//! affordance dispatches through the live RPC bridge each frame —
//! that closes the loop "user clicks a primitive → daemon handler
//! fires" without any extra plumbing.

use std::sync::Arc;

use eframe::egui;

use crate::live::{Live, Snapshot};
use crate::shell::desktop::{self, Desktop};

/// Root entry — see [`crate::apps::dispatch`]. The composer body fills
/// the rect below the heading; the affordance loop posts each pending
/// dispatch through `live.submit` so daemon handlers see them on the
/// next tick.
pub fn show(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    desk: &mut Desktop,
    live: &Arc<Live>,
    snap: &Snapshot,
) {
    super::paint_heading(ui, rect, "WeftOS Admin");

    // Body fills the rect below the heading band (paint_heading uses
    // 24px top padding + ~18px line height; the apps stubs all reserve
    // 64px for the heading so leave that here too — keeps the body
    // baseline consistent across apps).
    let body = egui::Rect::from_min_max(egui::pos2(rect.left(), rect.top() + 64.0), rect.max);

    // Render into a child Ui scoped to the body rect so the composer's
    // grid honours the panel bounds (otherwise it expands into the
    // heading band on the first frame). `scope_builder` finalises the
    // child's `min_rect` for the next-frame hit-test pass — without
    // it the composer's pressables wouldn't register clicks.
    ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
        desktop::render_selected_app(ui, desk, live, snap);
    });
}
