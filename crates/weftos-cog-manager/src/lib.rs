//! weft-cog-manager (COG-009): the WeftOS appliance console.
//!
//! This is the OS showing its own interface — the cogs it runs and the marketplace it can pull
//! from — talking to a `weft-cog-host` over its lifecycle API. A left nav has **Cogs** (running +
//! marketplace), **Apps** (a placeholder for what lands next), and **System**. Runs natively and in
//! the browser; `?host=<url>` (wasm) or `WEFTOS_HOST` (native) points it at a host.

// egui 0.34 panel API (`.show(ctx)`, `.default_width`) is deprecated upstream but is what the
// companion/scope crates use; silence it crate-wide rather than scatter per-method allows.
#![allow(deprecated)]

pub mod client;

use client::{Client, HostCog, HostStatus, Net, Settings};
use eframe::egui::{self, Color32, RichText};
use weftos_cog_market::{Catalog, CatalogItem, Source};

const GREEN: Color32 = Color32::from_rgb(0x4c, 0xc2, 0x7a);
const RED: Color32 = Color32::from_rgb(0xe0, 0x5a, 0x5a);
const GREY: Color32 = Color32::from_rgb(0x88, 0x88, 0x88);
const AMBER: Color32 = Color32::from_rgb(0xd8, 0xa0, 0x3a);
const WL: Color32 = Color32::from_rgb(0x6c, 0x9c, 0xe8); // WeaveLogic blue
const COG: Color32 = Color32::from_rgb(0xb0, 0x82, 0xd8); // Cognitum purple

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Cogs,
    Sensors,
    Network,
    Apps,
    System,
}

pub struct Manager {
    client: Client,
    section: Section,
    host_draft: String,
}

impl Manager {
    pub fn new() -> Self {
        let s = Settings::default();
        let host_draft = s.host.clone();
        Self { client: Client::new(s), section: Section::Cogs, host_draft }
    }
}

impl Default for Manager {
    fn default() -> Self {
        Self::new()
    }
}

fn dot(ui: &mut egui::Ui, on: bool, label: &str) {
    let (c, glyph) = if on { (GREEN, "●") } else { (RED, "●") };
    ui.label(RichText::new(glyph).color(c));
    ui.label(label);
}

fn source_badge(ui: &mut egui::Ui, source: Source) {
    let (c, t) = match source {
        Source::WeaveLogic => (WL, "WeaveLogic"),
        Source::Cognitum => (COG, "Cognitum"),
    };
    ui.label(RichText::new(t).color(c).small());
}

fn source_badge_str(ui: &mut egui::Ui, source: &str) {
    let (c, t) = match source {
        "weavelogic" => (WL, "WeaveLogic"),
        "cognitum" => (COG, "Cognitum"),
        _ => (GREY, "local"),
    };
    ui.label(RichText::new(t).color(c).small());
}

