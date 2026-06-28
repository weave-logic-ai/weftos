//! `ui://tabs` — simple tab strip primitive (ADR-001 row 21,
//! re-promoted 2026-04-20).
//!
//! Distinct from `ui://dock` in both state shape (single `usize` vs.
//! tree of splits) and interaction contract (click-to-swap-body vs.
//! drag-to-rearrange-workspace). The implementation is a loop over
//! `selectable_label` on a horizontal strip followed by a caller-
//! driven body area keyed on the selected index.

use std::borrow::Cow;

use eframe::egui;

use super::CanonWidget;
use super::response::CanonResponse;
use super::types::{Affordance, Confidence, IdentityUri, MutationAxis, Tooltip, VariantId};

const IDENTITY: &str = "ui://tabs";

static AFFORDANCES_ACTIVE: &[Affordance] = &[Affordance {
    name: Cow::Borrowed("switch-tab"),
    verb: Cow::Borrowed("wsp.set"),
    actors: &[],
    args_schema: None,
    reorderable: false,
}];

static MUTATION_AXES: &[MutationAxis] = &[
    MutationAxis::new("label-position"),
    MutationAxis::new("tab-density"),
];

/// Simple tab strip. The caller owns `&mut usize selected`; the body
/// builder receives the post-switch selected index.
pub struct Tabs<'b, F>
where
    F: FnOnce(&mut egui::Ui, usize),
{
    id_source: egui::Id,
    labels: &'b [&'b str],
    selected: &'b mut usize,
    tooltip: Option<Tooltip>,
    variant: VariantId,
    body: F,
}

impl<'b, F> Tabs<'b, F>
where
    F: FnOnce(&mut egui::Ui, usize),
{
    pub fn new(
        id_source: impl std::hash::Hash,
        labels: &'b [&'b str],
        selected: &'b mut usize,
        body: F,
    ) -> Self {
        Self {
            id_source: egui::Id::new(("canon.tabs", id_source)),
            labels,
            selected,
            tooltip: None,
            variant: 0,
            body,
        }
    }

    pub fn tooltip(mut self, text: impl Into<Tooltip>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    pub fn variant(mut self, variant: VariantId) -> Self {
        self.variant = variant;
        self
    }
}

impl<F> CanonWidget for Tabs<'_, F>
where
    F: FnOnce(&mut egui::Ui, usize),
{
    fn id(&self) -> egui::Id {
        self.id_source
    }

    fn identity_uri(&self) -> IdentityUri {
        Cow::Borrowed(IDENTITY)
    }

    fn affordances(&self) -> &[Affordance] {
        AFFORDANCES_ACTIVE
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
        let tooltip = self.tooltip.clone();
        let labels = self.labels;
        let selected = self.selected;

        let mut changed = false;
        let strip_resp = ui
            .horizontal(|ui| {
                for (i, label) in labels.iter().enumerate() {
                    if ui.selectable_label(*selected == i, *label).clicked() && *selected != i {
                        *selected = i;
                        changed = true;
                    }
                }
            })
            .response;

        ui.separator();
        (self.body)(ui, *selected);

        let mut resp = strip_resp;
        if let Some(tt) = &tooltip {
            resp = resp.on_hover_text(tt.as_ref());
        }

        let chosen: Option<&'static str> = if changed { Some("switch-tab") } else { None };

        CanonResponse::from_egui(resp, Cow::Borrowed(IDENTITY), variant, chosen).with_id_hint(id)
    }
}
