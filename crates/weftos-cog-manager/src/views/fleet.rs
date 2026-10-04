//! Fleet manager P2: the mesh node list from the daemon's `fleet.snapshot` (through the ADR-102
//! gateway) and a node detail panel with Overview, Workloads, Health and Raw tabs. Every fact is
//! shown with its provenance. The console never offers an Admin verb: start/stop is only for cogs
//! on the connected cog-host (the same host POSTs as the Cogs tab); remote work is shown with the
//! CLI command that does it.

use super::fleet_tabs as tabs;
use crate::app::Manager;
use crate::fleet::{self, FleetRow};
use crate::sensor_detail::Event;
use crate::style;
use crate::{AMBER, GREEN, GREY, RED, WL};
use eframe::egui::{self, RichText, Ui};
use serde_json::Value;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum NodeTab {
    #[default]
    Overview,
    Workloads,
    Health,
    Trust,
    Software,
    Raw,
}

pub(crate) fn now_unix() -> u64 {
    web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub(crate) fn prov_pill(ui: &mut Ui, p: Option<&str>) {
    if let Some(p) = p {
        let color = match p {
            "signed_fact" => GREEN,
            "daemon_observed" => WL,
            "operator_claimed" | "peer_claimed" => AMBER,
            _ => GREY,
        };
        style::pill(ui, p, color).on_hover_text(fleet::explain(p));
    }
}

impl NodeTab {
    /// Tab from a deep-link name (`overview`, `workloads`, `health`, `trust`, `software`, `raw`).
    pub(crate) fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "workloads" | "cogs" => Self::Workloads,
            "health" => Self::Health,
            "trust" | "licence" | "license" => Self::Trust,
            "software" | "firmware" => Self::Software,
            "raw" => Self::Raw,
            _ => Self::Overview,
        }
    }
}

impl Manager {
    /// The gateway settings and the node list (the Network tab's first section).
    pub(crate) fn fleet_section(&mut self, ui: &mut Ui) {
        style::subhead(ui, "Mesh nodes (fleet snapshot)");
        ui.horizontal_wrapped(|ui| {
            ui.label(style::dim(ui, "gateway"));
            ui.add(egui::TextEdit::singleline(&mut self.gw_draft).desired_width(220.0).hint_text("http://<daemon>:<gateway port>"));
            ui.label(style::dim(ui, "token"));
            ui.add(egui::TextEdit::singleline(&mut self.gw_token_draft).password(true).desired_width(110.0).hint_text("read-only token"));
            if ui.button("Use").clicked() {
                let mut s = self.client.s.clone();
                s.gateway = self.gw_draft.trim().to_string();
                s.gateway_token = self.gw_token_draft.trim().to_string();
                self.client.reconnect(s);
                self.fleet_node = None;
            }
        });
        let fleet = self.client.snapshot().fleet.clone();
        let snap = match fleet {
            _ if self.client.s.gateway.trim().is_empty() => {
                ui.label(style::dim(ui, "Set the gateway (WEFTOS_GATEWAY or ?gw=) to see every node the daemon knows. A read-only token is enough: weft token issue --read-only."));
                return;
            }
            None => {
                ui.label(style::dim(ui, "Querying the gateway…"));
                return;
            }
            Some(Err(e)) => {
                ui.colored_label(RED, format!("Can't read the fleet snapshot: {e}"));
                return;
            }
            Some(Ok(v)) => v,
        };
        let rows = fleet::rows(&snap);
        let now = now_unix();
        ui.label(style::dim(ui, format!("{} node(s) · click a node for its detail", rows.len())));
        egui::Grid::new("fleet_nodes").num_columns(9).striped(true).spacing([14.0, 5.0]).show(ui, |ui| {
            for h in ["node", "state", "trust", "mesh", "rtt", "load", "seen", "cogs", "location"] {
                ui.label(RichText::new(h).strong().small());
            }
            ui.end_row();
            for r in &rows {
                self.fleet_row(ui, r, now);
                ui.end_row();
            }
        });
        for d in snap["degraded"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            ui.label(RichText::new(format!("not available: {d}")).color(GREY).small());
        }
        let open = self.fleet_node.clone();
        if let Some(id) = open {
            match fleet::node(&snap, &id) {
                Some(n) => self.node_detail(ui, n, &id, now),
                None => self.fleet_node = None,
            }
        }
    }

