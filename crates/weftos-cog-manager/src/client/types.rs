//! Shapes the host and the console exchange, and the shared state the UI reads.

use super::*;

// ---- host /status shape (mirror of weftos_cog_host::supervise::CogStatus) ----

#[derive(Deserialize, Clone, Default, Debug)]
pub struct HostCog {
    pub id: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub running: bool,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub restarts: u32,
    #[serde(default)]
    pub rss_kb: Option<u64>,
    #[serde(default)]
    pub uptime_s: Option<u64>,
    #[serde(default)]
    pub last_exit: Option<String>,
    #[serde(default)]
    pub signed: bool,
    /// Licence-gate refusal code from the host (`no_grant`, ...), when it refused to run the cog.
    #[serde(default)]
    pub licence_refusal: Option<String>,
    /// Output-log size and age (seconds since last write), when the host reports them.
    #[serde(default)]
    pub log_bytes: Option<u64>,
    #[serde(default)]
    pub log_age_s: Option<u64>,
    /// TCP ports the running cog listens on (its export port), reported by the host from `/proc`.
    #[serde(default)]
    pub export_ports: Vec<u16>,
}

/// One node in the host's `/mesh/cogs` answer.
#[derive(Deserialize, Clone, Default)]
pub struct MeshNodeRaw {
    #[serde(default)]
    pub node: String,
    #[serde(default)]
    pub ip: String,
    #[serde(default, rename = "self")]
    pub is_self: bool,
    #[serde(default)]
    pub reachable: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub cogs: Vec<HostCog>,
}

/// The host's `/mesh/cogs` answer: this host plus the tailnet peers that answered as cog hosts.
#[derive(Deserialize, Clone, Default)]
pub struct MeshCogs {
    /// `mesh` | `this_host_only`
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub nodes: Vec<MeshNodeRaw>,
}

/// `GET /hw/buses`: what a node can host (devices the host can see, kernel console, enabled cogs).
#[derive(Deserialize, Clone, Default, Debug)]
pub struct NodeFacts {
    #[serde(default)]
    pub node: String,
    #[serde(default)]
    pub arch: String,
    #[serde(default)]
    pub uart: UartFacts,
    #[serde(default)]
    pub i2c: DevList,
    #[serde(default)]
    pub usb_serial: Vec<String>,
    #[serde(default)]
    pub enabled_cogs: Vec<EnabledCog>,
}

#[derive(Deserialize, Clone, Default, Debug)]
pub struct UartFacts {
    #[serde(default)]
    pub devices: Vec<String>,
    /// `None` = the kernel command line was unreadable (unknown, not "off").
    #[serde(default)]
    pub console_on_uart: Option<bool>,
    #[serde(default)]
    pub console: Option<String>,
}

#[derive(Deserialize, Clone, Default, Debug)]
pub struct DevList {
    #[serde(default)]
    pub devices: Vec<String>,
}

#[derive(Deserialize, Clone, Default, Debug)]
pub struct EnabledCog {
    pub id: String,
    #[serde(default)]
    pub args: Vec<String>,
}

/// The mesh fetch: refetched at most every 6 s while a detail panel or catalog list is showing.
#[derive(Clone)]
pub struct MeshFetch {
    pub fired: Instant,
    pub in_flight: bool,
    /// `Err("unsupported")` when the host predates `/mesh/cogs` (HTTP 404).
    pub result: Option<Result<MeshCogs, String>>,
}

#[derive(Deserialize, Clone, Default)]
pub struct HostStatus {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub root: String,
    #[serde(default)]
    pub running: usize,
    #[serde(default)]
    pub cogs: Vec<HostCog>,
}

// ---- /network shape ----

#[derive(Deserialize, Clone, Default)]
pub struct NetPeer {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub ip: String,
    #[serde(default)]
    pub os: String,
    #[serde(default)]
    pub online: bool,
    #[serde(default, rename = "self")]
    pub is_self: bool,
}

#[derive(Deserialize, Clone, Default)]
pub struct Tailscale {
    #[serde(default)]
    pub available: bool,
    #[serde(default)]
    pub peers: Vec<NetPeer>,
}

