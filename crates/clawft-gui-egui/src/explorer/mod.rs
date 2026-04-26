//! Ontology Explorer — left-tree / right-detail panel for walking the
//! substrate namespace live. Spec: `.planning/explorer/PROJECT-PLAN.md`.
//!
//! Architecture:
//! - [`Explorer`] owns panel state: expansion set, selection, cached
//!   tree children, per-path activity timestamps, and the RPC
//!   subscription lifecycle.
//! - Tree expansion fires `substrate.list { prefix, depth: 1 }` via
//!   [`Live::submit`](crate::live::Live::submit) and caches the result.
//! - Selection fires `substrate.read` for the selected path, with a
//!   slow re-poll so the viewer updates in near-real-time. When
//!   selection changes, the prior poll handle is dropped — the next
//!   tick simply reads the new path.
//!
//! Graceful degradation: if `substrate.list` comes back with
//! "method not allowed" (backend worker hasn't landed the RPC yet),
//! the tree renders just the virtual root and shows a small
//! backend-unavailable hint. No synthetic in-memory children.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use eframe::egui;
use serde_json::Value;

use crate::live::{self, Command, Live, ReplyRx};

pub mod chat;
pub mod control_toggle;
pub mod tree;
pub mod viewers;
pub mod workshop;

/// A path is considered "live" if its value has changed within this
/// window. Used for the ● activity dot in the tree.
pub const ACTIVITY_WINDOW: Duration = Duration::from_secs(3);

/// How often to re-list every currently-expanded prefix, so newly
/// appearing paths show up without the user re-clicking.
pub const SLOW_TICK: Duration = Duration::from_millis(1000);

/// How often to re-read the selected path's value so the right pane
/// tracks updates. In place of `substrate.subscribe` streaming frames
/// (which Command::Raw can't model as a single oneshot), we poll the
/// one selected path. Cheap: a single read per tick.
pub const SELECT_POLL: Duration = Duration::from_millis(400);

/// Opaque subscription handle. Today: the one-path we're re-polling.
/// Wrapping this in a type lets us drop it atomically when selection
/// changes — no dangling in-flight reads on the wrong path.
pub struct SubscriptionHandle {
    /// The path this handle is tracking.
    pub path: String,
    /// The in-flight reply channel, if a read is currently outstanding.
    /// Dropped when the handle itself is dropped — back-pressure at
    /// the channel level.
    pub pending: Option<ReplyRx>,
    /// When the last successful read landed, used to space out polls.
    pub last_poll: web_time::Instant,
}

impl SubscriptionHandle {
    fn new(path: String) -> Self {
        Self {
            path,
            pending: None,
            // Epoch-ish: first poll fires immediately. `checked_sub`
            // because on WASM `Instant::now()` at early page-load can be
            // less than `SELECT_POLL * 2`, and unchecked subtraction
            // panics with "overflow when subtracting duration from
            // instant". When the saturating fallback hits, the first
            // poll just fires on the next `SELECT_POLL` tick instead of
            // immediately — acceptable cost to avoid the WASM crash.
            last_poll: web_time::Instant::now()
                .checked_sub(SELECT_POLL * 2)
                .unwrap_or_else(web_time::Instant::now),
        }
    }
}

/// One child entry in the cached tree. Mirrors the `substrate.list`
/// response row shape.
#[derive(Clone, Debug)]
pub struct TreeNode {
    pub path: String,
    pub has_value: bool,
    pub child_count: u64,
}

/// Pending list request we've fired but haven't yet received a reply for.
struct PendingList {
    prefix: String,
    /// `None` until [`Explorer::tick`] dispatches the RPC; once fired,
    /// `Some(rx)` drains through [`Explorer::drain_replies`]. Splitting
    /// into two states (vs. probing a closed placeholder rx) keeps us
    /// from accidentally consuming a real reply in the dispatch probe.
    rx: Option<ReplyRx>,
}

/// Panel state. One instance lives on the [`Desktop`](crate::shell::desktop::Desktop)
/// and is shown when the user toggles the Explorer tray chip.
pub struct Explorer {
    /// Set of prefixes the user has expanded in the left tree.
    pub expanded: HashSet<String>,
    /// Currently selected substrate path (right pane focus).
    pub selected: Option<String>,
    /// Cache of `substrate.list` results, keyed by the prefix requested.
    /// An entry of `Some(vec)` means we have children; `None` means we
    /// haven't fetched yet (renders as "loading…").
    pub tree_children: HashMap<String, Vec<TreeNode>>,
    /// Active subscription for the currently-selected path. `None` when
    /// nothing is selected. Swapped (NOT mutated) when selection
    /// changes, so the old handle's [`ReplyRx`] drops cleanly.
    pub subscription_handle: Option<SubscriptionHandle>,
    /// Last-activity instant per path, used to drive the ● activity dot.
    pub activity: HashMap<String, web_time::Instant>,
    /// Most recent value retrieved for the selected path, used by the
    /// right-hand detail pane. Replaced on every successful read.
    pub selected_value: Option<Value>,
    /// Populated when the backend's `substrate.list` isn't available
    /// (method not allowed, connection error, …). Shown as a small
    /// header chip in the tree. Cleared on the first successful list.
    pub backend_hint: Option<String>,

