//! `ui://sheet` — scrollable region primitive (ADR-001 row 11).
//!
//! Session-5 §11 maps this to `egui::ScrollArea::vertical` with
//! stick-to-bottom + id_salt. We expose `max-height`, stick-to-bottom,
//! and an optional sticky-header closure that runs in a pinned strip
//! above the scrolling body.

use std::borrow::Cow;

use eframe::egui;

use super::CanonWidget;
use super::response::CanonResponse;
use super::types::{Affordance, Confidence, IdentityUri, MutationAxis, Tooltip, VariantId};

const IDENTITY: &str = "ui://sheet";

static AFFORDANCES_ACTIVE: &[Affordance] = &[
    Affordance {
        name: Cow::Borrowed("scroll"),
        verb: Cow::Borrowed("wsp.invoke"),
        actors: &[],
        args_schema: None,
        reorderable: false,
    },
    Affordance {
        name: Cow::Borrowed("subscribe"),
        verb: Cow::Borrowed("wsp.subscribe"),
        actors: &[],
        args_schema: None,
        reorderable: false,
    },
];

static MUTATION_AXES: &[MutationAxis] = &[
    MutationAxis::new("max-height"),
    MutationAxis::new("sticky-header-ymax"),
];

/// Scrollable region. Takes an optional sticky-header builder and a
/// body builder.
pub struct Sheet<H, B>
where
    H: FnOnce(&mut egui::Ui),
    B: FnOnce(&mut egui::Ui),
{
    id_source: egui::Id,
    max_height: Option<f32>,
    stick_to_bottom: bool,
    tooltip: Option<Tooltip>,
    variant: VariantId,
    header: Option<H>,
    body: B,
}

impl<B> Sheet<fn(&mut egui::Ui), B>
where
    B: FnOnce(&mut egui::Ui),
{
    /// Construct a `Sheet` with no sticky header. Using `fn` as the
    /// unused header type keeps the generic parameter inferrable.
    pub fn new(id_source: impl std::hash::Hash, body: B) -> Self {
        Self {
            id_source: egui::Id::new(("canon.sheet", id_source)),
            max_height: None,
            stick_to_bottom: false,
            tooltip: None,
            variant: 0,
            header: None,
            body,
        }
    }
}

impl<H, B> Sheet<H, B>
where
    H: FnOnce(&mut egui::Ui),
    B: FnOnce(&mut egui::Ui),
{
    /// Replace the (possibly absent) sticky header with `header`. This
    /// changes the `H` type parameter of the returned `Sheet`.
    pub fn with_header<H2>(self, header: H2) -> Sheet<H2, B>
    where
        H2: FnOnce(&mut egui::Ui),
    {
        Sheet {
            id_source: self.id_source,
            max_height: self.max_height,
            stick_to_bottom: self.stick_to_bottom,
            tooltip: self.tooltip,
            variant: self.variant,
            header: Some(header),
            body: self.body,
        }
    }

    pub fn max_height(mut self, h: f32) -> Self {
        self.max_height = Some(h);
        self
    }

    pub fn stick_to_bottom(mut self, stick: bool) -> Self {
        self.stick_to_bottom = stick;
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

impl<H, B> CanonWidget for Sheet<H, B>
where
    H: FnOnce(&mut egui::Ui),
    B: FnOnce(&mut egui::Ui),
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
        let tooltip = self.tooltip.clone();

        let mut resp = ui
            .scope(|ui| {
                if let Some(header) = self.header {
                    // Sticky header strip. Draws first so it stays at
                    // the top of the allocated rect; caller's header
                    // closure owns the visual treatment.
                    ui.horizontal(|ui| header(ui));
                    ui.separator();
                }

                let mut area = egui::ScrollArea::vertical()
                    .id_salt(id)
                    .stick_to_bottom(self.stick_to_bottom);
                if let Some(max) = self.max_height {
                    area = area.max_height(max);
                }
                let out = area.show(ui, self.body);
                // `ScrollArea::show` doesn't return a Response
                // directly — synthesise one from the inner rect so
                // hover/drag on the scrolled region is attributable
                // to this Sheet.
                ui.allocate_rect(out.inner_rect, egui::Sense::hover())
            })
            .inner;

        if let Some(tt) = &tooltip {
            resp = resp.on_hover_text(tt.as_ref());
        }

        CanonResponse::from_egui(resp, Cow::Borrowed(IDENTITY), variant, None).with_id_hint(id)
    }
}