    fn fleet_row(&mut self, ui: &mut Ui, r: &FleetRow, now: u64) {
        ui.horizontal(|ui| {
            let live = r.heartbeat.as_deref() == Some("alive") || r.local;
            style::dot(ui, if r.revoked { RED } else if live { GREEN } else { GREY });
            let mut label = RichText::new(&r.name);
            if r.local {
                label = label.strong().color(WL);
            }
            let selected = self.fleet_node.as_deref() == Some(r.id.as_str());
            if ui.selectable_label(selected, label).on_hover_text(&r.id).clicked() {
                self.fleet_node = if selected { None } else { Some(r.id.clone()) };
                self.fleet_tab = NodeTab::Overview;
            }
            if r.local {
                ui.label(style::dim(ui, "this node"));
            }
            if r.revoked {
                style::pill(ui, "revoked", RED);
            }
            if r.unknown {
                style::pill(ui, "label only", GREY).on_hover_text("only an operator location label names this id");
            }
        });
        ui.label(RichText::new(&r.state).small());
        ui.label(RichText::new(&r.trust).small());
        let mesh = match &r.class {
            Some(c) => format!("{c}{} {}", if r.verified { "" } else { " unverified" }, r.heartbeat.as_deref().unwrap_or("")),
            None => "-".into(),
        };
        ui.label(RichText::new(mesh.trim()).small());
        ui.label(RichText::new(r.rtt_ms.map_or_else(|| "-".into(), |v| format!("{v:.0} ms"))).small());
        ui.label(RichText::new(r.load1.map_or_else(|| "-".into(), |v| format!("{v:.2}"))).small());
        ui.label(RichText::new(r.seen_unix.map_or_else(|| "-".into(), |t| fleet::age(now, t))).small());
        ui.label(RichText::new(r.cogs.to_string()).small());
        ui.label(RichText::new(r.location.as_deref().unwrap_or("-")).small());
    }