    /// In-flight list requests waiting for a reply.
    pending_lists: Vec<PendingList>,
    /// Last time we re-polled every expanded prefix for changes.
    last_slow_tick: web_time::Instant,
    /// Live Workshop-composition state, reused across frames so
    /// per-panel subscriptions don't restart every paint. The view
    /// is keyed to the currently-selected Workshop path; changing
    /// selection clears it via [`Explorer::on_select`] so the new
    /// Workshop (if any) starts from a clean slate.
    workshop_view: workshop::WorkshopView,
    /// Live chat-window state. Holds conversation history + draft
    /// input + in-flight reply channel across frames so a paint
    /// doesn't lose a pending `llm.prompt` reply. Cleared on selection
    /// change for the same reason as `workshop_view` — a fresh chat
    /// sentinel selection should start with an empty history rather
    /// than inherit the previous panel's turns.
    chat_view: chat::ChatView,
}

impl Default for Explorer {
    fn default() -> Self {
        let mut expanded = HashSet::new();
        // Seed with the substrate root expanded so the slow tick
        // re-fetches the top-level node list. `tree::paint` also
        // re-asserts this every frame; this seed just avoids one
        // wasted frame at startup before the first paint.
        expanded.insert(crate::explorer::tree::ROOT_PREFIX.to_string());
        Self {
            expanded,
            selected: None,
            tree_children: HashMap::new(),
            subscription_handle: None,
            activity: HashMap::new(),
            selected_value: None,
            backend_hint: None,
            pending_lists: Vec::new(),
            // `now - slow tick` so the first update() fires the slow
            // refresh immediately. `checked_sub` avoids the WASM
            // `overflow when subtracting duration from instant` panic
            // when the browser time-origin is fresh; fallback means
            // the first slow tick fires a `SLOW_TICK` later instead.
            last_slow_tick: web_time::Instant::now()
                .checked_sub(SLOW_TICK * 2)
                .unwrap_or_else(web_time::Instant::now),
            workshop_view: workshop::WorkshopView::default(),
            chat_view: chat::ChatView::default(),
        }
    }
}

impl Explorer {
    /// Paint the two-pane Explorer layout inside `ui`. `live` is the
    /// RPC transport handle used to fire substrate.list / substrate.read.
    pub fn show(&mut self, ui: &mut egui::Ui, live: &Arc<Live>) {
        // 1. Tick: fire any due list re-polls and selected-path reads.
        self.tick(live);

        // 2. Drain any completed RPC replies so the UI reflects fresh
        //    data before we paint.
        self.drain_replies();

        // 3. Layout: left SidePanel for tree, CentralPanel for detail.
        //    ~40/60 split per the spec.
        let total_w = ui.available_width();
        let tree_w = (total_w * 0.4).clamp(220.0, 480.0);

        egui::SidePanel::left("weft_explorer_tree")
            .resizable(true)
            .default_width(tree_w)
            .width_range(180.0..=640.0)
            .show_inside(ui, |ui| {
                ui.heading("Substrate");
                ui.separator();
                if let Some(newly) = tree::paint(ui, self) {
                    self.on_select(newly, live);
                }
            });

        egui::CentralPanel::default().show_inside(ui, |ui| {
            self.paint_detail(ui, live);
        });
    }

    /// Called by the layout code after a new path is selected. Drops
    /// the previous subscription and starts tracking the new one.
    fn on_select(&mut self, path: String, _live: &Arc<Live>) {
        // Dropping the existing `SubscriptionHandle` closes its pending
        // ReplyRx — no leaked in-flight reads on the old path.
        self.subscription_handle = Some(SubscriptionHandle::new(path.clone()));
        self.selected = Some(path);
        self.selected_value = None;
        // Reset Workshop state when selection moves. If the new path
        // is also a Workshop, its subscriptions rebuild on the next
        // paint; if not, the old per-panel polls stop immediately.
        self.workshop_view = workshop::WorkshopView::default();
        // Reset chat state too. A pending `llm.prompt` reply against
        // the previous selection is dropped (the ReplyRx falls out of
        // scope, the daemon's response is ignored). In-memory only —
        // there is no on-disk conversation to restore.
        self.chat_view = chat::ChatView::default();
    }

