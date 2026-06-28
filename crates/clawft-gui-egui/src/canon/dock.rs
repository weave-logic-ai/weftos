//! `ui://dock` — resizable tabbed / split panels primitive
//! (ADR-001 row 10). Session-5 §10: the Mission Console chrome.
//!
//! Implementation uses `egui_dock::DockArea` + `DockState` — community
//! crate that already solves tab dragging, splitting, and detachment.
//! egui_dock is pure-egui (no native-only APIs), so the same code
//! path compiles for both native and wasm targets.
//!
//! The caller owns both the `DockState` (so geometry persists across
//! frames) and the `TabViewer` implementation (so tab rendering is
//! caller-typed).

use std::borrow::Cow;

use eframe::egui;

use super::CanonWidget;
use super::response::CanonResponse;
use super::types::{Affordance, Confidence, IdentityUri, MutationAxis, Tooltip, VariantId};

const IDENTITY: &str = "ui://dock";

static AFFORDANCES_ACTIVE: &[Affordance] = &[
    Affordance {
        name: Cow::Borrowed("switch-tab"),
        verb: Cow::Borrowed("wsp.set"),
        actors: &[],
        args_schema: None,
        reorderable: false,
    },
    Affordance {
        name: Cow::Borrowed("split"),
        verb: Cow::Borrowed("wsp.invoke"),
        actors: &[],
        args_schema: None,
        reorderable: false,
    },
    Affordance {
        name: Cow::Borrowed("detach"),
        verb: Cow::Borrowed("wsp.invoke"),
        actors: &[],
        args_schema: None,
        reorderable: false,
    },
    Affordance {
        name: Cow::Borrowed("drop-tab"),
        verb: Cow::Borrowed("wsp.invoke"),
        actors: &[],
        args_schema: None,
        reorderable: true,
    },
];

static MUTATION_AXES: &[MutationAxis] = &[
    MutationAxis::new("default-split"),
    MutationAxis::new("drag-preview-style"),
];

/// Resizable tabbed / split panels. Caller supplies a mutable
/// `DockState<Tab>` and a `TabViewer<Tab = Tab>`.
pub struct Dock<'s, 'v, Tab, V>
where
    V: egui_dock::TabViewer<Tab = Tab>,
{
    id_source: egui::Id,
    state: &'s mut egui_dock::DockState<Tab>,
    viewer: &'v mut V,
    tooltip: Option<Tooltip>,
    variant: VariantId,
}

impl<'s, 'v, Tab, V> Dock<'s, 'v, Tab, V>
where
    V: egui_dock::TabViewer<Tab = Tab>,
{
    pub fn new(
        id_source: impl std::hash::Hash,
        state: &'s mut egui_dock::DockState<Tab>,
        viewer: &'v mut V,
    ) -> Self {
        Self {
            id_source: egui::Id::new(("canon.dock", id_source)),
            state,
            viewer,
            tooltip: None,
            variant: 0,
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

impl<Tab, V> CanonWidget for Dock<'_, '_, Tab, V>
where
    V: egui_dock::TabViewer<Tab = Tab>,
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
        let tooltip = self.tooltip;

        // Wrap the DockArea in a scope so we can attribute a Response
        // back to the Dock primitive (DockArea::show_inside does not
        // return a Response directly).
        let scope = ui.scope(|ui| {
            let style = egui_dock::Style::from_egui(ui.style().as_ref());
            egui_dock::DockArea::new(self.state)
                .id(id)
                .style(style)
                .show_inside(ui, self.viewer);
        });

        let mut resp = scope.response;
        if let Some(tt) = &tooltip {
            resp = resp.on_hover_text(tt.as_ref());
        }

        CanonResponse::from_egui(resp, Cow::Borrowed(IDENTITY), variant, None).with_id_hint(id)
    }
}
