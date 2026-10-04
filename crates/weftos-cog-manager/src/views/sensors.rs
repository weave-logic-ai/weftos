//! The Sensors tab: every bundled hook-up guide, and the guide reader.

use crate::app::Manager;
use crate::client::HostStatus;
use crate::sensor_install;
use crate::style;
use crate::{AMBER, GREEN, GREY};
use eframe::egui::{self, RichText};
use weftos_sensor_guide::{GuideBundle, GuideView};

impl Manager {
    pub(crate) fn sensors_view(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        // Guide open? Render it; otherwise list every sensor whose hook-up guide ships with the
        // catalog. Reading a guide needs no host, no install and no running cog.
        if self.guide_cog.is_some() {
            self.guide_detail(ui, ctx);
            return;
        }

        style::section_header(ui, "Sensors", "hook-up guides: wiring, pins, bus, power. They ship with the catalog, so read one before you install");
        let host: Option<HostStatus> = self.client.snapshot().host.clone().and_then(|r| r.ok());
        for (id, _) in weftos_cog_market::guides::GUIDES {
            let cog = host.as_ref().and_then(|h| h.cogs.iter().find(|c| c.id == *id));
            let mut open_guide = false;
            style::card_frame(ui).show(ui, |ui| {
                ui.horizontal(|ui| {
                    style::truncated(ui, id, true);
                    match cog {
                        Some(c) if c.running => style::pill(ui, "running", GREEN),
                        Some(_) => style::pill(ui, "installed", AMBER),
                        None => style::pill(ui, "not installed", GREY),
                    };
                    self.hw_links(ui, id);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        open_guide = ui.button("📖  Open guide").clicked();
                    });
                });
            });
            ui.add_space(style::GAP_XS);
            if open_guide {
                self.open_guide(id, None, Some(ctx));
            }
        }
        ui.add_space(style::GAP_M);
        ui.label(style::dim(ui, "Live dashboards (ECG waveform, ToF heatmap, generic trace) come next.").italics());
    }

    /// Render the open cog's `/guide` bundle (fetched into `Shared.guide`), with a back button and
    /// an editable export port for cogs whose default port we don't know.
    pub(crate) fn guide_detail(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let id = self.guide_cog.clone().unwrap_or_default();
        ui.horizontal(|ui| {
            if ui.button("‹  Sensors").clicked() {
                self.guide_cog = None;
                self.guide_bundle = None;
                self.client.clear_guide();
            }
            ui.add_space(style::GAP_S);
            style::h2(ui, format!("{id} — guide"));
        });
        ui.add_space(style::GAP_XS);
        if !self.guide_source.is_empty() {
            ui.label(style::dim(ui, format!("Guide source: {}", self.guide_source)));
        }
        let from_cog = self.guide_source.starts_with("served by");
        if from_cog {
            ui.horizontal(|ui| {
                ui.label(style::dim(ui, "export port"));
                ui.add(egui::TextEdit::singleline(&mut self.guide_port_draft).desired_width(70.0));
                let load = ui.button("Load").clicked().then(|| self.guide_port_draft.trim().parse::<u16>().ok()).flatten();
                if let Some(p) = load {
                    self.guide_bundle = None;
                    self.guide_view = GuideView::at(None);
                    self.client.fetch_guide(&id, p, ctx);
                }
            });
        }
        ui.separator();

        // A running cog's own guide arrives as JSON: parse it into a GuideBundle once, then cache.
        if from_cog && self.guide_bundle.is_none() {
            let fetched = {
                let sh = self.client.snapshot();
                match &sh.guide {
                    Some(g) if g.id == id => g.result.clone(),
                    _ => None,
                }
            };
            if let Some(res) = fetched {
                self.guide_bundle = Some(res.and_then(|v| GuideBundle::from_json(&v)));
            }
        }

        match &self.guide_bundle {
            None if !from_cog => {
                ui.add_space(style::GAP_S);
                ui.label(RichText::new(format!("No hook-up guide ships for {id} yet.")).color(AMBER));
                ui.label(style::dim(ui, "The wiring facts the catalog has for its hardware are on the module's card (Catalog tab). To add a guide, follow the sensor-cog skill."));
                let hw: Vec<String> = self.catalog.modules_for_cog(&id).iter().map(|m| m.id.clone()).collect();
                for m in hw {
                    if ui.button(format!("🔧 Open {m} in the Catalog")).clicked() {
                        *self.pending_hw.borrow_mut() = Some(m);
                    }
                }
            }
            None => {
                ui.add_space(style::GAP_S);
                if self.guide_port_draft.trim().is_empty() {
                    ui.label(RichText::new("No hook-up guide ships for this cog, and its export port is not known.").color(AMBER));
                } else {
                    ui.label(style::dim(ui, "Loading the guide from the running cog…"));
                }
            }
            Some(Ok(bundle)) => {
                let bundle = bundle.clone();
                self.guide_view.show(ui, &bundle);
            }
            Some(Err(e)) => {
                ui.add_space(style::GAP_S);
                ui.label(RichText::new(format!("Couldn't read {id}'s guide from the node.")).color(AMBER));
                ui.label(style::dim(ui, "It is served by the running cog on its own port, which may be bound to the node only. Check that the cog is running, or adjust the port above."));
                if sensor_install::is_loopback_host(&self.client.s.host) {
                    ui.label(RichText::new("The console is pointed at this machine (127.0.0.1), not a remote node: set the node's address in the top bar.").color(AMBER));
                }
                ui.label(style::dim(ui, format!("detail: {e}")));
            }
        }
    }
}
