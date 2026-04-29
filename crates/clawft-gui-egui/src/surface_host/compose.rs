//! Composer runtime — walk a [`SurfaceTree`] and drive the canon
//! primitives in this crate. This is the hot path from ADR-016 §4
//! ("Frame time").
//!
//! For M1.5 the wiring covers:
//! - `ui://stack`, `ui://strip`, `ui://grid` containers.
//! - `ui://pressable`, `ui://chip`, `ui://gauge`, `ui://table`,
//!   `ui://stream-view` leaves.
//!
//! Every other canon IRI falls through to [`render_todo`] which
//! paints a visible `"TODO: <iri> not wired in M1.5"` label so the
//! surface still renders without a panic. Sibling M1.6+ milestones
//! light these up one by one.
//!
//! **M1.5.1a**: affordances declared on a `ui://table` or `ui://gauge`
//! are wired end-to-end. A row click on a table with an affordance, or
//! a click on a gauge's affordance button, produces a
//! [`PendingDispatch`] in the returned [`ComposeOutcome`] which the
//! desktop shell drains and submits via the `Live` RPC bridge. For a
//! table row, `params.pid` is pulled from the clicked row's `pid`
//! field; for a gauge, `params.name` is the last path segment of the
//! node id. The fixture's `rpc.` verb prefix is stripped before
//! dispatch so `rpc.kernel.kill-process` reaches the daemon as
//! `kernel.kill-process` (matching the extension's ALLOWED_METHODS
//! allowlist + the daemon handlers added in M1.5.1a).
//!
//! Governance intersection (ADR-006 rule 2) is still stubbed: every
//! node receives `variant_id = 0` and affordances are passed through
//! unfiltered. The honest GEPA-gated intersection lands with M2's
//! active-radar loop — at that point [`honest_affordances`] grows a
//! real implementation and the composer stops dispatching verbs the
//! gate would have denied.

use clawft_surface::eval::{eval_binding, Value};
use clawft_surface::substrate::OntologySnapshot;
use clawft_surface::tree::{AffordanceDecl, AttrValue, IdentityIri, SurfaceNode, SurfaceTree};

use crate::canon::{
    pressable::PressableStyle, CanonResponse, CanonWidget, CellSize, Chip, ChipTone, Field,
    FieldKind, FieldValue, Gauge, Grid, Pressable, Sheet, Slider, Stack, StackAxis, StreamView,
    Strip, StripAxis, Table, TableColumn, Tabs, Toggle,
};

/// A verb activation picked up by the composer during a frame. The
/// caller (the desktop shell's admin-app render path) drains these
/// after `compose()` returns and submits them through the `Live` RPC
/// bridge.
///
/// The shape is deliberately flat + serde-friendly so the desktop
/// doesn't have to know which primitive produced the dispatch — just
/// the verb string and the param object.
#[derive(Debug, Clone)]
pub struct PendingDispatch {
    /// Surface-tree path of the node whose affordance fired (e.g.
    /// `/root/processes`).
    pub source_path: String,
    /// Declared affordance name (e.g. `kill`, `restart`).
    pub affordance: String,
    /// RPC method already normalized — the `rpc.` prefix has been
    /// stripped so this maps directly to a daemon handler name.
    pub verb: String,
    /// JSON params for the RPC call. Shape is verb-specific
    /// (`{"pid": <u64>}` for `kernel.kill-process`,
    /// `{"name": <str>}` for `kernel.restart-service`).
    pub params: serde_json::Value,
}

/// Per-frame output from [`compose`]. The composer collects both the
/// flat list of [`CanonResponse`]s (for the observation walker) and
/// the list of affordance dispatches (for the RPC bridge). Both are
/// in depth-first surface-tree order.
#[derive(Debug, Default)]
pub struct ComposeOutcome {
    pub responses: Vec<CanonResponse>,
    pub dispatches: Vec<PendingDispatch>,
}

/// Internal call frame — every recursive `render_*` takes this
/// together so affordance-emitting primitives can push dispatches
/// alongside their CanonResponse without plumbing a second argument
/// through every signature.
///
/// **WEFT-249**: Uses `&mut Vec` rather than `RefCell<Vec>` so the
/// borrow checker statically rejects re-entrant pushes during a
/// child render. The previous shape was a deadlock-class bug — a
/// container's `body` callback could trigger a nested
/// `render_node` that would itself try to `borrow_mut` the same
/// `RefCell`, panicking on overlap. The cell-based shape worked
/// only because no caller ever held the borrow across a
/// `render_node` call; this invariant is now compiler-enforced.
///
/// Containers that pass an `&mut egui::Ui` body callback into a
/// canon widget (`Stack`, `Strip`, `Grid`) cannot put a
/// `&mut Frame` directly inside the callback because the callback
/// is `'static` against the frame's lifetime. We work around that
/// by collecting a per-container child outcome (responses +
/// dispatches) inside the closure and merging it back into the
/// parent frame after the closure returns. See `render_stack` /
/// `render_strip` / `render_grid` for the merge sites.
struct Frame<'a> {
    responses: &'a mut Vec<CanonResponse>,
    dispatches: &'a mut Vec<PendingDispatch>,
}

impl<'a> Frame<'a> {
    fn push_response(&mut self, r: CanonResponse) {
        self.responses.push(r);
    }
    fn push_dispatch(&mut self, d: PendingDispatch) {
        self.dispatches.push(d);
    }
    /// Drain a child outcome into this frame. Used by container
    /// renderers whose body closure captured a child frame in its
    /// own buffers.
    fn merge(&mut self, mut child: ComposeOutcome) {
        self.responses.append(&mut child.responses);
        self.dispatches.append(&mut child.dispatches);
    }
}

