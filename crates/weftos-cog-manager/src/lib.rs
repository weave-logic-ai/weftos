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
mod hw_dex;
mod hw_identify;
mod sensor_detail;
mod sensor_link;
mod style;

use client::{Client, HostCog, HostStatus, Net, Settings};
use eframe::egui::{self, Color32, RichText};
use weftos_cog_market::hw::{Chip, HwCatalog, Module, Project};
use weftos_cog_market::{Catalog, CatalogItem, Source};
use sensor_detail::{Event, GuideCache, PanelCtx};
use sensor_link::{default_export_port, fmt_dur};
use std::cell::RefCell;
use weftos_sensor_guide::{GuideBundle, GuideView};

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
    Catalog,
    Network,
    Apps,
    System,
}

pub struct Manager {
    client: Client,
    section: Section,
    host_draft: String,
    token_draft: String,
    /// Sensors tab: the cog whose guide is open (None = the list), its parsed bundle, and the
    /// renderer. `guide_port_draft` lets the user correct the export port if the default is wrong.
    guide_cog: Option<String>,
    guide_view: GuideView,
    guide_bundle: Option<Result<GuideBundle, String>>,
    guide_port_draft: String,
    /// Catalog tab: the embedded Projects/Modules/Chips inventory + its view state.
    catalog: HwCatalog,
    cat_tab: u8, // 0 projects, 1 modules, 2 chips
    cat_search: String,
    cat_kind: String, // module kind filter: all|board|sensor|display|actuator|tool
    /// Catalog tab: the "Identify hardware" USB scan modal.
    hw: hw_identify::HwIdentify,
    dex: hw_dex::DexUi,
    /// Sensor detail panel (ADR-107): per-cog guide cache, the clicks it raised this frame, a
    /// pending "go to this module" cross-link from another tab, and the module card to open.
    guides: RefCell<GuideCache>,
    events: RefCell<Vec<Event>>,
    pending_hw: RefCell<Option<String>>,
    focus_module: Option<String>,
}

impl Manager {
    pub fn new() -> Self {
        let s = Settings::default();
        let host_draft = s.host.clone();
        let token_draft = s.token.clone();
        // Deep link: `?module=<id>` (wasm) or `WEFTOS_MODULE=<id>` (native) opens that module's
        // card in the Catalog.
        let deep = client::setting("WEFTOS_MODULE", "module", "");
        let deep = (!deep.trim().is_empty()).then(|| deep.trim().to_string());
        Self {
            client: Client::new(s),
            section: if deep.is_some() { Section::Catalog } else { Section::Cogs },
            host_draft,
            token_draft,
            guide_cog: None,
            guide_view: GuideView::at(None),
            guide_bundle: None,
            guide_port_draft: String::new(),
            catalog: HwCatalog::bundled(),
            cat_tab: 1,
            cat_search: deep.clone().unwrap_or_default(),
            cat_kind: "all".into(),
            hw: hw_identify::HwIdentify::default(),
            dex: hw_dex::DexUi::default(),
            guides: RefCell::new(GuideCache::new()),
            events: RefCell::new(Vec::new()),
            pending_hw: RefCell::new(None),
            focus_module: deep,
        }
    }
}

fn mod_hay(m: &Module) -> String {
    format!("{} {} {} {} {} {} {} {}", m.id, m.name, m.vendor, m.kind, m.summary, m.chips.join(" "), m.good_for.join(" "), m.spec.values().cloned().collect::<Vec<_>>().join(" ")).to_lowercase()
}
fn chip_hay(c: &Chip) -> String {
    format!("{} {} {} {} {} {} {}", c.id, c.name, c.manufacturer, c.role, c.summary, c.tags.join(" "), c.spec.values().cloned().collect::<Vec<_>>().join(" ")).to_lowercase()
}
fn proj_hay(p: &Project) -> String {
    format!("{} {} {} {} {}", p.name, p.category, p.difficulty, p.summary, p.modules.join(" ")).to_lowercase()
}

