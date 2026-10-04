//! The console app: `Manager` state, the top bar and nav, host switching and the frame loop. The
//! tabs live in `views/`.

use crate::client::{self, Client, HostStatus, Settings};
use crate::sensor_detail::{Event, GuideCache};
use crate::sensor_install;
use crate::style;
use crate::sensor_link::export_port;
use crate::views::cogs::dot;
use crate::{hw_dex, hw_identify};
use crate::AMBER;
use eframe::egui::{self, RichText};
use std::cell::RefCell;
use weftos_cog_market::hw::HwCatalog;
use weftos_sensor_guide::{GuideBundle, GuideView};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Section {
    Cogs,
    Sensors,
    Catalog,
    Network,
    Apps,
    System,
}

pub struct Manager {
    pub(crate) client: Client,
    pub(crate) section: Section,
    pub(crate) host_draft: String,
    pub(crate) token_draft: String,
    /// Sensors tab: the cog whose guide is open (None = the list), its parsed bundle, and the
    /// renderer. `guide_port_draft` lets the user correct the export port if the default is wrong.
    pub(crate) guide_cog: Option<String>,
    pub(crate) guide_view: GuideView,
    pub(crate) guide_bundle: Option<Result<GuideBundle, String>>,
    pub(crate) guide_port_draft: String,
    /// Where the open guide came from, shown above it.
    pub(crate) guide_source: &'static str,
    /// Catalog tab: the embedded Projects/Modules/Chips inventory + its view state.
    pub(crate) catalog: HwCatalog,
    pub(crate) cat_tab: u8, // 0 projects, 1 modules, 2 chips
    pub(crate) cat_search: String,
    pub(crate) cat_kind: String, // module kind filter: all|board|sensor|display|actuator|tool
    /// Catalog tab: the "Identify hardware" USB scan modal.
    pub(crate) hw: hw_identify::HwIdentify,
    pub(crate) dex: hw_dex::DexUi,
    /// Sensor detail panel (ADR-107): per-cog guide cache, the clicks it raised this frame, a
    /// pending "go to this module" cross-link from another tab, and the module card to open.
    pub(crate) guides: RefCell<GuideCache>,
    pub(crate) events: RefCell<Vec<Event>>,
    pub(crate) pending_hw: RefCell<Option<String>>,
    /// Install flow: the node picked for each cog (cog id -> node name).
    pub(crate) targets: RefCell<std::collections::BTreeMap<String, String>>,
    /// Host token per host address, so switching nodes never sends one node's token to another.
    pub(crate) tokens: std::collections::BTreeMap<String, String>,
    pub(crate) focus_module: Option<String>,
    pub(crate) focus_step: Option<u8>,
    /// Network tab (fleet P2): gateway drafts, the open node and its detail tab.
    pub(crate) gw_draft: String,
    pub(crate) gw_token_draft: String,
    pub(crate) seeds_draft: String,
    pub(crate) fleet_node: Option<String>,
    pub(crate) fleet_tab: crate::views::fleet::NodeTab,
    /// Network tab: the edge node whose reported fields are expanded.
    pub(crate) edge_open: Option<String>,
}

