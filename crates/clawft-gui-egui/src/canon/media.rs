//! `ui://media` — decoded-image / icon / glyph primitive (ADR-001 row 17).
//!
//! Session-5 §17 maps this to `egui::Image::new(uri)` backed by
//! `egui_extras` loaders (already a dep on this crate). We accept any
//! `ImageSource`-convertible value and expose a `MediaFit` enum for
//! the `fit` mutation axis.

use std::borrow::Cow;

use eframe::egui;

use super::CanonWidget;
use super::response::CanonResponse;
use super::types::{
    Affordance, Confidence, ConfidenceSource, IdentityUri, MutationAxis, Tooltip, VariantId,
};

const IDENTITY: &str = "ui://media";

static AFFORDANCES_OPEN: &[Affordance] = &[Affordance {
    name: Cow::Borrowed("open"),
    verb: Cow::Borrowed("wsp.activate"),
    actors: &[],
    args_schema: None,
    reorderable: false,
}];
static AFFORDANCES_NONE: &[Affordance] = &[];

static MUTATION_AXES: &[MutationAxis] =
    &[MutationAxis::new("fit"), MutationAxis::new("placeholder")];

/// How the image should fit inside its allocated rect. Maps to egui's
/// `Image` fit methods.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum MediaFit {
    /// Shrink to fit without cropping — default.
    #[default]
    Contain,
    /// Fill and crop as needed.
    Cover,
    /// Stretch in both axes (may distort).
    Stretch,
}

/// Decoded-image / icon / glyph primitive.
pub struct Media {
    id_source: egui::Id,
    uri: Cow<'static, str>,
    alt: Option<Cow<'static, str>>,
    fit: MediaFit,
    max_size: Option<egui::Vec2>,
    clickable: bool,
    tooltip: Option<Tooltip>,
    variant: VariantId,
}

impl Media {
    pub fn new(id_source: impl std::hash::Hash, uri: impl Into<Cow<'static, str>>) -> Self {
        Self {
            id_source: egui::Id::new(("canon.media", id_source)),
            uri: uri.into(),
            alt: None,
            fit: MediaFit::default(),
            max_size: None,
            clickable: false,
            tooltip: None,
            variant: 0,
        }
    }

    pub fn alt(mut self, alt: impl Into<Cow<'static, str>>) -> Self {
        self.alt = Some(alt.into());
        self
    }

    pub fn fit(mut self, fit: MediaFit) -> Self {
        self.fit = fit;
        self
    }

    pub fn max_size(mut self, size: egui::Vec2) -> Self {
        self.max_size = Some(size);
        self
    }

    pub fn clickable(mut self, clickable: bool) -> Self {
        self.clickable = clickable;
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

impl CanonWidget for Media {
    fn id(&self) -> egui::Id {
        self.id_source
    }

    fn identity_uri(&self) -> IdentityUri {
        Cow::Borrowed(IDENTITY)
    }

    fn affordances(&self) -> &[Affordance] {
        if self.clickable {
            AFFORDANCES_OPEN
        } else {
            AFFORDANCES_NONE
        }
    }

    fn confidence(&self) -> Confidence {
        // Decoded-image provenance is deterministic from its URI, but
        // the image itself is a cache hit until the loader decodes it.
        Confidence {
            source: ConfidenceSource::Cache,
            value: Some(1.0),
            interval: None,
        }
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
        let clickable = self.clickable;
        let tooltip = self.tooltip.clone();

        let mut image = egui::Image::new(self.uri.clone().into_owned());
        image = match self.fit {
            MediaFit::Contain => image.fit_to_fraction(egui::vec2(1.0, 1.0)),
            MediaFit::Cover => image.fit_to_original_size(1.0),
            MediaFit::Stretch => image.fit_to_fraction(egui::vec2(1.0, 1.0)),
        };
        if let Some(sz) = self.max_size {
            image = image.max_size(sz);
        }
        if clickable {
            image = image.sense(egui::Sense::click());
        }

        let mut resp = ui.add(image);
        // egui 0.29 has no Image::alt_text; fall back to hover text.
        if let Some(alt) = self.alt.as_ref() {
            resp = resp.on_hover_text(alt.as_ref());
        }
        if let Some(tt) = &tooltip {
            resp = resp.on_hover_text(tt.as_ref());
        }

        let chosen: Option<&'static str> = if clickable && resp.clicked() {
            Some("open")
        } else {
            None
        };

        CanonResponse::from_egui(resp, Cow::Borrowed(IDENTITY), variant, chosen).with_id_hint(id)
    }
}
