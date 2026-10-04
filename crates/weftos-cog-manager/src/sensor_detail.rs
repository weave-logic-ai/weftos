//! The sensor detail panel (ADR-107): what a module card shows when expanded. Software (the
//! driving cog, where to get it, install state, actions), Setup, Firmware, Docs and, when the cog
//! is running, Stats. The rules live in `sensor_link` (pure, tested); this file only draws and
//! turns clicks into [`Event`]s that the app applies with the same client calls the Cogs tab uses.

use crate::client::{Client, HostStatus};
use crate::sensor_link::{self as link, Badge, CogOutput, CogView, DocTarget, FwRead};
use crate::sensor_mesh::MeshView;
use crate::style;
use eframe::egui::{self, Color32, RichText, Ui};
use std::cell::RefCell;
use std::collections::BTreeMap;
use weftos_cog_market::hw::{HwCatalog, Module};
use weftos_cog_market::{Catalog, Source};
use weftos_sensor_guide::{GuideBundle, GuideDoc};

pub(crate) const GREEN: Color32 = Color32::from_rgb(0x4c, 0xc2, 0x7a);
pub(crate) const RED: Color32 = Color32::from_rgb(0xe0, 0x5a, 0x5a);
pub(crate) const GREY: Color32 = Color32::from_rgb(0x88, 0x88, 0x88);
pub(crate) const AMBER: Color32 = Color32::from_rgb(0xd8, 0xa0, 0x3a);
pub(crate) const WL: Color32 = Color32::from_rgb(0x6c, 0x9c, 0xe8);
pub(crate) const COG: Color32 = Color32::from_rgb(0xb0, 0x82, 0xd8);

/// What the panel asks the app to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Install { id: String, source: Source, version: String },
    Lifecycle { id: String, action: &'static str },
    /// Open the Sensors tab guide for a cog, optionally at a page.
    OpenGuide { cog: String, page: Option<&'static str> },
    /// Point the console at another node's cog-host (to check and install there).
    SwitchHost { url: String },
}

/// Per-cog guide state for the Setup section (the cog serves `/guide` only while running).
pub enum GuideState {
    Ready(Box<GuideDoc>),
    Failed(String),
}

pub type GuideCache = BTreeMap<String, GuideState>;

/// Everything the panel reads, plus the two things it writes (events, guide cache).
pub struct PanelCtx<'a> {
    pub client: &'a Client,
    pub egui: &'a egui::Context,
    pub catalog: &'a HwCatalog,
    pub market: Option<&'a Catalog>,
    pub host: Option<&'a HostStatus>,
    pub mesh: Option<&'a MeshView>,
    pub guides: &'a RefCell<GuideCache>,
    pub events: &'a RefCell<Vec<Event>>,
    /// The node the user picked to install each cog on (cog id -> node name).
    pub target: &'a RefCell<BTreeMap<String, String>>,
    /// Deep link (`WEFTOS_STEP` / `?step=`): scroll the install flow's step N into view.
    pub focus_step: Option<u8>,
}

pub fn badge_pill(ui: &mut Ui, b: Badge) {
    let color = match b {
        Badge::Running => GREEN,
        Badge::Installed => WL,
        Badge::Available => COG,
        Badge::Linked => GREY,
        Badge::NoCog => return,
    };
    if let Some(t) = b.label() {
        style::pill(ui, t, color);
    }
}

pub fn views(m: &Module, market: Option<&Catalog>, host: Option<&HostStatus>, mesh: Option<&MeshView>) -> Vec<CogView> {
    m.cogs.iter().map(|c| link::cog_view(c, market, host, mesh)).collect()
}

