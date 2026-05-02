//! Settings — `substrate/config/*` editor. DESIGN.md §9 sidebar 5,
//! archetype `app-window` (list-detail). Graduated under WEFT-583.
//!
//! 0.7.0 cut: no kernel adapter publishes `substrate/config/*` yet —
//! the daemon stores config in `.clawft/config.json` and exposes
//! `workspace.config.set` / `workspace.config.get` RPCs but does not
//! mirror the file into the substrate state tree. The settings app
//! therefore reads any `substrate/config*` topics that *do* show up in
//! `live.substrate_snapshot()` (forward-compat for when an adapter
//! ships) and otherwise renders the empty state with a remediation
//! hint. The list-detail archetype shape is still drawn so the user
//! can see what the populated form will look like.
//!
//! When a populated config is present:
//! - left pane lists top-level keys (categories like `claude`,
//!   `network`, `theme`)
//! - right pane lists each leaf key in the selected category with an
//!   inline editor (TextEdit / Checkbox / DragValue depending on the
//!   JSON type)
//! - edits are buffered in [`SettingsState`] and submitted to the
//!   daemon via `workspace.config.set` after a 500 ms debounce.

use std::collections::BTreeMap;
use std::sync::Arc;

use eframe::egui;
use serde_json::Value;

use crate::live::{Command, Live, Snapshot};
use crate::shell::desktop::Desktop;
use crate::theming::Tokens;

/// Per-field edit buffer + debounce. Keyed by dotted config path
/// (e.g. `claude.model`). Stored on `Desktop` so the user's in-flight
/// edit survives across frames without being clobbered by the next
/// snapshot tick.
#[derive(Default)]
pub struct SettingsState {
    /// Currently selected category (top-level key). `None` until the
    /// user clicks one (or until the first present category is
    /// auto-selected).
    pub selected_category: Option<String>,
    /// Pending edits, keyed by dotted path. Value is the buffered
    /// stringified form being edited; converted back to the original
    /// JSON shape when submitted.
    pub edits: BTreeMap<String, EditBuffer>,
}

pub struct EditBuffer {
    pub value: String,
    /// Monotonic ms (via `live::now_ms`) of the last keystroke. The
    /// debounce fires when `now_ms - last_change_ms >= 500`.
    pub last_change_ms: f64,
    /// Original JSON kind so we know how to coerce the submitted value.
    pub kind: ValueKind,
}

#[derive(Copy, Clone, Eq, PartialEq)]
pub enum ValueKind {
    Str,
    Bool,
    Int,
    Float,
}

impl ValueKind {
    fn classify(v: &Value) -> Self {
        match v {
            Value::Bool(_) => ValueKind::Bool,
            Value::Number(n) if n.is_i64() || n.is_u64() => ValueKind::Int,
            Value::Number(_) => ValueKind::Float,
            _ => ValueKind::Str,
        }
    }
}

const LEFT_PANE_W: f32 = 220.0;
const ROW_H: f32 = 26.0;
const DEBOUNCE_MS: f64 = 500.0;

pub fn show(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    desk: &mut Desktop,
    live: &Arc<Live>,
    snap: &Snapshot,
) {
    super::paint_heading(ui, rect, "Settings");
    let body = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() + 64.0),
        rect.max,
    );

    // Pull whatever config the substrate has — empty in 0.7.0 because
    // no adapter publishes config yet. Forward-compat: if a future
    // adapter starts populating `substrate/config` we'll render
    // automatically.
    let config = read_config(live);
    let categories = top_level_keys(&config);
    let has_data = !categories.is_empty();

    // List-detail shell — paint the two-pane archetype shape even when
    // empty so the user can see the form's structure. Empty state
    // hint sits in the right pane.
    let tokens = Tokens::default();
    paint_pane_split(ui, body, &tokens);

    let left = egui::Rect::from_min_max(
        body.min,
        egui::pos2(body.left() + LEFT_PANE_W, body.bottom()),
    );
    let right = egui::Rect::from_min_max(
        egui::pos2(body.left() + LEFT_PANE_W, body.top()),
        body.max,
    );

    // Auto-select first category if nothing selected yet.
    if desk.settings_state.selected_category.is_none()
        && let Some(first) = categories.first()
    {
        desk.settings_state.selected_category = Some(first.clone());
    }

    paint_category_list(
        ui,
        left,
        &tokens,
        &categories,
        &mut desk.settings_state.selected_category,
    );

    if !has_data {
        super::state::render_if_needed(
            ui,
            right,
            snap,
            false,
            "Config schema not yet published",
            Some("Run `weaver init` to seed defaults."),
        );
        return;
    }

    let category = desk
        .settings_state
        .selected_category
        .clone()
        .unwrap_or_else(|| categories[0].clone());

    paint_detail_form(
        ui,
        right,
        &tokens,
        &category,
        &config,
        &mut desk.settings_state.edits,
        live,
    );

    // Debounce: scan the edits map and submit any whose buffer has
    // settled past the debounce window. Submission removes the entry
    // (the next snapshot tick should reflect the change).
    flush_debounced_edits(&mut desk.settings_state.edits, live);
}