impl Manager {
    fn top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("mgr_top").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("WeftOS").size(20.0).strong());
                ui.label(RichText::new("appliance console").color(GREY));
                ui.separator();
                ui.label("host");
                ui.add(egui::TextEdit::singleline(&mut self.host_draft).desired_width(230.0).hint_text("http://<ip>:9480"));
                if ui.button("Connect").clicked() {
                    let mut s = self.client.s.clone();
                    s.host = self.host_draft.clone();
                    self.client.reconnect(s);
                }
                ui.separator();
                let sh = self.client.snapshot();
                match &sh.host {
                    Some(Ok(h)) => {
                        dot(ui, true, &format!("connected · {} cog(s) running", h.running));
                    }
                    Some(Err(e)) => dot(ui, false, &format!("no host ({e})")),
                    None => {
                        ui.label(RichText::new("◌").color(AMBER));
                        ui.label("connecting…");
                    }
                }
            });
            ui.add_space(4.0);
        });
    }

    fn nav(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("mgr_nav").resizable(false).default_width(150.0).show(ctx, |ui| {
            ui.add_space(8.0);
            ui.selectable_value(&mut self.section, Section::Cogs, RichText::new("⚙  Cogs").size(16.0));
            ui.add_space(2.0);
            ui.selectable_value(&mut self.section, Section::Sensors, RichText::new("📈  Sensors").size(16.0));
            ui.add_space(2.0);
            ui.selectable_value(&mut self.section, Section::Network, RichText::new("🌐  Network").size(16.0));
            ui.add_space(2.0);
            ui.selectable_value(&mut self.section, Section::Apps, RichText::new("▦  Apps").size(16.0));
            ui.add_space(2.0);
            ui.selectable_value(&mut self.section, Section::System, RichText::new("🖥  System").size(16.0));
            ui.add_space(12.0);
            ui.separator();
            ui.label(RichText::new("the OS runs cogs here with\nno per-slot cap; apps land\nnext (COG-009).").color(GREY).small());
        });
    }

    fn cogs_view(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let (host, catalog, action) = {
            let sh = self.client.snapshot();
            (sh.host.clone(), sh.catalog.clone(), sh.last_action.clone())
        };

        ui.heading("Running on this OS");
        ui.label(RichText::new("cogs supervised by weft-cog-host — restarted on exit, no 3-cog agent cap").color(GREY).small());
        ui.add_space(4.0);
        match &host {
            Some(Ok(h)) => self.running_table(ui, ctx, h),
            Some(Err(e)) => {
                ui.colored_label(RED, format!("Can't reach the cog-host: {e}"));
                ui.label(RichText::new("Start it on the appliance: weft-cog-host serve --port 9480, then set the host above.").color(GREY).small());
            }
            None => {
                ui.label("Connecting to the host…");
            }
        }
        if let Some(a) = &action {
            ui.add_space(2.0);
            ui.label(RichText::new(a).color(GREY).small());
        }

        ui.add_space(14.0);
        ui.separator();
        ui.heading("Marketplace");
        ui.label(RichText::new("WeaveLogic (signed) + a mirror of Cognitum's registry — one catalog").color(GREY).small());
        ui.add_space(4.0);
        match &catalog {
            Some(cat) => self.marketplace(ui, ctx, cat, host.as_ref().and_then(|r| r.as_ref().ok())),
            None => {
                ui.label("Loading the marketplace…");
            }
        }
    }

    fn running_table(&self, ui: &mut egui::Ui, ctx: &egui::Context, h: &HostStatus) {
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
                        if ui.small_button("Stop").clicked() {
                            self.client.lifecycle(&c.id, "stop", ctx);
                        }
                    } else if ui.small_button("Start").clicked() {
                        self.client.lifecycle(&c.id, "start", ctx);
                    }
                });
                ui.end_row();
            }
        });
    }

    fn state_cell(&self, ui: &mut egui::Ui, c: &HostCog) {
        ui.horizontal(|ui| {
            if c.running {
                ui.label(RichText::new("●").color(GREEN));
                ui.label(format!("running {}", c.uptime_s.map(fmt_dur).unwrap_or_default()));
            } else if c.enabled {
                ui.label(RichText::new("●").color(AMBER));
                ui.label("starting…").on_hover_text(c.last_exit.clone().unwrap_or_default());
            } else {
                ui.label(RichText::new("●").color(GREY));
                ui.label("stopped");
            }
        });
    }

    fn marketplace(&self, ui: &mut egui::Ui, ctx: &egui::Context, cat: &Catalog, host: Option<&HostStatus>) {
        let installed = |id: &str| host.is_some_and(|h| h.cogs.iter().any(|c| c.id == id));
        egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
            let mut cat_name = String::new();
            for item in &cat.items {
                if item.category != cat_name {
                    cat_name = item.category.clone();
                    ui.add_space(6.0);
                    ui.label(RichText::new(if cat_name.is_empty() { "other" } else { &cat_name }).strong().color(AMBER));
                }
                self.market_row(ui, ctx, item, installed(&item.id));
            }
        });
    }

    fn market_row(&self, ui: &mut egui::Ui, ctx: &egui::Context, item: &CatalogItem, installed: bool) {
        ui.horizontal(|ui| {
            if item.signed {
                ui.label(RichText::new("🛡").color(WL)).on_hover_text("Ed25519-signed; verified before install");
            }
            ui.label(RichText::new(&item.name).strong());
            ui.label(RichText::new(format!("v{}", item.version)).color(GREY).small());
            source_badge(ui, item.source);
            if item.also_in_other_source {
                ui.label(RichText::new("(also upstream)").color(GREY).small());
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if installed {
                    ui.label(RichText::new("installed").color(GREEN).small());
                } else {
                    let resp = ui.add(egui::Button::new("Install").small());
                    let resp = if item.signed {
                        resp.on_hover_text("Fetch + Ed25519-verify against the pinned WeaveLogic key, then install on the host")
                    } else {
                        resp.on_hover_text("Cognitum cog (unsigned): fetched and sha256-checked on the host before it lands")
                    };
                    if resp.clicked() {
                        self.client.install(&item.id, item.source, item.version.clone(), ctx);
                    }
                }
                ui.label(RichText::new(item.arches.join("/")).color(GREY).small());
            });
        });
        if !item.description.is_empty() {
            ui.label(RichText::new(&item.description).color(GREY).small());
        }
        ui.add_space(2.0);
    }

    fn network_view(&self, ui: &mut egui::Ui) {
        let net = self.client.snapshot().net.clone();
        ui.heading("Network");
        ui.label(RichText::new("the fleet this OS is part of — tailnet peers + the Cognitum mesh overlay").color(GREY).small());
        ui.add_space(6.0);
        match &net {
            Some(Ok(n)) => self.fleet_tables(ui, n),
            Some(Err(e)) => {
                ui.colored_label(RED, format!("Can't reach the host's /network: {e}"));
            }
            None => {
                ui.label("Querying the mesh…");
            }
        }
    }

    fn fleet_tables(&self, ui: &mut egui::Ui, n: &Net) {
        if !n.node.is_empty() {
            ui.label(RichText::new(format!("this node: {}", n.node)).strong());
        }
        ui.add_space(4.0);

        ui.label(RichText::new("Tailnet fleet").strong().color(AMBER));
        if !n.tailscale.available {
            ui.label(RichText::new("tailscale not available on this node").color(GREY).small());
        } else {
            let online = n.tailscale.peers.iter().filter(|p| p.online).count();
            ui.label(RichText::new(format!("{} nodes, {} online", n.tailscale.peers.len(), online)).color(GREY).small());
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

        ui.add_space(10.0);
        ui.label(RichText::new("Cognitum mesh overlay").strong().color(AMBER));
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

        ui.add_space(10.0);
        ui.label(RichText::new("Fleet nodes (edge / ESP32)").strong().color(AMBER));
        if n.fleet.is_empty() {
            ui.label(RichText::new("none checked in. Edge nodes POST /fleet/heartbeat to appear here (COG-010); firmware is the next step.").color(GREY).small());
        } else {
            let online = n.fleet.iter().filter(|f| f.online).count();
            ui.label(RichText::new(format!("{} node(s), {} online", n.fleet.len(), online)).color(GREY).small());
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

    fn sensors_view(&self, ui: &mut egui::Ui) {
        ui.heading("Sensors");
        ui.label(RichText::new("live dashboards for running sensor cogs").color(GREY).small());
        ui.add_space(8.0);
        let host = self.client.snapshot().host.clone();
        let running: Vec<HostCog> = match &host {
            Some(Ok(h)) => h.cogs.iter().filter(|c| c.running).cloned().collect(),
            _ => Vec::new(),
        };
        if running.is_empty() {
            ui.label(RichText::new("No sensor cogs running. Start one in the Cogs tab, then its live dash appears here.").color(GREY));
        } else {
            ui.label("Running sensor cogs:");
            for c in &running {
                ui.label(format!("  • {} (v{})", c.id, c.version));
            }
        }
        ui.add_space(10.0);
        ui.label(RichText::new("Dashes coming next: generic vector/metrics/timeline for any cog, plus the ECG waveform (sen0213-ecg) and 8×8 ToF heatmap (sen0628-tof) reusing the scope views.").color(GREY).italics());
    }

    fn apps_view(&self, ui: &mut egui::Ui) {
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("▦").size(48.0).color(GREY));
            ui.add_space(8.0);
            ui.heading("No apps yet");
            ui.label(RichText::new("Apps install here alongside cogs. The OS runs them the same way —\nsupervised by WeftOS, no agent cap. Coming next.").color(GREY));
        });
    }

    fn system_view(&self, ui: &mut egui::Ui) {
        let sh = self.client.snapshot();
        ui.heading("System");
        ui.add_space(4.0);
        egui::Grid::new("sys").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
            ui.label("cog-host");
            match &sh.host {
                Some(Ok(h)) => ui.label(RichText::new(format!("reachable · root {} · {} running", h.root, h.running)).color(GREEN)),
                Some(Err(e)) => ui.label(RichText::new(e).color(RED)),
                None => ui.label("connecting…"),
            };
            ui.end_row();
            ui.label("host url");
            ui.label(&self.client.s.host);
            ui.end_row();
            ui.label("WeaveLogic registry");
            match &sh.our_reg {
                Some(Ok(r)) => ui.label(RichText::new(format!("{} signed cog(s)", r.cogs.len())).color(GREEN)),
                Some(Err(e)) => ui.label(RichText::new(e.as_str()).color(GREY)),
                None => ui.label("…"),
            };
            ui.end_row();
            ui.label("Cognitum registry (mirror)");
            match &sh.cognitum_reg {
                Some(Ok(r)) => ui.label(RichText::new(format!("{} cog(s)", r.cogs.len())).color(GREEN)),
                Some(Err(e)) => ui.label(RichText::new(e.as_str()).color(GREY)),
                None => ui.label("…"),
            };
            ui.end_row();
            ui.label("interconnect");
            ui.label(RichText::new("Cognitum agent store (:80) — cogs ingest there; WeftOS owns lifecycle (COG-009)").color(GREY));
            ui.end_row();
        });
    }
}