#[derive(Deserialize, Clone, Default)]
pub struct FleetNode {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub sensor: String,
    #[serde(default)]
    pub ip: String,
    #[serde(default)]
    pub rssi: Option<i32>,
    #[serde(default)]
    pub battery: Option<f64>,
    #[serde(default)]
    pub fw: String,
    #[serde(default)]
    pub age_s: u64,
    #[serde(default)]
    pub online: bool,
    // ---- heartbeat v2 (self-reported, unauthenticated; display only) ----
    #[serde(default)]
    pub uptime_s: Option<u64>,
    /// Fraction of one core busy.
    #[serde(default)]
    pub load: Option<f64>,
    #[serde(default)]
    pub free_heap: Option<u64>,
    #[serde(default)]
    pub reset_reason: Option<String>,
    #[serde(default)]
    pub chip: Option<String>,
    #[serde(default)]
    pub mac: Option<String>,
    #[serde(default)]
    pub channel: Option<u32>,
    #[serde(default)]
    pub sample_hz: Option<f64>,
    #[serde(default)]
    pub provenance: String,
}

#[derive(Deserialize, Clone, Default)]
pub struct Net {
    #[serde(default)]
    pub node: String,
    #[serde(default)]
    pub tailscale: Tailscale,
    /// Cognitum mesh overlay peers (count + peers), kept raw.
    #[serde(default)]
    pub cognitum_mesh: serde_json::Value,
    /// Edge nodes that checked in (COG-010 Fleet).
    #[serde(default)]
    pub fleet: Vec<FleetNode>,
}

/// A running cog's `/guide` bundle fetch (the ADR-104 guide the cog serves on its export port).
#[derive(Default)]
pub struct GuideFetch {
    pub id: String,
    pub port: u16,
    /// None while in flight; Some once the request resolves.
    pub result: Option<Result<serde_json::Value, String>>,
}

#[derive(Default)]
pub struct Shared {
    /// Bumped on every reconnect. A fetch remembers the epoch it started in and drops its answer if
    /// the console has since switched host, so a slow reply from the old node never lands in the
    /// new node's state.
    pub epoch: u64,
    /// Identify-hardware modal: last scan (None = not scanned yet / in flight), when it landed,
    /// and per-device agent answers. On demand only; never polled.
    pub hw_usb: Option<Result<HwUsbReport, String>>,
    pub hw_usb_at: Option<Instant>,
    pub hw_identify: std::collections::BTreeMap<String, Identify>,
    /// Hardware Dex (Catalog > Dex): fetched on demand when the tab opens, and after a catch.
    pub hw_dex: Option<Result<HwDexReport, String>>,
    pub host: Option<Result<HostStatus, String>>,
    pub net: Option<Result<Net, String>>,
    pub our_reg: Option<Result<WlRegistry, String>>,
    pub cognitum_reg: Option<Result<CognitumRegistry, String>>,
    pub catalog: Option<Catalog>,
    pub last_action: Option<String>,
    /// The guide currently being viewed in the Sensors tab (one at a time).
    pub guide: Option<GuideFetch>,
    /// Latest `/status` output line of each cog whose detail panel is open, by cog id.
    pub cog_out: std::collections::BTreeMap<String, CogOutFetch>,
    pub mesh: Option<MeshFetch>,
    /// Pre-install facts of the connected host (`/hw/buses`); `Err` carries a human reason.
    pub node_facts: Option<Result<NodeFacts, String>>,
    pub node_facts_at: Option<Instant>,
    /// The daemon's `fleet.snapshot` through the gateway (None = not fetched / not configured).
    pub fleet: Option<Result<serde_json::Value, String>>,
    /// RTT / load readings per node from successive snapshots (sparklines).
    pub fleet_hist: crate::fleet::History,
    /// What each Seed agent reported, by base URL.
    pub seeds: std::collections::BTreeMap<String, crate::fleet_unify::SeedView>,
    /// Session bearer just issued by the connected host. The frame loop copies it onto the token.
    pub mesh_token: String,
    /// Why a mesh key is not on this connection yet. Empty once one is held.
    pub mesh_note: String,
    /// The host rejected the bearer we sent. The next frame registers again.
    pub mesh_rejected: bool,
    /// Drop the per-host token the console was holding for this connection.
    pub mesh_clear: bool,
    /// `GET /mesh/enroll` is missing on this host. Stop asking it.
    pub mesh_unsupported: bool,
}

/// One cog's export `/status` fetch (the cog's own endpoint, not a host endpoint).
#[derive(Clone)]
pub struct CogOutFetch {
    pub fired: Instant,
    pub in_flight: bool,
    pub result: Option<Result<serde_json::Value, String>>,
}
