//! Chat — first-class sidebar app (WEFT-588). Concierge-bot agent
//! stream graduated from `crates/clawft-gui-egui/src/explorer/chat.rs`.
//! DESIGN.md §9 sidebar 10.
//!
//! All the real work — markdown rendering, system-prompt editor,
//! heartbeat label, identity-drift warning, `agent.chat` RPC plumbing
//! — lives in `explorer::chat`. This module is a thin host: it owns
//! nothing, paints the canonical heading, and delegates the body to
//! the standalone [`ChatView`](crate::explorer::chat::ChatView)
//! instance on [`Desktop`].
//!
//! State-lifting note: `desk.chat` is independent from
//! `desk.explorer.chat_view` (which backs the substrate-sentinel
//! dispatch path inside the Explorer detail pane). Two conversations
//! by design — the sidebar app is the user's persistent
//! concierge-bot surface; the substrate-sentinel chat is whatever the
//! substrate topology surfaces under a `{kind:"chat"}` value.
//!
//! Wire shape: `chat::paint` takes a substrate path string and a
//! sentinel value. We synthesise both — the sidebar app has no
//! substrate path of its own (it's a permanent UI surface, not a
//! topology-mounted panel) and we hardcode the model name. Both
//! arguments are cosmetic only inside `chat::paint` — the path is
//! used solely for the muted footer hint and the value is queried
//! only for the model display string. Neither drives any RPC.

use std::sync::Arc;

use eframe::egui;
use serde_json::json;

use crate::live::{Live, Snapshot};
use crate::shell::desktop::Desktop;

const HEADING_BAND_H: f32 = 64.0;

/// Cosmetic substrate path shown in the chat panel's muted footer.
/// Not a real substrate mount — the sidebar Chat app is a permanent
/// surface, not topology-driven.
const SIDEBAR_CHAT_PATH: &str = "ui://sidebar/chat";

pub fn show(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    desk: &mut Desktop,
    live: &Arc<Live>,
    _snap: &Snapshot,
) {
    super::paint_heading(ui, rect, "Chat · concierge-bot");
    let body = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() + HEADING_BAND_H),
        rect.max,
    );

    // Synthesised sentinel — `chat::paint` only reads `model` for the
    // small chip above the heading and ignores everything else. Kept
    // here (not on `ChatView`) so it's obvious at the call site that
    // this is a cosmetic stand-in, not real substrate data.
    let sentinel = json!({ "kind": "chat", "model": "local" });

    ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
        crate::explorer::chat::paint(
            ui,
            SIDEBAR_CHAT_PATH,
            &sentinel,
            &mut desk.chat,
            live,
        );
    });
}