    /// Clear the subscription handle. Called by the mount site when
    /// the Explorer panel is closed so no poll is left running against
    /// a hidden panel.
    pub fn close(&mut self) {
        self.subscription_handle = None;
        self.selected_value = None;
        // Drop all Workshop per-panel subscriptions so hidden panels
        // stop polling.
        self.workshop_view = workshop::WorkshopView::default();
        // Drop chat state on close — same reason as Workshop: hidden
        // panels shouldn't keep an in-flight RPC against the daemon.
        self.chat_view = chat::ChatView::default();
        // Keep expanded + tree_children so reopening is instant.
    }

    /// Enqueue a `substrate.list` request. Public for tree::paint.
    pub fn queue_list(&mut self, prefix: String) {
        // Using `Live::submit` would require the &Arc<Live>, which
        // paint-time borrow rules forbid here. Instead we stash the
        // prefix and let `tick` dispatch it on the next frame. This
        // also naturally coalesces duplicate requests within a frame.
        if !self.pending_lists.iter().any(|p| p.prefix == prefix) {
            self.pending_lists.push(PendingList {
                prefix,
                rx: None,
            });
        }
    }

    /// Per-frame tick: fire any newly-queued list requests, schedule
    /// slow-tick re-polls, and re-read the selected path if its poll
    /// interval has elapsed.
    fn tick(&mut self, live: &Arc<Live>) {
        // (a) Dispatch list requests that don't have an rx yet.
        for p in self.pending_lists.iter_mut() {
            if p.rx.is_none() {
                let (tx, rx) = live::reply_channel();
                p.rx = Some(rx);
                live.submit(Command::Raw {
                    method: "substrate.list".into(),
                    params: serde_json::json!({
                        "prefix": p.prefix,
                        "depth": 1,
                    }),
                    reply: Some(tx),
                });
            }
        }

        // (b) Slow tick: re-list every currently-expanded prefix. This
        //     is how newly appearing paths show up without user action.
        if self.last_slow_tick.elapsed() >= SLOW_TICK {
            self.last_slow_tick = web_time::Instant::now();
            let prefixes: Vec<String> = self.expanded.iter().cloned().collect();
            for prefix in prefixes {
                self.queue_list(prefix);
            }
        }

        // (c) Selected-path re-poll. `substrate.read` oneshots are the
        //     simplest thing that tracks a single value — streaming
        //     subscribe would demand new plumbing through Live.
        if let Some(handle) = self.subscription_handle.as_mut()
            && handle.pending.is_none()
            && handle.last_poll.elapsed() >= SELECT_POLL
        {
            let (tx, rx) = live::reply_channel();
            handle.pending = Some(rx);
            handle.last_poll = web_time::Instant::now();
            live.submit(Command::Raw {
                method: "substrate.read".into(),
                params: serde_json::json!({ "path": handle.path }),
                reply: Some(tx),
            });
        }
    }

    /// Drain completed RPC replies. Updates caches + activity.
    fn drain_replies(&mut self) {
        // (a) Tree listings.
        let mut still = Vec::with_capacity(self.pending_lists.len());
        let pending = std::mem::take(&mut self.pending_lists);
        for mut p in pending {
            // Entries without an rx yet haven't been dispatched — leave
            // them for the next tick() pass to fire.
            let Some(rx) = p.rx.as_mut() else {
                still.push(p);
                continue;
            };
            match live::try_recv_reply(rx) {
                live::TryReply::Done(Ok(value)) => {
                    let kids = tree::parse_list_response(&value);
                    self.tree_children.insert(p.prefix.clone(), kids);
                    // First success clears any prior backend hint.
                    self.backend_hint = None;
                }
                live::TryReply::Done(Err(err)) => {
                    // Only stash the hint if we've never seen success
                    // yet — if we once had children for this prefix,
                    // a transient error shouldn't flip the UI into
                    // "backend unavailable" mode.
                    if self.tree_children.is_empty() {
                        self.backend_hint = Some(format!(
                            "substrate.list not yet available: {err}"
                        ));
                    }
                    // Record an empty cache so the row shows `(empty)`
                    // instead of an infinite "loading…".
                    self.tree_children.entry(p.prefix.clone()).or_default();
                }
                live::TryReply::Empty => {
                    // Still in flight — keep it.
                    still.push(p);
                }
                live::TryReply::Closed => {
                    // The transport dropped its end without replying —
                    // treat like a transient error: record an empty
                    // cache and let the slow tick re-list.
                    self.tree_children.entry(p.prefix.clone()).or_default();
                }
            }
        }
        self.pending_lists = still;

        // (b) Selected-path read.
        if let Some(handle) = self.subscription_handle.as_mut()
            && let Some(rx) = handle.pending.as_mut()
        {
            match live::try_recv_reply(rx) {
                live::TryReply::Done(Ok(value)) => {
                    handle.pending = None;
                    // substrate.read shape: { value, tick, sensitivity }
                    let new_value = value.get("value").cloned().unwrap_or(Value::Null);
                    let changed = self.selected_value.as_ref() != Some(&new_value);
                    if changed {
                        self.activity
                            .insert(handle.path.clone(), web_time::Instant::now());
                    }
                    self.selected_value = Some(new_value);
                }
                live::TryReply::Done(Err(_)) | live::TryReply::Closed => {
                    handle.pending = None;
                    // No value update; tick() will retry on next cycle.
                }
                live::TryReply::Empty => { /* still in flight */ }
            }
        }
    }

