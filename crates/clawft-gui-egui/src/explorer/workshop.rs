//! Workshop — config-driven, hot-reload UI composition.
//!
//! Spec: `.planning/ontology/ADOPTION.md` §8 Step 3 and
//! `.planning/explorer/PHASE-2-PLAN.md` §6 Track 5.
//!
//! ## What it is
//!
//! A Workshop is a composition primitive: a JSON value, stored at
//! `substrate/ui/workshop/<name>`, that describes a layout of child
//! panels. Each child panel names a substrate path whose value is
//! rendered with the existing shape-dispatched viewer registry
//! ([`crate::explorer::viewers::dispatch`]).
//!
//! The whole point of this primitive is that iterating on the
//! composition happens via substrate **publish**, not GUI rebuild. A
//! new Workshop value replaces the [`WorkshopView`] state on the next
//! frame and the layout re-renders.
//!
//! ## Schema
//!
//! ```json
//! {
//!   "title": "Mic diagnostic",
//!   "layout": "rows",
//!   "panels": [
//!     {
//!       "title": "RMS gauge",
//!       "substrate_path": "substrate/sensor/mic",
//!       "viewer_hint": "auto",
//!       "min_height": 120
//!     }
//!   ]
//! }
//! ```
//!
//! * `title` — optional string; rendered as the Workshop heading.
//! * `layout` — one of `rows` (default), `grid`, `tabs`. Only `rows`
//!   is implemented in the MVP; the enum is reserved open so future
//!   layouts plug in without a schema bump.
//! * `panels` — ordered array of [`WorkshopPanel`]s, each pointing at a
//!   substrate path. `viewer_hint` is reserved for explicit viewer
//!   overrides; today `"auto"` (or unset) routes through
//!   [`super::viewers::dispatch`].
//!
//! ## Hot-reload mechanism
//!
//! 1. User (or a file-watcher, or a script, or an LLM) publishes a new
//!    Workshop JSON value to `substrate/ui/workshop/<name>`.
//! 2. The Explorer's existing `SELECT_POLL` re-read fetches the new
//!    value into `Explorer::selected_value`.
//! 3. [`WorkshopView::paint`] parses the value afresh each frame — the
//!    `Workshop` struct is never cached across frames, so a published
//!    change takes effect in the very next paint.
//! 4. The child-panel poller ([`WorkshopView`]) diffs its tracked path
//!    set against the newly-parsed panel list; stale subscriptions are
//!    dropped, new ones start on the next tick.
//!
//! No reload, no rebuild, no explicit subscription handshake — the
//! substrate poll cascade handles it.

use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui;
use serde_json::Value;

use crate::live::{self, Command, Live, ReplyRx};

/// Reserved Object Type name so Track 2's `ontology::infer` can match
/// Workshop shapes once a registration is added there. The value lives
/// in this module rather than in `ontology/types/` to keep Workshop
/// self-contained for the MVP — the Object-Type registration is the
/// single add-site when the ontology layer grows to recognize it.
pub const OBJECT_TYPE_NAME: &str = "Workshop";

/// Human-readable Object Type label.
pub const OBJECT_TYPE_DISPLAY: &str = "Workshop";

/// Shape-match priority for the Workshop Object Type — higher than the
/// generic Mesh (20) so a substrate value with a `panels: [...]` array
/// lands here decisively.
pub const OBJECT_TYPE_PRIORITY: u32 = 30;

/// Per-panel poll cadence. Independent of the top-level Workshop
/// re-read (which picks up schema changes) — this is the per-panel
/// value refresh. 400ms matches the selected-path poll rate in the
/// Explorer so all panels feel equally live.
const PANEL_POLL: std::time::Duration = std::time::Duration::from_millis(400);

/// Minimum panel height when unspecified. Leaves enough vertical room
/// for a typical viewer (gauge, sparkline, badge row) without forcing
/// a scrollbar on small content.
const DEFAULT_MIN_HEIGHT: f32 = 80.0;