pub(crate) fn buy_and_datasheet(ui: &mut egui::Ui, buy: Option<(&str, &str)>, datasheet: &str) {
    if buy.is_none_or(|(_, u)| u.is_empty()) && datasheet.is_empty() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        if let Some((price, url)) = buy.filter(|(_, u)| !u.is_empty()) {
            ui.hyperlink_to(RichText::new(format!("🛒 {} ↗", if price.is_empty() { "Mouser" } else { price })).color(GREEN), url);
        }
        if !datasheet.is_empty() {
            ui.hyperlink_to(RichText::new("datasheet ↗").color(WL), datasheet);
        }
    });
}

/// A tag strip (chips / modules / sensor tags) rendered as uniform pills so every card's
/// metadata row reads the same.
pub(crate) fn tag_row<'a>(ui: &mut egui::Ui, tags: impl Iterator<Item = &'a str>, color: Color32) {
    ui.horizontal_wrapped(|ui| {
        for t in tags {
            style::pill(ui, t, color);
        }
    });
}

fn chip_card(ui: &mut egui::Ui, c: &Chip) {
    style::card(
        ui,
        ("chip", &c.id),
        |ui| {
            style::truncated(ui, &c.name, true);
            if !c.role.is_empty() {
                style::pill(ui, &c.role, COG);
            }
            if !c.manufacturer.is_empty() {
                style::truncated(ui, &c.manufacturer, false);
            }
        },
        |ui| {
            if !c.summary.is_empty() {
                ui.label(style::body(&c.summary));
            }
            if !c.tags.is_empty() {
                tag_row(ui, c.tags.iter().map(String::as_str), WL);
            }
            buy_and_datasheet(ui, None, &c.datasheet);
            style::spec_grid(ui, ("cs", &c.id), c.spec.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        },
    );
}

fn project_card(ui: &mut egui::Ui, p: &Project) {
    style::card(
        ui,
        ("proj", &p.id),
        |ui| {
            style::truncated(ui, &p.name, true);
            if !p.category.is_empty() {
                style::pill(ui, &p.category, AMBER);
            }
            if !p.difficulty.is_empty() {
                ui.label(style::dim(ui, &p.difficulty));
            }
        },
        |ui| {
            if !p.summary.is_empty() {
                ui.label(style::body(&p.summary));
            }
            if !p.modules.is_empty() {
                tag_row(ui, p.modules.iter().map(String::as_str), GREY);
            }
        },
    );
}

/// Count line (scannable, at the top) + the cards, or a centered "nothing matched" when the
/// filter is empty. Shared by the Projects / Modules / Chips tabs so they behave identically.
fn render_catalog_list<T>(ui: &mut egui::Ui, noun: &str, items: &[&T], search: &str, render: impl Fn(&mut egui::Ui, &T)) {
    ui.label(style::dim(ui, format!("{} {noun}", items.len())));
    ui.add_space(style::GAP_XS);
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        if items.is_empty() {
            ui.add_space(style::GAP_M);
            ui.vertical_centered(|ui| {
                let msg = if search.is_empty() { format!("No {noun} here yet.") } else { format!("No {noun} match “{search}”.") };
                ui.label(style::dim(ui, msg));
            });
            return;
        }
        for it in items {
            render(ui, it);
        }
    });
}

impl Default for Manager {
    fn default() -> Self {
        Self::new()
    }
}

fn dot(ui: &mut egui::Ui, on: bool, label: &str) {
    style::status_dot(ui, if on { GREEN } else { RED }, label);
}

fn source_badge(ui: &mut egui::Ui, source: Source) {
    let (c, t) = match source {
        Source::WeaveLogic => (WL, "WeaveLogic"),
        Source::Cognitum => (COG, "Cognitum"),
    };
    style::pill(ui, t, c);
}

fn source_badge_str(ui: &mut egui::Ui, source: &str) {
    let (c, t) = match source {
        "weavelogic" => (WL, "WeaveLogic"),
        "cognitum" => (COG, "Cognitum"),
        _ => (GREY, "local"),
    };
    style::pill(ui, t, c);
}

impl Manager {
    fn top_bar(&mut self, ctx: &egui::Context) {
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
                    s.host = self.host_draft.clone();
                    s.token = self.token_draft.clone();
                    self.client.reconnect(s);
                    self.guides.borrow_mut().clear();
                }
                ui.separator();
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

