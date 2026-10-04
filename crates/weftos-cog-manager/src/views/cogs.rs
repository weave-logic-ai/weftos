//! The Cogs tab: what the host runs, and the marketplace.

use crate::app::Manager;
use crate::client::{HostCog, HostStatus};
use crate::sensor_link::{self, fmt_dur};
use crate::style;
use crate::{AMBER, COG, GREEN, GREY, RED, WL};
use eframe::egui::{self, RichText};
use weftos_cog_market::{Catalog, CatalogItem, Source};

pub(crate) fn dot(ui: &mut egui::Ui, on: bool, label: &str) {
    style::status_dot(ui, if on { GREEN } else { RED }, label);
}

fn source_badge(ui: &mut egui::Ui, source: Source) {
    let (c, t) = match source {
        Source::WeaveLogic => (WL, "WeaveLogic"),
        Source::Cognitum => (COG, "Cognitum"),
    };
    style::pill(ui, t, c);
}

fn source_badge_str(ui: &mut egui::Ui, source: &str) {
    let (c, t) = match source {
        "weavelogic" => (WL, "WeaveLogic"),
        "cognitum" => (COG, "Cognitum"),
        _ => (GREY, "local"),
    };
    style::pill(ui, t, c);
}

impl Manager {
    pub(crate) fn cogs_view(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let (host, catalog, action) = {
            let sh = self.client.snapshot();
            (sh.host.clone(), sh.catalog.clone(), sh.last_action.clone())
        };

        style::section_header(ui, "Running on this OS", "cogs supervised by weft-cog-host — restarted on exit, no 3-cog agent cap");
        match &host {
            Some(Ok(h)) => self.running_table(ui, ctx, h),
            Some(Err(e)) => {
                ui.colored_label(RED, format!("Can't reach the cog-host: {e}"));
                ui.label(style::dim(ui, "Start it on the appliance: weft-cog-host serve --port 9480, then set the host above."));
            }
            None => {
                ui.label(style::dim(ui, "Connecting to the host…"));
            }
        }
        if let Some(a) = &action {
            ui.add_space(style::GAP_XS);
            ui.label(style::dim(ui, a));
        }

        ui.add_space(style::GAP_L);
        ui.separator();
        ui.add_space(style::GAP_S);
        style::section_header(ui, "Marketplace", "WeaveLogic (signed) + a mirror of Cognitum's registry — one catalog");
        match &catalog {
            Some(cat) => self.marketplace(ui, ctx, cat, host.as_ref().and_then(|r| r.as_ref().ok())),
            None => {
                ui.label(style::dim(ui, "Loading the marketplace…"));
            }
        }
    }

    /// Small links from a cog to the hardware it drives; clicking opens that module in the Catalog.
    pub(crate) fn hw_links(&self, ui: &mut egui::Ui, cog_id: &str) {
        let mods = self.catalog.modules_for_cog(cog_id);
        for m in mods.iter().take(2) {
            if ui.small_button(RichText::new(format!("🔧 {}", sensor_link::short_name(&m.name))).color(WL)).on_hover_text(format!("Needs: {} — open it in the Catalog", m.name)).clicked() {
                *self.pending_hw.borrow_mut() = Some(m.id.clone());
            }
        }
        if mods.len() > 2 {
            ui.label(style::dim(ui, format!("+{}", mods.len() - 2)));
        }
    }