impl Manager {
    pub fn new() -> Self {
        let s = Settings::default();
        let host_draft = s.host.clone();
        let token_draft = s.token.clone();
        let (gw_draft, gw_token_draft) = (s.gateway.clone(), s.gateway_token.clone());
        // Deep link: `?module=<id>` (wasm) or `WEFTOS_MODULE=<id>` (native) opens that module's
        // card in the Catalog.
        let deep = client::setting("WEFTOS_MODULE", "module", "");
        let deep = (!deep.trim().is_empty()).then(|| deep.trim().to_string());
        let mut me = Self {
            client: Client::new(s),
            section: match client::setting("WEFTOS_TAB", "tab", "").as_str() {
                "sensors" => Section::Sensors,
                "catalog" => Section::Catalog,
                "network" => Section::Network,
                "system" => Section::System,
                "cogs" => Section::Cogs,
                _ if deep.is_some() => Section::Catalog,
                _ => Section::Cogs,
            },
            host_draft,
            token_draft,
            guide_cog: None,
            guide_view: GuideView::at(None),
            guide_bundle: None,
            guide_port_draft: String::new(),
            guide_source: "",
            catalog: HwCatalog::bundled(),
            cat_tab: 1,
            cat_search: deep.clone().unwrap_or_default(),
            cat_kind: "all".into(),
            hw: hw_identify::HwIdentify::default(),
            dex: hw_dex::DexUi::default(),
            guides: RefCell::new(GuideCache::new()),
            events: RefCell::new(Vec::new()),
            pending_hw: RefCell::new(None),
            targets: RefCell::new(Default::default()),
            tokens: Default::default(),
            focus_module: deep,
            focus_step: client::setting("WEFTOS_STEP", "step", "").trim().parse().ok(),
            gw_draft,
            gw_token_draft,
            seeds_draft: Settings::default().seeds.join(", "),
            // Deep link: `WEFTOS_NODE=<node id>` / `?node=` opens that node's detail on the
            // Network tab; `WEFTOS_NODE_TAB` / `?nodetab=` picks its tab.
            fleet_node: Some(client::setting("WEFTOS_NODE", "node", "")).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
            fleet_tab: crate::views::fleet::NodeTab::parse(&client::setting("WEFTOS_NODE_TAB", "nodetab", "")),
            // Deep link: `WEFTOS_EDGE=<edge node id>` / `?edge=` expands that edge node's fields.
            edge_open: Some(client::setting("WEFTOS_EDGE", "edge", "")).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
        };
        // Deep link: `WEFTOS_GUIDE=<cog id>` / `?guide=` opens that cog's bundled guide.
        let g = client::setting("WEFTOS_GUIDE", "guide", "");
        if !g.trim().is_empty() {
            me.open_guide(g.trim(), None, None);
        }
        me
    }
}

impl Default for Manager {
    fn default() -> Self {
        Self::new()
    }
}

