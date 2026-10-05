//! The detail panel's Software section: the install flow for a sensor cog (ADR-107).
//!
//! 1. read the hook-up guide (ships with the catalog: no install, no host, no device needed);
//! 2. pick the node to install on (the mesh nodes the connected host knows);
//! 3. pre-install check on that node (bus present, serial console off, port conflicts, arch);
//! 4. install (or start / stop) with the same client calls the Cogs tab uses;
//! 5. post-install check (running, sensor found vs `no_source`).
//!
//! The rules are in `sensor_install` / `sensor_link` (pure, tested); this file draws them.

use crate::sensor_detail::{section, Event, PanelCtx, AMBER, COG, GREEN, GREY, RED, WL};
use crate::sensor_install::{self as inst, Check, Verdict};
use crate::sensor_link::{self as link, Action, CogState, CogView};
use crate::sensor_mesh::{self as mesh, Scope};
use crate::style;
use eframe::egui::{self, RichText, Ui};
use weftos_cog_market::hw::Module;
use weftos_cog_market::Source;

pub(crate) fn software(ui: &mut Ui, m: &Module, cogs: &[CogView], pc: &PanelCtx) {
    section(ui, "Software");
    if cogs.is_empty() {
        ui.label(RichText::new("No cog drives this module yet.").color(AMBER));
        ui.label(style::dim(
            ui,
            "To get one: ask for it on the dashboard board (cite this module), or build it with the sensor-cog skill: identify the part, verify the protocol from the vendor documents, write a resyncing parser with fixtures, add --simulate, a guide and an ADR.",
        ));
        ui.label(style::dim(ui, link::SENSOR_COG_SKILL));
        return;
    }
    for v in cogs {
        cog_block(ui, v, m, pc);
    }
}

fn cog_block(ui: &mut Ui, v: &CogView, m: &Module, pc: &PanelCtx) {
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
        install_flow(ui, v, m, pc);
    });
    ui.add_space(style::GAP_XS);
}

fn step(ui: &mut Ui, pc: &PanelCtx, n: u8, title: &str) {
    ui.add_space(style::GAP_XS);
    let r = ui.label(RichText::new(format!("{n}. {title}")).strong().size(12.5));
    if pc.focus_step == Some(n) {
        r.scroll_to_me(Some(egui::Align::TOP));
    }
}

