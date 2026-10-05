//! Fleet manager P2: the mesh node list from the daemon's `fleet.snapshot` (through the ADR-102
//! gateway) and a node detail panel with Overview, Workloads, Health and Raw tabs. Every fact is
//! shown with its provenance. The console never offers an Admin verb: start/stop is only for cogs
//! on the connected cog-host (the same host POSTs as the Cogs tab); remote work is shown with the
//! CLI command that does it.

use super::fleet_tabs as tabs;
use crate::app::Manager;
use crate::client::MintState;
use crate::fleet::{self, FleetRow};
use crate::fleet_unify;
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
    /// The fleet manager list (the Network tab's first section): every machine and device from
    /// every source — mesh nodes (daemon snapshot via the gateway), Seeds, tailnet hosts and edge
    /// nodes — merged into one table, each with the sources that reported it.
    pub(crate) fn fleet_section(&mut self, ui: &mut Ui) {
        style::subhead(ui, "Fleet");
        ui.horizontal_wrapped(|ui| {
            ui.label(style::dim(ui, "gateway"));
            ui.add(egui::TextEdit::singleline(&mut self.gw_draft).desired_width(200.0).hint_text("http://<daemon>:<gateway port>"));
            ui.label(style::dim(ui, "token"));
            ui.add(egui::TextEdit::singleline(&mut self.gw_token_draft).password(true).desired_width(100.0).hint_text("read-only token"));
            ui.label(style::dim(ui, "seeds"));
            ui.add(egui::TextEdit::singleline(&mut self.seeds_draft).desired_width(200.0).hint_text("http://<seed>, ..."))
                .on_hover_text("Cognitum Seed agents to read (comma-separated). The connected host's own agent is found automatically.");
            if ui.button("Use").clicked() {
                let mut s = self.client.s.clone();
                s.gateway = self.gw_draft.trim().to_string();
                s.gateway_token = self.gw_token_draft.trim().to_string();
                s.seeds = crate::client::parse_seeds(&self.seeds_draft);
                self.client.reconnect(s);
                self.fleet_node = None;
            }
        });
        let (fleet, net, seeds) = {
            let sh = self.client.snapshot();
            (sh.fleet.clone(), sh.net.clone(), sh.seeds.values().cloned().collect::<Vec<_>>())
        };
        let snap = fleet.as_ref().and_then(|r| r.as_ref().ok());
        let net_ok = net.as_ref().and_then(|r| r.as_ref().ok());
        let entries = fleet_unify::unify(snap, net_ok, &seeds);
        let now = now_unix();
        // Why a source is missing, said once above the table.
        if self.client.s.gateway.trim().is_empty() {
            ui.label(style::dim(ui, "No gateway set, so mesh-node detail (trust, load, workloads) is not shown. Set WEFTOS_GATEWAY / ?gw= with a read-only token (weft token issue --read-only)."));
        } else if let Some(Err(e)) = &fleet {
            ui.colored_label(RED, format!("fleet snapshot unavailable: {e}"));
        }
        match self.client.mint_state() {
            MintState::Done(m) if self.client.s.gateway_token.trim().is_empty() => {
                let until = m.expires_at.map_or_else(String::new, |e| format!(", expires in {}", fleet::age(e, now).trim_end_matches(" ago")));
                ui.label(style::dim(ui, format!("token issued by the gateway for this session (memory only{until})")));
            }
            MintState::Pending => {
                ui.label(style::dim(ui, "asking the gateway for a console token…"));
            }
            MintState::Failed(why) if self.client.s.gateway_token.trim().is_empty() => {
                ui.label(style::dim(ui, format!("{why}; enter a read-only token above.")));
            }
            _ => {}
        }
        // Project scope: the nodes that host none of the project's instances stay listed, marked.
        let hosting = (!self.project.is_empty()).then(|| snap.map(|s| crate::project::hosting_nodes(s, &self.project))).flatten();
        if !self.project.is_empty() {
            ui.label(style::dim(ui, match &hosting {
                Some(h) => format!("project {}: {} node(s) host its instances; the rest are marked", self.project, h.len()),
                None => format!("project {}: the fleet snapshot is needed to tell which nodes host it (set the gateway)", self.project),
            }));
        }
        if let Some(Err(e)) = &net {
            ui.colored_label(RED, format!("connected host's /network unavailable: {e}"));
        }
        let rows: std::collections::BTreeMap<String, FleetRow> =
            snap.map(fleet::rows).unwrap_or_default().into_iter().map(|r| (r.id.clone(), r)).collect();
        let online = entries.iter().filter(|e| e.online == Some(true)).count();
        ui.label(style::dim(ui, format!("{} device(s), {online} online · click one for its detail", entries.len())));
        egui::Grid::new("fleet_all").num_columns(9).striped(true).spacing([14.0, 5.0]).show(ui, |ui| {
            for h in ["device", "class", "address", "os / chip", "firmware", "rtt", "load", "seen", "from"] {
                ui.label(RichText::new(h).strong().small());
            }
            ui.end_row();
            for e in &entries {
                self.fleet_entry_row(ui, e, rows.get(e.node_id.as_deref().unwrap_or("")), now, hosting.as_ref());
                ui.end_row();
            }
        });
        if let Some(s) = snap {
            for d in s["degraded"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                ui.label(RichText::new(format!("not available: {d}")).color(GREY).small());
            }
        }
        let Some(key) = self.fleet_node.clone() else { return };
        let Some(e) = entries.iter().find(|e| e.key == key || e.node_id.as_deref() == Some(key.as_str())).cloned() else {
            return;
        };
        if let Some(n) = e.node_id.as_deref().and_then(|id| snap.and_then(|s| fleet::node(s, id))) {
            let id = e.node_id.clone().unwrap_or_default();
            self.node_detail(ui, n, &id, now);
        } else if let Some(sv) = e.seed.as_ref().and_then(|u| seeds.iter().find(|s| &s.url == u)) {
            ui.add_space(style::GAP_S);
            style::card_frame(ui).show(ui, |ui| {
                self.entry_header(ui, &e);
                super::fleet_seed::seed_detail(ui, sv);
            });
        } else {
            ui.add_space(style::GAP_S);
            style::card_frame(ui).show(ui, |ui| {
                self.entry_header(ui, &e);
                entry_facts(ui, &e);
            });
        }
    }

    fn entry_header(&mut self, ui: &mut Ui, e: &fleet_unify::Entry) {
        ui.horizontal_wrapped(|ui| {
            ui.label(style::h1(&e.name));
            style::pill(ui, e.class.label(), WL);
            if ui.small_button("close").clicked() {
                self.fleet_node = None;
            }
        });
    }

    fn fleet_entry_row(&mut self, ui: &mut Ui, e: &fleet_unify::Entry, r: Option<&FleetRow>, now: u64, hosting: Option<&std::collections::BTreeSet<String>>) {
        ui.horizontal(|ui| {
            let revoked = r.is_some_and(|r| r.revoked);
            style::dot(ui, match (revoked, e.online) {
                (true, _) => RED,
                (_, Some(true)) => GREEN,
                (_, Some(false)) => GREY,
                _ => AMBER,
            });
            let mut label = RichText::new(&e.name);
            if e.this_host || r.is_some_and(|r| r.local) {
                label = label.strong().color(WL);
            }
            let selected = self.fleet_node.as_deref().is_some_and(|k| k == e.key || e.node_id.as_deref() == Some(k));
            if ui.selectable_label(selected, label).on_hover_text(&e.key).clicked() {
                self.fleet_node = if selected { None } else { Some(e.key.clone()) };
                self.fleet_tab = NodeTab::Overview;
                if e.class == fleet_unify::Class::Edge {
                    self.edge_open = e.key.strip_prefix("edge:").map(str::to_owned);
                }
            }
            if r.is_some_and(|r| r.local) {
                ui.label(style::dim(ui, "this daemon"));
            } else if e.this_host {
                ui.label(style::dim(ui, "connected host"));
            }
            if revoked {
                style::pill(ui, "revoked", RED);
            }
            if hosting.is_some_and(|h| !e.node_id.as_deref().is_some_and(|id| h.contains(id))) {
                style::pill(ui, "none of this project", GREY).on_hover_text("No instance of the project runs on this machine");
            }
        });
        ui.label(RichText::new(e.class.label()).small());
        ui.label(RichText::new(if e.addrs.is_empty() { "-".into() } else { e.addrs.join(", ") }).small());
        ui.label(RichText::new(e.os.as_deref().unwrap_or("-")).small());
        ui.label(RichText::new(e.firmware.as_deref().unwrap_or("-")).small());
        ui.label(RichText::new(r.and_then(|r| r.rtt_ms).map_or_else(|| "-".into(), |v| format!("{v:.0} ms"))).small());
        ui.label(RichText::new(r.and_then(|r| r.load1).map_or_else(|| "-".into(), |v| format!("{v:.2}"))).small());
        ui.label(RichText::new(r.and_then(|r| r.seen_unix).map_or_else(|| "-".into(), |t| fleet::age(now, t))).small());
        ui.horizontal(|ui| {
            for src in &e.sources {
                style::pill(ui, src.kind, GREY).on_hover_text(format!("{}: {}", src.provenance, fleet::explain(src.provenance)));
            }
        });
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
        let mut placed = fleet::instances(n);
        if !self.project.is_empty() {
            placed.retain(|i| i.project.eq_ignore_ascii_case(&self.project));
        }
        style::subhead(ui, if self.project.is_empty() { "Placed workloads" } else { "Placed workloads (this project)" });
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

/// Detail for an entry with no richer source (a tailnet host, or an edge node).
fn entry_facts(ui: &mut Ui, e: &fleet_unify::Entry) {
    egui::Grid::new(("entry", &e.key)).num_columns(2).spacing([14.0, 4.0]).show(ui, |ui| {
        let rows = [
            ("status", match e.online { Some(true) => "online".to_string(), Some(false) => "offline".into(), None => "unknown".into() }),
            ("addresses", if e.addrs.is_empty() { "-".into() } else { e.addrs.join(", ") }),
            ("os / chip", e.os.clone().unwrap_or_else(|| "-".into())),
            ("firmware", e.firmware.clone().unwrap_or_else(|| "-".into())),
            ("reported by", e.sources.iter().map(|s| format!("{} ({})", s.kind, s.provenance)).collect::<Vec<_>>().join(", ")),
        ];
        for (k, v) in rows {
            ui.label(style::dim(ui, k));
            ui.label(style::body(v));
            ui.end_row();
        }
    });
    if e.class == fleet_unify::Class::Host {
        ui.label(style::dim(ui, "Only the tailnet knows this machine. It is not a mesh node, Seed or cog-host the console can read; join it to the mesh for trust, load and workloads."));
    } else if e.class == fleet_unify::Class::Edge {
        ui.label(style::dim(ui, "Self-reported edge heartbeat; the full report is in the edge table below."));
    }
}
