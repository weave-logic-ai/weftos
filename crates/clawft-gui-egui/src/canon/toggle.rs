//! `ui://toggle` — boolean binding primitive (ADR-001 row 4).
//!
//! Distinguished from `Field` because agents reason about booleans
//! specifically (session-5 §3 "it's a verb-capable state"). The caller
//! owns the `&mut bool` binding; `Toggle` only decorates + fires.

use std::borrow::Cow;

use eframe::egui;

use super::CanonWidget;
use super::response::CanonResponse;
use super::types::{Affordance, Confidence, IdentityUri, MutationAxis, Tooltip, VariantId};

const IDENTITY: &str = "ui://toggle";

static AFFORDANCES_ACTIVE: &[Affordance] = &[Affordance {
    name: Cow::Borrowed("toggle"),
    verb: Cow::Borrowed("wsp.set"),
    actors: &[],
    args_schema: None,
    reorderable: false,
}];
static AFFORDANCES_DISABLED: &[Affordance] = &[];

static MUTATION_AXES: &[MutationAxis] =
    &[MutationAxis::new("label-copy"), MutationAxis::new("style")];

/// Presentation variant. `Switch` uses `ui.toggle_value`'s default
/// pressable-label rendering; `Checkbox` uses `egui::Checkbox`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum ToggleStyle {
    #[default]
    Switch,
    Checkbox,
}

/// Boolean binding. Borrow the bool you want to drive, configure the
/// label/style/variant, then `show(ui)`.
pub struct Toggle<'b> {
    id_source: egui::Id,
    label: Cow<'static, str>,
    value: &'b mut bool,
    style: ToggleStyle,
    enabled: bool,
    tooltip: Option<Tooltip>,
    variant: VariantId,
}

impl<'b> Toggle<'b> {
    pub fn new(
        id_source: impl std::hash::Hash,
        label: impl Into<Cow<'static, str>>,
        value: &'b mut bool,
    ) -> Self {
        Self {
            id_source: egui::Id::new(("canon.toggle", id_source)),
            label: label.into(),
            value,
            style: ToggleStyle::default(),
            enabled: true,
            tooltip: None,
            variant: 0,
        }
    }

    pub fn style(mut self, style: ToggleStyle) -> Self {
        self.style = style;
        self
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
}

impl CanonWidget for Toggle<'_> {
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
        let label = self.label.clone();
        let tooltip = self.tooltip.clone();

        let mut resp = ui
            .scope(|ui| {
                if !enabled {
                    ui.disable();
                }
                match self.style {
                    ToggleStyle::Switch => ui.toggle_value(self.value, label.as_ref()),
                    ToggleStyle::Checkbox => ui.checkbox(self.value, label.as_ref()),
                }
            })
            .inner;

        if let Some(tt) = &tooltip {
            resp = resp.on_hover_text(tt.as_ref());
        }

        let chosen: Option<&'static str> = if enabled && resp.changed() {
            Some("toggle")
        } else {
            None
        };

        CanonResponse::from_egui(resp, Cow::Borrowed(IDENTITY), variant, chosen).with_id_hint(id)
    }
}
