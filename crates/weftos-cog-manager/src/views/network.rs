//! The Network tab: the tailnet fleet, the Cognitum mesh overlay and edge nodes.

use crate::app::Manager;
use crate::client::{self, Net};
use crate::style;
use crate::{AMBER, GREEN, GREY, RED, WL};
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

    pub(crate) fn fleet_tables(&mut self, ui: &mut egui::Ui, n: &Net) {
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
                        style::dot(ui, if p.online { GREEN } else { GREY });
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
            return;
        }
        let online = n.fleet.iter().filter(|f| f.online).count();
        ui.horizontal_wrapped(|ui| {
            ui.label(style::dim(ui, format!("{} node(s), {} online", n.fleet.len(), online)));
            style::pill(ui, "self-reported", AMBER).on_hover_text("Edge heartbeats are unauthenticated: every value here is what the node says about itself. Nothing reads them for placement or policy.");
        });
        egui::Grid::new("fleet").num_columns(8).striped(true).spacing([14.0, 5.0]).show(ui, |ui| {
            for h in ["node", "kind / chip", "sensor", "firmware", "signal / batt", "load / heap", "uptime", "seen"] {
                ui.label(RichText::new(h).strong().small());
            }
            ui.end_row();
            for f in &n.fleet {
                ui.horizontal(|ui| {
                    style::dot(ui, if f.online { GREEN } else { GREY });
                    let open = self.edge_open.as_deref() == Some(f.id.as_str());
                    if ui.selectable_label(open, RichText::new(&f.id).strong()).on_hover_text("show all reported fields").clicked() {
                        self.edge_open = if open { None } else { Some(f.id.clone()) };
                    }
                    if !f.ip.is_empty() {
                        ui.label(RichText::new(&f.ip).color(GREY).small());
                    }
                });
                let chip = f.chip.as_deref().map(|c| format!(" · {c}")).unwrap_or_default();
                ui.label(RichText::new(format!("{}{chip}", f.kind)).small());
                ui.label(RichText::new(if f.sensor.is_empty() { "—" } else { &f.sensor }).small());
                ui.label(RichText::new(if f.fw.is_empty() { "—" } else { &f.fw }).small());
                let sig = f.rssi.map(|r| format!("{r} dBm")).unwrap_or_else(|| "—".into());
                let batt = f.battery.map(|b| format!(" · {b:.1}V")).unwrap_or_default();
                ui.label(RichText::new(format!("{sig}{batt}")).small());
                let load = f.load.map(|l| format!("{:.0}%", l * 100.0)).unwrap_or_else(|| "—".into());
                let heap = f.free_heap.map(|h| format!(" · {} KiB", h / 1024)).unwrap_or_default();
                ui.label(RichText::new(format!("{load}{heap}")).small());
                ui.label(RichText::new(f.uptime_s.map_or_else(|| "—".into(), uptime)).small());
                let seen = if f.online { format!("{}s ago", f.age_s) } else { format!("offline ({})", uptime(f.age_s)) };
                ui.label(RichText::new(seen).color(if f.online { GREEN } else { GREY }).small());
                ui.end_row();
            }
        });
        if let Some(f) = self.edge_open.as_ref().and_then(|id| n.fleet.iter().find(|f| &f.id == id)) {
            style::card_frame(ui).show(ui, |ui| {
                style::h2(ui, &f.id);
                let opt = |v: Option<String>| v.unwrap_or_else(|| "not reported".into());
                let rows = [
                        ("chip", opt(f.chip.clone())),
                        ("mac", opt(f.mac.clone())),
                        ("firmware", if f.fw.is_empty() { "not reported".into() } else { f.fw.clone() }),
                        ("reset reason", opt(f.reset_reason.clone())),
                        ("wifi channel", opt(f.channel.map(|c| c.to_string()))),
                        ("sample rate", opt(f.sample_hz.map(|h| format!("{h:.1} Hz")))),
                        ("free heap", opt(f.free_heap.map(|h| format!("{} KiB", h / 1024)))),
                        ("provenance", if f.provenance.is_empty() { "self_reported".into() } else { f.provenance.clone() }),
                    ];
                egui::Grid::new(("edge", &f.id)).num_columns(2).spacing([14.0, 3.0]).show(ui, |ui| {
                    for (k, v) in &rows {
                        ui.label(style::dim(ui, *k));
                        ui.label(style::body(v.as_str()));
                        ui.end_row();
                    }
                });
            });
        }
    }
}

/// `45s`, `12m`, `3h 5m`, `2d 4h`.
fn uptime(s: u64) -> String {
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m", s / 60),
        3600..=86_399 => format!("{}h {}m", s / 3600, (s % 3600) / 60),
        _ => format!("{}d {}h", s / 86_400, (s % 86_400) / 3600),
    }
}
