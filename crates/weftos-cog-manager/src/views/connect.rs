//! The first screen: this machine, then the address book. The rest of the console stays hidden
//! until a cog-host answers.

use crate::address_book::{self, Reach, Source};
use crate::app::Manager;
use crate::style;
use crate::{AMBER, GREEN};
use eframe::egui::{self, RichText};

impl Manager {
    pub(crate) fn wait_view(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(48.0);
            ui.vertical_centered(|ui| {
                ui.label(style::h1("Weave Manager"));
                ui.add_space(8.0);
                ui.label(style::body(format!("Connecting to {}", self.client.s.host)));
                if address_book::classify(&self.client.s.host) == Reach::ThisMachine {
                    ui.label(style::dim(ui, "Trying the cog-host on this machine first."));
                }
            });
        });
    }

    pub(crate) fn connect_view(&mut self, ctx: &egui::Context) {
        self.poll_tailnet();
        let rows = address_book::merged(&self.book, &self.tail_peers);
        let err = {
            let sh = self.client.snapshot();
            sh.host.as_ref().and_then(|r| r.as_ref().err()).cloned()
        };
        let mut chosen: Option<String> = None;
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(28.0);
            ui.label(style::h1("Weave Manager"));
            ui.label(style::body("Pick the node to open. The menu follows that node once it answers."));
            ui.add_space(6.0);
            if let Some(err) = err {
                ui.colored_label(AMBER, format!("{} did not answer: {err}", self.client.s.host));
            } else if self.can_return {
                ui.label(style::dim(ui, format!("Still on {}. Pick another node, or go back.", self.client.s.host)));
            } else {
                ui.label(style::dim(ui, "This machine's cog-host did not answer. Choose a saved node or a tailnet peer, or type an address."));
            }
            ui.add_space(8.0);
            ui.label(style::dim(ui, "Tailnet addresses are the ones the mesh can attach to. A LAN address is a direct connection from this computer."));
            if !self.tail_note.is_empty() {
                ui.label(style::dim(ui, &self.tail_note));
            }
            ui.add_space(10.0);
            egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                if rows.is_empty() {
                    ui.label(style::dim(ui, "The address book is empty, and no tailnet peers were listed."));
                }
                for row in &rows {
                    ui.horizontal(|ui| {
                        let reach = row.reach.label();
                        let color = if row.reach == Reach::Tailnet { GREEN } else { AMBER };
                        ui.label(RichText::new(reach).color(color).small());
                        let presence = match row.online {
                            Some(true) => "online",
                            Some(false) => "offline",
                            None => "",
                        };
                        ui.label(style::body(row.label.as_str()));
                        if !presence.is_empty() {
                            ui.label(style::dim(ui, presence));
                        }
                        ui.label(style::dim(ui, row.url.as_str()));
                        if row.source == Source::Tailnet {
                            ui.label(style::dim(ui, "from tailscale"));
                        }
                        if ui.button("Open").clicked() {
                            chosen = Some(row.url.clone());
                        }
                    });
                }
            });
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.label(style::dim(ui, "Address"));
                ui.add(egui::TextEdit::singleline(&mut self.ip_draft).desired_width(320.0).hint_text("100.109.125.18 or a LAN IP"));
                if ui.button("Open").clicked() {
                    chosen = Some(self.ip_draft.clone());
                }
            });
            if let Some(err) = &self.connect_error {
                ui.colored_label(AMBER, err);
            }
            if self.can_return && ui.button("Back").clicked() {
                self.door_back = true;
            }
        });
        if let Some(raw) = chosen {
            self.door_back = false;
            match address_book::normalize_target(&raw) {
                Ok(url) => self.attach_to(url),
                Err(e) => self.connect_error = Some(e),
            }
        } else if self.door_back {
            self.door_back = false;
            self.can_return = false;
            self.pending = false;
            self.phase = crate::app::Phase::Open;
        }
    }
}
