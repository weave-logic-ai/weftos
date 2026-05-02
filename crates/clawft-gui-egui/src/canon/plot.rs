//! `ui://plot` — continuous time-series. ADR-001 row 17.
//!
//! Wraps `egui_plot::Plot` + `Line`. The canon mutation axes declare
//! `y-autoscale` and `sample-density` as legal GEPA variation points.

use std::borrow::Cow;

use eframe::egui;
use egui_plot::{Line, Plot as EguiPlot, PlotPoints};

use super::response::CanonResponse;
use super::types::{Affordance, Confidence, IdentityUri, MutationAxis, Tooltip, VariantId};
use super::CanonWidget;

const IDENTITY: &str = "ui://plot";

static AFFORDANCES: &[Affordance] = &[
    Affordance {
        name: Cow::Borrowed("zoom"),
        verb: Cow::Borrowed("wsp.update"),
        actors: &[],
        args_schema: None,
        reorderable: false,
    },
    Affordance {
        name: Cow::Borrowed("pan"),
        verb: Cow::Borrowed("wsp.update"),
        actors: &[],
        args_schema: None,
        reorderable: false,
    },
    Affordance {
        name: Cow::Borrowed("reset-bounds"),
        verb: Cow::Borrowed("wsp.update"),
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
    MutationAxis::new("y-autoscale"),
    MutationAxis::new("sample-density"),
    MutationAxis::new("line-colour"),
    MutationAxis::new("show-grid"),
    MutationAxis::new("aspect-ratio"),
];

pub struct Plot<'a> {
    id_source: egui::Id,
    points: &'a [(f64, f64)],
    allow_zoom: bool,
    allow_drag: bool,
    allow_scroll: bool,
    show_axes: [bool; 2],
    view_aspect: Option<f32>,
    y_bounds: Option<(f64, f64)>,
    x_window: Option<(f64, f64)>,
    line_colour: egui::Color32,
    tooltip: Option<Tooltip>,
    variant: VariantId,
}

impl<'a> Plot<'a> {
    pub fn new(id_source: impl std::hash::Hash) -> Self {
        Self {
            id_source: egui::Id::new(("canon.plot", id_source)),
            points: &[],
            allow_zoom: true,
            allow_drag: true,
            allow_scroll: true,
            show_axes: [true, true],
            view_aspect: None,
            y_bounds: None,
            x_window: None,
            line_colour: egui::Color32::from_rgb(120, 220, 160),
            tooltip: None,
            variant: 0,
        }
    }

    pub fn points(mut self, p: &'a [(f64, f64)]) -> Self {
        self.points = p;
        self
    }

    pub fn allow_zoom(mut self, allow: bool) -> Self {
        self.allow_zoom = allow;
        self
    }

    pub fn allow_drag(mut self, allow: bool) -> Self {
        self.allow_drag = allow;
        self
    }

    pub fn allow_scroll(mut self, allow: bool) -> Self {
        self.allow_scroll = allow;
        self
    }

    pub fn show_axes(mut self, axes: [bool; 2]) -> Self {
        self.show_axes = axes;
        self
    }

    pub fn view_aspect(mut self, aspect: f32) -> Self {
        self.view_aspect = Some(aspect);
        self
    }

    pub fn y_bounds(mut self, lo: f64, hi: f64) -> Self {
        self.y_bounds = Some((lo, hi));
        self
    }

    pub fn x_window(mut self, lo: f64, hi: f64) -> Self {
        self.x_window = Some((lo, hi));
        self
    }

    pub fn line_colour(mut self, c: egui::Color32) -> Self {
        self.line_colour = c;
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

impl<'a> CanonWidget for Plot<'a> {
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

        let points: PlotPoints = self.points.iter().map(|&(x, y)| [x, y]).collect();
        let line = Line::new("series", points).color(self.line_colour);

        let mut plot = EguiPlot::new(id)
            .allow_zoom(self.allow_zoom)
            .allow_drag(self.allow_drag)
            .allow_scroll(self.allow_scroll)
            .show_axes(self.show_axes);

        if let Some(a) = self.view_aspect {
            plot = plot.view_aspect(a);
        }
        if let Some((lo, hi)) = self.y_bounds {
            plot = plot.include_y(lo).include_y(hi);
        }
        if let Some((lo, hi)) = self.x_window {
            plot = plot.include_x(lo).include_x(hi);
        }

        let inner = plot.show(ui, |plot_ui| {
            plot_ui.line(line);
        });

        let plot_resp = inner.response;
        let mut resp = plot_resp;
        if let Some(tt) = &tooltip {
            resp = resp.on_hover_text(tt.as_ref());
        }

        // Map egui_plot interactions onto canonical affordances:
        //   drag → pan; scroll → zoom.
        let chosen: Option<&'static str> = if resp.dragged() {
            Some("pan")
        } else if ui.input(|i| i.smooth_scroll_delta.y.abs() > 0.1) && resp.hovered() {
            Some("zoom")
        } else {
            None
        };

        CanonResponse::from_egui(resp, Cow::Borrowed(IDENTITY), variant, chosen).with_id_hint(id)
    }
}