fn fmt_dur(s: u64) -> String {
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else {
        format!("{}h", s / 3600)
    }
}

impl eframe::App for Manager {
    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}

    #[allow(deprecated)]
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(web_time::Duration::from_millis(400));
        self.client.tick(ctx);
        self.top_bar(ctx);
        self.nav(ctx);
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| match self.section {
                Section::Cogs => self.cogs_view(ui, ctx),
                Section::Sensors => self.sensors_view(ui),
                Section::Network => self.network_view(ui),
                Section::Apps => self.apps_view(ui),
                Section::System => self.system_view(ui),
            });
        });
    }
}

/// Native entry point.
#[cfg(not(target_arch = "wasm32"))]
pub fn run_native() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 780.0])
            .with_min_inner_size([820.0, 520.0])
            .with_title("WeftOS — appliance console"),
        ..Default::default()
    };
    eframe::run_native("WeftOS console", options, Box::new(|cc| {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        Ok(Box::new(Manager::new()))
    }))
}

/// Browser entry exported to JS (`www/index.html` calls `mgr_start`).
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub async fn mgr_start(canvas_id: String) -> Result<(), wasm_bindgen::JsValue> {
    start_web(&canvas_id).await
}

/// Browser entry: mounts on `<canvas id=canvas_id>`. `?host=<url>` picks the cog-host.
#[cfg(target_arch = "wasm32")]
pub async fn start_web(canvas_id: &str) -> Result<(), wasm_bindgen::JsValue> {
    use wasm_bindgen::JsCast;
    console_error_panic_hook::set_once();
    let document = web_sys::window().and_then(|w| w.document()).ok_or_else(|| wasm_bindgen::JsValue::from_str("no document"))?;
    let canvas = document
        .get_element_by_id(canvas_id)
        .ok_or_else(|| wasm_bindgen::JsValue::from_str("canvas not found"))?
        .dyn_into::<web_sys::HtmlCanvasElement>()?;
    eframe::WebRunner::new()
        .start(canvas, eframe::WebOptions::default(), Box::new(|cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            Ok(Box::new(Manager::new()))
        }))
        .await
}