fn now_ms() -> u64 {
    web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// A module card for the catalog list: the summary facts, then the detail panel when expanded.
/// `focus` forces it open and scrolls to it (a cross-link from another tab landed here).
pub fn module_card(ui: &mut Ui, m: &Module, pc: &PanelCtx, focus: bool) {
    let badge = link::module_badge(&views(m, pc.market, pc.host, pc.mesh));
    style::card_focus(
        ui,
        ("mod", &m.id),
        focus,
        |ui| {
            style::truncated(ui, &m.name, true);
            if !m.kind.is_empty() {
                style::pill(ui, &m.kind, WL);
            }
            badge_pill(ui, badge);
            if !m.vendor.is_empty() {
                style::truncated(ui, &m.vendor, false);
            }
        },
        |ui| {
            let (cogs, out) = prelude(m, pc);
            if !m.summary.is_empty() {
                ui.label(style::body(&m.summary));
            }
            if !m.chips.is_empty() {
                crate::views::catalog::tag_row(ui, m.chips.iter().map(String::as_str), GREY);
            }
            crate::views::catalog::buy_and_datasheet(ui, m.buy.first().map(|b| (b.price.as_str(), b.url.as_str())), &m.datasheet);
            // Software first: how to get, install and run the cog is what the card is for.
            ui.add_space(style::GAP_XS);
            crate::sensor_software::software(ui, m, &cogs, pc);
            section(ui, "Specs");
            if !m.good_for.is_empty() {
                ui.label(style::dim(ui, format!("Good for: {}", m.good_for.join(" · "))));
            }
            if !m.not_for.is_empty() {
                ui.label(style::dim(ui, format!("Not for: {}", m.not_for.join(" · "))));
            }
            for n in &m.notes {
                ui.label(RichText::new(format!("⚠ {n}")).color(AMBER).size(12.0));
            }
            style::spec_grid(ui, ("ms", &m.id), m.spec.iter().map(|(k, v)| (k.as_str(), v.as_str())));
            setup(ui, m, &cogs, pc);
            firmware(ui, m, &cogs, out.as_ref());
            docs(ui, m, &cogs, pc);
            stats(ui, &cogs, pc, out.as_ref());
        },
    );
}

/// Per-frame upkeep for a visible card: the cog views, plus the guide / output / mesh fetches for
/// the ones that are up.
fn prelude(m: &Module, pc: &PanelCtx) -> (Vec<CogView>, Option<CogOutput>) {
    let cogs = views(m, pc.market, pc.host, pc.mesh);
    for v in &cogs {
        pump_guide(&v.id, pc);
        if v.state() == Some(link::CogState::Running) {
            pc.client.ensure_cog_output(&v.id, link::export_port(&v.id, pc.host), pc.egui);
        }
    }
    let out = cog_output(&cogs, pc);
    (cogs, out)
}

/// Parse the cog's bundled hook-up guide into the cache once. The guide ships with the catalog,
/// so this needs no host, no running cog and no network.
fn pump_guide(id: &str, pc: &PanelCtx) {
    if pc.guides.borrow().contains_key(id) {
        return;
    }
    let Some(json) = weftos_cog_market::guides::bundled(id) else { return };
    let parsed = serde_json::from_str::<serde_json::Value>(json).map_err(|e| e.to_string()).and_then(|v| GuideBundle::from_json(&v));
    pc.guides.borrow_mut().insert(
        id.to_string(),
        match parsed {
            Ok(b) => GuideState::Ready(Box::new(b.doc)),
            Err(e) => GuideState::Failed(e),
        },
    );
}

fn cog_output(cogs: &[CogView], pc: &PanelCtx) -> Option<CogOutput> {
    let sh = pc.client.snapshot();
    cogs.iter()
        .filter(|v| v.state() == Some(link::CogState::Running))
        .find_map(|v| match sh.cog_out.get(&v.id)?.result.as_ref()? {
            Ok(j) => Some(CogOutput::parse(j)),
            Err(_) => None,
        })
}

pub(crate) fn section(ui: &mut Ui, title: &str) {
    ui.add_space(style::GAP_S);
    style::subhead(ui, title);
}

pub(crate) fn bullet(ui: &mut Ui, t: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.label(style::dim(ui, "•"));
        ui.label(style::body(t));
    });
}

