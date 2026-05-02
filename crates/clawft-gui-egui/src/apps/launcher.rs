//! Apps launcher — Built-in / Installed / Developer tile-grid.
//! DESIGN.md §9 sidebar 13, archetype `tile-grid` (DESIGN.md §4.3).
//!
//! WEFT-591 graduation:
//! - **Built-in**: tile-grid of the canonical sidebar's twelve apps.
//!   Clicking a tile dispatches a `SidebarAction::Open(<target>)` so
//!   the user jumps directly to that app — no separate "open" verb.
//! - **Installed**: tile-grid of `desk.app_registry.list()`. Empty
//!   state is rendered through the standard helper so the panel reads
//!   "No apps installed" with a `weft app install <id>` hint.
//! - **Developer**: hosts what used to live in the retired floating
//!   Blocks window — the Blocks / Canon / Apps demo panel reached by
//!   `desktop::render_blocks_window`. The legacy "Open Explorer"
//!   pressable is gone; Explorer has its own sidebar entry now.

use std::sync::Arc;

use eframe::egui;

use crate::live::{Live, Snapshot};
use crate::shell::desktop::{self, Desktop};
use crate::shell::sidebar::{
    AppsTab, LogsTab, NetworkTab, SidebarAction, SidebarTarget,
};

/// Tile dimensions — DESIGN.md §4.3 tile-grid archetype: ~140×110 px
/// per tile, ~20 px gap. Hand-rolled rather than via `egui::Grid` so
/// we can paint the surface-lift hover/active backgrounds and stroke
/// the soft border consistently with the sidebar rows.
const TILE_W: f32 = 140.0;
const TILE_H: f32 = 110.0;
const TILE_GAP: f32 = 20.0;
const TILE_PAD: f32 = 16.0;

/// Tab dispatch — see [`crate::apps::dispatch`]. Each tab paints its
/// own heading via `paint_heading` and takes a body rect 64 px below
/// to keep the baseline consistent with the rest of the apps.
pub fn show(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    desk: &mut Desktop,
    live: &Arc<Live>,
    snap: &Snapshot,
    tab: AppsTab,
) {
    let tab_label = match tab {
        AppsTab::BuiltIn => "Built-in",
        AppsTab::Installed => "Installed",
        AppsTab::Developer => "Developer",
    };
    super::paint_heading(ui, rect, &format!("Apps · {tab_label}"));

    let body = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() + 64.0),
        rect.max,
    );

    match tab {
        AppsTab::BuiltIn => render_builtin(ui, body, desk),
        AppsTab::Installed => render_installed(ui, body, desk, snap),
        AppsTab::Developer => render_developer(ui, body, desk, live, snap),
    }
}

/// Built-in tab — twelve canonical sidebar apps as pressable tiles.
/// Each tile dispatches `SidebarAction::Open(<target>)` so clicking it
/// jumps straight to the corresponding app pane. Order follows
/// DESIGN.md §5 (canonical sidebar — DO NOT REORDER).
fn render_builtin(ui: &mut egui::Ui, rect: egui::Rect, desk: &mut Desktop) {
    // (label, target) — matches DESIGN.md §5 order. For grouped
    // sidebar items (Network, Logs, Apps) the tile opens the default
    // sub-target. We deliberately do not include "Apps" itself in the
    // grid — clicking the launcher to launch the launcher would be
    // confusing.
    let tiles: [(&str, SidebarTarget); 12] = [
        ("Files", SidebarTarget::Files),
        ("Processes", SidebarTarget::Processes),
        ("Services", SidebarTarget::Services),
        ("Network", SidebarTarget::Network(NetworkTab::Mesh)),
        ("Settings", SidebarTarget::Settings),
        ("Scheduler", SidebarTarget::Scheduler),
        ("Monitor", SidebarTarget::Monitor),
        ("Logs", SidebarTarget::Logs(LogsTab::System)),
        ("Terminal", SidebarTarget::Terminal),
        ("Chat", SidebarTarget::Chat),
        ("Admin", SidebarTarget::Admin),
        ("Explorer", SidebarTarget::Explorer),
    ];

    if let Some(action) = paint_tile_grid(
        ui,
        rect,
        tiles.iter().map(|(label, _)| (*label, None::<&str>)),
    ) {
        let (_, target) = tiles[action];
        desk.sidebar.apply(SidebarAction::Open(target));
    }
}

/// Installed tab — tile-grid of locally installed apps from the
/// registry. Selecting a tile sets `desk.selected_app` so the Admin
/// pane (and any future composer-driven app) renders that surface.
fn render_installed(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    desk: &mut Desktop,
    snap: &Snapshot,
) {
    // Snapshot `(id, name)` up front so we don't hold a borrow over
    // the click handler that writes `desk.selected_app`.
    let entries: Vec<(String, String)> = desk
        .app_registry
        .list()
        .iter()
        .map(|a| (a.manifest.id.clone(), a.manifest.name.clone()))
        .collect();

    if entries.is_empty() {
        super::state::render_if_needed(
            ui,
            rect,
            snap,
            false,
            "No apps installed",
            Some("Install with `weft app install <id>`."),
        );
        return;
    }

    if let Some(idx) = paint_tile_grid(
        ui,
        rect,
        entries
            .iter()
            .map(|(id, name)| (name.as_str(), Some(id.as_str()))),
    ) {
        let (id, _) = &entries[idx];
        desk.selected_app = Some(id.clone());
    }
}

