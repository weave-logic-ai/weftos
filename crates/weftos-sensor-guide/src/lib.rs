//! Sensor guides that ship with sensor cogs (ADR-104).
//!
//! A cog carries `guide/guide.toml` (pages + diagram data: header pins, parts and wires, body
//! placements, signal flow) and one Markdown file per page, and serves the bundle as JSON at
//! `/guide`. This crate validates a bundle and renders it in egui as a small searchable wiki
//! with painted diagrams, natively and in the browser. Companion apps embed [`GuideView`].

pub mod bundle;
pub mod diagrams;

pub use bundle::{GuideBundle, GuideDoc, GuidePage, Segment};

use egui::RichText;
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

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
            ui.label(RichText::new(format!("{} sensors", sensors.len())).weak().small());
        });
        let s = &sensors[self.sensor];
        let banner = if s.location.is_empty() {
            format!("Configuring: {}", s.name)
        } else {
            format!("Configuring: {}  ·  {}", s.name, s.location)
        };
        ui.label(
            RichText::new(banner)
                .strong()
                .color(egui::Color32::WHITE)
                .background_color(egui::Color32::from_rgb(38, 96, 58)),
        );
        if !s.detail.is_empty() {
            ui.label(RichText::new(&s.detail).small().weak());
        }
        ui.add_space(2.0);
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
                ui.label(RichText::new(&bundle.doc.title).strong());
                if !bundle.doc.cog.is_empty() {
                    ui.label(
                        RichText::new(format!("cog {} {}", bundle.doc.cog, bundle.doc.cog_version))
                            .small()
                            .weak(),
                    );
                }
                ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("search the guide"));
                ui.add_space(6.0);
                let q = self.search.to_lowercase();
                for p in &bundle.pages {
                    if !q.is_empty() && !p.markdown.to_lowercase().contains(&q) {
                        continue;
                    }
                    if ui
                        .selectable_label(p.id == current, RichText::new(&p.title).strong())
                        .on_hover_text(&p.summary)
                        .clicked()
                    {
                        self.page = Some(p.id.clone());
                    }
                }
                if !bundle.doc.medical {
                    ui.add_space(10.0);
                    ui.label(
                        RichText::new("Not a medical device.")
                            .small()
                            .color(egui::Color32::from_rgb(230, 170, 40)),
                    );
                }
            });
        let Some(page) = bundle.page(&current) else {
            return;
        };
        egui::ScrollArea::both()
            .id_salt(("sensor_guide_page", &page.id))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(4.0);
                for seg in bundle::segments(&page.markdown) {
                    match seg {
                        Segment::Text(t) => {
                            CommonMarkViewer::new().show(ui, &mut self.cache, &displayable(&t));
                        }
                        Segment::Image { alt, name } => {
                            ui.add_space(4.0);
                            if bundle.images.contains_key(&name) {
                                let uri = std::borrow::Cow::Owned(format!("bytes://{name}"));
                                let w = ui.available_width().min(760.0);
                                ui.add(egui::Image::new(egui::ImageSource::Uri(uri)).max_width(w).corner_radius(4.0));
                            } else {
                                ui.label(RichText::new(format!("[image '{name}' not bundled]")).weak());
                            }
                            if !alt.is_empty() {
                                ui.label(RichText::new(&alt).small().weak());
                            }
                            ui.add_space(6.0);
                        }
                        Segment::Diagram(kind) => {
                            ui.add_space(4.0);
                            match kind.as_str() {
                                "header" => diagrams::header(ui, &bundle.doc),
                                "wiring" => diagrams::wiring(ui, &bundle.doc),
                                "placements" => {
                                    diagrams::placements(ui, &bundle.doc, &mut self.placement)
                                }
                                "flow" => diagrams::flow(ui, &bundle.doc),
                                "grid" => diagrams::grid(ui, &bundle.doc),
                                other => {
                                    ui.label(
                                        RichText::new(format!("(unknown diagram '{other}')"))
                                            .weak(),
                                    );
                                }
                            }
                            ui.add_space(6.0);
                        }
                    }
                }
                ui.add_space(24.0);
            });
    }
}
