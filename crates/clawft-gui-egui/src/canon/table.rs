//! `ui://table` — sortable tabular data. ADR-001 row 14.
//!
//! Wraps `egui_extras::TableBuilder`. The caller supplies a column
//! schema and a per-row renderer. Sort state is owned by the caller
//! and mutated via the returned `TableOutcome`.

use std::borrow::Cow;

use eframe::egui;
use egui_extras::{Column, TableBuilder};

use super::CanonWidget;
use super::response::CanonResponse;
use super::types::{Affordance, Confidence, IdentityUri, MutationAxis, Tooltip, VariantId};

const IDENTITY: &str = "ui://table";

static AFFORDANCES: &[Affordance] = &[
    Affordance {
        name: Cow::Borrowed("sort"),
        verb: Cow::Borrowed("wsp.update"),
        actors: &[],
        args_schema: None,
        reorderable: false,
    },
    Affordance {
        name: Cow::Borrowed("select-row"),
        verb: Cow::Borrowed("wsp.update"),
        actors: &[],
        args_schema: None,
        reorderable: false,
    },
    Affordance {
        name: Cow::Borrowed("reorder"),
        verb: Cow::Borrowed("wsp.update"),
        actors: &[],
        args_schema: None,
        reorderable: true,
    },
];

static MUTATION_AXES: &[MutationAxis] = &[
    MutationAxis::new("sort-column"),
    MutationAxis::new("sort-direction"),
    MutationAxis::new("column-widths"),
    MutationAxis::new("striped"),
    MutationAxis::new("row-height"),
];

/// Declarative column spec. Name is shown in the header; `min_width`
/// bounds the rendered column's lower size. `remainder` means "eat any
/// leftover horizontal space" and should be set on at most one column.
#[derive(Clone, Debug)]
pub struct TableColumn {
    pub name: Cow<'static, str>,
    pub min_width: f32,
    pub remainder: bool,
}

impl TableColumn {
    pub fn new(name: impl Into<Cow<'static, str>>) -> Self {
        Self {
            name: name.into(),
            min_width: 60.0,
            remainder: false,
        }
    }

    pub fn min_width(mut self, w: f32) -> Self {
        self.min_width = w;
        self
    }

    pub fn remainder(mut self) -> Self {
        self.remainder = true;
        self
    }
}

/// Per-frame output from the table. The caller inspects this to flip
/// sort state and record the selected row.
#[derive(Clone, Debug, Default)]
pub struct TableOutcome {
    /// Column index whose header was clicked this frame, if any.
    pub sort_clicked: Option<usize>,
    /// Row index whose primary cell was clicked this frame, if any.
    pub row_clicked: Option<usize>,
}

/// Sortable tabular data.
pub struct Table<'a, F>
where
    F: FnMut(&mut egui_extras::TableRow<'_, '_>, usize),
{
    id_source: egui::Id,
    columns: &'a [TableColumn],
    row_count: usize,
    row_height: f32,
    header_height: f32,
    striped: bool,
    sort_col: Option<usize>,
    sort_asc: bool,
    selected_row: Option<usize>,
    tooltip: Option<Tooltip>,
    variant: VariantId,
    render_row: Option<F>,
}

