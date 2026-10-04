//! The sensor detail panel (ADR-107): what a module card shows when expanded. Software (the
//! driving cog, where to get it, install state, actions), Setup, Firmware, Docs and, when the cog
//! is running, Stats. The rules live in `sensor_link` (pure, tested); this file only draws and
//! turns clicks into [`Event`]s that the app applies with the same client calls the Cogs tab uses.

use crate::client::{Client, HostStatus};
use crate::sensor_link::{self as link, Action, Badge, CogOutput, CogView, CogState, DocTarget, FwRead};
use crate::sensor_mesh::{self as mesh, MeshView, Scope};
use crate::style;
use eframe::egui::{self, Color32, RichText, Ui};
use std::cell::RefCell;
use std::collections::BTreeMap;
use weftos_cog_market::hw::Module;
use weftos_cog_market::{Catalog, Source};
use weftos_sensor_guide::{GuideBundle, GuideDoc};

const GREEN: Color32 = Color32::from_rgb(0x4c, 0xc2, 0x7a);
const RED: Color32 = Color32::from_rgb(0xe0, 0x5a, 0x5a);
const GREY: Color32 = Color32::from_rgb(0x88, 0x88, 0x88);
const AMBER: Color32 = Color32::from_rgb(0xd8, 0xa0, 0x3a);
const WL: Color32 = Color32::from_rgb(0x6c, 0x9c, 0xe8);
const COG: Color32 = Color32::from_rgb(0xb0, 0x82, 0xd8);

/// What the panel asks the app to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Install { id: String, source: Source, version: String },
    Lifecycle { id: String, action: &'static str },
    /// Open the Sensors tab guide for a cog, optionally at a page.
    OpenGuide { cog: String, page: Option<&'static str> },
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
    pub market: Option<&'a Catalog>,
    pub host: Option<&'a HostStatus>,
    pub mesh: Option<&'a MeshView>,
    pub guides: &'a RefCell<GuideCache>,
    pub events: &'a RefCell<Vec<Event>>,
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
                crate::tag_row(ui, m.chips.iter().map(String::as_str), GREY);
            }
            crate::buy_and_datasheet(ui, m.buy.first().map(|b| (b.price.as_str(), b.url.as_str())), &m.datasheet);
            // Software first: how to get, install and run the cog is what the card is for.
            ui.add_space(style::GAP_XS);
            software(ui, &cogs, pc);
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
    for v in cogs.iter().filter(|v| v.state() == Some(link::CogState::Running)) {
        pump_guide(&v.id, pc);
        let port = link::default_export_port(&v.id);
        if port != 0 {
            pc.client.ensure_cog_output(&v.id, port, pc.egui);
        }
    }
    let out = cog_output(&cogs, pc);
    (cogs, out)
}

