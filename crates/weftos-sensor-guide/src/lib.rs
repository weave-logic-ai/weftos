//! Sensor guides that ship with sensor cogs (ADR-104).
//!
//! A cog carries `guide/guide.toml` (pages + diagram data: header pins, parts and wires, body
//! placements, signal flow) and one Markdown file per page, and serves the bundle as JSON at
//! `/guide`. This crate validates a bundle and renders it in egui as a small searchable wiki
//! with painted diagrams, natively and in the browser. Companion apps embed [`GuideView`].

pub mod bundle;
pub mod diagrams;

pub use bundle::{GuideBundle, GuideDoc, GuidePage, Segment};

use egui::{Color32, RichText};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

/// WeaveLogic green, used for the "Configuring:" banner (legible on both egui themes).
const BANNER_BG: Color32 = Color32::from_rgb(38, 96, 58);
/// Amber caution used for the medical-device disclaimer.
const CAUTION: Color32 = Color32::from_rgb(230, 170, 40);

/// Legible type scale for guide chrome, replacing egui's `.small()` (~10.5 px) which is hard to
/// read on the dark theme. `BODY` is for labels and buttons, `CAPTION` for secondary text — both
/// stay well above egui's default so nothing is cramped or tiny.
const BODY: f32 = 15.0;
const CAPTION: f32 = 13.0;

/// A secondary text colour that keeps adequate contrast on either theme. egui's `weak` colour
/// blends ~55 % into the background (faint, low-contrast on dark); this blends only ~25 %, so
/// captions read as secondary yet stay clearly legible. Used instead of `.weak()` everywhere.
fn muted(ui: &egui::Ui) -> Color32 {
    let v = ui.visuals();
    blend(v.text_color(), v.window_fill(), 0.25)
}