    /// Right-pane detail rendering. Picks a viewer via [`viewers::dispatch`]
    /// and paints it.
    ///
    /// Workshop shortcut: when the selected value shape-matches a
    /// Workshop (see [`workshop::matches`]), the detail pane renders
    /// it as a composition — nested panels with their own substrate
    /// subscriptions — instead of falling through to the generic
    /// viewer. This is what makes config-driven hot-reload work:
    /// publishing a new Workshop JSON replaces `selected_value`, which
    /// the Workshop renderer re-parses on the next frame.
    fn paint_detail(&mut self, ui: &mut egui::Ui, live: &Arc<Live>) {
        let Some(path) = self.selected.clone() else {
            ui.vertical_centered(|ui| {
                ui.add_space(64.0);
                ui.label(
                    egui::RichText::new("Select a path from the tree")
                        .italics()
                        .color(egui::Color32::from_rgb(170, 170, 180)),
                );
            });
            return;
        };
        match self.selected_value.clone() {
            Some(v) => {
                // Object Type badge: shape-infer a type and render a
                // small label above the viewer. When no type is
                // inferred we render nothing — viewer dispatch stays
                // exactly as Phase 1 shipped it.
                if let Some(inferred) = crate::ontology::infer(&v) {
                    paint_object_type_badge(ui, inferred);
                }
                // Workshop dispatch: if the value shape-matches a
                // Workshop, render the composition primitive instead
                // of the generic viewer cascade. Shape-only; no path
                // whitelist. ADOPTION §8 Step 3: "The shape of a value
                // at a substrate path determines which Object Type it
                // instantiates, which Viewers render it…"
                if workshop::matches(&v) > 0 {
                    self.workshop_view.paint(ui, &v, live);
                    return;
                }
                // Chat sentinel: dispatched ahead of control_toggle so
                // a `{kind:"chat"}` value lands in the chat panel even
                // if some future control intent shape brushes against
                // it. Needs the Live RPC handle to fire `llm.prompt`
                // and the persistent `chat_view` state for history +
                // in-flight reply tracking.
                if chat::matches(&v) > 0 {
                    chat::paint(ui, &path, &v, &mut self.chat_view, live);
                    return;
                }
                // Control-intent toggle: shape-match precedes the
                // generic viewer cascade. Lives outside the
                // SubstrateViewer trait because it needs the Live
                // RPC handle to fire `control.set_enabled` on click.
                if control_toggle::matches(&v) > 0 {
                    control_toggle::paint(ui, &path, &v, live);
                    return;
                }
                viewers::dispatch(ui, &path, &v);
            }
            None => {
                ui.horizontal(|ui| {
                    ui.monospace(&path);
                    ui.separator();
                    ui.label(
                        egui::RichText::new("reading…")
                            .italics()
                            .color(egui::Color32::from_rgb(170, 170, 180)),
                    );
                });
            }
        }
    }
}

/// Render the Object Type badge for an inferred type.
///
/// Visual: a muted-blue pill with `[DisplayName]` above whatever the
/// viewer registry paints. Kept deliberately small + passive — the
/// badge is informational, not interactive. If/when property panels
/// or Action affordances arrive, they attach here.
fn paint_object_type_badge(ui: &mut egui::Ui, inferred: crate::ontology::InferredType) {
    ui.horizontal(|ui| {
        let label = egui::RichText::new(format!("[{}]", inferred.display))
            .monospace()
            .small()
            .color(egui::Color32::from_rgb(140, 175, 220));
        ui.label(label);
    });
    ui.add_space(2.0);
}