/// Request the cog's guide once and parse it into the cache when it lands.
fn pump_guide(id: &str, pc: &PanelCtx) {
    if pc.guides.borrow().contains_key(id) {
        return;
    }
    let port = link::default_export_port(id);
    if port == 0 {
        return;
    }
    let slot = pc.client.snapshot().guide.as_ref().map(|g| (g.id.clone(), g.result.clone()));
    match slot {
        Some((gid, Some(res))) if gid == id => {
            let parsed = res.and_then(|v| GuideBundle::from_json(&v));
            pc.guides.borrow_mut().insert(
                id.to_string(),
                match parsed {
                    Ok(b) => GuideState::Ready(Box::new(b.doc)),
                    Err(e) => GuideState::Failed(e),
                },
            );
            pc.client.clear_guide();
        }
        Some((gid, None)) if gid == id => {}
        Some((_, None)) => {} // the slot is busy fetching another cog's guide; retry next frame
        _ => pc.client.fetch_guide(id, port, pc.egui),
    }
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

fn section(ui: &mut Ui, title: &str) {
    ui.add_space(style::GAP_S);
    style::subhead(ui, title);
}

fn software(ui: &mut Ui, cogs: &[CogView], pc: &PanelCtx) {
    section(ui, "Software");
    if cogs.is_empty() {
        ui.label(RichText::new("No cog drives this module yet.").color(AMBER));
        ui.label(style::dim(
            ui,
            "To get one: ask for it on the dashboard board (cite this module), or build it with the sensor-cog skill: identify the part, verify the protocol from the vendor documents, write a resyncing parser with fixtures, add --simulate, a guide and an ADR.",
        ));
        ui.label(RichText::new(link::SENSOR_COG_SKILL).monospace().size(12.0));
        return;
    }
    for v in cogs {
        cog_block(ui, v, pc);
    }
}

fn cog_block(ui: &mut Ui, v: &CogView, pc: &PanelCtx) {
    style::card_frame(ui).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(&v.id).strong());
            match v.state() {
                Some(link::CogState::Running) => style::pill(ui, "running", GREEN),
                Some(link::CogState::Refused) => style::pill(ui, "refused", RED),
                Some(s) => style::pill(ui, s.label(), AMBER),
                None => style::pill(ui, "not installed", GREY),
            };
            ui.label(style::dim(ui, link::software_line(v)));
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(style::dim(ui, "available from"));
            if v.available.is_empty() {
                let why = if v.marketplace_loaded { format!("no marketplace source lists it{}", unchecked_sources(pc)) } else { "marketplace loading...".into() };
                ui.label(style::dim(ui, why));
            }
            for a in &v.available {
                let (c, n) = match a.source {
                    Source::WeaveLogic => (WL, "WeaveLogic"),
                    Source::Cognitum => (COG, "Cognitum"),
                };
                let ver = if a.version.is_empty() { String::new() } else { format!(" v{}", a.version) };
                let signed = if a.signed { " · signed" } else { " · unsigned" };
                style::pill(ui, &format!("{n}{ver}{signed}"), c);
            }
        });
        if let Some(code) = v.installed.as_ref().and_then(|i| i.refusal.as_ref()) {
            ui.label(RichText::new(format!("The host refused to run it: {code}. Fix the licence grant, then start it.")).color(RED).size(12.0));
        }
        mesh_table(ui, v, pc);
        ui.horizontal_wrapped(|ui| {
            for a in link::actions(v) {
                if action_button(ui, &a) {
                    push_action(v, a.action, pc);
                }
            }
        });
    });
    ui.add_space(style::GAP_XS);
}

/// " (WeaveLogic registry not configured, ...)" when a source was not actually consulted, so an
/// empty availability list is not mistaken for "nobody publishes this".
fn unchecked_sources(pc: &PanelCtx) -> String {
    let sh = pc.client.snapshot();
    let mut miss = Vec::new();
    if let Some(Err(e)) = &sh.our_reg {
        miss.push(format!("WeaveLogic registry: {e}"));
    }
    if let Some(Err(e)) = &sh.cognitum_reg {
        miss.push(format!("Cognitum registry: {e}"));
    }
    if miss.is_empty() { String::new() } else { format!(" ({})", miss.join("; ")) }
}

/// Where the cog is on the mesh: per node version, state, restarts, last output and output-log
/// size. The log is the only backlog a host holds (cogs post straight to the store, so there is no
/// queue to show), and it is labelled as such.
fn mesh_table(ui: &mut Ui, v: &CogView, pc: &PanelCtx) {
    let scope_line = match pc.mesh.map(|m| &m.scope) {
        Some(Scope::Mesh) => format!("mesh-wide: {} node(s) asked", pc.mesh.map(|m| m.nodes.len()).unwrap_or(0)),
        Some(Scope::ThisHostOnly(why)) => format!("this host only ({why})"),
        None => "this host only".into(),
    };
    let tally = if v.mesh.on.is_empty() { String::new() } else { format!(" · running on {} of {} node(s) that have it", v.mesh.running_nodes(), v.mesh.on.len()) };
    ui.label(style::dim(ui, format!("On the mesh · {scope_line}{tally}")));
    if v.mesh.on.is_empty() {
        ui.label(style::dim(ui, "not installed on any node that answered"));
    } else {
        egui::ScrollArea::horizontal().id_salt(("mesh_scroll", &v.id)).show(ui, |ui| {
        egui::Grid::new(("mesh", &v.id)).num_columns(7).spacing([12.0, 3.0]).show(ui, |ui| {
            for h in ["node", "version", "state", "restarts", "up", "last output", "output log"] {
                ui.label(RichText::new(h).strong().small());
            }
            ui.end_row();
            for n in &v.mesh.on {
                let name = if n.is_self { format!("{} (connected)", n.node) } else { n.node.clone() };
                ui.label(RichText::new(name).strong().size(12.0)).on_hover_text(if n.ip.is_empty() { "the connected host" } else { n.ip.as_str() });
                ui.label(RichText::new(format!("v{}", n.version)).size(12.0));
                let c = match n.state {
                    CogState::Running => GREEN,
                    CogState::Refused => RED,
                    _ => AMBER,
                };
                ui.label(RichText::new(n.refusal.clone().map(|r| format!("refused: {r}")).unwrap_or_else(|| n.state.label().into())).color(c).size(12.0));
                ui.label(RichText::new(n.restarts.to_string()).size(12.0));
                ui.label(RichText::new(n.uptime_s.map(link::fmt_dur).unwrap_or_else(|| "—".into())).size(12.0));
                ui.label(RichText::new(n.log_age_s.map(|a| format!("{} ago", link::fmt_dur(a))).unwrap_or_else(|| "—".into())).size(12.0));
                ui.label(RichText::new(n.log_bytes.map(mesh::fmt_bytes).unwrap_or_else(|| "—".into())).size(12.0));
                ui.end_row();
            }
        });
        });
        ui.label(style::dim(ui, "output log = the cog's stdout/stderr file on that node; cogs post straight to the store, so there is no queue to count."));
    }
    if !v.mesh.without.is_empty() {
        ui.label(style::dim(ui, format!("not installed on: {}", v.mesh.without.join(", "))));
    }
    for (n, why) in &v.mesh.unreachable {
        ui.label(style::dim(ui, format!("{n}: no answer ({why})")));
    }
}