fn blend(a: Color32, b: Color32, t: f32) -> Color32 {
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

/// egui's default fonts lack arrows, comparison signs and ticks; show ASCII equivalents
/// (authors may use either; ADR-104).
pub fn displayable(md: &str) -> String {
    const MAP: [(&str, &str); 10] = [
        ("→", "->"),
        ("←", "<-"),
        ("⇒", "=>"),
        ("↔", "<->"),
        ("≥", ">="),
        ("≤", "<="),
        ("≈", "~"),
        ("✓", "[ok]"),
        ("✕", "x"),
        ("✗", "x"),
    ];
    MAP.iter()
        .fold(md.to_string(), |s, (from, to)| s.replace(from, to))
}

/// Viewer state: the open page, the search box, the selected placement, and (for multi-sensor cogs)
/// the selected sensor plus its filter box and which embedded images have been registered.
#[derive(Default)]
pub struct GuideView {
    pub page: Option<String>,
    pub search: String,
    pub placement: usize,
    pub sensor: usize,
    sensor_filter: String,
    loaded_images: std::collections::BTreeSet<String>,
    /// Whether the full-screen pinout/help overlay is open (the "Show pinout / Help" button).
    pinout_open: bool,
    cache: CommonMarkCache,
}

impl GuideView {
    /// A view that opens on `page` (or the first page when `None`).
    pub fn at(page: Option<String>) -> Self {
        Self {
            page,
            ..Default::default()
        }
    }

    /// Opens the page a checklist step links to (via `[links]` in guide.toml), if any.
    pub fn open_for_step(&mut self, bundle: &GuideBundle, step_id: &str) -> bool {
        match bundle.doc.links.get(step_id) {
            Some(page) => {
                self.page = Some(page.clone());
                true
            }
            None => false,
        }
    }

    /// A searchable sensor picker + a "Configuring: X" banner, for cogs that read more than one
    /// sensor. The picker is a combo box with a filter so it scales from two to hundreds.
    fn sensor_bar(&mut self, ui: &mut egui::Ui, bundle: &GuideBundle) {
        let sensors = &bundle.doc.sensors;
        if sensors.is_empty() {
            return;
        }
        self.sensor = self.sensor.min(sensors.len() - 1);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Sensor").strong());
            egui::ComboBox::from_id_salt("guide_sensor_pick")
                .width(300.0)
                .selected_text(sensors[self.sensor].name.clone())
                .show_ui(ui, |ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.sensor_filter).hint_text("filter sensors"));
                    let q = self.sensor_filter.to_lowercase();
                    for (i, s) in sensors.iter().enumerate() {
                        let hay = format!("{} {} {}", s.name, s.location, s.id).to_lowercase();
                        if !q.is_empty() && !hay.contains(&q) {
                            continue;
                        }
                        let label = if s.location.is_empty() { s.name.clone() } else { format!("{}  —  {}", s.name, s.location) };
                        if ui.selectable_label(i == self.sensor, label).clicked() {
                            self.sensor = i;
                            if !s.page.is_empty() {
                                self.page = Some(s.page.clone());
                            }
                        }
                    }
                });
            let m = muted(ui);
            ui.label(RichText::new(format!("{} sensors", sensors.len())).size(CAPTION).color(m));
        });
        ui.add_space(4.0);
        // "Configuring: X" banner as a rounded panel with real padding, instead of a bare
        // highlighted label — reads as a persistent context chip.
        let s = &sensors[self.sensor];
        egui::Frame::new()
            .fill(BANNER_BG)
            .corner_radius(egui::CornerRadius::same(6))
            .inner_margin(egui::Margin::symmetric(10, 5))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Configuring").size(13.0).color(Color32::from_rgb(190, 224, 203)));
                    ui.label(RichText::new(&s.name).strong().color(Color32::WHITE));
                    if !s.location.is_empty() {
                        ui.label(RichText::new(format!("· {}", s.location)).color(Color32::from_rgb(210, 230, 218)));
                    }
                });
            });
        if !s.detail.is_empty() {
            ui.add_space(4.0);
            let m = muted(ui);
            ui.label(RichText::new(&s.detail).size(CAPTION).color(m));
        }
        ui.add_space(6.0);
    }

    /// Register this bundle's embedded images with the egui context (once each) so `![](name)`
    /// figures resolve against `bytes://name`.
    fn register_images(&mut self, ui: &egui::Ui, bundle: &GuideBundle) {
        if bundle.images.is_empty() {
            return;
        }
        egui_extras::install_image_loaders(ui.ctx());
        for (name, bytes) in &bundle.images {
            if self.loaded_images.insert(name.clone()) {
                ui.ctx().include_bytes(format!("bytes://{name}"), bytes.clone());
            }
        }
    }

    /// Page list (with search) on the left, the page on the right.
    pub fn show(&mut self, ui: &mut egui::Ui, bundle: &GuideBundle) {
        self.register_images(ui, bundle);
        self.sensor_bar(ui, bundle);
        if !bundle.doc.sensors.is_empty() {
            ui.separator();
        }
        let current = self
            .page
            .clone()
            .filter(|p| bundle.page(p).is_some())
            .or_else(|| bundle.pages.first().map(|p| p.id.clone()));
        let Some(current) = current else {
            ui.label("This guide has no pages.");
            return;
        };
        egui::Panel::left("sensor_guide_nav")
            .resizable(false)
            .exact_size(220.0)
            .show_inside(ui, |ui| {
                ui.label(RichText::new(&bundle.doc.title).size(17.0).strong());
                if !bundle.doc.cog.is_empty() {
                    let m = muted(ui);
                    ui.label(
                        RichText::new(format!("cog {} {}", bundle.doc.cog, bundle.doc.cog_version))
                            .size(CAPTION)
                            .color(m),
                    );
                }
                ui.add_space(8.0);
                ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("🔍 search the guide").desired_width(f32::INFINITY));
                ui.add_space(8.0);
                let q = self.search.to_lowercase();
                let mut shown = 0;
                for p in &bundle.pages {
                    if !q.is_empty() && !p.markdown.to_lowercase().contains(&q) {
                        continue;
                    }
                    shown += 1;
                    // Selected page reads as a highlighted row (built-in selectable hover/active
                    // states), not just bolder text.
                    if ui
                        .selectable_label(p.id == current, RichText::new(&p.title))
                        .on_hover_text(&p.summary)
                        .clicked()
                    {
                        self.page = Some(p.id.clone());
                    }
                }
                if shown == 0 {
                    let m = muted(ui);
                    ui.label(RichText::new("no pages match").size(CAPTION).color(m));
                }
                if !bundle.doc.medical {
                    ui.add_space(12.0);
                    ui.separator();
                    ui.add_space(6.0);
                    ui.label(RichText::new("Not a medical device.").size(CAPTION).color(CAUTION));
                }
            });
        let Some(page) = bundle.page(&current) else {
            return;
        };
        egui::ScrollArea::both()
            .id_salt(("sensor_guide_page", &page.id))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(6.0);
                let mut pinout_button_drawn = false;
                for seg in bundle::segments(&page.markdown) {
                    match seg {
                        Segment::Text(t) => {
                            CommonMarkViewer::new().show(ui, &mut self.cache, &displayable(&t));
                        }
                        // The pinout photo and the header pin diagram are "how do I find the
                        // port?" helpers — tiny and in the way inline. One button collapses both
                        // behind the full-screen overlay (see `pinout_modal`), shown once where
                        // the first such element would have appeared.
                        Segment::Image { .. } => {
                            if !pinout_button_drawn {
                                pinout_button_drawn = true;
                                self.pinout_button(ui);
                            }
                        }
                        Segment::Diagram(kind) if kind == "header" => {
                            if !pinout_button_drawn {
                                pinout_button_drawn = true;
                                self.pinout_button(ui);
                            }
                        }
                        // Content diagrams stay inline — they are the page, not port-finding.
                        Segment::Diagram(kind) => {
                            ui.add_space(6.0);
                            match kind.as_str() {
                                "wiring" => diagrams::wiring(ui, &bundle.doc),
                                "placements" => {
                                    diagrams::placements(ui, &bundle.doc, &mut self.placement)
                                }
                                "flow" => diagrams::flow(ui, &bundle.doc),
                                "grid" => diagrams::grid(ui, &bundle.doc),
                                other => {
                                    let m = muted(ui);
                                    ui.label(
                                        RichText::new(format!("(unknown diagram '{other}')"))
                                            .size(CAPTION)
                                            .color(m),
                                    );
                                }
                            }
                            ui.add_space(8.0);
                        }
                    }
                }
                ui.add_space(24.0);
            });
        if self.pinout_open {
            self.pinout_modal(ui, bundle, &current);
        }
    }

    /// Inline affordance that opens the full-screen pinout overlay ([`Self::pinout_modal`]).
    fn pinout_button(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        let m = muted(ui);
        if ui
            .add(
                egui::Button::new(RichText::new("Show pinout / Help").size(BODY))
                    .min_size(egui::vec2(0.0, 34.0)),
            )
            .on_hover_text("Open the port location photo and pin diagram full-screen")
            .clicked()
        {
            self.pinout_open = true;
        }
        ui.add_space(2.0);
        ui.label(
            RichText::new("Shows where the port is and which pins to use, full-screen.")
                .size(CAPTION)
                .color(m),
        );
        ui.add_space(8.0);
    }

    /// Full-screen overlay with the header pin diagram and every bundled photo for `page_id`,
    /// sized to ~90 % of the screen so the pinout is actually readable. A backdrop click, Escape,
    /// or the Close button dismiss it. Works natively and in the browser (egui `Modal`).
    fn pinout_modal(&mut self, ui: &mut egui::Ui, bundle: &GuideBundle, page_id: &str) {
        let Some(page) = bundle.page(page_id) else {
            self.pinout_open = false;
            return;
        };
        let ctx = ui.ctx().clone();
        let screen = ctx.content_rect();
        let width = (screen.width() * 0.92).clamp(320.0, 1200.0);
        let max_h = (screen.height() * 0.8).max(240.0);
        let modal = egui::Modal::new(egui::Id::new("guide_pinout_modal")).show(&ctx, |ui| {
            ui.set_width(width);
            let mut close = false;
            ui.horizontal(|ui| {
                ui.label(RichText::new("Pinout & port location").size(20.0).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(
                            egui::Button::new(RichText::new("Close").size(BODY))
                                .min_size(egui::vec2(0.0, 34.0)),
                        )
                        .clicked()
                    {
                        close = true;
                    }
                });
            });
            ui.add_space(4.0);
            let m = muted(ui);
            ui.label(
                RichText::new(
                    "Match the highlighted pins. Click outside this panel or press Esc to close.",
                )
                .size(CAPTION)
                .color(m),
            );
            ui.separator();
            egui::ScrollArea::vertical()
                .max_height(max_h)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let mut any = false;
                    for seg in bundle::segments(&page.markdown) {
                        match seg {
                            Segment::Diagram(kind) if kind == "header" => {
                                any = true;
                                ui.add_space(6.0);
                                diagrams::header(ui, &bundle.doc);
                                ui.add_space(12.0);
                            }
                            Segment::Image { alt, name } => {
                                any = true;
                                ui.add_space(6.0);
                                if bundle.images.contains_key(&name) {
                                    let uri = std::borrow::Cow::Owned(format!("bytes://{name}"));
                                    let w = ui.available_width().min(1100.0);
                                    ui.add(
                                        egui::Image::new(egui::ImageSource::Uri(uri))
                                            .max_width(w)
                                            .corner_radius(6.0),
                                    );
                                } else {
                                    let m = muted(ui);
                                    ui.label(
                                        RichText::new(format!("[image '{name}' not bundled]"))
                                            .size(CAPTION)
                                            .color(m),
                                    );
                                }
                                if !alt.is_empty() {
                                    ui.add_space(2.0);
                                    let m = muted(ui);
                                    ui.label(RichText::new(&alt).size(CAPTION).color(m));
                                }
                                ui.add_space(12.0);
                            }
                            _ => {}
                        }
                    }
                    if !any {
                        let m = muted(ui);
                        ui.label(
                            RichText::new("No pinout diagram or photo on this page.")
                                .size(BODY)
                                .color(m),
                        );
                    }
                });
            close
        });
        if modal.should_close() || modal.inner {
            self.pinout_open = false;
        }
    }
}