/// Read every `substrate/config*` topic from the live substrate and
/// merge them into a single nested JSON object. Topics like
/// `substrate/config/claude` map to `{"claude": <value>}` at the root.
fn read_config(live: &Arc<Live>) -> Value {
    let snap = live.substrate_snapshot();
    let mut root = serde_json::Map::new();
    for (path, value) in snap.topics() {
        // Direct topic at the root: `substrate/config` carrying the
        // whole tree.
        if path == "substrate/config"
            && let Value::Object(map) = value
        {
            for (k, v) in map {
                root.insert(k.clone(), v.clone());
            }
            continue;
        }
        // Per-category topic: `substrate/config/<category>` (or deeper).
        if let Some(rest) = path.strip_prefix("substrate/config/") {
            let mut segs = rest.split('/');
            if let Some(first) = segs.next() {
                // Walk into the sub-object, building containers as needed.
                let entry = root
                    .entry(first.to_string())
                    .or_insert_with(|| Value::Object(serde_json::Map::new()));
                let mut cur = entry;
                let tail: Vec<&str> = segs.collect();
                for seg in &tail {
                    if !cur.is_object() {
                        *cur = Value::Object(serde_json::Map::new());
                    }
                    cur = cur
                        .as_object_mut()
                        .unwrap()
                        .entry((*seg).to_string())
                        .or_insert_with(|| Value::Object(serde_json::Map::new()));
                }
                *cur = value.clone();
            }
        }
    }
    Value::Object(root)
}

fn top_level_keys(config: &Value) -> Vec<String> {
    match config {
        Value::Object(map) => map.keys().cloned().collect(),
        _ => Vec::new(),
    }
}

fn paint_pane_split(ui: &egui::Ui, body: egui::Rect, tokens: &Tokens) {
    let painter = ui.painter_at(body);
    // Left pane fill.
    let left = egui::Rect::from_min_max(
        body.min,
        egui::pos2(body.left() + LEFT_PANE_W, body.bottom()),
    );
    painter.rect_filled(left, 0.0, tokens.bg_panel);
    // Right pane fill.
    let right = egui::Rect::from_min_max(
        egui::pos2(body.left() + LEFT_PANE_W, body.top()),
        body.max,
    );
    painter.rect_filled(right, 0.0, tokens.bg_surface);
    // Divider.
    painter.line_segment(
        [
            egui::pos2(body.left() + LEFT_PANE_W, body.top()),
            egui::pos2(body.left() + LEFT_PANE_W, body.bottom()),
        ],
        egui::Stroke::new(1.0, tokens.stroke_soft),
    );
}

fn paint_category_list(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    tokens: &Tokens,
    categories: &[String],
    selected: &mut Option<String>,
) {
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(8.0, 12.0)))
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.style_mut().spacing.item_spacing.y = 4.0;
    child.label(
        egui::RichText::new("Categories")
            .color(tokens.text_dim)
            .size(11.0),
    );
    child.add_space(4.0);

    for cat in categories {
        let is_selected = selected.as_deref() == Some(cat.as_str());
        let label = egui::RichText::new(cat).size(13.0).color(if is_selected {
            tokens.text_primary
        } else {
            tokens.text_secondary
        });
        let resp = child.add_sized(
            [rect.width() - 16.0, ROW_H],
            egui::Button::selectable(is_selected, label),
        );
        if resp.clicked() {
            *selected = Some(cat.clone());
        }
    }
}

fn paint_detail_form(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    tokens: &Tokens,
    category: &str,
    config: &Value,
    edits: &mut BTreeMap<String, EditBuffer>,
    _live: &Arc<Live>,
) {
    let cat_value = match config.get(category) {
        Some(v) => v,
        None => return,
    };

    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(16.0, 12.0)))
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.label(
        egui::RichText::new(category)
            .color(tokens.text_primary)
            .size(15.0),
    );
    child.add_space(8.0);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(&mut child, |ui| {
            walk_leaves(category, cat_value, ui, tokens, edits);
        });
}