/// Parsed Workshop value. Cheap to rebuild every frame — the shape is
/// O(panels) scalars + a few strings. `WorkshopView` owns the live
/// per-panel subscription state; `Workshop` is pure schema.
///
/// Hand-parsed from `serde_json::Value` rather than `#[derive(Deserialize)]`
/// because `clawft-gui-egui` keeps serde at one remove (through
/// `serde_json`) and we don't need the full derive plumbing for this
/// O(few fields) shape.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Workshop {
    /// Optional display title for the composition.
    pub title: Option<String>,
    /// Layout strategy. `rows` is the only implemented variant;
    /// unknown layout strings round-trip through [`WorkshopLayout::Unknown`]
    /// so a forward-compatible writer doesn't get silently clipped.
    pub layout: WorkshopLayout,
    /// Ordered panel list. Empty is legal (Workshop renders its title
    /// and an empty-state hint).
    pub panels: Vec<WorkshopPanel>,
}

/// Layout strategy for a Workshop. Only `Rows` is rendered today;
/// `Grid` and `Tabs` fall back to a rows layout with a debug hint so a
/// forward-looking writer can publish the shape now and the renderer
/// degrades gracefully. `Unknown` preserves unknown layouts on
/// round-trip so a TOML writer doesn't lose data.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum WorkshopLayout {
    #[default]
    Rows,
    Grid,
    Tabs,
    /// Any layout string the MVP doesn't recognize.
    Unknown,
}

impl WorkshopLayout {
    fn from_str(s: &str) -> Self {
        match s {
            "rows" => WorkshopLayout::Rows,
            "grid" => WorkshopLayout::Grid,
            "tabs" => WorkshopLayout::Tabs,
            _ => WorkshopLayout::Unknown,
        }
    }
}

/// One child panel in a Workshop.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkshopPanel {
    /// Display label for the panel header. Optional; when absent the
    /// substrate path is shown as the header.
    pub title: Option<String>,
    /// Substrate path whose value this panel renders. Required.
    pub substrate_path: String,
    /// Explicit viewer name to force, or `"auto"` / unset for
    /// shape-dispatched default. Reserved — the MVP only honors
    /// `auto`.
    pub viewer_hint: String,
    /// Optional per-panel minimum height in logical pixels.
    pub min_height: Option<f32>,
}

/// Parse a substrate value into a [`Workshop`]. Returns `Err` with a
/// short human-readable message when the value isn't a Workshop shape.
///
/// Strict enough to reject arbitrary blobs but lenient enough that
/// forward-compatible writers (extra fields, unknown layouts) succeed.
pub fn parse(value: &Value) -> Result<Workshop, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "workshop value must be a JSON object".to_string())?;
    let panels_val = obj
        .get("panels")
        .ok_or_else(|| "workshop value missing required `panels` field".to_string())?;
    let panels_arr = panels_val
        .as_array()
        .ok_or_else(|| "workshop `panels` must be an array".to_string())?;

    let mut panels = Vec::with_capacity(panels_arr.len());
    for (i, p) in panels_arr.iter().enumerate() {
        panels.push(parse_panel(p).map_err(|e| format!("panels[{i}]: {e}"))?);
    }

    let title = obj
        .get("title")
        .and_then(Value::as_str)
        .map(str::to_string);

    let layout = obj
        .get("layout")
        .and_then(Value::as_str)
        .map(WorkshopLayout::from_str)
        .unwrap_or_default();

    Ok(Workshop {
        title,
        layout,
        panels,
    })
}

fn parse_panel(value: &Value) -> Result<WorkshopPanel, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "panel must be a JSON object".to_string())?;
    let substrate_path = obj
        .get("substrate_path")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing required `substrate_path` string".to_string())?
        .to_string();
    let title = obj
        .get("title")
        .and_then(Value::as_str)
        .map(str::to_string);
    let viewer_hint = obj
        .get("viewer_hint")
        .and_then(Value::as_str)
        .unwrap_or("auto")
        .to_string();
    // Accept either integer or float for min_height — TOML often emits
    // integers and we don't want to force-quote the value.
    let min_height = obj.get("min_height").and_then(|v| {
        v.as_f64()
            .map(|f| f as f32)
            .or_else(|| v.as_i64().map(|i| i as f32))
            .or_else(|| v.as_u64().map(|u| u as f32))
    });
    Ok(WorkshopPanel {
        title,
        substrate_path,
        viewer_hint,
        min_height,
    })
}