/// Main entry point. Walks `tree.root` and drives primitives. Returns
/// a [`ComposeOutcome`] with the flat response list + any pending
/// RPC dispatches produced by affordance activations this frame.
pub fn compose(
    tree: &SurfaceTree,
    snapshot: &OntologySnapshot,
    ui: &mut egui::Ui,
) -> ComposeOutcome {
    let mut responses: Vec<CanonResponse> = Vec::new();
    let mut dispatches: Vec<PendingDispatch> = Vec::new();
    let mut frame = Frame {
        responses: &mut responses,
        dispatches: &mut dispatches,
    };
    // Debug-only re-entrancy guard. The `&mut Vec` shape already
    // makes re-entrant `compose` calls a compile error, but we still
    // want a runtime assertion if the egui body closure loop ever
    // tries to recurse through `compose` itself rather than
    // `render_node`. The guard fires only in debug builds.
    debug_assert!(
        !COMPOSE_REENTRANCY_GUARD.with(|c| c.get()),
        "compose() called re-entrantly — see WEFT-249"
    );
    COMPOSE_REENTRANCY_GUARD.with(|c| c.set(true));
    render_node(&tree.root, snapshot, ui, &mut frame);
    COMPOSE_REENTRANCY_GUARD.with(|c| c.set(false));
    ComposeOutcome {
        responses,
        dispatches,
    }
}

#[cfg(debug_assertions)]
thread_local! {
    static COMPOSE_REENTRANCY_GUARD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(not(debug_assertions))]
thread_local! {
    static COMPOSE_REENTRANCY_GUARD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn render_node(
    node: &SurfaceNode,
    snap: &OntologySnapshot,
    ui: &mut egui::Ui,
    frame: &mut Frame<'_>,
) {
    // Conditional rendering (ADR-016 §6).
    if let Some(when) = &node.when {
        match eval_binding(when, snap) {
            Ok(v) => {
                if !v.as_bool().unwrap_or(false) {
                    return;
                }
            }
            Err(e) => {
                ui.colored_label(
                    egui::Color32::from_rgb(220, 80, 80),
                    format!("when-expr error at {}: {}", node.path, e),
                );
                return;
            }
        }
    }

    match node.kind {
        IdentityIri::Stack => render_stack(node, snap, ui, frame),
        IdentityIri::Strip => render_strip(node, snap, ui, frame),
        IdentityIri::Grid => render_grid(node, snap, ui, frame),
        IdentityIri::Chip => render_chip(node, snap, ui, frame),
        IdentityIri::Pressable => render_pressable(node, snap, ui, frame),
        IdentityIri::Gauge => render_gauge(node, snap, ui, frame),
        IdentityIri::Table => render_table(node, snap, ui, frame),
        IdentityIri::StreamView => render_stream_view(node, snap, ui, frame),
        IdentityIri::Heatmap => render_heatmap(node, snap, ui, frame),
        IdentityIri::Waveform => render_waveform(node, snap, ui, frame),
        // WEFT-244: ui://field — text/number/choice input.
        IdentityIri::Field => render_field(node, snap, ui, frame),
        // WEFT-245: the 10 remaining canon IRIs.
        IdentityIri::Toggle => render_toggle(node, snap, ui, frame),
        IdentityIri::Select => render_select(node, snap, ui, frame),
        IdentityIri::Slider => render_slider(node, snap, ui, frame),
        IdentityIri::Sheet => render_sheet(node, snap, ui, frame),
        IdentityIri::Modal => render_modal(node, snap, ui, frame),
        IdentityIri::Dock => render_dock(node, snap, ui, frame),
        IdentityIri::Tabs => render_tabs(node, snap, ui, frame),
        // ui://tree (canon "list") — hierarchical disclosure.
        IdentityIri::Tree => render_tree(node, snap, ui, frame),
        // Plot ≈ canon "menu" placement (ADR-016 §4 — readonly view
        // primitive with a small affordance strip). Plot is also the
        // closest canon to the requested `menu` IRI; expose both so
        // surfaces declaring `ui://plot` render rather than
        // fall-through to the TODO label.
        IdentityIri::Plot => render_plot(node, snap, ui, frame),
        other => render_todo(other, &node.path, ui),
    }
}

// ── Containers ─────────────────────────────────────────────────────

fn render_stack(
    node: &SurfaceNode,
    snap: &OntologySnapshot,
    ui: &mut egui::Ui,
    frame: &mut Frame<'_>,
) {
    let axis = attr_str(node, "axis")
        .and_then(|s| match s {
            "horizontal" => Some(StackAxis::Horizontal),
            "vertical" => Some(StackAxis::Vertical),
            _ => None,
        })
        .unwrap_or(StackAxis::Vertical);
    let wrap = attr_bool(node, "wrap").unwrap_or(false);
    let children = &node.children;

    // Container body buffers — live for the duration of the closure
    // and are merged into the parent frame after the widget returns.
    // WEFT-249: the explicit pre/post structure means there is no
    // re-entrant access to the parent frame's buffers.
    let mut child = ComposeOutcome::default();

    let stack = Stack::new(&node.path).axis(axis).wrap(wrap).body(
        |ui: &mut egui::Ui| {
            let mut child_frame = Frame {
                responses: &mut child.responses,
                dispatches: &mut child.dispatches,
            };
            for c in children {
                render_node(c, snap, ui, &mut child_frame);
            }
        },
    );
    let resp = stack.show(ui);
    frame.merge(child);
    frame.push_response(resp);
}

fn render_strip(
    node: &SurfaceNode,
    snap: &OntologySnapshot,
    ui: &mut egui::Ui,
    frame: &mut Frame<'_>,
) {
    let axis = attr_str(node, "axis")
        .and_then(|s| match s {
            "horizontal" => Some(StripAxis::Horizontal),
            "vertical" => Some(StripAxis::Vertical),
            _ => None,
        })
        .unwrap_or(StripAxis::Horizontal);

    let cells: Vec<CellSize> = (0..node.children.len()).map(|_| CellSize::Remainder).collect();
    let children = &node.children;

    let mut child = ComposeOutcome::default();

    let strip = Strip::new(&node.path).axis(axis).cells(cells).body(
        |strip: &mut egui_extras::Strip<'_, '_>| {
            let mut child_frame = Frame {
                responses: &mut child.responses,
                dispatches: &mut child.dispatches,
            };
            for c in children {
                strip.cell(|ui| {
                    render_node(c, snap, ui, &mut child_frame);
                });
            }
        },
    );
    let resp = strip.show(ui);
    frame.merge(child);
    frame.push_response(resp);
}

fn render_grid(
    node: &SurfaceNode,
    snap: &OntologySnapshot,
    ui: &mut egui::Ui,
    frame: &mut Frame<'_>,
) {
    let columns = attr_int(node, "columns").unwrap_or(1).max(1) as usize;
    let children = &node.children;

    // Wrap each child in a framed card so the grid reads as
    // partitioned quadrants rather than a stream of orphan widgets.
    // M1.5.1a polish — the fixture is a 2×2 layout that was visually
    // collapsing in narrow webviews.
    let mut child = ComposeOutcome::default();
    let grid = Grid::new(&node.path, columns, |ui: &mut egui::Ui| {
        let mut child_frame = Frame {
            responses: &mut child.responses,
            dispatches: &mut child.dispatches,
        };
        for (i, c) in children.iter().enumerate() {
            egui::Frame::group(ui.style())
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    render_node(c, snap, ui, &mut child_frame);
                });
            if (i + 1) % columns == 0 {
                ui.end_row();
            }
        }
    });
    let resp = grid.show(ui);
    frame.merge(child);
    frame.push_response(resp);
}

