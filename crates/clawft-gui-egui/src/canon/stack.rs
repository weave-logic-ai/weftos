//! `ui://stack` — one-axis container. ADR-001 row 7.
//!
//! Wraps `ui.horizontal` / `ui.vertical`. Callers supply a child-builder
//! closure; the stack emits no affordances unless declared `reorderable`
//! (per ADR-006 §8), in which case it publishes `reorder`.

use std::borrow::Cow;

use eframe::egui;

use super::CanonWidget;
use super::response::CanonResponse;
use super::types::{Affordance, Confidence, IdentityUri, MutationAxis, Tooltip, VariantId};

const IDENTITY: &str = "ui://stack";

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
    MutationAxis::new("gap"),
    MutationAxis::new("alignment"),
    MutationAxis::new("wrap"),
];

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum StackAxis {
    Horizontal,
    #[default]
    Vertical,
}

/// One-axis layout container. The child-builder closure is stored as
/// a type parameter so callers can place arbitrary widgets inside.
pub struct Stack<F> {
    id_source: egui::Id,
    axis: StackAxis,
    wrap: bool,
    reorderable: bool,
    tooltip: Option<Tooltip>,
    variant: VariantId,
    body: Option<F>,
}

impl<F> Stack<F>
where
    F: FnOnce(&mut egui::Ui),
{
    pub fn new(id_source: impl std::hash::Hash) -> Self {
        Self {
            id_source: egui::Id::new(("canon.stack", id_source)),
            axis: StackAxis::default(),
            wrap: false,
            reorderable: false,
            tooltip: None,
            variant: 0,
            body: None,
        }
    }

    pub fn axis(mut self, axis: StackAxis) -> Self {
        self.axis = axis;
        self
    }

    pub fn horizontal(mut self) -> Self {
        self.axis = StackAxis::Horizontal;
        self
    }

    pub fn vertical(mut self) -> Self {
        self.axis = StackAxis::Vertical;
        self
    }

    /// Horizontal-only: wrap children across lines when out of room.
    /// No-op on the vertical axis.
    pub fn wrap(mut self, wrap: bool) -> Self {
        self.wrap = wrap;
        self
    }

    /// Declare that children may be drag-reordered (ADR-006 §8). Publishes
    /// the `reorder` affordance with `reorderable = true`.
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

    /// Set the child-builder. The closure runs once per frame inside
    /// the containing `ui.horizontal` / `ui.vertical`.
    pub fn body(mut self, build: F) -> Self {
        self.body = Some(build);
        self
    }
}

impl<F> CanonWidget for Stack<F>
where
    F: FnOnce(&mut egui::Ui),
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
        let wrap = self.wrap;
        let body = self.body;

        let inner = ui.scope(|ui| {
            if let Some(build) = body {
                match axis {
                    StackAxis::Horizontal => {
                        if wrap {
                            ui.horizontal_wrapped(build);
                        } else {
                            ui.horizontal(build);
                        }
                    }
                    StackAxis::Vertical => {
                        ui.vertical(build);
                    }
                }
            }
        });

        let mut resp = inner.response;
        if let Some(tt) = &tooltip {
            resp = resp.on_hover_text(tt.as_ref());
        }

        // The container itself has no per-frame chosen affordance: reorder
        // fires inside the child closure, not on the container shell.
        let chosen: Option<&'static str> = None;

        CanonResponse::from_egui(resp, Cow::Borrowed(IDENTITY), variant, chosen).with_id_hint(id)
    }
}
