//! The console app: `Manager` state, the top bar and nav, host switching and the frame loop. The
//! tabs live in `views/`.

use crate::address_book::{self, AddressBook, BookEntry, Reach};
use crate::client::{self, Client, HostStatus, Settings};
use crate::sensor_detail::{Event, GuideCache};
use crate::sensor_install;
use crate::style;
use crate::sensor_link::export_port;
use crate::views::cogs::dot;
use crate::{hw_dex, hw_identify};
use crate::{AMBER, WL};
use eframe::egui::{self, RichText};
use std::cell::RefCell;
use weftos_cog_market::hw::HwCatalog;
use weftos_sensor_guide::{GuideBundle, GuideView};
use web_time::{Duration, Instant};

/// Launch door. Wait tries the configured host (localhost unless `WEFTOS_HOST` is set).
/// Ask is the address book. Open is the console, and its menu follows that node.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Phase {
    Wait,
    Ask,
    Open,
}

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
    /// The WeftOS project ULID the console is scoped to (empty = unscoped), and why a given one
    /// was ignored. Clearing the filter empties `project`.
    pub(crate) project: String,
    pub(crate) project_warning: Option<String>,
    /// User-local names and URLs. Tokens stay in `tokens`, never in this book.
    pub(crate) book: AddressBook,
    pub(crate) ip_draft: String,
    tail_rx: Option<std::sync::mpsc::Receiver<Result<Vec<BookEntry>, String>>>,
    pub(crate) tail_peers: Vec<BookEntry>,
    pub(crate) tail_note: String,
    tail_started: bool,
    pub(crate) connect_error: Option<String>,
    /// A connect is in flight. Cleared when the user opens the book without switching,
    /// so a still-good session does not snap the door shut.
    pub(crate) pending: bool,
    pub(crate) can_return: bool,
    pub(crate) door_back: bool,
    tried_at: Instant,
    pub(crate) phase: Phase,
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
        let (project, project_warning) = (s.project.clone(), s.project_warning.clone());
        let mut me = Self {
            project,
            project_warning,
            client: Client::new(s),
            section: match client::setting("WEFTOS_TAB", "tab", "").as_str() {
                "sensors" => Section::Sensors,
                "catalog" => Section::Catalog,
                "network" | "tree" => Section::Network,
                "system" => Section::System,
                "cogs" => Section::Cogs,
                _ if deep.is_some() => Section::Catalog,
                _ => Section::Network,
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
            book: AddressBook::load(),
            ip_draft: String::new(),
            tail_rx: None,
            tail_peers: Vec::new(),
            tail_note: String::new(),
            tail_started: false,
            connect_error: None,
            pending: true,
            can_return: false,
            door_back: false,
            tried_at: Instant::now(),
            phase: Phase::Wait,
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
                ui.label(style::h1("Weave"));
                ui.label(style::dim(ui, "manager"));
                ui.separator();
                ui.label(style::dim(ui, "host"));
                ui.add(egui::TextEdit::singleline(&mut self.host_draft).desired_width(230.0).hint_text("http://<ip>:9480"));
                let mesh_note = self.client.snapshot().mesh_note.clone();
                let auto_key = cfg!(not(target_arch = "wasm32"))
                    && matches!(address_book::classify(&self.client.s.host), Reach::ThisMachine | Reach::Tailnet);
                if auto_key {
                    let label = if !self.client.s.token.trim().is_empty() {
                        "mesh key".to_string()
                    } else if mesh_note.is_empty() {
                        "registering a mesh key".into()
                    } else {
                        mesh_note
                    };
                    ui.label(style::dim(ui, label)).on_hover_text("This connection registers its own key with the cog-host. The address book does not store it.");
                } else {
                    ui.label(style::dim(ui, "token"));
                    ui.add(egui::TextEdit::singleline(&mut self.token_draft).password(true).desired_width(140.0).hint_text("direct connection"));
                }
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
                let (reach_label, reach_color, reach_tip) = match address_book::classify(&self.client.s.host) {
                    Reach::ThisMachine => ("this machine", AMBER, "The console is on this computer's cog-host."),
                    Reach::Tailnet => ("tailnet", crate::GREEN, "This address is on the tailnet. Nodes under it attach the same way."),
                    Reach::Lan => ("lan", AMBER, "Direct connection from this computer. A LAN address is not a mesh hop."),
                    Reach::Other => ("named host", AMBER, "A hostname. The mesh attaches by tailnet address."),
                };
                style::pill(ui, reach_label, reach_color).on_hover_text(reach_tip);
                let host_line = match &self.client.snapshot().host {
                    Some(Ok(h)) => format!("up:{}", h.running),
                    Some(Err(e)) => format!("down:{e}"),
                    None => "dial".into(),
                };
                if let Some(n) = host_line.strip_prefix("up:") {
                    dot(ui, true, &format!("connected · {n} cog(s) running"));
                } else if let Some(e) = host_line.strip_prefix("down:") {
                    dot(ui, false, &format!("no host ({e})"));
                } else {
                    style::status_dot(ui, AMBER, "connecting…");
                }
                if ui.button("Nodes").on_hover_text("Pick another node. This session stays until you open one.").clicked() {
                    self.phase = Phase::Ask;
                    self.can_return = true;
                    self.pending = false;
                }
            });
            self.project_bar(ui);
            ui.add_space(style::GAP_S);
        });
    }

    /// The project context line: the ULID (and name, when the fleet snapshot lists it) with a
    /// control to clear the filter, or the warning for a project that was ignored.
    fn project_bar(&mut self, ui: &mut egui::Ui) {
        if let Some(w) = &self.project_warning {
            ui.colored_label(AMBER, w);
        }
        if self.project.is_empty() {
            return;
        }
        let name = {
            let sh = self.client.snapshot();
            sh.fleet.as_ref().and_then(|r| r.as_ref().ok()).and_then(|s| crate::project::name(s, &self.project))
        };
        ui.horizontal_wrapped(|ui| {
            style::pill(ui, "project", WL);
            if let Some(n) = &name {
                ui.label(style::h1(n));
            }
            ui.label(style::dim(ui, &self.project));
            ui.label(style::dim(ui, format!("showing project {} only", name.as_deref().unwrap_or(&self.project))));
            if ui.small_button("clear filter").on_hover_text("Show every node and cog, not just this project's").clicked() {
                self.project.clear();
            }
        });
    }

    pub(crate) fn nav(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("mgr_nav").resizable(false).exact_width(164.0).show(ctx, |ui| {
            ui.add_space(style::GAP_M);
            // The menu follows the node. A cog-host that answered gets the full set. Anything
            // else keeps Network (the node tree), the sensor guides, and the catalog.
            let live = self.host_live();
            let full: &[&[(Section, &str)]] = &[
                &[(Section::Network, "🌐   Network")],
                &[(Section::Cogs, "⚙   Cogs")],
                &[(Section::Sensors, "📈   Sensors"), (Section::Catalog, "📚   Catalog")],
                &[(Section::Apps, "▦   Apps"), (Section::System, "🖥   System")],
            ];
            let quiet: &[&[(Section, &str)]] = &[
                &[(Section::Network, "🌐   Network"), (Section::Sensors, "📈   Sensors"), (Section::Catalog, "📚   Catalog")],
            ];
            let groups = if live { full } else { quiet };
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
            ui.label(style::dim(ui, "The menu follows the node you opened."));
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

    /// Point the console at another node. A saved session stays with the node it was issued for
    /// and is not sent to the next one. A tailnet or local node registers its own mesh key.
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

    /// Point the console at `url` and wait for that node to answer. The current token stays
    /// with the node being left. A tailnet address and a LAN address both connect directly;
    /// the tree only offers the tailnet ones as child hops.
    pub(crate) fn attach_to(&mut self, url: String) {
        self.switch_host(url);
        self.pending = true;
        self.can_return = false;
        self.door_back = false;
        self.connect_error = None;
        self.tried_at = Instant::now();
        self.phase = Phase::Wait;
    }

    /// One local `tailscale status` while the address book is showing. A missing binary
    /// is a note. Peers are tailnet addresses only.
    pub(crate) fn poll_tailnet(&mut self) {
        #[cfg(target_arch = "wasm32")]
        {
            if self.tail_note.is_empty() && self.tail_peers.is_empty() {
                self.tail_note = "This browser reads tailnet peers from the node you open.".into();
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            if !self.tail_started {
                self.tail_started = true;
                let (tx, rx) = std::sync::mpsc::channel();
                self.tail_rx = Some(rx);
                std::thread::spawn(move || {
                    let _ = tx.send(address_book::read_local_tailnet());
                });
            }
            let msg = self.tail_rx.as_ref().and_then(|rx| rx.try_recv().ok());
            if let Some(msg) = msg {
                self.tail_rx = None;
                match msg {
                    Ok(peers) => {
                        if peers.is_empty() {
                            self.tail_note = "No other tailnet peers from this computer.".into();
                        }
                        self.tail_peers = peers;
                    }
                    Err(e) => self.tail_note = e,
                }
            }
        }
    }

    fn host_state(&self) -> Option<bool> {
        match &self.client.snapshot().host {
            Some(Ok(_)) => Some(true),
            Some(Err(_)) => Some(false),
            None => None,
        }
    }

    fn host_live(&self) -> bool {
        self.host_state() == Some(true)
    }

    fn clamp_section(&mut self) {
        if self.host_live() {
            return;
        }
        if !matches!(self.section, Section::Network | Section::Sensors | Section::Catalog) {
            self.section = Section::Network;
        }
    }

    fn remember(&mut self) {
        let url = self.client.s.host.clone();
        let named = self.client.snapshot().net.as_ref().and_then(|r| r.as_ref().ok()).map(|n| n.node.trim().to_string()).filter(|s| !s.is_empty());
        let label = named.unwrap_or_else(|| {
            if sensor_install::is_loopback_host(&url) { "this machine".into() } else { url.clone() }
        });
        self.book.upsert(label, &url);
        let _ = self.book.save();
    }

    fn advance_door(&mut self) {
        let waited = self.tried_at.elapsed() >= Duration::from_secs(4);
        let (phase, pending) = step_door(self.phase, self.pending, self.host_state(), waited);
        let opened = phase == Phase::Open && self.phase != Phase::Open;
        self.phase = phase;
        self.pending = pending;
        if opened {
            self.can_return = false;
            self.remember();
        }
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
        self.advance_door();
        if self.phase == Phase::Open {
            self.client.ensure_mesh_session(ctx);
            if self.client.take_mesh_clear() {
                self.tokens.remove(&self.client.s.host);
                self.token_draft.clear();
                self.client.s.token.clear();
            }
            if let Some(tok) = self.client.take_issued_token() {
                self.client.s.token = tok.clone();
                self.token_draft = tok.clone();
                self.tokens.insert(self.client.s.host.clone(), tok);
            }
            let auto = cfg!(not(target_arch = "wasm32")) && matches!(address_book::classify(&self.client.s.host), Reach::ThisMachine | Reach::Tailnet);
            if auto && self.token_draft.is_empty() && !self.client.s.token.is_empty() {
                let tok = self.client.s.token.clone();
                self.token_draft = tok.clone();
                self.tokens.insert(self.client.s.host.clone(), tok);
            }
        }
        match self.phase {
            Phase::Wait => {
                self.wait_view(ctx);
                return;
            }
            Phase::Ask => {
                self.connect_view(ctx);
                return;
            }
            Phase::Open => {}
        }
        self.clamp_section();
        self.top_bar(ctx);
        self.nav(ctx);
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| match self.section {
                Section::Cogs => self.cogs_view(ui, ctx),
                Section::Sensors => self.sensors_view(ui, ctx),
                Section::Catalog => self.catalog_view(ui, ctx),
                Section::Network => self.network_view(ui, ctx),
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

/// `host_ok`: Some(true) answered, Some(false) refused, None still dialing.
/// A book opened over a live session has `pending` false, so a still-good host does not
/// close the book. A late answer after the wait timed out does open, until the user picks.
fn step_door(phase: Phase, pending: bool, host_ok: Option<bool>, waited: bool) -> (Phase, bool) {
    if !pending {
        return (phase, false);
    }
    if host_ok == Some(true) {
        return (Phase::Open, false);
    }
    if phase == Phase::Wait && (host_ok == Some(false) || waited) {
        return (Phase::Ask, true);
    }
    (phase, true)
}

#[cfg(test)]
mod door_tests {
    use super::*;

    #[test]
    fn localhost_wait_opens_when_the_host_answers() {
        assert_eq!(step_door(Phase::Wait, true, Some(true), false), (Phase::Open, false));
    }

    #[test]
    fn refused_or_slow_wait_opens_the_address_book() {
        assert_eq!(step_door(Phase::Wait, true, Some(false), false), (Phase::Ask, true));
        assert_eq!(step_door(Phase::Wait, true, None, true), (Phase::Ask, true));
        assert_eq!(step_door(Phase::Wait, true, None, false), (Phase::Wait, true));
    }

    #[test]
    fn a_late_answer_opens_unless_the_user_is_switching_nodes() {
        assert_eq!(step_door(Phase::Ask, true, Some(true), true), (Phase::Open, false));
        assert_eq!(step_door(Phase::Ask, false, Some(true), true), (Phase::Ask, false));
    }
}
