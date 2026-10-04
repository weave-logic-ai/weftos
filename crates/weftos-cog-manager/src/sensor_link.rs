//! Hardware <-> software link logic for the sensor detail panel (ADR-107).
//!
//! Everything here is egui-independent and pure, so the link resolution and the panel's state
//! rules are unit-tested without a window. The catalog says which cogs drive a module
//! (`Module::cogs`); the marketplace says where a cog can be had; the host says whether it is
//! installed and how it is doing. This module joins the three.

use crate::client::{HostCog, HostStatus};
use crate::sensor_mesh::{cog_mesh, CogMesh, MeshView};
use weftos_cog_market::hw::Module;
use weftos_cog_market::{Catalog, Source};
use weftos_sensor_guide::GuideDoc;

/// The export port each known sensor cog serves `/guide` (and `/status`) on, from each cog.toml
/// `[api].bind_port`. Unknown cogs return 0 (the Sensors tab then prompts for a port).
pub fn default_export_port(id: &str) -> u16 {
    match id {
        "sen0213-ecg" => 8046,
        "sen0628-tof" => 8047,
        "bridge" => 8048,
        "sound-detect" => 8049,
        "rd-03e" => 8050,
        "hlk-as201" => 8051,
        "ld2450-radar" => 8052,
        "catalog" => 8060,
        "mentra-live" => 8061,
        _ => 0,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CogState {
    Running,
    /// Enabled but not up yet (starting, or restarting after an exit).
    Starting,
    Stopped,
    /// The host's licence gate refused to run it.
    Refused,
}

impl CogState {
    pub fn label(self) -> &'static str {
        match self {
            CogState::Running => "running",
            CogState::Starting => "starting",
            CogState::Stopped => "stopped",
            CogState::Refused => "refused",
        }
    }
}

pub fn cog_state(c: &HostCog) -> CogState {
    if c.licence_refusal.is_some() {
        CogState::Refused
    } else if c.running {
        CogState::Running
    } else if c.enabled {
        CogState::Starting
    } else {
        CogState::Stopped
    }
}

/// A cog as one source offers it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Availability {
    pub source: Source,
    /// Empty when the source's version is not known (the shadowed copy of a dual-listed cog).
    pub version: String,
    pub signed: bool,
}

/// A cog as installed on the connected host.
#[derive(Clone, Debug, PartialEq)]
pub struct Installed {
    pub version: String,
    pub state: CogState,
    pub refusal: Option<String>,
    pub uptime_s: Option<u64>,
    pub restarts: u32,
    pub pid: Option<u32>,
    pub rss_kb: Option<u64>,
    pub last_exit: Option<String>,
}

/// Everything the panel needs to know about one cog.
#[derive(Clone, Debug, PartialEq)]
pub struct CogView {
    pub id: String,
    pub available: Vec<Availability>,
    /// False until the marketplace registries have loaded (availability unknown, not "none").
    pub marketplace_loaded: bool,
    /// True when a host answered `/status`; installed/state are meaningless otherwise.
    pub connected: bool,
    pub installed: Option<Installed>,
    /// Where it runs across the mesh (empty `on` when the mesh view has nothing).
    pub mesh: CogMesh,
}

impl CogView {
    /// Strongest state anywhere: on the connected host or on any mesh node.
    pub fn best_state(&self) -> Option<CogState> {
        let rank = |s: &CogState| match s {
            CogState::Running => 0,
            CogState::Starting => 1,
            CogState::Refused => 2,
            CogState::Stopped => 3,
        };
        self.state().into_iter().chain(self.mesh.best_state()).min_by_key(rank)
    }

    pub fn state(&self) -> Option<CogState> {
        self.installed.as_ref().map(|i| i.state)
    }
    /// The source to install from: signed WeaveLogic first.
    pub fn preferred_source(&self) -> Option<&Availability> {
        self.available.iter().find(|a| a.source == Source::WeaveLogic).or_else(|| self.available.first())
    }
}