impl<'a, F> Table<'a, F>
where
    F: FnMut(&mut egui_extras::TableRow<'_, '_>, usize),
{
    pub fn new(id_source: impl std::hash::Hash, columns: &'a [TableColumn]) -> Self {
        Self {
            id_source: egui::Id::new(("canon.table", id_source)),
            columns,
            row_count: 0,
            row_height: 22.0,
            header_height: 24.0,
            striped: true,
            sort_col: None,
            sort_asc: true,
            selected_row: None,
            tooltip: None,
            variant: 0,
            render_row: None,
        }
    }

    pub fn rows(mut self, n: usize) -> Self {
        self.row_count = n;
        self
    }

    pub fn row_height(mut self, h: f32) -> Self {
        self.row_height = h;
        self
    }

    pub fn header_height(mut self, h: f32) -> Self {
        self.header_height = h;
        self
    }

    pub fn striped(mut self, striped: bool) -> Self {
        self.striped = striped;
        self
    }

    pub fn sort(mut self, col: Option<usize>, asc: bool) -> Self {
        self.sort_col = col;
        self.sort_asc = asc;
        self
    }

    pub fn selected_row(mut self, idx: Option<usize>) -> Self {
        self.selected_row = idx;
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

    /// Register the per-row renderer. The closure receives the egui
    /// table row handle and the row index; call `.col(|ui| ...)` once
    /// per column.
    pub fn render(mut self, render: F) -> Self {
        self.render_row = Some(render);
        self
    }

    /// Show the table and return both the canon response and the
    /// per-frame outcome (sort / row click).
    pub fn show_with_outcome(self, ui: &mut egui::Ui) -> (CanonResponse, TableOutcome) {
        let id = self.id_source;
        let variant = self.variant;
        let tooltip = self.tooltip.clone();
        let columns = self.columns;
        let row_count = self.row_count;
        let row_height = self.row_height;
        let header_height = self.header_height;
        let striped = self.striped;
        let sort_col = self.sort_col;
        let sort_asc = self.sort_asc;
        let mut render_row = self.render_row;

        let mut outcome = TableOutcome::default();

        let inner = ui.scope(|ui| {
            let mut builder = TableBuilder::new(ui).striped(striped);
            for col in columns {
                let c = if col.remainder {
                    Column::remainder()
                } else {
                    Column::auto().at_least(col.min_width)
                };
                builder = builder.column(c);
            }

            builder
                .header(header_height, |mut h| {
                    for (i, col) in columns.iter().enumerate() {
                        h.col(|ui| {
                            let mut label = egui::RichText::new(col.name.as_ref()).strong();
                            if sort_col == Some(i) {
                                let arrow = if sort_asc { " ▲" } else { " ▼" };
                                label =
                                    egui::RichText::new(format!("{}{}", col.name.as_ref(), arrow))
                                        .strong();
                            }
                            if ui.button(label).clicked() {
                                outcome.sort_clicked = Some(i);
                            }
                        });
                    }
                })
                .body(|mut body| {
                    if let Some(render) = render_row.as_mut() {
                        for idx in 0..row_count {
                            body.row(row_height, |mut row| {
                                render(&mut row, idx);
                            });
                        }
                    }
                });
        });

        let mut resp = inner.response;
        if let Some(tt) = &tooltip {
            resp = resp.on_hover_text(tt.as_ref());
        }

        // Row-click attribution is the caller's job (via its per-row
        // renderer), but we detect header-sort clicks above. Prefer
        // sort over select-row when both fire in the same frame — sort
        // is the structural change, select-row is local.
        let chosen: Option<&'static str> = if outcome.sort_clicked.is_some() {
            Some("sort")
        } else {
            None
        };

        let canon = CanonResponse::from_egui(resp, Cow::Borrowed(IDENTITY), variant, chosen)
            .with_id_hint(id);
        (canon, outcome)
    }
}

impl<'a, F> CanonWidget for Table<'a, F>
where
    F: FnMut(&mut egui_extras::TableRow<'_, '_>, usize),
{
    fn id(&self) -> egui::Id {
        self.id_source
    }

    fn identity_uri(&self) -> IdentityUri {
        Cow::Borrowed(IDENTITY)
    }

    fn affordances(&self) -> &[Affordance] {
        AFFORDANCES
    }

    fn confidence(&self) -> Confidence {
        Confidence::deterministic()
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
        // Discard the outcome for the base CanonWidget path. Callers who
        // need row-click / sort attribution should use `show_with_outcome`.
        self.show_with_outcome(ui).0
    }
}