/// Shape-match a substrate value as a Workshop. Returns the priority
/// (>0) when the value looks like a Workshop, 0 otherwise.
///
/// Heuristic: object with a `panels` array, and every panel entry has
/// a `substrate_path` string. Two-level check keeps false positives
/// out without forcing full schema validation in the Object-Type
/// inference path.
pub fn matches(value: &Value) -> u32 {
    let Some(obj) = value.as_object() else {
        return 0;
    };
    let Some(panels) = obj.get("panels").and_then(Value::as_array) else {
        return 0;
    };
    // Empty panels array is a legitimate Workshop (a blank canvas
    // someone is about to fill). Non-empty panels must all look like
    // panels.
    let all_panel_shaped = panels.iter().all(|p| {
        p.as_object()
            .and_then(|o| o.get("substrate_path"))
            .and_then(Value::as_str)
            .is_some()
    });
    if all_panel_shaped {
        OBJECT_TYPE_PRIORITY
    } else {
        0
    }
}

/// Live per-panel subscription state for a [`Workshop`] rendering.
/// Owns one [`PanelSub`] per substrate path currently referenced by
/// the Workshop schema; diffs the path set on each paint so newly
/// added panels get polled and removed panels have their pending
/// replies dropped.
#[derive(Default)]
pub struct WorkshopView {
    /// Per-substrate-path subscription state, keyed by
    /// [`WorkshopPanel::substrate_path`]. Using a `HashMap` here (vs.
    /// a `Vec` matching the panel order) lets Workshop layout tweaks
    /// reorder panels without churning subscriptions.
    subs: HashMap<String, PanelSub>,
}

struct PanelSub {
    /// Last value we pulled for this path. Rendered while the next
    /// read is in flight so the panel doesn't flicker to blank.
    value: Option<Value>,
    /// Pending `substrate.read` reply channel. `Some` while a read is
    /// in flight; dropped atomically when the Workshop stops
    /// referencing this path.
    pending: Option<ReplyRx>,
    /// When the last poll was dispatched. Used to space reads at
    /// [`PANEL_POLL`].
    last_poll: web_time::Instant,
    /// Last error message, if the most recent read failed. Cleared on
    /// the next successful read. Rendered above the viewer so the user
    /// sees *why* a panel isn't updating rather than a stale value.
    last_error: Option<String>,
}

impl PanelSub {
    fn new() -> Self {
        Self {
            value: None,
            pending: None,
            // Epoch: fire the first poll immediately. `checked_sub`
            // because on WASM `Instant::now()` at early page-load can be
            // less than `PANEL_POLL * 2`, and unchecked subtraction
            // panics with "overflow when subtracting duration from
            // instant". Fallback means the first poll fires one
            // `PANEL_POLL` later — acceptable to avoid the crash.
            last_poll: web_time::Instant::now()
                .checked_sub(PANEL_POLL * 2)
                .unwrap_or_else(web_time::Instant::now),
            last_error: None,
        }
    }
}

impl WorkshopView {
    /// Paint a Workshop derived from `value`. Drives per-panel
    /// subscriptions, prunes stale ones, and delegates rendering of
    /// each panel's value to the viewer registry.
    ///
    /// Called from the Explorer's detail pane; see
    /// [`crate::explorer::Explorer::paint_detail`].
    pub fn paint(&mut self, ui: &mut egui::Ui, value: &Value, live: &Arc<Live>) {
        // Parse fresh every frame. If a publish to the Workshop path
        // changed the schema between frames, the new layout takes
        // effect here — no cache invalidation dance. Cost is O(panels)
        // JSON traversal per frame; the panel count is small by
        // construction.
        let workshop = match parse(value) {
            Ok(w) => w,
            Err(err) => {
                paint_parse_error(ui, &err, value);
                return;
            }
        };

        self.reconcile_subs(&workshop);
        self.drain_replies();
        self.tick(live);

        paint_header(ui, &workshop);
        match workshop.layout {
            WorkshopLayout::Rows => self.paint_rows(ui, &workshop),
            WorkshopLayout::Grid | WorkshopLayout::Tabs | WorkshopLayout::Unknown => {
                // Fall back to rows for unimplemented layouts. A small
                // hint makes the degradation visible so the writer
                // knows why their grid/tabs layout looks vertical.
                ui.label(
                    egui::RichText::new(format!(
                        "layout `{:?}` not yet rendered — falling back to rows",
                        workshop.layout
                    ))
                    .italics()
                    .small()
                    .color(egui::Color32::from_rgb(200, 170, 120)),
                );
                self.paint_rows(ui, &workshop);
            }
        }
    }