// ── Leaves ─────────────────────────────────────────────────────────

fn render_chip(
    node: &SurfaceNode,
    snap: &OntologySnapshot,
    ui: &mut egui::Ui,
    frame: &mut Frame<'_>,
) {
    let label = bound_string(node, "label", snap).unwrap_or_else(|| node.path.clone());
    let tone = bound_string(node, "tone", snap)
        .and_then(|s| tone_from_str(&s))
        .unwrap_or(ChipTone::Neutral);

    let mut chip = Chip::new(&node.path, label).tone(tone).variant(0);
    if !node.affordances.is_empty() {
        chip = chip.activatable(true);
    }
    let resp = chip.show(ui);
    if resp.inner.clicked()
        && let Some(dispatch) = build_dispatch(node, None)
    {
        frame.push_dispatch(dispatch);
    }
    frame.push_response(resp);
}

fn render_pressable(
    node: &SurfaceNode,
    snap: &OntologySnapshot,
    ui: &mut egui::Ui,
    frame: &mut Frame<'_>,
) {
    let label = bound_string(node, "label", snap).unwrap_or_else(|| node.path.clone());
    let style = attr_str(node, "style")
        .and_then(|s| match s {
            "primary" => Some(PressableStyle::Primary),
            "secondary" => Some(PressableStyle::Secondary),
            "ghost" => Some(PressableStyle::Ghost),
            "destructive" => Some(PressableStyle::Destructive),
            _ => None,
        })
        .unwrap_or(PressableStyle::Primary);
    let enabled = attr_bool(node, "enabled").unwrap_or(true);

    let p = Pressable::new(&node.path, label).style(style).enabled(enabled).variant(0);
    let resp = p.show(ui);
    if resp.inner.clicked()
        && let Some(dispatch) = build_dispatch(node, None)
    {
        frame.push_dispatch(dispatch);
    }
    frame.push_response(resp);
}

fn render_gauge(
    node: &SurfaceNode,
    snap: &OntologySnapshot,
    ui: &mut egui::Ui,
    frame: &mut Frame<'_>,
) {
    let value = bound_value(node, "value", snap).and_then(|v| v.as_f64()).unwrap_or(0.0);
    let lo = attr_number(node, "min").unwrap_or(0.0);
    let hi = attr_number(node, "max").unwrap_or(1.0);
    let label = bound_string(node, "label", snap);

    let mut g = Gauge::new(&node.path, value, (lo, hi)).variant(0);
    if let Some(l) = label {
        g = g.label(l);
    }
    let resp = g.show(ui);
    frame.push_response(resp);

    // If the node declares affordances, render a small action strip
    // underneath the gauge. Per-affordance click → dispatch is
    // collected in a local Vec inside the closure (egui's
    // `ui.horizontal` body cannot borrow `&mut frame`) and merged
    // afterward. WEFT-249.
    if !node.affordances.is_empty() {
        let mut local: Vec<PendingDispatch> = Vec::new();
        ui.horizontal(|ui| {
            for aff in &node.affordances {
                let label = prettify(&aff.name);
                if ui
                    .small_button(format!("↻ {label}"))
                    .on_hover_text(format!("{} — {}", aff.name, aff.verb))
                    .clicked()
                    && let Some(dispatch) = build_dispatch(node, Some(aff))
                {
                    local.push(dispatch);
                }
            }
        });
        for d in local {
            frame.push_dispatch(d);
        }
    }
}

fn render_stream_view(
    node: &SurfaceNode,
    snap: &OntologySnapshot,
    ui: &mut egui::Ui,
    frame: &mut Frame<'_>,
) {
    let lines: Vec<String> = bound_value(node, "stream", snap)
        .and_then(|v| v.as_list())
        .map(|xs| xs.into_iter().map(|v| v.to_display_string()).collect())
        .unwrap_or_default();

    let width = ui.available_width();
    let sv = StreamView::new(&node.path)
        .lines(&lines)
        .desired_width(width)
        .variant(0);
    frame.push_response(sv.show(ui));
}