pub fn cog_view(id: &str, market: Option<&Catalog>, host: Option<&HostStatus>, mesh: Option<&MeshView>) -> CogView {
    let mut available = Vec::new();
    if let Some(item) = market.and_then(|m| m.find(id)) {
        available.push(Availability { source: item.source, version: item.version.clone(), signed: item.signed });
        if item.also_in_other_source {
            let other = if item.source == Source::WeaveLogic { Source::Cognitum } else { Source::WeaveLogic };
            available.push(Availability { source: other, version: String::new(), signed: other == Source::WeaveLogic });
        }
    }
    let installed = host.and_then(|h| h.cogs.iter().find(|c| c.id == id)).map(|c| Installed {
        version: c.version.clone(),
        state: cog_state(c),
        refusal: c.licence_refusal.clone(),
        uptime_s: c.uptime_s,
        restarts: c.restarts,
        pid: c.pid,
        rss_kb: c.rss_kb,
        last_exit: c.last_exit.clone(),
    });
    CogView { id: id.into(), available, marketplace_loaded: market.is_some(), connected: host.is_some(), installed, mesh: mesh.map(|m| cog_mesh(m, id)).unwrap_or_default() }
}

/// What a module card advertises in the catalog list, weakest to strongest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Badge {
    /// No cog is linked to this module.
    NoCog,
    /// A cog is linked but no marketplace source lists it (source-only, or still loading).
    Linked,
    Available,
    Installed,
    Running,
}

impl Badge {
    pub fn label(self) -> Option<&'static str> {
        match self {
            Badge::NoCog => None,
            Badge::Linked => Some("cog (not published)"),
            Badge::Available => Some("cog available"),
            Badge::Installed => Some("installed"),
            Badge::Running => Some("running"),
        }
    }
}

pub fn module_badge(cogs: &[CogView]) -> Badge {
    cogs.iter()
        .map(|c| match c.best_state() {
            Some(CogState::Running) => Badge::Running,
            Some(_) => Badge::Installed,
            None if !c.available.is_empty() => Badge::Available,
            None => Badge::Linked,
        })
        .max()
        .unwrap_or(Badge::NoCog)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Install(Source),
    Start,
    Stop,
    /// Opens the cog's guide at its config-keys page (the host has no config-write endpoint).
    Configure,
    OpenGuide,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionState {
    pub action: Action,
    pub enabled: bool,
    /// Why it is disabled (shown on hover), empty when enabled.
    pub why: &'static str,
}

fn on(action: Action) -> ActionState {
    ActionState { action, enabled: true, why: "" }
}
fn off(action: Action, why: &'static str) -> ActionState {
    ActionState { action, enabled: false, why }
}

/// The buttons for one cog and whether each can be pressed right now. Empty when there is nothing
/// to do (not installed and no source offers it).
pub fn actions(c: &CogView) -> Vec<ActionState> {
    match (&c.installed, c.preferred_source()) {
        (None, Some(a)) => vec![if c.connected {
            on(Action::Install(a.source))
        } else {
            off(Action::Install(a.source), "connect to a host to install")
        }],
        (None, None) => vec![],
        (Some(i), _) => {
            let up = i.state == CogState::Running;
            let mut v = Vec::new();
            v.push(match i.state {
                CogState::Running => on(Action::Stop),
                CogState::Refused => off(Action::Start, "the host's licence gate refused this cog"),
                _ => on(Action::Start),
            });
            v.push(if up { on(Action::Configure) } else { off(Action::Configure, "start the cog first; its config keys are in its guide") });
            v.push(if up { on(Action::OpenGuide) } else { off(Action::OpenGuide, "the guide is served by the running cog") });
            v
        }
    }
}

/// A one-line plain-language summary of a cog's software state.
pub fn software_line(c: &CogView) -> String {
    match (&c.installed, c.connected) {
        (Some(i), _) => match (&i.refusal, i.state) {
            (Some(code), _) => format!("installed v{} on the host, refused ({code})", i.version),
            (None, s) => format!("installed v{} on the host, {}", i.version, s.label()),
        },
        (None, true) => "not installed on the connected host".into(),
        (None, false) => "no host connected: availability only".into(),
    }
}

/// The cog's own `/status` output line (the latest window), parsed leniently: sensor cogs share
/// the `weavelogic.cog-output.v0` shape but a missing field is just absent.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CogOutput {
    pub health: Option<String>,
    pub quality: Option<f64>,
    pub frame_rate_hz: Option<f64>,
    pub parse_errors: Option<u64>,
    pub timestamp_ms: Option<u64>,
    pub simulated: bool,
    pub firmware: Option<String>,
    pub reasons: Vec<String>,
}

