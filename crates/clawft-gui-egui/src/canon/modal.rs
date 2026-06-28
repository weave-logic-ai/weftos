//! `ui://modal` — floating surface with typed modality (ADR-001 row 12,
//! ADR-014 typed modality).
//!
//! One primitive, four behaviours. The caller picks a `Modality` at
//! construction time; the `show()` implementation branches accordingly:
//!
//! - `Modal` — scrim + `egui::Window` anchored centre, not movable.
//!   Consent-flow affordances (`confirm` / `cancel`); mutation-axes
//!   are deliberately empty per foundations §active-radar loop /
//!   ADR-014.
//! - `Floating` — `egui::Window`, movable + resizable, no scrim.
//! - `Tool` — docked-right `egui::Window` styled as a tool pane, not
//!   movable by default.
//! - `Toast` — `egui::Area` at `Order::Tooltip`, auto-dismissing
//!   after `duration_ms` (deadline persisted in `memory.data`).

use std::borrow::Cow;

use eframe::egui;

use super::CanonWidget;
use super::response::CanonResponse;
use super::types::{
    Affordance, Confidence, IdentityUri, Modality, MutationAxis, Tooltip, VariantId,
};

const IDENTITY: &str = "ui://modal";

static AFFORDANCES_MODAL: &[Affordance] = &[
    Affordance {
        name: Cow::Borrowed("confirm"),
        verb: Cow::Borrowed("wsp.activate"),
        actors: &[],
        args_schema: None,
        reorderable: false,
    },
    Affordance {
        name: Cow::Borrowed("cancel"),
        verb: Cow::Borrowed("wsp.activate"),
        actors: &[],
        args_schema: None,
        reorderable: false,
    },
];
static AFFORDANCES_DISMISSABLE: &[Affordance] = &[Affordance {
    name: Cow::Borrowed("dismiss"),
    verb: Cow::Borrowed("wsp.activate"),
    actors: &[],
    args_schema: None,
    reorderable: false,
}];

/// Non-safety modalities expose style / placement. `Modal`'s axes are
/// frozen (see `mutation_axes` dispatch below).
static MUTATION_AXES_NON_SAFETY: &[MutationAxis] =
    &[MutationAxis::new("style"), MutationAxis::new("placement")];
static MUTATION_AXES_FROZEN: &[MutationAxis] = &[];

/// Floating-surface primitive with typed modality.
pub struct Modal<F>
where
    F: FnOnce(&mut egui::Ui),
{
    id_source: egui::Id,
    modality: Modality,
    title: Cow<'static, str>,
    open: bool,
    duration_ms: u64,
    tooltip: Option<Tooltip>,
    variant: VariantId,
    body: F,
}

impl<F> Modal<F>
where
    F: FnOnce(&mut egui::Ui),
{
    pub fn new(
        id_source: impl std::hash::Hash,
        modality: Modality,
        title: impl Into<Cow<'static, str>>,
        body: F,
    ) -> Self {
        Self {
            id_source: egui::Id::new(("canon.modal", id_source)),
            modality,
            title: title.into(),
            open: true,
            duration_ms: 3_000,
            tooltip: None,
            variant: 0,
            body,
        }
    }

    /// Controls whether the surface is currently shown. Modal /
    /// floating surfaces honour this; toasts ignore it in favour of
    /// their own TTL deadline.
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// Toast-only: lifetime in ms before auto-dismiss.
    pub fn duration_ms(mut self, ms: u64) -> Self {
        self.duration_ms = ms;
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

impl<F> CanonWidget for Modal<F>
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
        match self.modality {
            Modality::Modal => AFFORDANCES_MODAL,
            Modality::Floating | Modality::Toast | Modality::Tool => AFFORDANCES_DISMISSABLE,
        }
    }

    fn confidence(&self) -> Confidence {
        Confidence::deterministic()
    }

    fn variant_id(&self) -> VariantId {
        self.variant
    }

    fn mutation_axes(&self) -> &[MutationAxis] {
        match self.modality {
            // ADR-014 + foundations §active-radar loop: consent flows
            // are frozen. No mutation axes on Modal.
            Modality::Modal => MUTATION_AXES_FROZEN,
            Modality::Floating | Modality::Toast | Modality::Tool => MUTATION_AXES_NON_SAFETY,
        }
    }

    fn tooltip(&self) -> Option<&Tooltip> {
        self.tooltip.as_ref()
    }

    fn show(self, ui: &mut egui::Ui) -> CanonResponse {
        let id = self.id_source;
        let variant = self.variant;
        let tooltip = self.tooltip.clone();
        let modality = self.modality;
        let title = self.title;
        let open = self.open;
        let duration_ms = self.duration_ms;
        let ctx = ui.ctx().clone();

        let resp = match modality {
            Modality::Modal => show_modal(ui, id, &title, open, self.body),
            Modality::Floating => show_floating(ui, id, &title, open, false, self.body),
            Modality::Tool => show_floating(ui, id, &title, open, true, self.body),
            Modality::Toast => show_toast(ui, id, duration_ms, self.body),
        };
        let _ = ctx;

        let mut resp = resp;
        if let Some(tt) = &tooltip {
            resp = resp.on_hover_text(tt.as_ref());
        }

        // The specific affordance depends on which control the user
        // touched. The detection is best-effort: a child button press
        // inside the body closure is not visible to us at this level,
        // so we report `dismiss` when the surface is closed this frame
        // and leave `confirm`/`cancel` to the caller to stamp via
        // `with_chosen_affordance` if the body needs that distinction.
        let chosen: Option<&'static str> = None;
        CanonResponse::from_egui(resp, Cow::Borrowed(IDENTITY), variant, chosen).with_id_hint(id)
    }
}

