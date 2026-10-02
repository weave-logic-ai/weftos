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

/// Viewer state: the open page, the search box, and the selected placement.
#[derive(Default)]
pub struct GuideView {
    pub page: Option<String>,
    pub search: String,
    pub placement: usize,
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

    /// Page list (with search) on the left, the page on the right.
    pub fn show(&mut self, ui: &mut egui::Ui, bundle: &GuideBundle) {
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
