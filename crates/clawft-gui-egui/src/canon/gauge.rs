//! `ui://gauge` — scalar with bounds + confidence halo. ADR-001 row 16.
//!
//! Linear-only in this retrofit. The canon schema lists `radial-vs-linear`
//! as a legal mutation axis; a radial painter variant is a stretch goal.

use std::borrow::Cow;

use eframe::egui;

use super::CanonWidget;
use super::response::CanonResponse;
use super::types::{
    Affordance, Confidence, ConfidenceSource, IdentityUri, MutationAxis, Tooltip, VariantId,
};

const IDENTITY: &str = "ui://gauge";

static AFFORDANCES: &[Affordance] = &[];

static MUTATION_AXES: &[MutationAxis] = &[
    MutationAxis::new("radial-vs-linear"),
    MutationAxis::new("threshold-labels"),
    MutationAxis::new("tint"),
    MutationAxis::new("copy"),
];

/// Threshold zones. The renderer picks the fill colour based on where
/// `value` falls in `(lo, hi)` and these two thresholds. Thresholds are
/// interpreted as fractions of the bound range.
#[derive(Copy, Clone, Debug)]
pub struct Thresholds {
    pub warn_at: f64,
    pub crit_at: f64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            warn_at: 0.7,
            crit_at: 0.9,
        }
    }
}

pub struct Gauge {
    id_source: egui::Id,
    label: Option<Cow<'static, str>>,
    value: f64,
    bounds: (f64, f64),
    thresholds: Thresholds,
    show_text: bool,
    desired_width: Option<f32>,
    confidence: Confidence,
    tooltip: Option<Tooltip>,
    variant: VariantId,
}

impl Gauge {
    pub fn new(id_source: impl std::hash::Hash, value: f64, bounds: (f64, f64)) -> Self {
        Self {
            id_source: egui::Id::new(("canon.gauge", id_source)),
            label: None,
            value,
            bounds,
            thresholds: Thresholds::default(),
            show_text: true,
            desired_width: None,
            confidence: Confidence::deterministic(),
            tooltip: None,
            variant: 0,
        }
    }

    pub fn label(mut self, label: impl Into<Cow<'static, str>>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn thresholds(mut self, t: Thresholds) -> Self {
        self.thresholds = t;
        self
    }

    pub fn show_text(mut self, show: bool) -> Self {
        self.show_text = show;
        self
    }

    pub fn desired_width(mut self, w: f32) -> Self {
        self.desired_width = Some(w);
        self
    }

    /// Override the confidence head field. Default is `Deterministic(1.0)`;
    /// for gauges fed from inference models, pass a typed `Confidence`.
    pub fn confidence(mut self, c: Confidence) -> Self {
        self.confidence = c;
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

    fn fraction(&self) -> f32 {
        let (lo, hi) = self.bounds;
        let span = hi - lo;
        if span <= 0.0 {
            return 0.0;
        }
        (((self.value - lo) / span).clamp(0.0, 1.0)) as f32
    }

    fn colour_for(&self, pct: f64) -> egui::Color32 {
        if pct >= self.thresholds.crit_at {
            egui::Color32::from_rgb(220, 70, 70)
        } else if pct >= self.thresholds.warn_at {
            egui::Color32::from_rgb(220, 160, 40)
        } else {
            egui::Color32::from_rgb(60, 160, 90)
        }
    }
}

impl CanonWidget for Gauge {
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
        self.confidence
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

        let pct = self.fraction();
        let colour = self.colour_for(pct as f64);
        let (lo, hi) = self.bounds;

        let inner = ui.scope(|ui| {
            if let Some(lbl) = &self.label {
                ui.label(lbl.as_ref());
            }
            let mut bar = egui::ProgressBar::new(pct).fill(colour);
            if self.show_text {
                bar = bar.text(format!("{:.2} / {:.2}", self.value, hi - lo + lo));
            }
            if let Some(w) = self.desired_width {
                bar = bar.desired_width(w);
            } else {
                bar = bar.desired_width(ui.available_width());
            }
            ui.add(bar);

            // Halo: draw an outline whose alpha reflects confidence width.
            // For now we only draw when confidence has a point value and
            // it's not at 1.0 — signals inference or uncertainty visually.
            let is_soft = matches!(
                self.confidence.source,
                ConfidenceSource::Inference | ConfidenceSource::Cache
            );
            if let (true, Some(v)) = (is_soft, self.confidence.value) {
                let alpha = ((1.0 - v.clamp(0.0, 1.0)) * 180.0) as u8;
                let stroke = egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgba_unmultiplied(220, 160, 40, alpha),
                );
                let rect = ui.min_rect();
                ui.painter()
                    .rect_stroke(rect, 4.0, stroke, egui::StrokeKind::Inside);
            }
        });

        let mut resp = inner.response;
        if let Some(tt) = &tooltip {
            resp = resp.on_hover_text(tt.as_ref());
        }

        // Gauges are read-only — no chosen affordance fires.
        let chosen: Option<&'static str> = None;

        CanonResponse::from_egui(resp, Cow::Borrowed(IDENTITY), variant, chosen).with_id_hint(id)
    }
}
