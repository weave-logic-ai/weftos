//! Shared visual language for the appliance console.
//!
//! One small type scale, one vertical rhythm, one card frame and one status pill, so the
//! Cogs list, Sensors list, Catalog cards and Network tables read as the same system
//! instead of a pile of ad-hoc `.size()` / `.small()` calls. Colours carry meaning
//! (status / source), never decoration, and the dim/muted text is pulled from the active
//! egui theme so it stays legible in both light and dark.

use eframe::egui::{self, Color32, Response, RichText, Ui};

// ---- vertical rhythm: a few steps, used everywhere instead of stray add_space values ----
/// Tight gap between rows inside a group (e.g. between cards).
pub const GAP_XS: f32 = 4.0;
/// Standard gap between a heading and its content, or between related blocks.
pub const GAP_S: f32 = 8.0;
/// Gap that separates one section's content from the next heading.
pub const GAP_M: f32 = 16.0;
/// Large gap before a new top-level section / `separator`.
pub const GAP_L: f32 = 24.0;

// ---- type scale ----
const H1_SIZE: f32 = 20.0;
const H2_SIZE: f32 = 16.0;
const BODY_SIZE: f32 = 13.5;
const SMALL_SIZE: f32 = 12.0;

/// Primary title (the product name in the top bar).
pub fn h1(text: impl Into<String>) -> RichText {
    RichText::new(text).size(H1_SIZE).strong()
}

/// Section heading inside a view. Consistent size/weight for every view's title.
pub fn h2(ui: &mut Ui, text: impl Into<String>) -> Response {
    ui.label(RichText::new(text).size(H2_SIZE).strong())
}

/// The one-line grey caption that sits under a heading and explains the section.
pub fn caption(ui: &mut Ui, text: impl Into<String>) -> Response {
    ui.label(RichText::new(text).size(SMALL_SIZE).color(muted(ui)))
}

/// Dim body text that adapts to the theme (brighter than the fixed GREY const on light bg).
pub fn dim(ui: &Ui, text: impl Into<String>) -> RichText {
    RichText::new(text).size(SMALL_SIZE).color(muted(ui))
}

/// Normal-weight body text at the shared body size.
pub fn body(text: impl Into<String>) -> RichText {
    RichText::new(text).size(BODY_SIZE)
}

/// The theme's muted text colour — legible on both light and dark backgrounds.
pub fn muted(ui: &Ui) -> Color32 {
    ui.visuals().weak_text_color()
}

/// A heading + caption pair with the standard rhythm around it. Every view opens with one
/// so titles line up and the spacing below is identical across tabs.
pub fn section_header(ui: &mut Ui, title: impl Into<String>, sub: impl Into<String>) {
    h2(ui, title);
    caption(ui, sub);
    ui.add_space(GAP_S);
}

/// An amber sub-section label (e.g. "Tailnet fleet", a marketplace category). Smaller and
/// lighter than `h2` so the hierarchy heading → sub-section → rows is unambiguous.
pub fn subhead(ui: &mut Ui, text: impl Into<String>) -> Response {
    ui.add_space(GAP_XS);
    let r = ui.label(RichText::new(text).size(13.0).strong().color(Color32::from_rgb(0xd8, 0xa0, 0x3a)));
    ui.add_space(GAP_XS);
    r
}

/// A small rounded chip for a status, kind or source tag. The colour is the meaning; the
/// fill/stroke are derived from it so every pill looks like the same component.
pub fn pill(ui: &mut Ui, text: &str, color: Color32) -> Response {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.16))
        .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.5)))
        .corner_radius(egui::CornerRadius::same(7))
        .inner_margin(egui::Margin::symmetric(6, 1))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(11.0).color(color));
        })
        .response
}

/// A painted filled circle. The bundled fonts have no `●`, so a glyph renders as an empty box.
pub fn dot(ui: &mut Ui, color: Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
    resp
}

/// A filled status dot + label, aligned on the baseline. Used for connection / peer state.
pub fn status_dot(ui: &mut Ui, color: Color32, text: impl Into<String>) {
    ui.horizontal(|ui| {
        dot(ui, color);
        ui.label(body(text));
    });
}

/// The uniform card frame: faint fill, hairline border, consistent inner padding and
/// rounding. Catalog cards, marketplace rows and anything "component-shaped" use this.
pub fn card_frame(ui: &Ui) -> egui::Frame {
    let v = ui.visuals();
    egui::Frame::new()
        .fill(v.faint_bg_color)
        .stroke(egui::Stroke::new(1.0, v.widgets.noninteractive.bg_stroke.color))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(12, 10))
}

/// A collapsing "card": uniform frame, a scannable header row (rendered by `header` —
/// put name + a key pill + vendor there so it reads collapsed), and a body that only
/// appears when expanded. Replaces the bare `CollapsingHeader` pile so every card in the
/// catalog shares one frame, one header rhythm and one disclosure affordance.
pub fn card(
    ui: &mut Ui,
    id_salt: impl std::hash::Hash,
    header: impl FnOnce(&mut Ui),
    body: impl FnOnce(&mut Ui),
) {
    card_focus(ui, id_salt, false, header, body);
}

/// [`card`], optionally forced open and scrolled into view (a deep link landed on it).
pub fn card_focus(
    ui: &mut Ui,
    id_salt: impl std::hash::Hash,
    focus: bool,
    header: impl FnOnce(&mut Ui),
    body: impl FnOnce(&mut Ui),
) {
    let frame = card_frame(ui).show(ui, |ui| {
        let id = ui.make_persistent_id(id_salt);
        let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);
        if focus {
            state.set_open(true);
        }
        state
            .show_header(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                header(ui);
            })
            .body_unindented(|ui| {
                ui.add_space(GAP_XS);
                body(ui);
            });
    });
    if focus {
        frame.response.scroll_to_me(Some(egui::Align::TOP));
    }
    ui.add_space(GAP_XS);
}

/// A truncating label with an ellipsis and a hover tooltip carrying the full string, so a
/// long name/vendor never overflows the row or pushes other columns out of alignment.
pub fn truncated(ui: &mut Ui, text: &str, strong: bool) -> Response {
    let mut rt = RichText::new(text).size(BODY_SIZE);
    if strong {
        rt = rt.strong();
    }
    ui.add(egui::Label::new(rt).truncate()).on_hover_text(text)
}

/// A key/value metadata grid used in the body of every catalog card, so specs line up the
/// same way whether they belong to a module, chip or project.
pub fn spec_grid<'a>(
    ui: &mut Ui,
    id_salt: impl std::hash::Hash,
    rows: impl Iterator<Item = (&'a str, &'a str)>,
) {
    egui::Grid::new(ui.make_persistent_id(id_salt))
        .num_columns(2)
        .spacing([14.0, 3.0])
        .show(ui, |ui| {
            for (k, v) in rows {
                ui.label(dim(ui, k));
                // long vendor/source strings truncate with the full text on hover, so one row
                // never widens the whole panel past the window
                ui.add(egui::Label::new(body(v)).truncate()).on_hover_text(v);
                ui.end_row();
            }
        });
}