fn render_table(
    node: &SurfaceNode,
    snap: &OntologySnapshot,
    ui: &mut egui::Ui,
    frame: &mut Frame<'_>,
) {
    let columns: Vec<TableColumn> = node
        .attrs
        .get("columns")
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|a| a.as_str())
                .map(|s| TableColumn::new(s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let rows: Vec<Value> = bound_value(node, "rows", snap).and_then(|v| v.as_list()).unwrap_or_default();
    let row_count = rows.len();

    let col_keys: Vec<String> = columns.iter().map(|c| c.name.to_string()).collect();

    // Row-click state is collected via the first cell of each row as a
    // selectable label. The primary cell is whichever column contains
    // the primary key (`pid` for kernel.ps, typically first column).
    // `Cell<Option<usize>>` rather than `RefCell` — we only ever
    // overwrite the slot, never observe through a borrow that
    // outlives an inner write. Keeps the re-entrancy story trivial.
    // WEFT-249.
    let clicked_row: std::cell::Cell<Option<usize>> = std::cell::Cell::new(None);

    let t = Table::new(&node.path, &columns)
        .rows(row_count)
        .variant(0)
        .render(|row, idx| {
            if let Some(val) = rows.get(idx) {
                for (i, key) in col_keys.iter().enumerate() {
                    row.col(|ui| {
                        let cell = val.field(key);
                        let text = cell.to_display_string();
                        if i == 0 && !node.affordances.is_empty() {
                            // First column is the click target when
                            // the table declares row-level affordances.
                            if ui.selectable_label(false, text).clicked() {
                                clicked_row.set(Some(idx));
                            }
                        } else {
                            ui.label(text);
                        }
                    });
                }
            }
        });
    let (resp, _outcome) = t.show_with_outcome(ui);
    frame.push_response(resp);

    // If a row was clicked and the node has a row-level affordance,
    // extract the row's `pid` (or the first column's value if there's
    // no `pid` field) and dispatch. The params shape is per-verb — for
    // `kernel.kill-process` the daemon expects `{"pid": u64}`.
    if let Some(idx) = clicked_row.get()
        && let Some(aff) = node.affordances.first()
        && let Some(row_val) = rows.get(idx)
    {
        let params = row_params_for(&aff.verb, row_val);
        let verb = strip_rpc_prefix(&aff.verb);
        frame.push_dispatch(PendingDispatch {
            source_path: node.path.clone(),
            affordance: aff.name.clone(),
            verb,
            params,
        });
    }
}

// ── Sensor-oriented leaves ─────────────────────────────────────────
//
// `ui://heatmap` and `ui://waveform` are added outside the ADR-001
// canonical 21 primitives. They're both read-only visualisations
// that pull a numeric array from a binding and render it directly:
//
// - `heatmap` expects either a flat `values: [f64; w*h]` (paired with
//   attrs `width`, `height`) **or** a full frame object `{width,
//   height, depths_mm, min_mm?, max_mm?}` via `values = "$path"`.
//   It auto-normalises over the observed range unless `min`/`max`
//   attrs override. 0xFFFF / 65535 in `depths_mm` is treated as
//   "no valid reading" (the VL53L5CX/L7CX sentinel) and renders grey.
//
// - `waveform` expects `samples: [f64; N]` + attr `range = [min, max]`
//   (or auto-scales). Draws a line plot of the N samples.

fn render_heatmap(
    node: &SurfaceNode,
    snap: &OntologySnapshot,
    ui: &mut egui::Ui,
    _frame: &mut Frame<'_>,
) {
    // Frame-object form: `values = "$substrate/sensor/tof"` resolves
    // to a `Value::Json(...)` holding the whole frame object. We also
    // support a flat-array form for synthetic / test cases where the
    // caller points `values` at a raw array and passes width/height
    // via attrs.
    let raw: Option<serde_json::Value> = node
        .bindings
        .get("values")
        .and_then(|b| clawft_surface::eval::eval_binding(b, snap).ok())
        .and_then(|v| match v {
            clawft_surface::eval::Value::Json(j) => Some(j),
            clawft_surface::eval::Value::List(xs) => Some(serde_json::Value::Array(
                xs.into_iter()
                    .map(|x| match x {
                        clawft_surface::eval::Value::Int(i) => serde_json::json!(i),
                        clawft_surface::eval::Value::Num(n) => serde_json::json!(n),
                        _ => serde_json::Value::Null,
                    })
                    .collect(),
            )),
            _ => None,
        });

    let (width, height, data, min_mm, max_mm) = match raw {
        Some(serde_json::Value::Object(map)) => {
            let w = map
                .get("width")
                .and_then(|v| v.as_u64())
                .unwrap_or(attr_int(node, "width").unwrap_or(8) as u64)
                as usize;
            let h = map
                .get("height")
                .and_then(|v| v.as_u64())
                .unwrap_or(attr_int(node, "height").unwrap_or(8) as u64)
                as usize;
            let data: Vec<u16> = map
                .get("depths_mm")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .map(|v| v.as_u64().unwrap_or(65535) as u16)
                        .collect()
                })
                .unwrap_or_default();
            let min_mm = map.get("min_mm").and_then(|v| v.as_u64()).map(|x| x as u16);
            let max_mm = map.get("max_mm").and_then(|v| v.as_u64()).map(|x| x as u16);
            (w, h, data, min_mm, max_mm)
        }
        Some(serde_json::Value::Array(arr)) => {
            let data: Vec<u16> = arr
                .iter()
                .map(|v| v.as_u64().unwrap_or(65535) as u16)
                .collect();
            let w = attr_int(node, "width").unwrap_or(8) as usize;
            let h = attr_int(node, "height").unwrap_or(8) as usize;
            (w, h, data, None, None)
        }
        _ => (
            attr_int(node, "width").unwrap_or(8) as usize,
            attr_int(node, "height").unwrap_or(8) as usize,
            Vec::new(),
            None,
            None,
        ),
    };

    if width == 0 || height == 0 || data.is_empty() || data.len() != width * height {
        ui.label(
            egui::RichText::new(format!(
                "heatmap: no/invalid data (w={width} h={height} len={})",
                data.len()
            ))
            .color(egui::Color32::from_rgb(160, 160, 170))
            .italics(),
        );
        return;
    }

    // Normalise.
    let (lo, hi) = match (min_mm, max_mm) {
        (Some(a), Some(b)) => (a, b),
        _ => {
            let mut min = u16::MAX;
            let mut max = 0u16;
            for d in &data {
                if *d == 65535 {
                    continue;
                }
                if *d < min { min = *d; }
                if *d > max { max = *d; }
            }
            if min == u16::MAX { (0, 1) } else if min == max { (min, min + 1) } else { (min, max) }
        }
    };

    let cell = attr_number(node, "cell_px").unwrap_or(28.0) as f32;
    let gap: f32 = 2.0;
    let total_w = width as f32 * cell + (width as f32 - 1.0) * gap;
    let total_h = height as f32 * cell + (height as f32 - 1.0) * gap;
    let (rect, _resp) =
        ui.allocate_exact_size(egui::vec2(total_w, total_h), egui::Sense::hover());
    let painter = ui.painter_at(rect);

    for row in 0..height {
        for col in 0..width {
            let idx = row * width + col;
            let raw_px = data[idx];
            let color = heatmap_color(raw_px, lo, hi);
            let x0 = rect.left() + col as f32 * (cell + gap);
            let y0 = rect.top() + row as f32 * (cell + gap);
            let cell_rect =
                egui::Rect::from_min_size(egui::pos2(x0, y0), egui::vec2(cell, cell));
            painter.rect_filled(cell_rect, 2.0, color);
        }
    }
    // Heatmap is read-only; no CanonResponse entry needed — nothing
    // downstream inspects a heatmap's response.
}

