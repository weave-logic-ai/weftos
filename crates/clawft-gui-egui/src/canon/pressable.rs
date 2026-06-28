//! `ui://pressable` — the Button primitive. Reference implementation
//! for the retrofit pattern every other primitive follows.
//!
//! Maps to ADR-001 row 2. Current `blocks/button.rs` is this primitive
//! minus affordance metadata and return-signal capture; the Pressable
//! here wraps `egui::Button` and fills in both.

use std::borrow::Cow;

use eframe::egui;

use super::CanonWidget;
use super::response::CanonResponse;
use super::types::{Affordance, Confidence, IdentityUri, MutationAxis, Tooltip, VariantId};

/// Visual variant. The canon mutation axis `style` ranges over these.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum PressableStyle {
    #[default]
    Primary,
    Secondary,
    Ghost,
    Destructive,
}

const IDENTITY: &str = "ui://pressable";

static AFFORDANCE_ACTIVATE: Affordance = Affordance::new("activate", "wsp.activate");
static AFFORDANCES_ACTIVE: &[Affordance] = &[Affordance {
    name: Cow::Borrowed("activate"),
    verb: Cow::Borrowed("wsp.activate"),
    actors: &[],
    args_schema: None,
    reorderable: false,
}];
static AFFORDANCES_DISABLED: &[Affordance] = &[];

static MUTATION_AXES: &[MutationAxis] = &[
    MutationAxis::new("copy"),
    MutationAxis::new("icon"),
    MutationAxis::new("tint"),
    MutationAxis::new("size"),
    MutationAxis::new("placement"),
];

/// A labelled, invoke-one-verb affordance. Construct with `Pressable::new`
/// then chain `.style(...)`, `.enabled(...)`, `.tooltip(...)`,
/// `.variant(...)` before calling `.show(ui)`.
pub struct Pressable {
    id_source: egui::Id,
    label: Cow<'static, str>,
    style: PressableStyle,
    enabled: bool,
    tooltip: Option<Tooltip>,
    variant: VariantId,
}

impl Pressable {
    pub fn new(id_source: impl std::hash::Hash, label: impl Into<Cow<'static, str>>) -> Self {
        Self {
            id_source: egui::Id::new(("canon.pressable", id_source)),
            label: label.into(),
            style: PressableStyle::Primary,
            enabled: true,
            tooltip: None,
            variant: 0,
        }
    }

    pub fn style(mut self, style: PressableStyle) -> Self {
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

    fn render_button(&self, ui: &mut egui::Ui) -> egui::Response {
        let text = egui::RichText::new(self.label.as_ref());
        let btn = match self.style {
            PressableStyle::Primary => egui::Button::new(text),
            PressableStyle::Secondary => egui::Button::new(text).fill(egui::Color32::from_gray(40)),
            PressableStyle::Ghost => egui::Button::new(text).frame(false),
            PressableStyle::Destructive => {
                egui::Button::new(text.color(egui::Color32::from_rgb(220, 80, 80)))
                    .fill(egui::Color32::from_rgb(50, 20, 20))
            }
        };
        ui.add_enabled(self.enabled, btn)
    }
}

impl CanonWidget for Pressable {
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
        let enabled = self.enabled;

        let mut resp = self.render_button(ui);
        if let Some(tt) = &tooltip {
            resp = resp.on_hover_text(tt.as_ref());
        }

        let chosen = if enabled && resp.clicked() {
            Some(AFFORDANCE_ACTIVATE.name.as_ref())
        } else {
            None
        };

        // We need a &'static str for the bearing encoding. The affordance
        // name is a Cow::Borrowed("activate"), so the narrowing is sound.
        let chosen_static: Option<&'static str> = chosen.map(|_| "activate");

        CanonResponse::from_egui(resp, Cow::Borrowed(IDENTITY), variant, chosen_static)
            .with_id_hint(id)
    }
}
