//! `ui://chip` — labelled status token. ADR-001 row 5.
//!
//! A compact, framed label that may optionally be activatable (clickable
//! as a button-like affordance) or selectable (toggleable). When neither
//! flag is set the chip renders read-only and exposes zero affordances.

use std::borrow::Cow;

use eframe::egui;

use super::CanonWidget;
use super::response::CanonResponse;
use super::types::{Affordance, Confidence, IdentityUri, MutationAxis, Tooltip, VariantId};

const IDENTITY: &str = "ui://chip";

static AFFORDANCES_NONE: &[Affordance] = &[];
static AFFORDANCES_ACTIVATE: &[Affordance] = &[Affordance {
    name: Cow::Borrowed("activate"),
    verb: Cow::Borrowed("wsp.activate"),
    actors: &[],
    args_schema: None,
    reorderable: false,
}];
static AFFORDANCES_SELECT: &[Affordance] = &[Affordance {
    name: Cow::Borrowed("select"),
    verb: Cow::Borrowed("wsp.update"),
    actors: &[],
    args_schema: None,
    reorderable: false,
}];

static MUTATION_AXES: &[MutationAxis] = &[
    MutationAxis::new("tint"),
    MutationAxis::new("icon"),
    MutationAxis::new("size"),
    MutationAxis::new("copy"),
];

/// Severity tint. The canon mutation axis `tint` varies over these.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum ChipTone {
    #[default]
    Neutral,
    Ok,
    Warn,
    Crit,
    Info,
}

impl ChipTone {
    fn colors(self) -> (egui::Color32, egui::Color32) {
        match self {
            ChipTone::Neutral => (egui::Color32::from_gray(28), egui::Color32::from_gray(210)),
            ChipTone::Ok => (
                egui::Color32::from_rgb(20, 48, 30),
                egui::Color32::from_rgb(110, 210, 140),
            ),
            ChipTone::Warn => (
                egui::Color32::from_rgb(52, 38, 10),
                egui::Color32::from_rgb(255, 205, 90),
            ),
            ChipTone::Crit => (
                egui::Color32::from_rgb(52, 20, 20),
                egui::Color32::from_rgb(255, 140, 140),
            ),
            ChipTone::Info => (
                egui::Color32::from_rgb(18, 32, 50),
                egui::Color32::from_rgb(120, 200, 255),
            ),
        }
    }
}

/// A labelled status token. Construct with `Chip::new` then optionally
/// chain `.tone(...)`, `.activatable(...)`, `.selected(...)`,
/// `.tooltip(...)`, `.variant(...)` before calling `.show(ui)`.
pub struct Chip {
    id_source: egui::Id,
    label: Cow<'static, str>,
    tone: ChipTone,
    activatable: bool,
    selectable: bool,
    selected: bool,
    tooltip: Option<Tooltip>,
    variant: VariantId,
}

impl Chip {
    pub fn new(id_source: impl std::hash::Hash, label: impl Into<Cow<'static, str>>) -> Self {
        Self {
            id_source: egui::Id::new(("canon.chip", id_source)),
            label: label.into(),
            tone: ChipTone::default(),
            activatable: false,
            selectable: false,
            selected: false,
            tooltip: None,
            variant: 0,
        }
    }

    pub fn tone(mut self, tone: ChipTone) -> Self {
        self.tone = tone;
        self
    }

    /// Make the chip click-activatable. Emits the `activate` affordance.
    pub fn activatable(mut self, activatable: bool) -> Self {
        self.activatable = activatable;
        self
    }

    /// Make the chip toggleable. Emits the `select` affordance; the
    /// caller is responsible for reading `resp.inner.clicked()` to flip
    /// the bound state.
    pub fn selectable(mut self, selectable: bool) -> Self {
        self.selectable = selectable;
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
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

impl CanonWidget for Chip {
    fn id(&self) -> egui::Id {
        self.id_source
    }

    fn identity_uri(&self) -> IdentityUri {
        Cow::Borrowed(IDENTITY)
    }

    fn affordances(&self) -> &[Affordance] {
        if self.selectable {
            AFFORDANCES_SELECT
        } else if self.activatable {
            AFFORDANCES_ACTIVATE
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
        let (fill, text_color) = self.tone.colors();

        // The chip is rendered as either a selectable label (when interactive)
        // or a plain framed label (when static).
        let interactive = self.selectable || self.activatable;
        let rich = egui::RichText::new(self.label.as_ref())
            .color(text_color)
            .monospace()
            .small();

        let inner = if interactive {
            egui::Frame::new()
                .fill(fill)
                .corner_radius(10.0)
                .inner_margin(egui::Margin::symmetric(8, 3))
                .show(ui, |ui| ui.selectable_label(self.selected, rich))
                .inner
        } else {
            let r = egui::Frame::new()
                .fill(fill)
                .corner_radius(10.0)
                .inner_margin(egui::Margin::symmetric(8, 3))
                .show(ui, |ui| ui.label(rich));
            r.response
        };

        let mut resp = inner;
        if let Some(tt) = &tooltip {
            resp = resp.on_hover_text(tt.as_ref());
        }

        let chosen: Option<&'static str> = if self.selectable && resp.clicked() {
            Some("select")
        } else if self.activatable && resp.clicked() {
            Some("activate")
        } else {
            None
        };

        CanonResponse::from_egui(resp, Cow::Borrowed(IDENTITY), variant, chosen).with_id_hint(id)
    }
}