/// The five steps. Reading the guide never depends on a host; the rest need a connected node.
fn install_flow(ui: &mut Ui, v: &CogView, m: &Module, pc: &PanelCtx) {
    ui.separator();
    // 1. guide
    step(ui, pc, 1, "Read the hook-up guide");
    if v.guide {
        if ui.button(RichText::new("📖  Open the full guide (wiring, pins, bus, power)").color(WL)).clicked() {
            pc.events.borrow_mut().push(Event::OpenGuide { cog: v.id.clone(), page: None });
        }
        ui.label(style::dim(ui, "ships with the catalog: readable now, before installing, with no device"));
    } else {
        ui.label(style::dim(ui, "no hook-up guide ships for this cog yet; use the catalog's Setup facts below"));
    }
    // 2. node
    step(ui, pc, 2, "Pick the node to install on");
    let Some(mv) = pc.mesh.filter(|mv| !mv.nodes.is_empty()) else {
        ui.label(style::dim(ui, "no host connected: connect one in the top bar to pick a node, check it and install"));
        return;
    };
    let reachable: Vec<&mesh::MeshNodeView> = mv.nodes.iter().filter(|n| n.reachable).collect();
    // nodes are picked by key (address; "self" for the connected host), never by name
    let picked = pc.target.borrow().get(&v.id).cloned().filter(|k| reachable.iter().any(|n| n.key() == *k)).unwrap_or_else(|| "self".into());
    let label_of = |n: &mesh::MeshNodeView| if n.is_self { format!("{} (connected)", n.name) } else if n.name.is_empty() { n.ip.clone() } else { format!("{} ({})", n.name, n.ip) };
    egui::ComboBox::from_id_salt(("target", &v.id))
        .selected_text(reachable.iter().find(|n| n.key() == picked).map(|n| label_of(n)).unwrap_or_default())
        .show_ui(ui, |ui| {
            for n in &reachable {
                if ui.selectable_label(n.key() == picked, label_of(n)).clicked() {
                    pc.target.borrow_mut().insert(v.id.clone(), n.key());
                }
            }
        });
    if matches!(&mv.scope, Scope::ThisHostOnly(_)) {
        ui.label(style::dim(ui, "only this host is known; other nodes appear once the host can see them (mesh view above)"));
    }
    if picked != "self" {
        let node = reachable.iter().find(|n| n.key() == picked);
        let name = node.map(|n| label_of(n)).unwrap_or_default();
        ui.label(style::dim(ui, "Checks and install run on that node's own cog-host. Opening it registers a mesh key there. This node's key stays here."));
        match node.and_then(|n| inst::peer_url(&pc.client.s.host, &n.ip)) {
            Some(url) => {
                if ui.button(format!("Switch the console to {url}")).on_hover_text(format!("{name}. Registers a mesh key on that node.")).clicked() {
                    pc.events.borrow_mut().push(Event::SwitchHost { url });
                }
            }
            None => {
                ui.label(RichText::new("this node's address is not a tailnet address, so the console will not switch to it").color(AMBER));
            }
        }
        return;
    }
    // 3. pre-install check
    step(ui, pc, 3, "Check this node");
    pc.client.ensure_node_facts(pc.egui);
    let (facts, why) = match pc.client.snapshot().node_facts.clone() {
        Some(Ok(f)) => (Some(f), String::new()),
        Some(Err(e)) => (None, e),
        None => (None, "checking…".into()),
    };
    let item = pc.market.and_then(|mk| mk.find(&v.id));
    let checks = inst::preinstall(m, &v.id, item, facts.as_ref(), pc.catalog);
    if !why.is_empty() {
        ui.label(style::dim(ui, format!("node facts unavailable: {why}")));
    }
    check_rows(ui, &("pre", &v.id), &checks);
    // 4. install
    step(ui, pc, 4, "Install");
    let ready = inst::ready(&checks);
    ui.horizontal_wrapped(|ui| {
        for a in link::actions(v) {
            if matches!(a.action, Action::Configure | Action::OpenGuide) {
                continue;
            }
            let label_override = (!ready && matches!(a.action, Action::Install(_))).then_some("Install anyway");
            if action_button(ui, &a, label_override) {
                push_action(v, a.action, pc);
            }
        }
        if v.installed.is_some() && v.guide && ui.button("Configure").on_hover_text("Opens the cog's guide at its config keys").clicked() {
            pc.events.borrow_mut().push(Event::OpenGuide { cog: v.id.clone(), page: Some("api") });
        }
    });
    if !link::actions(v).iter().any(|a| matches!(a.action, Action::Install(_) | Action::Start | Action::Stop)) {
        ui.label(style::dim(ui, "no marketplace source offers this cog, so there is nothing to install from yet. Publish it to a source, or stage it on the node with `weft-cog-host add`."));
    }
    if !ready {
        ui.label(style::dim(ui, "a check failed: fix it first, or install anyway and the post-install check will say what it sees"));
    }
    // 5. post-install check
    step(ui, pc, 5, "Verify after install");
    let out = pc.client.snapshot().cog_out.get(&v.id).and_then(|f| f.result.as_ref().and_then(|r| r.as_ref().ok().map(link::CogOutput::parse)));
    check_rows(ui, &("post", &v.id), &inst::postinstall(v.installed.as_ref(), out.as_ref()));
}

fn check_rows(ui: &mut Ui, id: &impl std::hash::Hash, checks: &[Check]) {
    egui::Grid::new(ui.make_persistent_id(id)).num_columns(3).spacing([10.0, 3.0]).show(ui, |ui| {
        for c in checks {
            let color = match c.verdict {
                Verdict::Pass => GREEN,
                Verdict::Fail => RED,
                Verdict::Warn => AMBER,
                Verdict::Manual => GREY,
            };
            ui.label(RichText::new(c.verdict.mark()).color(color).strong());
            ui.label(RichText::new(c.name).size(12.0));
            ui.vertical(|ui| {
                ui.label(RichText::new(&c.detail).size(12.0).color(color));
                if !c.fix.is_empty() {
                    ui.label(style::dim(ui, format!("fix: {}", c.fix)));
                }
            });
            ui.end_row();
        }
    });
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

fn action_button(ui: &mut Ui, a: &link::ActionState, label_override: Option<&str>) -> bool {
    let (label, color) = match a.action {
        Action::Install(s) => (format!("{} ({})", label_override.unwrap_or("Install"), s.label()), GREEN),
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