fn setup(ui: &mut Ui, m: &Module, cogs: &[CogView], pc: &PanelCtx) {
    section(ui, "Setup");
    if !m.pins.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.label(style::dim(ui, "pins"));
            for p in &m.pins {
                style::pill(ui, p, GREY);
            }
        });
    }
    let bus = link::detect_bus(m);
    let steps = link::bus_steps(bus);
    if steps.is_empty() && m.pins.is_empty() {
        ui.label(style::dim(ui, "The catalog has no wiring facts for this module yet."));
    }
    for s in steps {
        bullet(ui, s);
    }
    let power = link::power_rows(m);
    if !power.is_empty() {
        style::spec_grid(ui, ("pw", &m.id), power.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    }
    // The cog's own guide (bundled with the catalog) is the authoritative wiring.
    let guides = pc.guides.borrow();
    for v in cogs {
        match guides.get(&v.id) {
            Some(GuideState::Ready(doc)) => {
                let rows = link::guide_wiring(doc);
                if !rows.is_empty() {
                    ui.label(style::dim(ui, format!("From {}'s guide:", v.id)));
                    for r in rows {
                        bullet(ui, &r);
                    }
                }
            }
            Some(GuideState::Failed(e)) => {
                ui.label(style::dim(ui, format!("{}'s guide is unavailable ({e}); showing catalog data.", v.id)));
            }
            None => {}
        }
    }
}

fn firmware(ui: &mut Ui, m: &Module, cogs: &[CogView], out: Option<&CogOutput>) {
    section(ui, "Firmware");
    let Some(fw) = &m.firmware else {
        ui.label(style::dim(ui, "No firmware facts in the catalog for this module. The cog does not read a version."));
        return;
    };
    let rows: Vec<(&str, &str)> = [("version / variant", fw.version.as_str()), ("update", fw.update.as_str()), ("notes", fw.notes.as_str())]
        .into_iter()
        .filter(|(_, v)| !v.is_empty())
        .collect();
    style::spec_grid(ui, ("fw", &m.id), rows.into_iter());
    if !fw.url.is_empty() {
        ui.hyperlink_to(RichText::new("firmware / vendor page ↗").color(WL), &fw.url);
    }
    match link::firmware_read(m, cogs, out) {
        FwRead::NotSupported => {
            ui.label(style::dim(ui, "No cog can read this module's firmware version."));
        }
        FwRead::Reported(v) => {
            ui.label(RichText::new(format!("Reported by the running cog: {v}")).color(GREEN));
        }
        FwRead::CanRead { cog, key } => {
            ui.label(style::body(format!("{cog} can read it: turn on `{key}`. {}", fw.read_with)));
        }
    }
    ui.label(style::dim(ui, "WeftOS does not update module firmware; use the vendor route above."));
}

fn docs(ui: &mut Ui, m: &Module, cogs: &[CogView], pc: &PanelCtx) {
    section(ui, "Docs");
    for d in link::doc_entries(m) {
        match d.target {
            DocTarget::Web(u) => {
                ui.hyperlink_to(RichText::new(format!("{} ↗", d.label)).color(WL), u);
            }
            DocTarget::Path(p) => {
                ui.horizontal_wrapped(|ui| {
                    ui.label(style::body(&d.label));
                    ui.label(RichText::new(p).monospace().size(11.5).color(style::muted(ui)));
                });
            }
            DocTarget::Guide(cog) => {
                let up = cogs.iter().any(|v| v.id == cog && v.guide);
                let r = ui.add_enabled(up, egui::Button::new(RichText::new(format!("📖  {}", d.label)).color(WL)));
                let r = if up { r } else { r.on_disabled_hover_text("no hook-up guide ships for this cog yet") };
                if r.clicked() {
                    pc.events.borrow_mut().push(Event::OpenGuide { cog, page: None });
                }
            }
        }
    }
}

fn stats(ui: &mut Ui, cogs: &[CogView], pc: &PanelCtx, out: Option<&CogOutput>) {
    let running: Vec<&CogView> = cogs.iter().filter(|v| v.state() == Some(link::CogState::Running)).collect();
    if running.is_empty() {
        return;
    }
    section(ui, "Stats");
    for v in running {
        let Some(inst) = &v.installed else { continue };
        ui.label(style::dim(ui, &v.id));
        let rows = link::stat_rows(inst, out, now_ms());
        style::spec_grid(ui, ("st", &v.id), rows.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        let sh = pc.client.snapshot();
        match sh.cog_out.get(&v.id).and_then(|f| f.result.as_ref()) {
            Some(Err(e)) => {
                ui.label(style::dim(ui, format!("cog output unavailable ({e}); showing host stats only")));
            }
            None if link::export_port(&v.id, pc.host) == 0 => {
                ui.label(style::dim(ui, "export port unknown for this cog; showing host stats only"));
            }
            _ => {}
        }
    }
}