fn render_waveform(
    node: &SurfaceNode,
    snap: &OntologySnapshot,
    ui: &mut egui::Ui,
    _frame: &mut Frame<'_>,
) {
    let samples: Vec<f64> = bound_value(node, "samples", snap)
        .and_then(|v| v.as_list())
        .map(|xs| xs.into_iter().filter_map(|v| v.as_f64()).collect())
        .unwrap_or_default();

    let height_attr = attr_number(node, "height_px").unwrap_or(80.0) as f32;
    let width_attr = attr_number(node, "width_px").unwrap_or(280.0) as f32;

    let (rect, _resp) =
        ui.allocate_exact_size(egui::vec2(width_attr, height_attr), egui::Sense::hover());
    let painter = ui.painter_at(rect);

    painter.rect_filled(rect, 2.0, egui::Color32::from_rgb(18, 18, 24));

    if samples.len() < 2 {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "no samples",
            egui::FontId::proportional(11.0),
            egui::Color32::from_rgb(130, 130, 140),
        );
    } else {
        let (lo, hi) = attr_range(node).unwrap_or_else(|| {
            let mut mn = f64::INFINITY;
            let mut mx = f64::NEG_INFINITY;
            for s in &samples {
                if s.is_finite() {
                    if *s < mn { mn = *s; }
                    if *s > mx { mx = *s; }
                }
            }
            if !mn.is_finite() || !mx.is_finite() || (mx - mn).abs() < f64::EPSILON {
                (0.0, 1.0)
            } else {
                (mn, mx)
            }
        });
        let span = (hi - lo).max(1e-9);
        let mut points = Vec::with_capacity(samples.len());
        for (i, s) in samples.iter().enumerate() {
            let t = i as f32 / (samples.len() - 1).max(1) as f32;
            let x = rect.left() + t * rect.width();
            let normalised = ((s - lo) / span).clamp(0.0, 1.0) as f32;
            let y = rect.bottom() - normalised * rect.height();
            points.push(egui::pos2(x, y));
        }
        painter.add(egui::epaint::PathShape::line(
            points,
            egui::Stroke::new(1.5, egui::Color32::from_rgb(110, 200, 150)),
        ));
    }
    // Waveform is read-only; no CanonResponse entry.
}

/// Parse a `range = [min, max]` attr into `(f64, f64)`.
fn attr_range(node: &SurfaceNode) -> Option<(f64, f64)> {
    let arr = node.attrs.get("range").and_then(|v| match v {
        AttrValue::Array(items) => Some(items),
        _ => None,
    })?;
    if arr.len() != 2 {
        return None;
    }
    let lo = arr[0].as_number()?;
    let hi = arr[1].as_number()?;
    Some((lo, hi))
}

/// 5-stop viridis-ish colormap mirroring `shell/desktop.rs::tof_pixel_color`.
/// 0xFFFF (65535) is the "no valid reading" sentinel → mid-grey.
fn heatmap_color(mm: u16, min: u16, max: u16) -> egui::Color32 {
    if mm == 65535 {
        return egui::Color32::from_rgb(64, 64, 72);
    }
    let span = (max.saturating_sub(min)).max(1) as f32;
    let clamped = mm.clamp(min, max);
    let t = ((clamped - min) as f32 / span).clamp(0.0, 1.0);
    let stops = [
        (0.00_f32, [38u8, 18, 110]),
        (0.25, [30, 120, 200]),
        (0.50, [50, 200, 120]),
        (0.75, [220, 200, 60]),
        (1.00, [220, 70, 60]),
    ];
    for i in 0..stops.len() - 1 {
        let (t0, c0) = stops[i];
        let (t1, c1) = stops[i + 1];
        if t <= t1 {
            let local = ((t - t0) / (t1 - t0)).clamp(0.0, 1.0);
            let r = (c0[0] as f32 + (c1[0] as f32 - c0[0] as f32) * local) as u8;
            let g = (c0[1] as f32 + (c1[1] as f32 - c0[1] as f32) * local) as u8;
            let b = (c0[2] as f32 + (c1[2] as f32 - c0[2] as f32) * local) as u8;
            return egui::Color32::from_rgb(r, g, b);
        }
    }
    egui::Color32::from_rgb(220, 70, 60)
}

// ── M4-C extra IRI handlers ─────────────────────────────────────────
//
// WEFT-244 (`ui://field`) + WEFT-245 (the 10 remaining canon IRIs).
//
// Inputs (Field, Toggle, Select, Slider) read their initial value
// from the bound substrate path (or an `attrs` default), then keep
// the in-frame mutable copy in egui memory keyed by the surface
// node's path. When the user edits the widget and a declared
// affordance is present, we emit a `PendingDispatch` so the desktop
// shell can submit the canonical write verb. Without an affordance
// the widget is editable but the change is local to the panel —
// matches the chip / pressable behaviour for declared-but-unbound
// nodes.
//
// Containers (Sheet, Tabs) descend into children with the same
// pre/post merge pattern used by Stack / Strip / Grid; child
// dispatches bubble up.
//
// Modal / Dock are partially wired — the runtime shape needs a
// `&mut DockState` and `TabViewer` (Dock) or persistent open-state
// (Modal). For M4-C we render a labelled placeholder so the IRI
// stops falling through to `render_todo`; full surface IR support
// for these two ships in the M5 surface-state milestone.

/// Convenience for read-only "value as JSON-ish display string" so a
/// number / bool / string binding can drive a one-liner field
/// preview.
fn binding_to_string(node: &SurfaceNode, slot: &str, snap: &OntologySnapshot) -> String {
    bound_value(node, slot, snap)
        .map(|v| v.to_display_string())
        .unwrap_or_default()
}

