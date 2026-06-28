//! `ui://select` — closed-choice picker primitive (ADR-001 row 5).
//!
//! Session-5 §5 maps this to `egui::ComboBox` for ≤N and a
//! `TableBuilder`-driven scrollable picker for large option sets so
//! the dropdown doesn't grow taller than the viewport. The crossover
//! threshold is configurable via [`Select::table_threshold`]; the
//! default ([`DEFAULT_TABLE_THRESHOLD`]) keeps small Selects rendered
//! as the lightweight ComboBox form and switches to the table layout
//! once the option count grows past a typical screen of choices.
//! ADR-001 row 5 alignment: Select owns both forms — the Combo arm is
//! the small-N rendering and the Table arm is the large-N rendering;
//! `ui://table` remains a separate primitive for arbitrary row data
//! (it is not the canonical home for "large-set choice"). [WEFT-267]

use std::borrow::Cow;

use eframe::egui;
use egui_extras::{Column, TableBuilder};

use super::CanonWidget;
use super::response::CanonResponse;
use super::types::{Affordance, Confidence, IdentityUri, MutationAxis, Tooltip, VariantId};

const IDENTITY: &str = "ui://select";

/// Default crossover from the ComboBox form to the TableBuilder form.
/// Picked to comfortably exceed the visible-row count of an average
/// dropdown without forcing the table on small selectors. Override
/// per-instance via [`Select::table_threshold`].
pub const DEFAULT_TABLE_THRESHOLD: usize = 32;

/// Pixel height of a single row in the table form. Matches egui's
/// default body-row sizing — using a constant lets the row count
/// drive a stable popup height rather than reflowing per-frame.
const TABLE_ROW_HEIGHT: f32 = 22.0;

static AFFORDANCES_ACTIVE: &[Affordance] = &[Affordance {
    name: Cow::Borrowed("select"),
    verb: Cow::Borrowed("wsp.set"),
    actors: &[],
    args_schema: None,
    reorderable: false,
}];
static AFFORDANCES_DISABLED: &[Affordance] = &[];

static MUTATION_AXES: &[MutationAxis] = &[
    MutationAxis::new("options-order"),
    MutationAxis::new("default-choice"),
];

/// Closed-choice picker over a static option slice. The caller owns
/// the `&mut usize` index binding. Above
/// [`Select::table_threshold`] options the picker switches from the
/// ComboBox form to a scrollable TableBuilder form.
pub struct Select<'b> {
    id_source: egui::Id,
    label: Cow<'static, str>,
    options: &'static [&'static str],
    selected: &'b mut usize,
    enabled: bool,
    tooltip: Option<Tooltip>,
    variant: VariantId,
    /// At or above this option count, render as a TableBuilder picker
    /// instead of a ComboBox. Defaults to [`DEFAULT_TABLE_THRESHOLD`].
    table_threshold: usize,
}