fn show_modal<F>(
    ui: &mut egui::Ui,
    id: egui::Id,
    title: &str,
    open: bool,
    body: F,
) -> egui::Response
where
    F: FnOnce(&mut egui::Ui),
{
    if !open {
        return ui.allocate_response(egui::Vec2::ZERO, egui::Sense::hover());
    }

    // Scrim: paint a translucent layer over the full ctx rect below
    // our own window, so background controls are visually dimmed.
    let ctx = ui.ctx().clone();
    let screen = ctx.content_rect();
    let scrim_layer = egui::LayerId::new(egui::Order::Background, id.with("scrim"));
    let scrim_painter = egui::Painter::new(ctx.clone(), scrim_layer, screen);
    scrim_painter.rect_filled(screen, 0.0, egui::Color32::from_black_alpha(140));

    let mut held_open = true;
    let window = egui::Window::new(title)
        .id(id)
        .order(egui::Order::Foreground)
        .collapsible(false)
        .resizable(false)
        .movable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .open(&mut held_open);

    let response = window.show(&ctx, |ui| body(ui));
    response
        .map(|r| r.response)
        .unwrap_or_else(|| ui.allocate_response(egui::Vec2::ZERO, egui::Sense::hover()))
}

fn show_floating<F>(
    ui: &mut egui::Ui,
    id: egui::Id,
    title: &str,
    open: bool,
    docked_right: bool,
    body: F,
) -> egui::Response
where
    F: FnOnce(&mut egui::Ui),
{
    if !open {
        return ui.allocate_response(egui::Vec2::ZERO, egui::Sense::hover());
    }

    let ctx = ui.ctx().clone();
    let mut held_open = true;
    let mut window = egui::Window::new(title)
        .id(id)
        .collapsible(!docked_right)
        .resizable(true)
        .movable(!docked_right)
        .open(&mut held_open);
    if docked_right {
        window = window.anchor(egui::Align2::RIGHT_CENTER, egui::vec2(-8.0, 0.0));
    }

    let response = window.show(&ctx, |ui| body(ui));
    response
        .map(|r| r.response)
        .unwrap_or_else(|| ui.allocate_response(egui::Vec2::ZERO, egui::Sense::hover()))
}

fn show_toast<F>(ui: &mut egui::Ui, id: egui::Id, duration_ms: u64, body: F) -> egui::Response
where
    F: FnOnce(&mut egui::Ui),
{
    let ctx = ui.ctx().clone();
    // Deadline is stored as f64 (seconds since ctx start) in
    // memory.data. First frame: set deadline = now + duration_ms.
    // Subsequent frames: render until now >= deadline.
    let now_s = ctx.input(|i| i.time);
    let deadline_key = egui::Id::new(("canon.modal.toast.deadline", id));
    let deadline: f64 = ctx.memory_mut(|m| {
        *m.data.get_temp_mut_or_insert_with::<f64>(deadline_key, || {
            now_s + (duration_ms as f64) / 1000.0
        })
    });

    if now_s >= deadline {
        // Clear the stored deadline so a subsequent re-open starts
        // a fresh lifetime.
        ctx.memory_mut(|m| m.data.remove::<f64>(deadline_key));
        return ui.allocate_response(egui::Vec2::ZERO, egui::Sense::hover());
    }

    // Request continuous repaints while the toast is alive so the
    // deadline check triggers even in idle UIs.
    ctx.request_repaint_after(std::time::Duration::from_millis(33));

    let area = egui::Area::new(id)
        .order(egui::Order::Tooltip)
        .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-16.0, 16.0));
    let response = area.show(&ctx, |ui| {
        egui::Frame::popup(&ctx.global_style()).show(ui, body);
    });
    response.response
}
