//! The Network tab: the tailnet fleet, the Cognitum mesh overlay and edge nodes.

use crate::app::Manager;
use crate::client::{self, Net};
use crate::style;
use crate::{GREEN, GREY, RED, WL};
use eframe::egui::{self, RichText};

impl Manager {
    pub(crate) fn network_view(&mut self, ui: &mut egui::Ui) {
        style::section_header(ui, "Network", "the fleet this OS is part of — mesh nodes, tailnet peers, the Cognitum overlay and edge nodes");
        self.fleet_section(ui);
        ui.add_space(style::GAP_S);
        let net = self.client.snapshot().net.clone();
        match &net {
            Some(Ok(n)) => self.fleet_tables(ui, n),
            Some(Err(e)) => {
                ui.colored_label(RED, format!("Can't reach the host's /network: {e}"));
            }
            None => {
                ui.label(style::dim(ui, "Querying the mesh…"));
            }
        }
    }

    pub(crate) fn fleet_tables(&self, ui: &mut egui::Ui, n: &Net) {
        if !n.node.is_empty() {
            ui.label(style::body(format!("this node: {}", n.node)).strong());
        }

        style::subhead(ui, "Tailnet fleet");
        if !n.tailscale.available {
            ui.label(style::dim(ui, "tailscale not available on this node"));
        } else {
            let online = n.tailscale.peers.iter().filter(|p| p.online).count();
            ui.label(style::dim(ui, format!("{} nodes, {} online", n.tailscale.peers.len(), online)));
            egui::Grid::new("tailnet").num_columns(4).striped(true).spacing([16.0, 5.0]).show(ui, |ui| {
                for h in ["node", "tailnet IP", "os", "state"] {
                    ui.label(RichText::new(h).strong().small());
                }
                ui.end_row();
                // self first, then online, then offline
                let mut peers: Vec<&client::NetPeer> = n.tailscale.peers.iter().collect();
                peers.sort_by_key(|p| (!p.is_self, !p.online, p.name.to_lowercase()));
                for p in peers {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("●").color(if p.online { GREEN } else { GREY }));
                        let name = if p.name.is_empty() { "(unnamed)" } else { &p.name };
                        if p.is_self {
                            ui.label(RichText::new(name).strong().color(WL)).on_hover_text("this node");
                        } else {
                            ui.label(name);
                        }
                    });
                    ui.label(RichText::new(&p.ip).color(GREY).small());
                    ui.label(RichText::new(&p.os).small());
                    ui.label(if p.online { RichText::new("online").color(GREEN).small() } else { RichText::new("offline").color(GREY).small() });
                    ui.end_row();
                }
            });
        }

        ui.add_space(style::GAP_S);
        style::subhead(ui, "Cognitum mesh overlay");
        let count = n.cognitum_mesh.get("count").and_then(|v| v.as_u64());
        match count {
            Some(0) => {
                ui.label(RichText::new("0 peers — cog0 is alone on the Cognitum mesh (discovery active)").color(GREY).small());
            }
            Some(c) => {
                ui.label(RichText::new(format!("{c} peer(s)")).color(GREEN).small());
                if let Some(arr) = n.cognitum_mesh.get("peers").and_then(|v| v.as_array()) {
                    for p in arr {
                        ui.label(RichText::new(format!("  {}", p)).small());
                    }
                }
            }
            None => {
                ui.label(RichText::new("mesh status unavailable").color(GREY).small());
            }
        }

        ui.add_space(style::GAP_S);
        style::subhead(ui, "Fleet nodes (edge / ESP32)");
        if n.fleet.is_empty() {
            ui.label(style::dim(ui, "none checked in. Edge nodes POST /fleet/heartbeat to appear here (COG-010); firmware is the next step."));
        } else {
            let online = n.fleet.iter().filter(|f| f.online).count();
            ui.label(style::dim(ui, format!("{} node(s), {} online", n.fleet.len(), online)));
            egui::Grid::new("fleet").num_columns(5).striped(true).spacing([14.0, 5.0]).show(ui, |ui| {
                for h in ["node", "kind", "sensor", "signal / batt", "seen"] {
                    ui.label(RichText::new(h).strong().small());
                }
                ui.end_row();
                for f in &n.fleet {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("●").color(if f.online { GREEN } else { GREY }));
                        ui.label(RichText::new(&f.id).strong());
                        if !f.ip.is_empty() {
                            ui.label(RichText::new(&f.ip).color(GREY).small());
                        }
                    });
                    ui.label(RichText::new(&f.kind).small());
                    ui.label(RichText::new(if f.sensor.is_empty() { "—" } else { &f.sensor }).small());
                    let sig = f.rssi.map(|r| format!("{r} dBm")).unwrap_or_else(|| "—".into());
                    let batt = f.battery.map(|b| format!(" · {b:.1}V")).unwrap_or_default();
                    ui.label(RichText::new(format!("{sig}{batt}")).small());
                    ui.label(RichText::new(if f.online { format!("{}s ago", f.age_s) } else { "offline".into() }).color(if f.online { GREEN } else { GREY }).small());
                    ui.end_row();
                }
            });
        }
    }
}