fn action_button(ui: &mut Ui, a: &link::ActionState) -> bool {
    let (label, color) = match a.action {
        Action::Install(s) => (format!("Install ({})", s.label()), GREEN),
        Action::Start => ("Start".into(), GREEN),
        Action::Stop => ("Stop".into(), RED),
        Action::Configure => ("Configure".into(), ui.visuals().text_color()),
        Action::OpenGuide => ("Open guide".into(), WL),
    };
    let btn = egui::Button::new(RichText::new(label).color(color));
    let r = ui.add_enabled(a.enabled, btn);
    let r = if a.enabled {
        match a.action {
            Action::Install(Source::WeaveLogic) => r.on_hover_text("Fetch + Ed25519-verify against the pinned WeaveLogic key, then install on the host"),
            Action::Install(_) => r.on_hover_text("Cognitum cog (unsigned): fetched and sha256-checked on the host before it lands"),
            Action::Configure => r.on_hover_text("Opens the cog's guide at its config keys"),
            _ => r,
        }
    } else {
        r.on_disabled_hover_text(a.why)
    };
    r.clicked()
}

fn push_action(v: &CogView, action: Action, pc: &PanelCtx) {
    let ev = match action {
        Action::Install(source) => {
            let version = v.available.iter().find(|a| a.source == source).map(|a| a.version.clone()).unwrap_or_default();
            Event::Install { id: v.id.clone(), source, version }
        }
        Action::Start => Event::Lifecycle { id: v.id.clone(), action: "start" },
        Action::Stop => Event::Lifecycle { id: v.id.clone(), action: "stop" },
        Action::Configure => Event::OpenGuide { cog: v.id.clone(), page: Some("api") },
        Action::OpenGuide => Event::OpenGuide { cog: v.id.clone(), page: None },
    };
    pc.events.borrow_mut().push(ev);
}

fn bullet(ui: &mut Ui, t: &str) {
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
    // The cog's own guide is the authoritative wiring when it is being served.
    let guides = pc.guides.borrow();
    let mut shown = false;
    for v in cogs {
        match guides.get(&v.id) {
            Some(GuideState::Ready(doc)) => {
                let rows = link::guide_wiring(doc);
                if !rows.is_empty() {
                    shown = true;
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
    if !shown && cogs.iter().all(|v| v.state() != Some(link::CogState::Running)) && !cogs.is_empty() {
        ui.label(style::dim(ui, "Start the cog to see its full wiring guide here."));
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
                let up = cogs.iter().any(|v| v.id == cog && v.state() == Some(link::CogState::Running));
                let r = ui.add_enabled(up, egui::Button::new(RichText::new(format!("📖  {}", d.label)).color(WL)));
                let r = if up { r } else { r.on_disabled_hover_text("the guide is served by the running cog") };
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
            None if link::default_export_port(&v.id) == 0 => {
                ui.label(style::dim(ui, "export port unknown for this cog; showing host stats only"));
            }
            _ => {}
        }
    }
}