impl Manager {
    pub(crate) fn top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("mgr_top").show(ctx, |ui| {
            ui.add_space(style::GAP_S);
            ui.horizontal_wrapped(|ui| {
                ui.label(style::h1("WeftOS"));
                ui.label(style::dim(ui, "appliance console"));
                ui.separator();
                ui.label(style::dim(ui, "host"));
                ui.add(egui::TextEdit::singleline(&mut self.host_draft).desired_width(230.0).hint_text("http://<ip>:9480"));
                ui.label(style::dim(ui, "token"));
                ui.add(egui::TextEdit::singleline(&mut self.token_draft).password(true).desired_width(110.0).hint_text("<root>/host.token"));
                if ui.button("Connect").clicked() {
                    let mut s = self.client.s.clone();
                    // a token typed for one host is never carried to a different one unchanged
                    let moved = self.host_draft.trim() != self.client.s.host.trim();
                    if moved {
                        let for_new = sensor_install::switch_token(&mut self.tokens, &self.client.s.host, &self.client.s.token, &self.host_draft);
                        if self.token_draft == self.client.s.token {
                            self.token_draft = for_new;
                        }
                    }
                    s.host = self.host_draft.clone();
                    s.token = self.token_draft.clone();
                    self.tokens.insert(s.host.clone(), s.token.clone());
                    self.client.reconnect(s);
                    self.guides.borrow_mut().clear();
                }
                ui.separator();
                if sensor_install::is_loopback_host(&self.client.s.host) {
                    style::pill(ui, "this machine", AMBER).on_hover_text("The console is pointed at 127.0.0.1, not a remote node. Set the host above (or WEFTOS_HOST) to reach an appliance.");
                }
                let sh = self.client.snapshot();
                match &sh.host {
                    Some(Ok(h)) => {
                        dot(ui, true, &format!("connected · {} cog(s) running", h.running));
                    }
                    Some(Err(e)) => dot(ui, false, &format!("no host ({e})")),
                    None => style::status_dot(ui, AMBER, "connecting…"),
                }
            });
            ui.add_space(style::GAP_S);
        });
    }

    pub(crate) fn nav(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("mgr_nav").resizable(false).exact_width(164.0).show(ctx, |ui| {
            ui.add_space(style::GAP_M);
            // Grouped by what each tab is about: runtime, then hardware/data, then the fleet,
            // then the rest. One rhythm between items, a little more between groups.
            let groups: [&[(Section, &str)]; 3] = [
                &[(Section::Cogs, "⚙   Cogs")],
                &[(Section::Sensors, "📈   Sensors"), (Section::Catalog, "📚   Catalog")],
                &[(Section::Network, "🌐   Network"), (Section::Apps, "▦   Apps"), (Section::System, "🖥   System")],
            ];
            for (gi, group) in groups.iter().enumerate() {
                if gi > 0 {
                    ui.add_space(style::GAP_S);
                }
                for (section, label) in *group {
                    ui.selectable_value(&mut self.section, *section, RichText::new(*label).size(15.0));
                    ui.add_space(2.0);
                }
            }
            ui.add_space(style::GAP_M);
            ui.separator();
            ui.add_space(style::GAP_S);
            ui.label(style::dim(ui, "The OS runs cogs here with no per-slot cap; apps land next (COG-009)."));
        });
    }

    /// Switch to the Sensors tab with this cog's guide open (optionally at a page). The guide
    /// bundled with the catalog is used when there is one; otherwise a running cog's own `/guide`.
    pub(crate) fn open_guide(&mut self, id: &str, page: Option<&'static str>, ctx: Option<&egui::Context>) {
        let host: Option<HostStatus> = self.client.snapshot().host.clone().and_then(|r| r.ok());
        let port = export_port(id, host.as_ref());
        self.section = Section::Sensors;
        self.guide_cog = Some(id.to_string());
        self.guide_view = GuideView::at(page.map(str::to_string));
        self.guide_port_draft = if port == 0 { String::new() } else { port.to_string() };
        self.guide_bundle = None;
        if let Some(json) = weftos_cog_market::guides::bundled(id) {
            self.guide_bundle = Some(serde_json::from_str::<serde_json::Value>(json).map_err(|e| e.to_string()).and_then(|v| GuideBundle::from_json(&v)));
            self.guide_source = "bundled with the catalog (readable offline, before install)";
        } else if let (true, Some(ctx)) = (port != 0, ctx) {
            self.guide_source = "served by the running cog";
            self.client.fetch_guide(id, port, ctx);
        } else {
            self.guide_source = "";
        }
    }

    /// Point the console at another node. Tokens are per host: the current host's token is kept
    /// under its own address and is NOT sent to the new node; the new node's token is whatever was
    /// entered for it before, else empty (the user is asked for it in the top bar).
    pub(crate) fn switch_host(&mut self, url: String) {
        let mut s = self.client.s.clone();
        s.token = sensor_install::switch_token(&mut self.tokens, &s.host, &s.token, &url);
        s.host = url.clone();
        self.host_draft = url;
        self.token_draft = s.token.clone();
        self.client.reconnect(s);
        self.guides.borrow_mut().clear();
        self.targets.borrow_mut().clear();
    }

    /// Apply what the panel and cross-links asked for this frame, with the same client calls the
    /// Cogs tab uses (install / start / stop) plus tab navigation.
    pub(crate) fn apply_requests(&mut self, ctx: &egui::Context) {
        if let Some(id) = self.pending_hw.borrow_mut().take() {
            self.section = Section::Catalog;
            self.cat_tab = 1;
            self.cat_kind = "all".into();
            self.cat_search = id.clone();
            self.focus_module = Some(id);
        }
        let events = std::mem::take(&mut *self.events.borrow_mut());
        for e in events {
            match e {
                Event::Install { id, source, version } => self.client.install(&id, source, version, ctx),
                Event::Lifecycle { id, action } => self.client.lifecycle(&id, action, ctx),
                Event::OpenGuide { cog, page } => self.open_guide(&cog, page, Some(ctx)),
                Event::SwitchHost { url } => self.switch_host(url),
            }
        }
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
                Section::Sensors => self.sensors_view(ui, ctx),
                Section::Catalog => self.catalog_view(ui, ctx),
                Section::Network => self.network_view(ui),
                Section::Apps => self.apps_view(ui),
                Section::System => self.system_view(ui),
            });
        });
        self.apply_requests(ctx);
        if let Some(j) = self.hw.show(ctx, &self.client, &self.catalog) {
            self.section = Section::Catalog;
            self.cat_tab = j.tab;
            self.cat_search = j.search;
            self.cat_kind = "all".into();
        }
    }
}