fn render_field(
    node: &SurfaceNode,
    snap: &OntologySnapshot,
    ui: &mut egui::Ui,
    frame: &mut Frame<'_>,
) {
    // The `kind` attr selects the field flavour: `text` (default),
    // `multiline`, `password`, `number`, `choice`. For `choice` we
    // expect an `options` attr; for `number` we accept `min`,
    // `max`, `step`. Defaults are conservative — a missing attr
    // falls back to a free-form text input rather than an error.
    let kind_str = attr_str(node, "kind").unwrap_or("text");
    let placeholder: std::borrow::Cow<'static, str> = attr_str(node, "placeholder")
        .map(|s| std::borrow::Cow::Owned(s.to_string()))
        .unwrap_or(std::borrow::Cow::Borrowed(""));

    // Read the bound initial value (if any) once, so the widget
    // shows the substrate-tracked value on first paint. Subsequent
    // edits live in egui memory keyed by the node path.
    let initial = binding_to_string(node, "value", snap);

    let kind = match kind_str {
        "multiline" => FieldKind::multiline(placeholder.clone()),
        "password" => FieldKind::password(placeholder.clone()),
        "number" => {
            let lo = attr_number(node, "min").unwrap_or(0.0);
            let hi = attr_number(node, "max").unwrap_or(100.0);
            let step = attr_number(node, "step").unwrap_or(1.0);
            FieldKind::number(lo, hi, step)
        }
        // Choice falls back to text — the canon `Field::choice`
        // requires `&'static [&'static str]` options which we
        // can't materialise from runtime attrs without leaking.
        // Surfaces wanting choice should use `ui://select`.
        _ => FieldKind::text(placeholder.clone()),
    };

    let key = egui::Id::new(("compose.field.value", &node.path));
    let mut value: FieldValue = ui.ctx().memory_mut(|m| {
        m.data
            .get_temp_mut_or_insert_with::<FieldValue>(key, || match kind {
                FieldKind::Number { .. } => FieldValue::Number(initial.parse().unwrap_or(0.0)),
                _ => FieldValue::Text(initial.clone()),
            })
            .clone()
    });

    let enabled = attr_bool(node, "enabled").unwrap_or(true);
    let f = Field::new(&node.path, kind, &mut value).enabled(enabled).variant(0);
    let resp = f.show(ui);
    let changed = resp.inner.changed();
    // Persist the (possibly mutated) value back into egui memory.
    ui.ctx().memory_mut(|m| {
        m.data.insert_temp(key, value);
    });
    if changed
        && let Some(dispatch) = build_dispatch(node, None)
    {
        frame.push_dispatch(dispatch);
    }
    frame.push_response(resp);
}

fn render_toggle(
    node: &SurfaceNode,
    snap: &OntologySnapshot,
    ui: &mut egui::Ui,
    frame: &mut Frame<'_>,
) {
    let label = bound_string(node, "label", snap).unwrap_or_else(|| node.path.clone());
    // Initial value comes from the binding; subsequent flips are
    // egui-memory-keyed so a click doesn't bounce back on the next
    // frame before the substrate write round-trips.
    let initial =
        bound_value(node, "value", snap).and_then(|v| v.as_bool()).unwrap_or(false);
    let key = egui::Id::new(("compose.toggle.value", &node.path));
    let mut value = ui.ctx().memory_mut(|m| {
        *m.data.get_temp_mut_or_insert_with::<bool>(key, || initial)
    });
    let enabled = attr_bool(node, "enabled").unwrap_or(true);
    let t = Toggle::new(&node.path, label, &mut value).enabled(enabled).variant(0);
    let resp = t.show(ui);
    ui.ctx().memory_mut(|m| m.data.insert_temp(key, value));
    if resp.inner.changed()
        && let Some(dispatch) = build_dispatch(node, None)
    {
        frame.push_dispatch(dispatch);
    }
    frame.push_response(resp);
}

fn render_select(
    node: &SurfaceNode,
    snap: &OntologySnapshot,
    ui: &mut egui::Ui,
    frame: &mut Frame<'_>,
) {
    // The canon `Select` widget requires `&'static [&'static str]`
    // for options — runtime attrs can't satisfy that without a
    // leak. Render a labelled fall-back combo using `egui::ComboBox`
    // directly, then synthesise a CanonResponse for the observation
    // walker so the surface-host invariants hold.
    let label = bound_string(node, "label", snap).unwrap_or_else(|| "select".to_string());
    let options: Vec<String> = node
        .attrs
        .get("options")
        .and_then(|a| a.as_array())
        .map(|arr| arr.iter().filter_map(|a| a.as_str().map(String::from)).collect())
        .unwrap_or_default();

    let initial = binding_to_string(node, "value", snap);
    let key = egui::Id::new(("compose.select.value", &node.path));
    let mut current = ui.ctx().memory_mut(|m| {
        m.data
            .get_temp_mut_or_insert_with::<String>(key, || initial.clone())
            .clone()
    });
    let prev = current.clone();
    let resp = ui
        .horizontal(|ui| {
            ui.label(label);
            let r = egui::ComboBox::new(("compose.select.combo", &node.path), "")
                .selected_text(current.clone())
                .show_ui(ui, |ui| {
                    for opt in &options {
                        ui.selectable_value(&mut current, opt.clone(), opt);
                    }
                });
            r.response
        })
        .inner;

    let changed = current != prev;
    ui.ctx().memory_mut(|m| m.data.insert_temp(key, current));
    let synth = CanonResponse::from_egui(resp, std::borrow::Cow::Borrowed("ui://select"), 0, None);
    if changed
        && let Some(dispatch) = build_dispatch(node, None)
    {
        frame.push_dispatch(dispatch);
    }
    frame.push_response(synth);
}

