//! WeftOS apps — first-class surfaces driven from the desktop sidebar.
//!
//! Phase 3 of the 0.8.0 desktop wave (see
//! `docs/plans/desktop-implementation-0.8.0.md`). Each module under
//! `apps::` corresponds to one entry in DESIGN.md §9 OOB manifest.
//!
//! The first cut ships **stubs** — heading + empty/loading/offline
//! state via [`state::render_if_needed`] against the bound substrate
//! paths. Each app's real content (table, tree, plot, surface
//! composer, …) is filled in by the swarm under follow-up Plane
//! tickets. The contract is: every app is one of the five archetypes
//! from DESIGN.md §4 and uses only the `blocks/` library + the
//! surface composer for rendering.

pub mod admin;
pub mod chat;
pub mod explorer;
pub mod files;
pub mod launcher;
pub mod logs;
pub mod monitor;
pub mod network;
pub mod processes;
pub mod scheduler;
pub mod services;
pub mod settings;
pub mod state;
pub mod terminal;

use std::sync::Arc;

use eframe::egui;

use crate::live::{Live, Snapshot};
use crate::shell::desktop::Desktop;
use crate::shell::sidebar::SidebarTarget;

/// Dispatch to the active app. Called from `desktop::show()` after
/// painting the sidebar + wallpaper.
///
/// Apps receive `&mut Desktop` so they can read/write their own state
/// (Explorer expansion set, blocks demo state, app registry, etc.) and
/// `&Arc<Live>` so they can submit RPC commands through the live bridge.
///
/// Lifecycle hygiene (WEFT-590): before dispatching the active app,
/// the previous active target is compared against the current one.
/// On a transition AWAY from an app that needs cleanup (today: only
/// Explorer, which holds substrate.list / substrate.read polls), the
/// per-app `close()` runs so background polls don't keep firing
/// against a hidden panel. `prev_active` is then refreshed.
///
/// Terminal/Chat sidebar apps intentionally do NOT close on
/// nav-away — the user might be mid-conversation or mid-shell-command
/// and expects to come back to the running session. Their state
/// survives across hides; only Explorer's RPC polls leak budget when
/// nobody's watching.
pub fn dispatch(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    desk: &mut Desktop,
    live: &Arc<Live>,
    snap: &Snapshot,
) {
    let target = desk.sidebar.active;
    if desk.prev_active != target {
        // Transition: run lifecycle cleanup on the app we're leaving.
        // Today only Explorer needs this.
        if let SidebarTarget::Explorer = desk.prev_active {
            desk.explorer.close(live);
        }
        desk.prev_active = target;
    }
    match target {
        SidebarTarget::Files => files::show(ui, rect, desk, live, snap),
        SidebarTarget::Processes => processes::show(ui, rect, desk, live, snap),
        SidebarTarget::Services => services::show(ui, rect, desk, live, snap),
        SidebarTarget::Network(tab) => network::show(ui, rect, desk, live, snap, tab),
        SidebarTarget::Settings => settings::show(ui, rect, desk, live, snap),
        SidebarTarget::Scheduler => scheduler::show(ui, rect, desk, live, snap),
        SidebarTarget::Monitor => monitor::show(ui, rect, desk, live, snap),
        SidebarTarget::Logs(tab) => logs::show(ui, rect, desk, live, snap, tab),
        SidebarTarget::Terminal => terminal::show(ui, rect, desk, live, snap),
        SidebarTarget::Chat => chat::show(ui, rect, desk, live, snap),
        SidebarTarget::Admin => admin::show(ui, rect, desk, live, snap),
        SidebarTarget::Explorer => explorer::show(ui, rect, desk, live, snap),
        SidebarTarget::Apps(tab) => launcher::show(ui, rect, desk, live, snap, tab),
    }
}

/// Common header — heading text rendered top-left of the app rect.
pub(crate) fn paint_heading(ui: &egui::Ui, rect: egui::Rect, heading: &str) {
    use crate::theming::Tokens;
    let tokens = Tokens::default();
    let painter = ui.painter_at(rect);
    painter.text(
        egui::pos2(rect.left() + 24.0, rect.top() + 24.0),
        egui::Align2::LEFT_TOP,
        heading,
        egui::FontId::proportional(18.0),
        tokens.text_primary,
    );
}