    fn nav(&mut self, ctx: &egui::Context) {
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

    fn cogs_view(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let (host, catalog, action) = {
            let sh = self.client.snapshot();
            (sh.host.clone(), sh.catalog.clone(), sh.last_action.clone())
        };

        style::section_header(ui, "Running on this OS", "cogs supervised by weft-cog-host — restarted on exit, no 3-cog agent cap");
        match &host {
            Some(Ok(h)) => self.running_table(ui, ctx, h),
            Some(Err(e)) => {
                ui.colored_label(RED, format!("Can't reach the cog-host: {e}"));
                ui.label(style::dim(ui, "Start it on the appliance: weft-cog-host serve --port 9480, then set the host above."));
            }
            None => {
                ui.label(style::dim(ui, "Connecting to the host…"));
            }
        }
        if let Some(a) = &action {
            ui.add_space(style::GAP_XS);
            ui.label(style::dim(ui, a));
        }

        ui.add_space(style::GAP_L);
        ui.separator();
        ui.add_space(style::GAP_S);
        style::section_header(ui, "Marketplace", "WeaveLogic (signed) + a mirror of Cognitum's registry — one catalog");
        match &catalog {
            Some(cat) => self.marketplace(ui, ctx, cat, host.as_ref().and_then(|r| r.as_ref().ok())),
            None => {
                ui.label(style::dim(ui, "Loading the marketplace…"));
            }
        }
    }

    /// Small links from a cog to the hardware it drives; clicking opens that module in the Catalog.
    fn hw_links(&self, ui: &mut egui::Ui, cog_id: &str) {
        let mods = self.catalog.modules_for_cog(cog_id);
        for m in mods.iter().take(2) {
            if ui.small_button(RichText::new(format!("🔧 {}", sensor_link::short_name(&m.name))).color(WL)).on_hover_text(format!("Needs: {} — open it in the Catalog", m.name)).clicked() {
                *self.pending_hw.borrow_mut() = Some(m.id.clone());
            }
        }
        if mods.len() > 2 {
            ui.label(style::dim(ui, format!("+{}", mods.len() - 2)));
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
                    self.hw_links(ui, &c.id);
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
                        if ui.button(RichText::new("Stop").color(RED)).clicked() {
                            self.client.lifecycle(&c.id, "stop", ctx);
                        }
                    } else if ui.button(RichText::new("Start").color(GREEN)).clicked() {
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
        egui::ScrollArea::vertical().max_height(380.0).auto_shrink([false, false]).show(ui, |ui| {
            let mut cat_name = String::new();
            for item in &cat.items {
                if item.category != cat_name {
                    cat_name = item.category.clone();
                    ui.add_space(style::GAP_S);
                    ui.label(RichText::new(if cat_name.is_empty() { "other" } else { &cat_name }).strong().color(AMBER).size(13.0));
                    ui.add_space(style::GAP_XS);
                }
                self.market_row(ui, ctx, item, installed(&item.id));
            }
        });
    }

    fn market_row(&self, ui: &mut egui::Ui, ctx: &egui::Context, item: &CatalogItem, installed: bool) {
        style::card_frame(ui).show(ui, |ui| {
            ui.horizontal(|ui| {
                if item.signed {
                    ui.label(RichText::new("🛡").color(WL)).on_hover_text("Ed25519-signed; verified before install");
                }
                style::truncated(ui, &item.name, true);
                ui.label(style::dim(ui, format!("v{}", item.version)));
                source_badge(ui, item.source);
                self.hw_links(ui, &item.id);
                if item.also_in_other_source {
                    ui.label(style::dim(ui, "(also upstream)"));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if installed {
                        style::pill(ui, "✓ installed", GREEN);
                    } else {
                        let resp = ui.add(egui::Button::new("Install"));
                        let resp = if item.signed {
                            resp.on_hover_text("Fetch + Ed25519-verify against the pinned WeaveLogic key, then install on the host")
                        } else {
                            resp.on_hover_text("Cognitum cog (unsigned): fetched and sha256-checked on the host before it lands")
                        };
                        if resp.clicked() {
                            self.client.install(&item.id, item.source, item.version.clone(), ctx);
                        }
                    }
                    ui.label(style::dim(ui, item.arches.join("/")));
                });
            });
            if !item.description.is_empty() {
                ui.label(style::dim(ui, &item.description));
            }
        });
        ui.add_space(style::GAP_XS);
    }

    fn catalog_view(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            style::h2(ui, "Hardware catalog");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("🔍  Identify hardware").on_hover_text("scan the host's USB bus and match devices to the catalog").clicked() {
                    self.hw.open(&self.client, ctx);
                }
            });
        });
        style::caption(
            ui,
            format!(
                "{} projects · {} modules · {} chips — Projects → Modules → Chips (explored across weftos, mentra, whitsentry)",
                self.catalog.projects.len(),
                self.catalog.modules.len(),
                self.catalog.chips.len()
            ),
        );
        ui.add_space(style::GAP_S);
        // Filter bar: the tab picker, the search field and (for modules) the kind chips read
        // as one toolbar instead of three loose widget rows.
        style::card_frame(ui).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.cat_tab, 0u8, "Projects");
                ui.selectable_value(&mut self.cat_tab, 1u8, "Modules");
                ui.selectable_value(&mut self.cat_tab, 2u8, "Chips");
                ui.selectable_value(&mut self.cat_tab, 3u8, "Dex");
                ui.separator();
                ui.label(style::dim(ui, "🔍"));
                ui.add(
                    egui::TextEdit::singleline(&mut self.cat_search)
                        .hint_text("search name, vendor, spec, what it senses…")
                        .desired_width(300.0),
                );
                if !self.cat_search.is_empty() && ui.button("✕").on_hover_text("clear search").clicked() {
                    self.cat_search.clear();
                }
            });
            if self.cat_tab == 1 {
                ui.add_space(style::GAP_XS);
                ui.horizontal_wrapped(|ui| {
                    ui.label(style::dim(ui, "kind"));
                    for k in ["all", "board", "sensor", "display", "actuator", "tool"] {
                        ui.selectable_value(&mut self.cat_kind, k.to_string(), k);
                    }
                });
            }
        });
        ui.add_space(style::GAP_S);
        if self.cat_tab == 3 {
            self.dex.show(ui, ctx, &self.client, &self.catalog);
            return;
        }
        let q = self.cat_search.to_lowercase();
        let (tab, kind) = (self.cat_tab, self.cat_kind.clone());
        let cat = &self.catalog;
        let search = &self.cat_search;
        // Filter first so the result count can sit at the top where it's scannable, and an
        // empty result reads as a clear "nothing matched" instead of a bare "0".
        match tab {
            0 => {
                let items: Vec<_> = cat.projects.iter().filter(|p| q.is_empty() || proj_hay(p).contains(&q)).collect();
                render_catalog_list(ui, "projects", &items, search, project_card);
            }
            2 => {
                let items: Vec<_> = cat.chips.iter().filter(|c| q.is_empty() || chip_hay(c).contains(&q)).collect();
                render_catalog_list(ui, "chips", &items, search, chip_card);
            }
            _ => {
                let items: Vec<_> = cat
                    .modules
                    .iter()
                    .filter(|m| (kind == "all" || m.kind == kind) && (q.is_empty() || mod_hay(m).contains(&q)))
                    .collect();
                let (market, host) = {
                    let sh = self.client.snapshot();
                    (sh.catalog.clone(), sh.host.clone().and_then(|r| r.ok()))
                };
                let pc = PanelCtx { client: &self.client, egui: ctx, market: market.as_ref(), host: host.as_ref(), guides: &self.guides, events: &self.events };
                let focus = self.focus_module.clone();
                render_catalog_list(ui, "modules", &items, search, |ui, m| sensor_detail::module_card(ui, m, &pc, focus.as_deref() == Some(m.id.as_str())));
                self.focus_module = None;
            }
        }
    }

    fn network_view(&self, ui: &mut egui::Ui) {
        let net = self.client.snapshot().net.clone();
        style::section_header(ui, "Network", "the fleet this OS is part of — tailnet peers + the Cognitum mesh overlay");
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

    fn fleet_tables(&self, ui: &mut egui::Ui, n: &Net) {
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

    fn sensors_view(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        // Guide open? Render it; otherwise list the running sensor cogs with a "Guide" button each.
        if self.guide_cog.is_some() {
            self.guide_detail(ui, ctx);
            return;
        }

        style::section_header(ui, "Sensors", "running sensor cogs and their hook-up guides");
        let running: Vec<HostCog> = match &self.client.snapshot().host {
            Some(Ok(h)) => h.cogs.iter().filter(|c| c.running).cloned().collect(),
            _ => Vec::new(),
        };
        if running.is_empty() {
            ui.add_space(style::GAP_M);
            ui.vertical_centered(|ui| {
                ui.label(style::dim(ui, "No cogs running. Start one in the Cogs tab, then open its guide here."));
            });
        } else {
            for c in &running {
                let mut open_guide = false;
                style::card_frame(ui).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        style::truncated(ui, &c.id, true);
                        style::pill(ui, &format!("v{}", c.version), GREY);
                        self.hw_links(ui, &c.id);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            open_guide = ui.button("📖  Open guide").clicked();
                        });
                    });
                });
                ui.add_space(style::GAP_XS);
                if open_guide {
                    self.open_guide(&c.id, None, ctx);
                }
            }
        }
        ui.add_space(style::GAP_M);
        ui.label(
            style::dim(
                ui,
                "The guide is served by each cog on its own export port — no Seed agent needed. \
                 Live dashboards (ECG waveform, ToF heatmap, generic trace) come next.",
            )
            .italics(),
        );
    }

    /// Switch to the Sensors tab with this cog's guide open (optionally at a page).
    fn open_guide(&mut self, id: &str, page: Option<&'static str>, ctx: &egui::Context) {
        let port = default_export_port(id);
        self.section = Section::Sensors;
        self.guide_cog = Some(id.to_string());
        self.guide_bundle = None;
        self.guide_view = GuideView::at(page.map(str::to_string));
        self.guide_port_draft = if port == 0 { String::new() } else { port.to_string() };
        if port != 0 {
            self.client.fetch_guide(id, port, ctx);
        }
    }

    /// Apply what the panel and cross-links asked for this frame, with the same client calls the
    /// Cogs tab uses (install / start / stop) plus tab navigation.
    fn apply_requests(&mut self, ctx: &egui::Context) {
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
                Event::OpenGuide { cog, page } => self.open_guide(&cog, page, ctx),
            }
        }
    }

    /// Render the open cog's `/guide` bundle (fetched into `Shared.guide`), with a back button and
    /// an editable export port for cogs whose default port we don't know.
    fn guide_detail(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
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
        ui.separator();

        // Parse the fetched JSON into a GuideBundle once, then cache it.
        if self.guide_bundle.is_none() {
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
            None => {
                ui.add_space(style::GAP_S);
                if self.guide_port_draft.trim().is_empty() {
                    ui.label(RichText::new("Unknown export port for this cog — enter it above and press Load.").color(AMBER));
                    ui.label(style::dim(ui, "(It's the cog's [api].bind_port, e.g. 8050 for rd-03e.)"));
                } else {
                    ui.label(style::dim(ui, "Loading guide…"));
                }
            }
            Some(Ok(bundle)) => {
                let bundle = bundle.clone();
                self.guide_view.show(ui, &bundle);
            }
            Some(Err(e)) => {
                ui.add_space(style::GAP_S);
                ui.label(RichText::new(format!("Couldn't load the guide: {e}")).color(RED));
                ui.label(style::dim(ui, "Is the cog running and its export port reachable? Adjust the port above and press Load."));
            }
        }
    }

    fn apps_view(&self, ui: &mut egui::Ui) {
        ui.add_space(style::GAP_L);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("▦").size(48.0).color(style::muted(ui)));
            ui.add_space(style::GAP_S);
            style::h2(ui, "No apps yet");
            ui.add_space(style::GAP_XS);
            ui.label(style::dim(ui, "Apps install here alongside cogs. The OS runs them the same way — supervised by WeftOS, no agent cap. Coming next."));
        });
    }

    fn system_view(&self, ui: &mut egui::Ui) {
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