    pub(crate) fn running_table(&self, ui: &mut egui::Ui, ctx: &egui::Context, h: &HostStatus) {
        if h.cogs.is_empty() {
            ui.label(RichText::new("No cogs installed on the host yet. Add one from the marketplace below, or stage with `weft-cog-host add`.").color(GREY));
            return;
        }
        egui::Grid::new("running").num_columns(6).striped(true).spacing([14.0, 6.0]).show(ui, |ui| {
            for head in ["cog", "source", "state", "pid / RSS", "restarts", ""] {
                ui.label(RichText::new(head).strong().small());
            }
            ui.end_row();
            for c in &h.cogs {
                ui.horizontal(|ui| {
                    if c.signed {
                        ui.label(RichText::new("🛡").color(WL)).on_hover_text("signed WeaveLogic cog");
                    }
                    ui.label(RichText::new(&c.id).strong());
                    ui.label(RichText::new(format!("v{}", c.version)).color(GREY).small());
                    self.hw_links(ui, &c.id);
                });
                source_badge_str(ui, &c.source);
                self.state_cell(ui, c);
                ui.label(match c.pid {
                    Some(p) => format!("{p} / {}", c.rss_kb.map(|k| format!("{} MB", k / 1024)).unwrap_or_else(|| "—".into())),
                    None => "—".into(),
                });
                ui.label(if c.restarts > 0 { c.restarts.to_string() } else { "—".into() });
                ui.horizontal(|ui| {
                    if c.running {
                        if ui.button(RichText::new("Stop").color(RED)).clicked() {
                            self.client.lifecycle(&c.id, "stop", ctx);
                        }
                    } else if ui.button(RichText::new("Start").color(GREEN)).clicked() {
                        self.client.lifecycle(&c.id, "start", ctx);
                    }
                });
                ui.end_row();
            }
        });
    }

    pub(crate) fn state_cell(&self, ui: &mut egui::Ui, c: &HostCog) {
        ui.horizontal(|ui| {
            if c.running {
                style::dot(ui, GREEN);
                ui.label(format!("running {}", c.uptime_s.map(fmt_dur).unwrap_or_default()));
            } else if c.enabled {
                style::dot(ui, AMBER);
                ui.label("starting…").on_hover_text(c.last_exit.clone().unwrap_or_default());
            } else {
                style::dot(ui, GREY);
                ui.label("stopped");
            }
        });
    }

    pub(crate) fn marketplace(&self, ui: &mut egui::Ui, ctx: &egui::Context, cat: &Catalog, host: Option<&HostStatus>) {
        let installed = |id: &str| host.is_some_and(|h| h.cogs.iter().any(|c| c.id == id));
        egui::ScrollArea::vertical().max_height(380.0).auto_shrink([false, false]).show(ui, |ui| {
            let mut cat_name = String::new();
            for item in &cat.items {
                if item.category != cat_name {
                    cat_name = item.category.clone();
                    ui.add_space(style::GAP_S);
                    ui.label(RichText::new(if cat_name.is_empty() { "other" } else { &cat_name }).strong().color(AMBER).size(13.0));
                    ui.add_space(style::GAP_XS);
                }
                self.market_row(ui, ctx, item, installed(&item.id));
            }
        });
    }

    pub(crate) fn market_row(&self, ui: &mut egui::Ui, ctx: &egui::Context, item: &CatalogItem, installed: bool) {
        style::card_frame(ui).show(ui, |ui| {
            ui.horizontal(|ui| {
                if item.signed {
                    ui.label(RichText::new("🛡").color(WL)).on_hover_text("Ed25519-signed; verified before install");
                }
                style::truncated(ui, &item.name, true);
                ui.label(style::dim(ui, format!("v{}", item.version)));
                source_badge(ui, item.source);
                self.hw_links(ui, &item.id);
                if item.also_in_other_source {
                    ui.label(style::dim(ui, "(also upstream)"));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if installed {
                        style::pill(ui, "✓ installed", GREEN);
                    } else {
                        let resp = ui.add(egui::Button::new("Install"));
                        let resp = if item.signed {
                            resp.on_hover_text("Fetch + Ed25519-verify against the pinned WeaveLogic key, then install on the host")
                        } else {
                            resp.on_hover_text("Cognitum cog (unsigned): fetched and sha256-checked on the host before it lands")
                        };
                        if resp.clicked() {
                            self.client.install(&item.id, item.source, item.version.clone(), ctx);
                        }
                    }
                    ui.label(style::dim(ui, item.arches.join("/")));
                });
            });
            if !item.description.is_empty() {
                ui.label(style::dim(ui, &item.description));
            }
        });
        ui.add_space(style::GAP_XS);
    }
}