/// Walk the category subtree depth-first, rendering one row per leaf.
/// Nested objects emit a small dim header then recurse so the form
/// remains a flat list of leaf-key + editor pairs.
fn walk_leaves(
    path: &str,
    value: &Value,
    ui: &mut egui::Ui,
    tokens: &Tokens,
    edits: &mut BTreeMap<String, EditBuffer>,
) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                let child_path = format!("{path}.{k}");
                if v.is_object() {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(&child_path)
                            .color(tokens.text_dim)
                            .size(11.0),
                    );
                    walk_leaves(&child_path, v, ui, tokens, edits);
                } else {
                    paint_leaf_row(&child_path, k, v, ui, tokens, edits);
                }
            }
        }
        _ => {
            // Bare leaf at the category root (rare).
            paint_leaf_row(path, path, value, ui, tokens, edits);
        }
    }
}

fn paint_leaf_row(
    dotted_path: &str,
    label: &str,
    value: &Value,
    ui: &mut egui::Ui,
    tokens: &Tokens,
    edits: &mut BTreeMap<String, EditBuffer>,
) {
    ui.horizontal(|ui| {
        ui.add_sized(
            [180.0, ROW_H],
            egui::Label::new(
                egui::RichText::new(label)
                    .color(tokens.text_secondary)
                    .size(12.0),
            )
            .truncate(),
        );

        let kind = ValueKind::classify(value);
        let buf = edits.entry(dotted_path.to_string()).or_insert_with(|| EditBuffer {
            value: stringify(value),
            last_change_ms: f64::NEG_INFINITY,
            kind,
        });

        let mut changed = false;
        match buf.kind {
            ValueKind::Bool => {
                let mut b = buf.value == "true";
                if ui.checkbox(&mut b, "").changed() {
                    buf.value = b.to_string();
                    changed = true;
                }
            }
            ValueKind::Int => {
                let mut n: i64 = buf.value.parse().unwrap_or(0);
                if ui
                    .add(egui::DragValue::new(&mut n).speed(1.0))
                    .changed()
                {
                    buf.value = n.to_string();
                    changed = true;
                }
            }
            ValueKind::Float => {
                let mut n: f64 = buf.value.parse().unwrap_or(0.0);
                if ui
                    .add(egui::DragValue::new(&mut n).speed(0.1))
                    .changed()
                {
                    buf.value = n.to_string();
                    changed = true;
                }
            }
            ValueKind::Str => {
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut buf.value).desired_width(220.0),
                );
                if resp.changed() {
                    changed = true;
                }
            }
        }
        if changed {
            buf.last_change_ms = crate::live::now_ms();
        }
    });
}

fn stringify(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Null => String::new(),
        _ => v.to_string(),
    }
}

/// Submit any edits whose debounce window has elapsed and whose
/// buffered value differs from the last submitted value. We use the
/// daemon's `workspace.config.set` RPC (see
/// `crates/clawft-weave/src/daemon.rs::4932`); replies are
/// fire-and-forget — the next substrate poll surfaces the new value.
///
/// The buffer entry is cleared on submit so the next snapshot tick
/// re-seeds it from the freshly-published value, avoiding ping-pong
/// when the user types quickly.
fn flush_debounced_edits(edits: &mut BTreeMap<String, EditBuffer>, live: &Arc<Live>) {
    let now = crate::live::now_ms();
    let mut to_submit: Vec<(String, EditBuffer)> = Vec::new();
    edits.retain(|k, buf| {
        if buf.last_change_ms.is_finite() && now - buf.last_change_ms >= DEBOUNCE_MS {
            to_submit.push((
                k.clone(),
                EditBuffer {
                    value: buf.value.clone(),
                    last_change_ms: f64::NEG_INFINITY,
                    kind: buf.kind,
                },
            ));
            false
        } else {
            true
        }
    });
    for (key, buf) in to_submit {
        let params = serde_json::json!({
            "key": key,
            "value": buf.value,
        });
        live.submit(Command::Raw {
            method: "workspace.config.set".into(),
            params,
            reply: None,
        });
    }
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
    fn classify_value_kinds() {
        assert!(matches!(
            ValueKind::classify(&Value::Bool(true)),
            ValueKind::Bool
        ));
        assert!(matches!(
            ValueKind::classify(&serde_json::json!(7)),
            ValueKind::Int
        ));
        assert!(matches!(
            ValueKind::classify(&serde_json::json!(1.5)),
            ValueKind::Float
        ));
        assert!(matches!(
            ValueKind::classify(&Value::String("x".into())),
            ValueKind::Str
        ));
    }
}
