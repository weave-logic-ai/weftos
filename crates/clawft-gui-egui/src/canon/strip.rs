//! `ui://strip` — fixed-ratio division. ADR-001 row 9.
//!
//! Wraps `egui_extras::StripBuilder`. The caller supplies a vector of
//! cell sizes and a child-builder closure that receives an iterator-like
//! handle over cells. In this retrofit we take the simpler shape: the
//! closure is called once with a `&mut egui_extras::Strip<'_, '_>` so
//! callers can place one child per declared cell.

use std::borrow::Cow;

use eframe::egui;
use egui_extras::{Size, StripBuilder};

use super::CanonWidget;
use super::response::CanonResponse;
use super::types::{Affordance, Confidence, IdentityUri, MutationAxis, Tooltip, VariantId};

const IDENTITY: &str = "ui://strip";

static AFFORDANCES_NONE: &[Affordance] = &[];
static AFFORDANCES_REORDER: &[Affordance] = &[Affordance {
    name: Cow::Borrowed("reorder"),
    verb: Cow::Borrowed("wsp.update"),
    actors: &[],
    args_schema: None,
    reorderable: true,
}];

static MUTATION_AXES: &[MutationAxis] = &[
    MutationAxis::new("axis"),
    MutationAxis::new("ratios"),
    MutationAxis::new("gap"),
];

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum StripAxis {
    #[default]
    Horizontal,
    Vertical,
}

/// One cell's sizing directive. Mirrors `egui_extras::Size` variants
/// but keeps us decoupled from `egui_extras` at the call site.
#[derive(Copy, Clone, Debug)]
pub enum CellSize {
    /// Fixed pixels.
    Exact(f32),
    /// Take what's left after all other cells are sized.
    Remainder,
    /// Minimum pixels; will grow to fit content.
    AtLeast(f32),
    /// Proportional share of remaining space (1.0 = equal share).
    Relative(f32),
}

impl CellSize {
    fn to_egui(self) -> Size {
        match self {
            CellSize::Exact(p) => Size::exact(p),
            CellSize::Remainder => Size::remainder(),
            CellSize::AtLeast(p) => Size::initial(p).at_least(p),
            CellSize::Relative(r) => Size::relative(r),
        }
    }
}

/// Fixed-ratio layout strip. The child closure receives a mutable
/// `egui_extras::Strip` handle; call `.cell(|ui| ...)` once per declared
/// cell to place children.
pub struct Strip<F> {
    id_source: egui::Id,
    axis: StripAxis,
    cells: Vec<CellSize>,
    reorderable: bool,
    tooltip: Option<Tooltip>,
    variant: VariantId,
    body: Option<F>,
}

impl<F> Strip<F>
where
    F: for<'a, 'b> FnOnce(&mut egui_extras::Strip<'a, 'b>),
{
    pub fn new(id_source: impl std::hash::Hash) -> Self {
        Self {
            id_source: egui::Id::new(("canon.strip", id_source)),
            axis: StripAxis::default(),
            cells: Vec::new(),
            reorderable: false,
            tooltip: None,
            variant: 0,
            body: None,
        }
    }

    pub fn axis(mut self, axis: StripAxis) -> Self {
        self.axis = axis;
        self
    }

    pub fn horizontal(mut self) -> Self {
        self.axis = StripAxis::Horizontal;
        self
    }

    pub fn vertical(mut self) -> Self {
        self.axis = StripAxis::Vertical;
        self
    }

    /// Push one cell onto the strip. Call this once per child slot.
    pub fn cell(mut self, size: CellSize) -> Self {
        self.cells.push(size);
        self
    }

    /// Replace the whole size list at once.
    pub fn cells(mut self, cells: Vec<CellSize>) -> Self {
        self.cells = cells;
        self
    }

    pub fn reorderable(mut self, reorderable: bool) -> Self {
        self.reorderable = reorderable;
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

    pub fn body(mut self, build: F) -> Self {
        self.body = Some(build);
        self
    }
}

impl<F> CanonWidget for Strip<F>
where
    F: for<'a, 'b> FnOnce(&mut egui_extras::Strip<'a, 'b>),
{
    fn id(&self) -> egui::Id {
        self.id_source
    }

    fn identity_uri(&self) -> IdentityUri {
        Cow::Borrowed(IDENTITY)
    }

    fn affordances(&self) -> &[Affordance] {
        if self.reorderable {
            AFFORDANCES_REORDER
        } else {
            AFFORDANCES_NONE
        }
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
        let id = self.id_source;
        let variant = self.variant;
        let tooltip = self.tooltip.clone();
        let axis = self.axis;
        let cells = self.cells;
        let body = self.body;

        let inner = ui.scope(|ui| {
            let mut builder = StripBuilder::new(ui);
            for size in &cells {
                builder = builder.size(size.to_egui());
            }
            let run = |strip: egui_extras::Strip<'_, '_>| {
                if let Some(build) = body {
                    let mut strip = strip;
                    build(&mut strip);
                } else {
                    // No body means no children; strip still needs to
                    // consume cells to satisfy egui_extras invariants.
                    let mut strip = strip;
                    for _ in 0..cells.len() {
                        strip.empty();
                    }
                }
            };
            match axis {
                StripAxis::Horizontal => builder.horizontal(run),
                StripAxis::Vertical => builder.vertical(run),
            }
        });

        let mut resp = inner.response;
        if let Some(tt) = &tooltip {
            resp = resp.on_hover_text(tt.as_ref());
        }

        let chosen: Option<&'static str> = None;

        CanonResponse::from_egui(resp, Cow::Borrowed(IDENTITY), variant, chosen).with_id_hint(id)
    }
}