fn render_slider(
    node: &SurfaceNode,
    snap: &OntologySnapshot,
    ui: &mut egui::Ui,
    frame: &mut Frame<'_>,
) {
    let label = bound_string(node, "label", snap).unwrap_or_else(|| node.path.clone());
    let lo = attr_number(node, "min").unwrap_or(0.0);
    let hi = attr_number(node, "max").unwrap_or(1.0);
    let step = attr_number(node, "step");
    let initial =
        bound_value(node, "value", snap).and_then(|v| v.as_f64()).unwrap_or(lo);
    let key = egui::Id::new(("compose.slider.value", &node.path));
    let mut value = ui.ctx().memory_mut(|m| {
        *m.data.get_temp_mut_or_insert_with::<f64>(key, || initial)
    });
    let enabled = attr_bool(node, "enabled").unwrap_or(true);
    let mut s = Slider::new(&node.path, label, &mut value, lo, hi)
        .enabled(enabled)
        .variant(0);
    if let Some(st) = step {
        s = s.step(st);
    }
    let resp = s.show(ui);
    ui.ctx().memory_mut(|m| m.data.insert_temp(key, value));
    if resp.inner.changed()
        && let Some(dispatch) = build_dispatch(node, None)
    {
        frame.push_dispatch(dispatch);
    }
    frame.push_response(resp);
}

fn render_sheet(
    node: &SurfaceNode,
    snap: &OntologySnapshot,
    ui: &mut egui::Ui,
    frame: &mut Frame<'_>,
) {
    let max_h = attr_number(node, "max_height").map(|v| v as f32);
    let stick = attr_bool(node, "stick_to_bottom").unwrap_or(false);
    let children = &node.children;

    let mut child = ComposeOutcome::default();
    let mut s = Sheet::new(&node.path, |ui: &mut egui::Ui| {
        let mut child_frame = Frame {
            responses: &mut child.responses,
            dispatches: &mut child.dispatches,
        };
        for c in children {
            render_node(c, snap, ui, &mut child_frame);
        }
    })
    .stick_to_bottom(stick)
    .variant(0);
    if let Some(h) = max_h {
        s = s.max_height(h);
    }
    let resp = s.show(ui);
    frame.merge(child);
    frame.push_response(resp);
}

fn render_modal(
    node: &SurfaceNode,
    _snap: &OntologySnapshot,
    ui: &mut egui::Ui,
    frame: &mut Frame<'_>,
) {
    // `ui://modal` needs a body callback (FnOnce<&mut egui::Ui>) and
    // a persistent open-state. Surface IR doesn't yet model an
    // open/dismiss handshake — for M4-C we render a labelled
    // placeholder so the IRI stops falling through to `render_todo`.
    // Full wiring (with a `Modality` attribute, body composition,
    // and dismiss-dispatch) ships with M5 surface state.
    let title = node
        .attrs
        .get("title")
        .and_then(AttrValue::as_str)
        .unwrap_or("modal");
    let resp = ui
        .group(|ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("[modal]").color(egui::Color32::from_rgb(180, 160, 110)));
                ui.label(title);
            });
            ui.label(
                egui::RichText::new(format!("({} children)", node.children.len()))
                    .small()
                    .color(egui::Color32::from_rgb(140, 140, 150)),
            );
        })
        .response;
    frame.push_response(CanonResponse::from_egui(
        resp,
        std::borrow::Cow::Borrowed("ui://modal"),
        0,
        None,
    ));
}

fn render_dock(
    node: &SurfaceNode,
    _snap: &OntologySnapshot,
    ui: &mut egui::Ui,
    frame: &mut Frame<'_>,
) {
    // `ui://dock` needs a `&mut DockState<Tab>` and `TabViewer`
    // implementation that the surface IR doesn't yet describe.
    // Render a labelled placeholder. Same milestone-cut as
    // `render_modal`.
    let resp = ui
        .group(|ui| {
            ui.label(egui::RichText::new("[dock]").color(egui::Color32::from_rgb(180, 160, 110)));
            ui.label(
                egui::RichText::new(format!("({} panes)", node.children.len()))
                    .small()
                    .color(egui::Color32::from_rgb(140, 140, 150)),
            );
        })
        .response;
    frame.push_response(CanonResponse::from_egui(
        resp,
        std::borrow::Cow::Borrowed("ui://dock"),
        0,
        None,
    ));
}

fn render_tabs(
    node: &SurfaceNode,
    snap: &OntologySnapshot,
    ui: &mut egui::Ui,
    frame: &mut Frame<'_>,
) {
    // Each child is one tab. Label is the child's `label` attr or
    // its last path segment.
    let labels_owned: Vec<String> = node
        .children
        .iter()
        .map(|c| {
            c.attrs
                .get("label")
                .and_then(AttrValue::as_str)
                .map(String::from)
                .unwrap_or_else(|| {
                    c.path
                        .rsplit('/')
                        .next()
                        .unwrap_or(&c.path)
                        .to_string()
                })
        })
        .collect();
    let labels: Vec<&str> = labels_owned.iter().map(|s| s.as_str()).collect();

    let key = egui::Id::new(("compose.tabs.selected", &node.path));
    let mut selected = ui.ctx().memory_mut(|m| {
        *m.data.get_temp_mut_or_insert_with::<usize>(key, || 0)
    });
    if selected >= node.children.len() {
        selected = 0;
    }

    let mut child = ComposeOutcome::default();
    let resp = if labels.is_empty() {
        let r = ui.label(
            egui::RichText::new("[tabs] (no children)")
                .color(egui::Color32::from_rgb(160, 160, 170))
                .italics(),
        );
        CanonResponse::from_egui(r, std::borrow::Cow::Borrowed("ui://tabs"), 0, None)
    } else {
        let children = &node.children;
        let t = Tabs::new(
            &node.path,
            &labels,
            &mut selected,
            |ui: &mut egui::Ui, idx: usize| {
                let mut child_frame = Frame {
                    responses: &mut child.responses,
                    dispatches: &mut child.dispatches,
                };
                if let Some(c) = children.get(idx) {
                    render_node(c, snap, ui, &mut child_frame);
                }
            },
        )
        .variant(0);
        t.show(ui)
    };

    ui.ctx().memory_mut(|m| m.data.insert_temp(key, selected));
    frame.merge(child);
    frame.push_response(resp);
}