impl<'b> Select<'b> {
    pub fn new(
        id_source: impl std::hash::Hash,
        label: impl Into<Cow<'static, str>>,
        options: &'static [&'static str],
        selected: &'b mut usize,
    ) -> Self {
        Self {
            id_source: egui::Id::new(("canon.select", id_source)),
            label: label.into(),
            options,
            selected,
            enabled: true,
            tooltip: None,
            variant: 0,
            table_threshold: DEFAULT_TABLE_THRESHOLD,
        }
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn tooltip(mut self, text: impl Into<Tooltip>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    pub fn variant(mut self, variant: VariantId) -> Self {
        self.variant = variant;
        self
    }

    /// Override the option-count threshold at which the picker
    /// switches from ComboBox to TableBuilder. Set to `usize::MAX` to
    /// force ComboBox; set to `0` to force TableBuilder.
    pub fn table_threshold(mut self, threshold: usize) -> Self {
        self.table_threshold = threshold;
        self
    }

    /// True when this Select would render as a TableBuilder for its
    /// configured option count. Surfaces the layout decision so tests
    /// can assert on the crossover without rendering.
    pub fn uses_table_form(&self) -> bool {
        self.options.len() >= self.table_threshold
    }
}

impl CanonWidget for Select<'_> {
    fn id(&self) -> egui::Id {
        self.id_source
    }

    fn identity_uri(&self) -> IdentityUri {
        Cow::Borrowed(IDENTITY)
    }

    fn affordances(&self) -> &[Affordance] {
        if self.enabled {
            AFFORDANCES_ACTIVE
        } else {
            AFFORDANCES_DISABLED
        }
    }

    fn confidence(&self) -> Confidence {
        Confidence::input()
    }

    fn variant_id(&self) -> VariantId {
        self.variant
    }

    fn mutation_axes(&self) -> &[MutationAxis] {
        MUTATION_AXES
    }

    fn tooltip(&self) -> Option<&Tooltip> {
        self.tooltip.as_ref()
    }

    fn show(self, ui: &mut egui::Ui) -> CanonResponse {
        let id = self.id_source;
        let variant = self.variant;
        let enabled = self.enabled;
        let tooltip = self.tooltip.clone();
        let options = self.options;
        let selected = self.selected;
        let label = self.label;
        let use_table = options.len() >= self.table_threshold;

        let current_text: &str = options
            .get(*selected)
            .copied()
            .unwrap_or_else(|| if options.is_empty() { "" } else { options[0] });

        let mut changed = false;
        let mut resp = ui
            .scope(|ui| {
                if !enabled {
                    ui.disable();
                }
                let main_resp = if use_table {
                    paint_table_form(ui, id, options, selected, &mut changed)
                } else {
                    paint_combo_form(ui, id, options, selected, current_text, &mut changed)
                };
                // Attach the static label next to the picker so the
                // primitive has a human-readable anchor.
                if !label.is_empty() {
                    ui.label(label.as_ref());
                }
                main_resp
            })
            .inner;

        if let Some(tt) = &tooltip {
            resp = resp.on_hover_text(tt.as_ref());
        }

        let chosen: Option<&'static str> = if enabled && changed {
            Some("select")
        } else {
            None
        };

        CanonResponse::from_egui(resp, Cow::Borrowed(IDENTITY), variant, chosen).with_id_hint(id)
    }
}

/// Small-N picker — the original ComboBox form. Extracted so the
/// trait `show` can dispatch to it without ballooning the inline
/// scope closure.
fn paint_combo_form(
    ui: &mut egui::Ui,
    id: egui::Id,
    options: &[&'static str],
    selected: &mut usize,
    current_text: &str,
    changed: &mut bool,
) -> egui::Response {
    let combo_resp = egui::ComboBox::from_id_salt(id)
        .selected_text(current_text)
        .show_ui(ui, |ui| {
            for (i, opt) in options.iter().enumerate() {
                if ui.selectable_label(*selected == i, *opt).clicked() && *selected != i {
                    *selected = i;
                    *changed = true;
                }
            }
        });
    combo_resp.response
}

/// Large-N picker — TableBuilder inside a fixed-height scroll region.
/// Renders one column (the option label) plus a checkmark column for
/// the current selection so the row stays scannable as the list
/// scrolls. Rows clip via egui's body virtualization, so a 10k-entry
/// option set still renders in O(visible-rows) per frame.
fn paint_table_form(
    ui: &mut egui::Ui,
    id: egui::Id,
    options: &[&'static str],
    selected: &mut usize,
    changed: &mut bool,
) -> egui::Response {
    // Cap the popup at ~12 rows tall so the picker doesn't dominate
    // the surrounding layout. The scroll region inside the table
    // handles the overflow.
    let table_height = TABLE_ROW_HEIGHT * 12.0;
    egui::Frame::group(ui.style())
        .show(ui, |ui| {
            ui.set_min_width(160.0);
            ui.push_id(id, |ui| {
                let table = TableBuilder::new(ui)
                    .striped(true)
                    .resizable(false)
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .column(Column::exact(20.0))
                    .column(Column::remainder())
                    .min_scrolled_height(0.0)
                    .max_scroll_height(table_height);
                table.body(|body| {
                    body.rows(TABLE_ROW_HEIGHT, options.len(), |mut row| {
                        let i = row.index();
                        let is_selected = *selected == i;
                        row.set_selected(is_selected);
                        // Column 0 — selection marker so the eye can
                        // find the current row even at 10k entries.
                        row.col(|ui| {
                            if is_selected {
                                ui.label("•");
                            }
                        });
                        let (_rect, resp) = row.col(|ui| {
                            ui.label(options[i]);
                        });
                        if resp.clicked() && !is_selected {
                            *selected = i;
                            *changed = true;
                        }
                    });
                });
            })
            .response
        })
        .response
}