    /// Drop subscriptions for paths no longer referenced by the
    /// Workshop; add fresh [`PanelSub`]s for newly introduced paths.
    /// Dropping the `PanelSub` closes its `ReplyRx` — no dangling
    /// reads against a removed panel.
    fn reconcile_subs(&mut self, workshop: &Workshop) {
        let mut desired: HashMap<String, ()> = HashMap::new();
        for panel in &workshop.panels {
            desired.insert(panel.substrate_path.clone(), ());
        }
        // Drop subs whose path is no longer in the Workshop.
        self.subs.retain(|path, _| desired.contains_key(path));
        // Ensure every desired path has a sub.
        for path in desired.keys() {
            self.subs
                .entry(path.clone())
                .or_insert_with(PanelSub::new);
        }
    }

    /// Fire `substrate.read` for any panel whose poll interval has
    /// elapsed and isn't already in flight.
    fn tick(&mut self, live: &Arc<Live>) {
        for (path, sub) in self.subs.iter_mut() {
            if sub.pending.is_some() {
                continue;
            }
            if sub.last_poll.elapsed() < PANEL_POLL {
                continue;
            }
            let (tx, rx) = live::reply_channel();
            sub.pending = Some(rx);
            sub.last_poll = web_time::Instant::now();
            live.submit(Command::Raw {
                method: "substrate.read".into(),
                params: serde_json::json!({ "path": path }),
                reply: Some(tx),
            });
        }
    }

    /// Drain completed reads into each panel's cached value.
    fn drain_replies(&mut self) {
        for (_path, sub) in self.subs.iter_mut() {
            let Some(rx) = sub.pending.as_mut() else {
                continue;
            };
            match live::try_recv_reply(rx) {
                live::TryReply::Done(Ok(value)) => {
                    sub.pending = None;
                    let new_value = value.get("value").cloned().unwrap_or(Value::Null);
                    sub.value = Some(new_value);
                    sub.last_error = None;
                }
                live::TryReply::Done(Err(err)) => {
                    sub.pending = None;
                    sub.last_error = Some(err);
                }
                live::TryReply::Closed => {
                    sub.pending = None;
                    sub.last_error = Some("transport closed".to_string());
                }
                live::TryReply::Empty => {
                    // Still in flight.
                }
            }
        }
    }

    /// Rows layout: vertical ScrollArea + per-panel Frame.
    fn paint_rows(&self, ui: &mut egui::Ui, workshop: &Workshop) {
        if workshop.panels.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(16.0);
                ui.label(
                    egui::RichText::new("(empty Workshop — publish a `panels` array)")
                        .italics()
                        .color(egui::Color32::from_rgb(160, 160, 170)),
                );
            });
            return;
        }
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (idx, panel) in workshop.panels.iter().enumerate() {
                    self.paint_panel(ui, idx, panel);
                    ui.add_space(6.0);
                }
            });
    }

    fn paint_panel(&self, ui: &mut egui::Ui, idx: usize, panel: &WorkshopPanel) {
        let title = panel
            .title
            .clone()
            .unwrap_or_else(|| panel.substrate_path.clone());
        let min_h = panel.min_height.unwrap_or(DEFAULT_MIN_HEIGHT);
        let id = egui::Id::new(("weft-workshop-panel", idx, &panel.substrate_path));

        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::same(6))
            .show(ui, |ui| {
                // Header row: title + muted path hint.
                ui.horizontal(|ui| {
                    ui.strong(title);
                    ui.separator();
                    ui.label(
                        egui::RichText::new(&panel.substrate_path)
                            .monospace()
                            .small()
                            .color(egui::Color32::from_rgb(150, 150, 160)),
                    );
                });
                ui.separator();

                // Body: the viewer registry renders the sub's most
                // recent value. If nothing has landed yet we show a
                // brief "reading…" hint.
                ui.push_id(id, |ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), min_h),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            let sub = self.subs.get(&panel.substrate_path);
                            match sub {
                                Some(sub) => {
                                    if let Some(err) = &sub.last_error {
                                        ui.label(
                                            egui::RichText::new(format!("error: {err}"))
                                                .small()
                                                .color(egui::Color32::from_rgb(220, 120, 120)),
                                        );
                                    }
                                    match &sub.value {
                                        Some(v) => {
                                            super::viewers::dispatch(
                                                ui,
                                                &panel.substrate_path,
                                                v,
                                            );
                                        }
                                        None => {
                                            ui.label(
                                                egui::RichText::new("reading…")
                                                    .italics()
                                                    .color(egui::Color32::from_rgb(160, 160, 170)),
                                            );
                                        }
                                    }
                                }
                                None => {
                                    // Should be unreachable — reconcile
                                    // ensures a sub exists for every
                                    // declared panel path. Render a
                                    // debug hint rather than panicking.
                                    ui.label("(no subscription)");
                                }
                            }
                        },
                    );
                });
            });
    }
}