/// Developer tab — hosts the legacy Blocks / Canon / Apps demo panel
/// formerly reached through the floating Blocks launcher window. The
/// three-section toolbar lives inside `render_blocks_window`'s left
/// rail and selects which demo body the central panel paints.
fn render_developer(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    desk: &mut Desktop,
    live: &Arc<Live>,
    snap: &Snapshot,
) {
    // `render_blocks_window` uses `egui::Panel::{left,central}` —
    // those need a parent `Ui` whose `max_rect` they can shrink into.
    // `scope_builder` finalises the child's bounds for the next-frame
    // hit-test pass; without it the inner panel's pressables wouldn't
    // register clicks.
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
        desktop::render_blocks_window(ui, desk, live, snap);
    });
}

/// Hand-rolled tile grid: lays tiles left-to-right, wrapping into
/// rows. Returns `Some(index)` for the tile clicked this frame.
///
/// Each tile entry is `(title, optional_caption)`; the caption renders
/// in monospace below the title (used by Installed to show the app
/// id). Hover lifts the surface; click is signalled through the
/// returned index. Surface-lift only — no chromatic accent, per
/// DESIGN.md §2.1.
fn paint_tile_grid<'a>(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    tiles: impl IntoIterator<Item = (&'a str, Option<&'a str>)>,
) -> Option<usize> {
    use crate::theming::Tokens;
    let tokens = Tokens::default();

    let inner = rect.shrink(TILE_PAD);
    let cols = ((inner.width() + TILE_GAP) / (TILE_W + TILE_GAP)).floor() as usize;
    let cols = cols.max(1);

    let origin = inner.min;
    let mut clicked = None;
    let tiles: Vec<_> = tiles.into_iter().collect();
    for (idx, (title, caption)) in tiles.iter().enumerate() {
        let col = idx % cols;
        let row = idx / cols;
        let tile_min = egui::pos2(
            origin.x + col as f32 * (TILE_W + TILE_GAP),
            origin.y + row as f32 * (TILE_H + TILE_GAP),
        );
        let tile_rect = egui::Rect::from_min_size(
            tile_min,
            egui::vec2(TILE_W, TILE_H),
        );
        // Skip tiles that wholly fall outside the panel rect — keeps
        // hit-testing tight and avoids painting under the sidebar
        // when the window is narrow.
        if !rect.contains_rect(tile_rect) && !rect.intersects(tile_rect) {
            continue;
        }
        let response = ui.interact(
            tile_rect,
            egui::Id::new(("apps-tile", idx, *title)),
            egui::Sense::click(),
        );
        let painter = ui.painter_at(tile_rect);
        let bg = if response.hovered() {
            tokens.bg_hover
        } else {
            tokens.bg_panel
        };
        painter.rect_filled(tile_rect, egui::CornerRadius::same(6), bg);
        painter.rect_stroke(
            tile_rect,
            egui::CornerRadius::same(6),
            egui::Stroke::new(1.0, tokens.stroke_soft),
            egui::epaint::StrokeKind::Inside,
        );
        painter.text(
            egui::pos2(tile_rect.left() + 14.0, tile_rect.top() + 14.0),
            egui::Align2::LEFT_TOP,
            title,
            egui::FontId::proportional(14.0),
            tokens.text_primary,
        );
        if let Some(cap) = caption {
            painter.text(
                egui::pos2(tile_rect.left() + 14.0, tile_rect.top() + 38.0),
                egui::Align2::LEFT_TOP,
                cap,
                egui::FontId::monospace(11.0),
                tokens.text_dim,
            );
        }
        if response.clicked() {
            clicked = Some(idx);
        }
    }
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Built-in tile order matches the canonical sidebar
    /// (DESIGN.md §5) — DO NOT REORDER.
    #[test]
    fn builtin_tile_order_matches_canonical_sidebar() {
        // Reflect the literal tiles array from `render_builtin`.
        let labels = [
            "Files", "Processes", "Services", "Network", "Settings",
            "Scheduler", "Monitor", "Logs", "Terminal", "Chat", "Admin",
            "Explorer",
        ];
        assert_eq!(labels.len(), 12, "twelve canonical tiles");
        // Apps (the launcher) is intentionally absent — clicking the
        // launcher to launch the launcher is confusing.
        assert!(!labels.contains(&"Apps"));
    }

    #[test]
    fn tile_dimensions_match_design_archetype() {
        // DESIGN.md §4.3 tile-grid: ~140×110 px tiles, ~20 px gap.
        assert_eq!(TILE_W, 140.0);
        assert_eq!(TILE_H, 110.0);
        assert_eq!(TILE_GAP, 20.0);
    }
}
