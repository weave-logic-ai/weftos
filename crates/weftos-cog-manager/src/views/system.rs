//! The Apps placeholder and the System tab.

use crate::app::Manager;
use crate::style;
use crate::{GREEN, RED};
use eframe::egui::{self, RichText};

impl Manager {
    pub(crate) fn apps_view(&self, ui: &mut egui::Ui) {
        ui.add_space(style::GAP_L);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("▦").size(48.0).color(style::muted(ui)));
            ui.add_space(style::GAP_S);
            style::h2(ui, "No apps yet");
            ui.add_space(style::GAP_XS);
            ui.label(style::dim(ui, "Apps install here alongside cogs. The OS runs them the same way — supervised by WeftOS, no agent cap. Coming next."));
        });
    }

    pub(crate) fn system_view(&self, ui: &mut egui::Ui) {
        let sh = self.client.snapshot();
        style::section_header(ui, "System", "the cog-host and registries this console is talking to");
        egui::Grid::new("sys").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
            ui.label(style::dim(ui, "cog-host"));
            match &sh.host {
                Some(Ok(h)) => ui.label(RichText::new(format!("reachable · root {} · {} running", h.root, h.running)).color(GREEN)),
                Some(Err(e)) => ui.label(RichText::new(e).color(RED)),
                None => ui.label(style::dim(ui, "connecting…")),
            };
            ui.end_row();
            ui.label(style::dim(ui, "host url"));
            ui.label(style::body(&self.client.s.host));
            ui.end_row();
            ui.label(style::dim(ui, "WeaveLogic registry"));
            match &sh.our_reg {
                Some(Ok(r)) => ui.label(RichText::new(format!("{} signed cog(s)", r.cogs.len())).color(GREEN)),
                Some(Err(e)) => ui.label(style::dim(ui, e.as_str())),
                None => ui.label(style::dim(ui, "…")),
            };
            ui.end_row();
            ui.label(style::dim(ui, "Cognitum registry (mirror)"));
            match &sh.cognitum_reg {
                Some(Ok(r)) => ui.label(RichText::new(format!("{} cog(s)", r.cogs.len())).color(GREEN)),
                Some(Err(e)) => ui.label(style::dim(ui, e.as_str())),
                None => ui.label(style::dim(ui, "…")),
            };
            ui.end_row();
            ui.label(style::dim(ui, "interconnect"));
            ui.label(style::dim(ui, "Cognitum agent store (:80) — cogs ingest there; WeftOS owns lifecycle (COG-009)"));
            ui.end_row();
        });
    }
}