fn render_tree(
    node: &SurfaceNode,
    snap: &OntologySnapshot,
    ui: &mut egui::Ui,
    frame: &mut Frame<'_>,
) {
    // The canon `Tree` widget owns its own state; for surface-IR
    // purposes we render a flat collapsing-list of the node's
    // children, descending the composer for each. This is the
    // "list" affordance from the canon spec — child nodes render
    // as nested items, with their children collapsed by default.
    // A real recursive `ui://tree` (per the canon's `TreeNode`
    // builder) needs a richer IR shape that's M5 work.
    let label = node
        .attrs
        .get("label")
        .and_then(AttrValue::as_str)
        .unwrap_or("list");
    let mut child = ComposeOutcome::default();
    let resp = egui::CollapsingHeader::new(label)
        .id_salt(("compose.tree", &node.path))
        .default_open(true)
        .show(ui, |ui| {
            let mut child_frame = Frame {
                responses: &mut child.responses,
                dispatches: &mut child.dispatches,
            };
            for c in &node.children {
                render_node(c, snap, ui, &mut child_frame);
            }
        })
        .header_response;
    frame.merge(child);
    frame.push_response(CanonResponse::from_egui(
        resp,
        std::borrow::Cow::Borrowed("ui://tree"),
        0,
        None,
    ));
}

fn render_plot(
    node: &SurfaceNode,
    snap: &OntologySnapshot,
    ui: &mut egui::Ui,
    frame: &mut Frame<'_>,
) {
    let series: Vec<(f64, f64)> = bound_value(node, "points", snap)
        .and_then(|v| v.as_list())
        .map(|xs| {
            xs.into_iter()
                .enumerate()
                .filter_map(|(i, v)| v.as_f64().map(|y| (i as f64, y)))
                .collect()
        })
        .unwrap_or_default();
    let p = crate::canon::Plot::new(&node.path).points(&series).variant(0);
    let resp = p.show(ui);
    frame.push_response(resp);
}

// ── TODO fallback ──────────────────────────────────────────────────

fn render_todo(kind: IdentityIri, path: &str, ui: &mut egui::Ui) {
    ui.label(
        egui::RichText::new(format!(
            "TODO: {} not wired in M1.5 ({})",
            kind.as_iri(),
            path
        ))
        .color(egui::Color32::from_rgb(200, 160, 60))
        .italics(),
    );
}

// ── Affordance dispatch helpers ─────────────────────────────────────

/// Strip the WSP namespace prefix so a fixture verb
/// `rpc.kernel.kill-process` dispatches as the daemon handler
/// `kernel.kill-process`. The daemon / extension allowlist use the
/// bare form.
fn strip_rpc_prefix(verb: &str) -> String {
    verb.strip_prefix("rpc.")
        .map(|s| s.to_string())
        .unwrap_or_else(|| verb.to_string())
}

/// Build the JSON params object for a verb that acts on a single
/// table row. Verb-specific extraction — for the M1.5.1a verbs:
/// `kernel.kill-process` pulls `pid` from the row.
fn row_params_for(verb: &str, row: &Value) -> serde_json::Value {
    let bare = strip_rpc_prefix(verb);
    match bare.as_str() {
        "kernel.kill-process" => {
            let pid = row.field("pid").as_i64().unwrap_or(0);
            serde_json::json!({ "pid": pid })
        }
        _ => serde_json::json!({}),
    }
}

/// Build a dispatch for a node whose primary affordance fires at the
/// node level (gauge button, chip activate, pressable click). Returns
/// `None` if the node has no affordances. For a specific affordance,
/// pass `Some(&aff)`; otherwise the first declared affordance is used.
fn build_dispatch(node: &SurfaceNode, aff: Option<&AffordanceDecl>) -> Option<PendingDispatch> {
    let aff = aff.or_else(|| node.affordances.first())?;
    let verb = strip_rpc_prefix(&aff.verb);
    let params = node_params_for(&aff.verb, node);
    Some(PendingDispatch {
        source_path: node.path.clone(),
        affordance: aff.name.clone(),
        verb,
        params,
    })
}

/// Params for a node-level affordance. For `kernel.restart-service`
/// the name comes from the last segment of the node path
/// (e.g. `/root/services/mesh-listener` → `"mesh-listener"`).
fn node_params_for(verb: &str, node: &SurfaceNode) -> serde_json::Value {
    let bare = strip_rpc_prefix(verb);
    match bare.as_str() {
        "kernel.restart-service" => {
            let name = node.path.rsplit('/').next().unwrap_or(&node.path);
            serde_json::json!({ "name": name })
        }
        _ => serde_json::json!({}),
    }
}

fn prettify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut first = true;
    for c in s.chars() {
        if first {
            out.extend(c.to_uppercase());
            first = false;
        } else {
            out.push(c);
        }
    }
    out
}

// ── Binding / attribute helpers ────────────────────────────────────

fn bound_value(node: &SurfaceNode, slot: &str, snap: &OntologySnapshot) -> Option<Value> {
    node.bindings.get(slot).and_then(|b| eval_binding(b, snap).ok())
}

fn bound_string(node: &SurfaceNode, slot: &str, snap: &OntologySnapshot) -> Option<String> {
    let v = bound_value(node, slot, snap)?;
    Some(v.to_display_string())
}

fn tone_from_str(s: &str) -> Option<ChipTone> {
    Some(match s {
        "ok" | "healthy" => ChipTone::Ok,
        "warn" | "at_risk" => ChipTone::Warn,
        "crit" | "down" | "error" => ChipTone::Crit,
        "info" => ChipTone::Info,
        "neutral" => ChipTone::Neutral,
        _ => return None,
    })
}

fn attr_str<'a>(node: &'a SurfaceNode, key: &str) -> Option<&'a str> {
    node.attrs.get(key).and_then(AttrValue::as_str)
}

fn attr_bool(node: &SurfaceNode, key: &str) -> Option<bool> {
    node.attrs.get(key).and_then(AttrValue::as_bool)
}

fn attr_int(node: &SurfaceNode, key: &str) -> Option<i64> {
    node.attrs.get(key).and_then(AttrValue::as_int)
}

fn attr_number(node: &SurfaceNode, key: &str) -> Option<f64> {
    node.attrs.get(key).and_then(AttrValue::as_number)
}

/// Exposed so a future governance pass can intersect affordances
/// against a policy. Currently identity — ADR-006 rule 2 TODO.
pub fn honest_affordances(_node: &SurfaceNode, raw: &[AffordanceDecl]) -> Vec<AffordanceDecl> {
    raw.to_vec()
}