    fn node_detail(&mut self, ui: &mut Ui, n: &Value, id: &str, now: u64) {
        ui.add_space(style::GAP_S);
        style::card_frame(ui).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let name = fleet::val(n, "name").as_str().map_or_else(|| fleet::short(id), str::to_owned);
                ui.label(style::h1(name));
                prov_pill(ui, fleet::provenance(n, "name"));
                if ui.small_button("copy id").on_hover_text(id).clicked() {
                    ui.ctx().copy_text(id.to_string());
                }
                if ui.small_button("close").clicked() {
                    self.fleet_node = None;
                }
            });
            ui.horizontal(|ui| {
                for (t, label) in [
                    (NodeTab::Overview, "Overview"),
                    (NodeTab::Workloads, "Workloads / cogs"),
                    (NodeTab::Health, "Health"),
                    (NodeTab::Trust, "Trust / licence"),
                    (NodeTab::Software, "Software / firmware"),
                    (NodeTab::Raw, "Raw (all details)"),
                ] {
                    ui.selectable_value(&mut self.fleet_tab, t, label);
                }
            });
            ui.separator();
            match self.fleet_tab {
                NodeTab::Overview => self.node_overview(ui, n, id),
                NodeTab::Workloads => self.node_workloads(ui, n),
                NodeTab::Health => {
                    let hist = self.client.snapshot().fleet_hist.get(id).cloned().unwrap_or_default();
                    tabs::node_health(ui, n, now, &hist);
                }
                NodeTab::Trust => {
                    let licence = self.client.snapshot().fleet.as_ref().and_then(|r| r.as_ref().ok()).map(|s| s["licence"].clone());
                    tabs::node_trust(ui, n, id, now, licence.as_ref());
                }
                NodeTab::Software => tabs::node_software(ui, n),
                NodeTab::Raw => tabs::node_raw(ui, n),
            }
        });
    }

    fn node_overview(&self, ui: &mut Ui, n: &Value, id: &str) {
        let facts = fleet::val(n, "facts");
        let announced = fleet::val(n, "announced");
        let loc = fleet::val(n, "location");
        let s = |v: &Value| v.as_str().map(str::to_owned).unwrap_or_else(|| if v.is_null() { "-".into() } else { v.to_string() });
        let rows: Vec<(&str, String, Option<&str>)> = vec![
            ("node id", id.to_string(), None),
            ("trust tier", s(&facts["trust_tier"]), fleet::provenance(n, "facts")),
            ("tier source", s(&facts["tier_source"]), fleet::provenance(n, "facts")),
            ("platform", s(&announced["platform"]), fleet::provenance(n, "announced")),
            ("address", s(&announced["address"]), fleet::provenance(n, "announced")),
            ("site", s(&loc["site"]), fleet::provenance(n, "location")),
            ("room", s(&loc["room"]), fleet::provenance(n, "location")),
        ];
        egui::Grid::new(("node_overview", id)).num_columns(3).spacing([14.0, 4.0]).show(ui, |ui| {
            for (k, v, p) in rows {
                ui.label(style::dim(ui, k));
                ui.label(style::body(&v));
                prov_pill(ui, p);
                ui.end_row();
            }
        });
        if fleet::provenance(n, "facts") == Some("signed_fact") {
            ui.label(RichText::new("✔ node facts signature verified by the daemon").color(GREEN).small());
        }
        if let Some(host) = announced["address"].as_str().and_then(|a| a.rsplit_once(':').map(|(h, _)| h.to_string())) {
            ui.horizontal(|ui| {
                ui.label(style::dim(ui, "ssh"));
                ui.code(format!("ssh <user>@{host}"));
            });
        }
        if fleet::val(n, "revoked").is_object() {
            ui.colored_label(RED, format!("revoked: {}", s(&fleet::val(n, "revoked")["reason"])));
        }
        if loc.is_null() {
            ui.label(style::dim(ui, format!("no location label; set one with: weaver fleet location set {id} --site <site> --room <room>")));
        }
    }

    fn node_workloads(&self, ui: &mut Ui, n: &Value) {
        let placed = fleet::instances(n);
        style::subhead(ui, "Placed workloads");
        prov_pill(ui, fleet::provenance(n, "instances"));
        if placed.is_empty() {
            ui.label(style::dim(ui, "no placed workloads on this node"));
        } else {
            egui::Grid::new("node_instances").num_columns(5).striped(true).spacing([14.0, 4.0]).show(ui, |ui| {
                for h in ["instance", "workload", "state", "restarts", "stop with"] {
                    ui.label(RichText::new(h).strong().small());
                }
                ui.end_row();
                for i in &placed {
                    ui.label(RichText::new(fleet::short(&i.id)).small()).on_hover_text(&i.id);
                    ui.label(RichText::new(&i.workload).small());
                    ui.label(RichText::new(&i.state).color(if i.state == "running" { GREEN } else { GREY }).small());
                    ui.label(RichText::new(i.restarts.to_string()).small());
                    ui.code(format!("weaver workload stop {}", i.id));
                    ui.end_row();
                }
            });
        }
        // Cogs on the connected cog-host: start/stop are the host's own POSTs (Cogs tab pattern).
        if n["local"] == true {
            ui.add_space(style::GAP_S);
            style::subhead(ui, "Cogs on the connected cog-host");
            let host = self.client.snapshot().host.clone();
            match host {
                Some(Ok(h)) if !h.cogs.is_empty() => {
                    egui::Grid::new("node_host_cogs").num_columns(4).striped(true).spacing([14.0, 4.0]).show(ui, |ui| {
                        for c in &h.cogs {
                            ui.label(RichText::new(&c.id).strong());
                            ui.label(RichText::new(if c.version.is_empty() { "-" } else { &c.version }).small());
                            match (&c.licence_refusal, c.running) {
                                (Some(code), _) => {
                                    ui.colored_label(RED, format!("refused: {code}"));
                                }
                                (None, true) => {
                                    ui.label(RichText::new("running").color(GREEN).small());
                                }
                                (None, false) => {
                                    ui.label(RichText::new("stopped").color(GREY).small());
                                }
                            }
                            let action = if c.running { "stop" } else { "start" };
                            if ui.small_button(action).clicked() {
                                self.events.borrow_mut().push(Event::Lifecycle { id: c.id.clone(), action });
                            }
                            ui.end_row();
                        }
                    });
                }
                Some(Ok(_)) => {
                    ui.label(style::dim(ui, "no cogs installed on this host"));
                }
                Some(Err(e)) => {
                    ui.label(style::dim(ui, format!("cog-host not reachable: {e}")));
                }
                None => {
                    ui.label(style::dim(ui, "connecting to the cog-host…"));
                }
            }
        }
    }
}