impl CogOutput {
    pub fn parse(v: &serde_json::Value) -> Self {
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
        let firmware = match v.get("firmware") {
            Some(serde_json::Value::String(f)) => Some(f.clone()),
            Some(serde_json::Value::Object(o)) => o.get("version").and_then(|x| x.as_str()).map(str::to_string),
            _ => None,
        };
        Self {
            health: s("health"),
            quality: v.get("quality").and_then(|x| x.as_f64()),
            frame_rate_hz: v.get("frame_rate_hz").and_then(|x| x.as_f64()),
            parse_errors: v.get("parse_errors").and_then(|x| x.as_u64()),
            timestamp_ms: v.get("timestamp_ms").and_then(|x| x.as_u64()),
            simulated: v.pointer("/source/simulated").and_then(|x| x.as_bool()).unwrap_or(false),
            firmware,
            reasons: v.get("reasons").and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default(),
        }
    }
}

pub fn fmt_dur(s: u64) -> String {
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else {
        format!("{}h", s / 3600)
    }
}

/// Stats rows for a running cog: host supervision facts first, then the cog's own output. Output
/// rows appear only when the cog's export answered.
pub fn stat_rows(i: &Installed, out: Option<&CogOutput>, now_ms: u64) -> Vec<(String, String)> {
    let mut r = vec![("state".to_string(), i.state.label().to_string())];
    if let Some(u) = i.uptime_s {
        r.push(("uptime".into(), fmt_dur(u)));
    }
    r.push(("restarts".into(), i.restarts.to_string()));
    if let Some(p) = i.pid {
        let rss = i.rss_kb.map(|k| format!(" / {} MB", k / 1024)).unwrap_or_default();
        r.push(("pid / RSS".into(), format!("{p}{rss}")));
    }
    if let Some(e) = i.last_exit.as_ref().filter(|e| !e.is_empty()) {
        r.push(("last exit".into(), e.clone()));
    }
    if let Some(o) = out {
        if let Some(h) = &o.health {
            let why = if o.reasons.is_empty() { String::new() } else { format!(" ({})", o.reasons.join(", ")) };
            r.push(("health".into(), format!("{h}{why}")));
        }
        if let Some(t) = o.timestamp_ms {
            r.push(("last output".into(), format!("{} ago", fmt_dur(now_ms.saturating_sub(t) / 1000))));
        }
        if let Some(f) = o.frame_rate_hz {
            r.push(("frame rate".into(), format!("{f:.1} Hz")));
        }
        if let Some(e) = o.parse_errors {
            r.push(("parse errors".into(), e.to_string()));
        }
        if let Some(q) = o.quality {
            r.push(("link quality".into(), format!("{:.0}%", q * 100.0)));
        }
        if o.simulated {
            r.push(("source".into(), "simulated (not the real sensor)".into()));
        }
    }
    r
}

/// A module's name without its parenthetical (`"HLK-LD2450 (24 GHz ...)"` -> `"HLK-LD2450"`), for
/// compact cross-links.
pub fn short_name(name: &str) -> &str {
    name.split(" (").next().unwrap_or(name).trim()
}

// ---- Setup ----------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bus {
    Uart,
    I2c,
    Usb,
    Analog,
    Unknown,
}

fn spec_text(m: &Module) -> String {
    let mut t = m.spec.iter().filter(|(k, _)| k.as_str() != "source").map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(" ");
    t.push(' ');
    t.push_str(&m.pins.join(" "));
    t.to_lowercase()
}

pub fn detect_bus(m: &Module) -> Bus {
    let t = spec_text(m);
    if t.contains("uart") {
        Bus::Uart
    } else if t.contains("i2c") {
        Bus::I2c
    } else if t.contains("usb") {
        Bus::Usb
    } else if t.contains("analog") {
        Bus::Analog
    } else {
        Bus::Unknown
    }
}