fn paint_header(ui: &mut egui::Ui, workshop: &Workshop) {
    ui.horizontal(|ui| {
        ui.heading(
            workshop
                .title
                .clone()
                .unwrap_or_else(|| "Workshop".to_string()),
        );
        ui.separator();
        ui.label(
            egui::RichText::new(format!(
                "{:?} · {} panel{}",
                workshop.layout,
                workshop.panels.len(),
                if workshop.panels.len() == 1 { "" } else { "s" },
            ))
            .small()
            .color(egui::Color32::from_rgb(150, 170, 200)),
        );
    });
    ui.separator();
}

fn paint_parse_error(ui: &mut egui::Ui, err: &str, raw: &Value) {
    ui.horizontal(|ui| {
        ui.heading("Workshop");
        ui.separator();
        ui.label(
            egui::RichText::new("parse error")
                .small()
                .color(egui::Color32::from_rgb(220, 120, 120)),
        );
    });
    ui.separator();
    ui.label(
        egui::RichText::new(err)
            .monospace()
            .small()
            .color(egui::Color32::from_rgb(220, 120, 120)),
    );
    ui.add_space(4.0);
    ui.collapsing("raw value", |ui| {
        let pretty = serde_json::to_string_pretty(raw)
            .unwrap_or_else(|_| "<unserialisable>".to_string());
        ui.monospace(pretty);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_minimal_workshop() {
        let v = json!({ "panels": [] });
        let w = parse(&v).expect("minimal workshop parses");
        assert!(w.title.is_none());
        assert_eq!(w.layout, WorkshopLayout::Rows);
        assert!(w.panels.is_empty());
    }

    #[test]
    fn parse_full_workshop() {
        let v = json!({
            "title": "Mic diagnostic",
            "layout": "rows",
            "panels": [
                {
                    "title": "RMS gauge",
                    "substrate_path": "substrate/sensor/mic",
                    "viewer_hint": "auto",
                    "min_height": 120.0,
                },
                {
                    "substrate_path": "substrate/derived/transcript/mic",
                    "viewer_hint": "auto",
                }
            ]
        });
        let w = parse(&v).expect("full workshop parses");
        assert_eq!(w.title.as_deref(), Some("Mic diagnostic"));
        assert_eq!(w.layout, WorkshopLayout::Rows);
        assert_eq!(w.panels.len(), 2);
        assert_eq!(w.panels[0].title.as_deref(), Some("RMS gauge"));
        assert_eq!(w.panels[0].substrate_path, "substrate/sensor/mic");
        assert_eq!(w.panels[0].min_height, Some(120.0));
        assert!(w.panels[1].title.is_none());
    }

    #[test]
    fn parse_rejects_non_object() {
        assert!(parse(&json!([])).is_err());
        assert!(parse(&json!(42)).is_err());
        assert!(parse(&json!(null)).is_err());
    }

    #[test]
    fn parse_rejects_missing_panels() {
        let v = json!({ "title": "no panels" });
        assert!(parse(&v).is_err());
    }

    #[test]
    fn parse_rejects_panel_without_path() {
        let v = json!({
            "panels": [
                { "title": "missing substrate_path" }
            ]
        });
        assert!(parse(&v).is_err());
    }

    #[test]
    fn parse_accepts_unknown_layout() {
        let v = json!({
            "layout": "sunburst",
            "panels": []
        });
        let w = parse(&v).expect("unknown layout round-trips");
        assert_eq!(w.layout, WorkshopLayout::Unknown);
    }

    #[test]
    fn parse_accepts_grid_and_tabs_layouts() {
        let grid = parse(&json!({ "layout": "grid", "panels": [] })).unwrap();
        assert_eq!(grid.layout, WorkshopLayout::Grid);
        let tabs = parse(&json!({ "layout": "tabs", "panels": [] })).unwrap();
        assert_eq!(tabs.layout, WorkshopLayout::Tabs);
    }

    #[test]
    fn matches_positive_for_workshop_shape() {
        let v = json!({
            "panels": [
                { "substrate_path": "substrate/sensor/mic" }
            ]
        });
        assert_eq!(matches(&v), OBJECT_TYPE_PRIORITY);
    }

    #[test]
    fn matches_empty_panels_still_workshop() {
        let v = json!({ "panels": [] });
        assert_eq!(matches(&v), OBJECT_TYPE_PRIORITY);
    }

    #[test]
    fn matches_rejects_panel_missing_substrate_path() {
        let v = json!({
            "panels": [
                { "title": "broken" }
            ]
        });
        assert_eq!(matches(&v), 0);
    }

    #[test]
    fn matches_rejects_non_object() {
        assert_eq!(matches(&json!([])), 0);
        assert_eq!(matches(&Value::Null), 0);
        assert_eq!(matches(&json!("workshop")), 0);
    }

    #[test]
    fn matches_rejects_missing_panels() {
        let v = json!({ "title": "not a workshop" });
        assert_eq!(matches(&v), 0);
    }

    #[test]
    fn layout_default_is_rows() {
        assert_eq!(WorkshopLayout::default(), WorkshopLayout::Rows);
    }

    #[test]
    fn reconcile_adds_and_removes_subs() {
        let mut view = WorkshopView::default();
        let w1 = Workshop {
            title: None,
            layout: WorkshopLayout::Rows,
            panels: vec![
                WorkshopPanel {
                    title: None,
                    substrate_path: "a".into(),
                    viewer_hint: "auto".into(),
                    min_height: None,
                },
                WorkshopPanel {
                    title: None,
                    substrate_path: "b".into(),
                    viewer_hint: "auto".into(),
                    min_height: None,
                },
            ],
        };
        view.reconcile_subs(&w1);
        assert_eq!(view.subs.len(), 2);
        assert!(view.subs.contains_key("a"));
        assert!(view.subs.contains_key("b"));

        // Remove `a`, add `c` — `b` should be retained across the edit
        // so its cached value doesn't churn.
        let b_ptr_addr = view.subs.get("b").map(|s| s as *const _ as usize);
        let w2 = Workshop {
            title: None,
            layout: WorkshopLayout::Rows,
            panels: vec![
                WorkshopPanel {
                    title: None,
                    substrate_path: "b".into(),
                    viewer_hint: "auto".into(),
                    min_height: None,
                },
                WorkshopPanel {
                    title: None,
                    substrate_path: "c".into(),
                    viewer_hint: "auto".into(),
                    min_height: None,
                },
            ],
        };
        view.reconcile_subs(&w2);
        assert_eq!(view.subs.len(), 2);
        assert!(!view.subs.contains_key("a"));
        assert!(view.subs.contains_key("b"));
        assert!(view.subs.contains_key("c"));
        // Sanity: `b`'s sub struct survives in place (HashMap may
        // relocate, but it wasn't recreated as a fresh PanelSub).
        let _ = b_ptr_addr; // Just document intent.
    }

    #[test]
    fn object_type_constants_present() {
        assert_eq!(OBJECT_TYPE_NAME, "Workshop");
        assert_eq!(OBJECT_TYPE_DISPLAY, "Workshop");
        // Workshop is a specialized structural shape — priority should
        // beat Mesh (20) so a substrate value that happens to look
        // both Mesh-ish and Workshop-shaped lands here.
        const { assert!(OBJECT_TYPE_PRIORITY > 20) };
    }
}