/// Bus enablement and wiring steps. Device changes (serial console, I2C) are approval-gated, so
/// the text says so rather than offering to do them.
pub fn bus_steps(bus: Bus) -> Vec<&'static str> {
    match bus {
        Bus::Uart => vec![
            "Wire sensor TX to the host RX and sensor RX to the host TX (crossed), at 3.3 V logic.",
            "Enable the hardware UART and free it from the login console. This changes the device's configuration and needs the owner's approval.",
            "Set the cog's serial device and baud to match the module.",
        ],
        Bus::I2c => vec![
            "Enable I2C on the host (this changes device configuration and needs the owner's approval).",
            "Wire SDA, SCL, power and ground; confirm the address with `i2cdetect -y 1` before starting the cog.",
        ],
        Bus::Usb => vec!["Plug the module in; it appears as /dev/ttyUSB0 or /dev/ttyACM0. Point the cog's device setting at it."],
        Bus::Analog => vec![
            "This module outputs an analog level: read it through an ADC (an ADS1115 on the I2C bus).",
            "Enable I2C on the host (needs the owner's approval) and wire the output to an ADC channel.",
        ],
        Bus::Unknown => vec![],
    }
}

/// Power-related spec rows (whatever the catalog carries under power-ish keys).
pub fn power_rows(m: &Module) -> Vec<(String, String)> {
    m.spec
        .iter()
        .filter(|(k, _)| matches!(k.as_str(), "power" | "vin" | "vcc" | "logic" | "current" | "voltage"))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// Header-pin and wire rows from a loaded guide bundle (the cog's own, authoritative wiring).
pub fn guide_wiring(doc: &GuideDoc) -> Vec<String> {
    let mut rows = Vec::new();
    if let Some(h) = &doc.header {
        for p in &h.used {
            rows.push(format!("header pin {} ({}) -> {}", p.pin, p.name, p.to));
        }
    }
    for w in &doc.wires {
        rows.push(format!("{} -> {}", w.from, w.to));
    }
    rows
}

// ---- Firmware -------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FwRead {
    /// No cog can read the version off this module.
    NotSupported,
    /// A cog can read it once this config key is on.
    CanRead { cog: String, key: String },
    /// The running cog reported it.
    Reported(String),
}

pub fn firmware_read(m: &Module, cogs: &[CogView], out: Option<&CogOutput>) -> FwRead {
    let Some(fw) = m.firmware.as_ref().filter(|f| !f.read_with.is_empty()) else {
        return FwRead::NotSupported;
    };
    if let Some(v) = out.and_then(|o| o.firmware.clone()) {
        return FwRead::Reported(v);
    }
    FwRead::CanRead { cog: cogs.first().map(|c| c.id.clone()).unwrap_or_default(), key: fw.read_config_key.clone() }
}

// ---- Docs -----------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocTarget {
    Web(String),
    /// A repo-relative path or `repo: path` reference (shown, not linked).
    Path(String),
    /// Deep link into the Sensors tab for this cog's guide.
    Guide(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocEntry {
    pub label: String,
    pub target: DocTarget,
}

pub const SENSOR_COG_SKILL: &str = "cognitum-cogs: .claude/skills/sensor-cog/SKILL.md";

fn target_of(url: &str) -> DocTarget {
    if url.starts_with("http://") || url.starts_with("https://") {
        DocTarget::Web(url.into())
    } else {
        DocTarget::Path(url.into())
    }
}

pub fn doc_entries(m: &Module) -> Vec<DocEntry> {
    let mut v = Vec::new();
    if !m.datasheet.is_empty() {
        v.push(DocEntry { label: "Datasheet".into(), target: target_of(&m.datasheet) });
    }
    for c in &m.cogs {
        v.push(DocEntry { label: format!("Sensor guide: {c} (Sensors tab)"), target: DocTarget::Guide(c.clone()) });
    }
    for d in &m.docs {
        v.push(DocEntry { label: d.label.clone(), target: target_of(&d.url) });
    }
    if let Some(b) = m.buy.iter().find(|b| !b.url.is_empty()) {
        let price = if b.price.is_empty() { String::new() } else { format!(" ({})", b.price) };
        v.push(DocEntry { label: format!("Buy: {}{price}", b.vendor), target: DocTarget::Web(b.url.clone()) });
    }
    for s in &m.seen_in {
        v.push(DocEntry { label: "Catalog source".into(), target: DocTarget::Path(s.clone()) });
    }
    v
}

#[cfg(test)]
#[path = "sensor_link_tests.rs"]
mod tests;
