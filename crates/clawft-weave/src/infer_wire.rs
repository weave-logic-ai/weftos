//! The daemon's inference placement wiring (card mesh-placement-19; ADR-101
//! section 5): the placement table, the loopback proxy per role, the
//! adapters that observe the model servers, the mesh hub, and the hook that
//! lets local agents use a placed role.
//!
//! Off unless the operator writes `<runtime>/inference.json`:
//!
//! ```json
//! { "roles": [ { "role": "hermes", "flavor": "llamacpp",
//!                "instance_port": 18090, "proxy_port": 8090,
//!                "on_occupied": "refuse", "provider": "local" } ],
//!   "mesh": { "expose": ["hermes"],
//!             "serve_peers":  { "hermes": ["<node id>"] },
//!             "remote_nodes": { "hermes": ["<node id>"] } } }
//! ```
//!
//! Everything defaults to off and deny: no `mesh` block means nothing is
//! exposed and no remote node is used; `serve_peers` and `remote_nodes` are
//! the allowlists (audited on the chain, also changed at run time by the
//! Admin verbs `infer.expose` and `infer.allow`). Roles are `Adopted`:
//! the adapter registers and health-checks a server somebody else runs and
//! never starts, stops or signals it. A role with `proxy_port` gets a
//! loopback proxy on that port; a port something else already holds is
//! refused, or adopted (left to the existing server) with `on_occupied`.
//!
//! Service mode (ADR-103): the daemon does not own the mesh there and the
//! service hands it deliveries as an unverified peer, so it holds no
//! admission grant for anyone. Local placement works; serving and using
//! remote instances do not, and the status says so.

use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use clawft_kernel::infer_proxy::{
    InferHub, InferProxy, OccupiedPolicy, PlacementTable, ProxyAudit, ProxyLimits, ServeGate,
    Started, Upstream,
};
use clawft_kernel::mesh_runtime::MeshRuntime;
use clawft_kernel::workload_pkg::manifest::valid_token;
use clawft_kernel::workload_runtime::infer::{InferConfig, InferFlavor, InferRuntime, InferenceSpec};
use clawft_kernel::workload_runtime::types::{
    InstanceHandle, RunMode, VerifiedWorkload, WorkloadConfig, WorkloadRuntime,
};
use clawft_kernel::workload_runtime::HostContract;
use serde::Deserialize;
use tokio::sync::Mutex;
use tracing::{info, warn};

/// Config file under the runtime dir.
pub const CONFIG_FILE: &str = "inference.json";
const MAX_FILE: u64 = 64 * 1024;
const MAX_ROLES: usize = 16;
const DEFAULT_SYNC: u64 = 5;
const DEFAULT_ADVERT: u64 = 20;
const MAX_LIST: usize = 64;

/// One served role.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleCfg {
    /// Stable role name.
    pub role: String,
    /// `llamacpp`, `mlx-lm` or `ollama`.
    pub flavor: String,
    /// Port of the server to observe (always on `127.0.0.1`).
    pub instance_port: u16,
    /// Stable loopback port the proxy keeps for consumers.
    #[serde(default)]
    pub proxy_port: Option<u16>,
    /// `refuse` (default) or `adopt` when `proxy_port` is already held.
    #[serde(default)]
    pub on_occupied: Option<String>,
    /// Provider name that follows this role (`local`).
    #[serde(default)]
    pub provider: Option<String>,
}

/// Mesh settings; empty means nothing exposed, nothing allowed.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeshCfg {
    /// Roles this node exposes to the mesh.
    #[serde(default)]
    pub expose: Vec<String>,
    /// Per role, the peers this node serves.
    #[serde(default)]
    pub serve_peers: BTreeMap<String, Vec<String>>,
    /// Per role, the nodes this node may use.
    #[serde(default)]
    pub remote_nodes: BTreeMap<String, Vec<String>>,
}

/// The file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileCfg {
    /// Roles.
    pub roles: Vec<RoleCfg>,
    /// Mesh settings.
    #[serde(default)]
    pub mesh: MeshCfg,
    /// Seconds between local re-checks (default 5).
    #[serde(default)]
    pub sync_secs: Option<u64>,
    /// Seconds between mesh announcements (default 20).
    #[serde(default)]
    pub advert_secs: Option<u64>,
}

pub(crate) fn flavor_of(s: &str) -> Option<InferFlavor> {
    match s {
        "llamacpp" => Some(InferFlavor::LlamaCpp),
        "mlx-lm" => Some(InferFlavor::MlxLm),
        "ollama" => Some(InferFlavor::Ollama),
        _ => None,
    }
}

impl FileCfg {
    /// Boundary validation.
    pub fn validate(&self) -> Result<(), String> {
        let bad = |m: String| Err(format!("{CONFIG_FILE}: {m}"));
        if self.roles.is_empty() || self.roles.len() > MAX_ROLES {
            return bad(format!("roles must list 1..={MAX_ROLES} entries"));
        }
        let mut seen = std::collections::HashSet::new();
        for r in &self.roles {
            if !valid_token(&r.role, 64) || r.role.contains('/') || !seen.insert(r.role.clone()) {
                return bad(format!("role {:?} is invalid or repeated", r.role));
            }
            if flavor_of(&r.flavor).is_none() {
                return bad(format!("flavor {:?}: use llamacpp, mlx-lm or ollama", r.flavor));
            }
            if r.instance_port == 0 || r.proxy_port == Some(0) || r.proxy_port == Some(r.instance_port) {
                return bad(format!("role {}: ports must be nonzero and different", r.role));
            }
            if !matches!(r.on_occupied.as_deref(), None | Some("refuse") | Some("adopt")) {
                return bad(format!("role {}: on_occupied is refuse or adopt", r.role));
            }
            if r.provider.as_deref().is_some_and(|p| !valid_token(p, 32)) {
                return bad(format!("role {}: bad provider", r.role));
            }
        }
        // An advert must be repeated well inside the receivers' TTL.
        let max_advert = clawft_kernel::infer_proxy::DEFAULT_ADVERT_TTL.as_secs() / 2;
        if self.advert_secs.is_some_and(|a| a == 0 || a > max_advert) {
            return bad(format!("advert_secs must be 1..={max_advert} (half the advert TTL)"));
        }
        if self.sync_secs.is_some_and(|s| s == 0 || s > 60) {
            return bad("sync_secs must be 1..=60".into());
        }
        // The announcement runs on the sync tick, so its real period is
        // the larger of the two.
        let (sync, advert) = (self.sync_secs.unwrap_or(DEFAULT_SYNC), self.advert_secs.unwrap_or(DEFAULT_ADVERT));
        if sync > advert {
            return bad(format!("sync_secs ({sync}) must not exceed advert_secs ({advert})"));
        }
        // The announcement fires on the first tick at or after advert_secs,
        // so its real period is ceil(advert / sync) * sync, and that must
        // still sit inside the half-TTL bound.
        let real = advert.div_ceil(sync) * sync;
        if real > max_advert {
            return bad(format!(
                "advert_secs ({advert}) on a {sync} s sync tick announces every {real} s; keep it at most {max_advert} s"
            ));
        }
        let known = |role: &String| self.roles.iter().any(|r| &r.role == role);
        let nodes_ok = |v: &Vec<String>| v.len() <= MAX_LIST && v.iter().all(|n| valid_token(n, 128));
        if self.mesh.expose.iter().any(|r| !known(r))
            || self.mesh.serve_peers.keys().chain(self.mesh.remote_nodes.keys()).any(|r| !known(r))
        {
            return bad("mesh settings name a role that is not in roles".into());
        }
        if !self.mesh.serve_peers.values().all(nodes_ok) || !self.mesh.remote_nodes.values().all(nodes_ok) {
            return bad("bad node id in the mesh allowlists".into());
        }
        Ok(())
    }
}

/// Read and validate the config; `None` when the operator did not enable it.
pub fn load_config(dir: &Path) -> Result<Option<FileCfg>, String> {
    let path = dir.join(CONFIG_FILE);
    match std::fs::metadata(&path) {
        Err(_) => Ok(None),
        Ok(m) if m.len() > MAX_FILE => Err(format!("{CONFIG_FILE} is too large")),
        Ok(_) => {
            let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
            let cfg: FileCfg = serde_json::from_str(&text).map_err(|e| format!("{CONFIG_FILE}: {e}"))?;
            cfg.validate()?;
            Ok(Some(cfg))
        }
    }
}

/// What became of a role's proxy.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyState {
    /// No `proxy_port`.
    Off,
    /// Listening here.
    Listening(SocketAddr),
    /// The port is held by another server, which stays the address.
    Adopted(SocketAddr),
    /// The port is held and the config said refuse (or the bind failed).
    Refused(String),
}

pub(crate) struct RoleState {
    pub cfg: RoleCfg,
    rt: InferRuntime,
    handle: Mutex<Option<InstanceHandle>>,
    pub proxy: std::sync::Mutex<ProxyState>,
    _proxy: std::sync::Mutex<Option<InferProxy>>,
}

/// Everything the daemon holds for inference placement.
pub struct InferState {
    pub(crate) node_id: String,
    pub(crate) table: Arc<PlacementTable>,
    pub(crate) hub: Option<Arc<InferHub>>,
    pub(crate) roles: Vec<RoleState>,
    pub(crate) mesh_note: String,
}

static STATE: OnceLock<Arc<InferState>> = OnceLock::new();

/// The running state, when inference placement is on.
pub fn state() -> Option<&'static Arc<InferState>> {
    STATE.get()
}

fn workload(r: &RoleCfg) -> Result<VerifiedWorkload, String> {
    let flavor = flavor_of(&r.flavor).ok_or("flavor")?;
    let mut spec = InferenceSpec::new(&r.role, flavor);
    spec.serve.port = Some(r.instance_port);
    VerifiedWorkload::inference(spec).map_err(|e| e.to_string())
}

fn wl_cfg(node_id: &str) -> WorkloadConfig {
    WorkloadConfig {
        mode: RunMode::Listener,
        args: vec![],
        host: HostContract::default_feed(),
        node_id: node_id.into(),
    }
}

impl InferState {
    /// Register the observed server if that has not worked yet.
    async fn ensure_loaded(&self, r: &RoleState) {
        let mut g = r.handle.lock().await;
        if g.is_some() {
            return;
        }
        let Ok(w) = workload(&r.cfg) else { return };
        // `admit` is a read-only probe of the loopback port; a server that
        // is not up yet is retried on the next tick.
        if r.rt.admit(&w).await.is_err() {
            return;
        }
        if let Ok(h) = r.rt.load(&w, &wl_cfg(&self.node_id)).await
            && r.rt.start(&h).await.is_ok()
        {
            *g = Some(h);
        }
    }

    /// One local pass: (re)register each observed server and update the table.
    pub async fn sync_once(&self) {
        for r in &self.roles {
            self.ensure_loaded(r).await;
            let h = r.handle.lock().await.clone();
            match h {
                Some(h) => {
                    self.table.sync_local(&r.cfg.role, &r.rt, &h).await;
                }
                None => self.table.deregister_local(&r.cfg.role),
            }
        }
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Where the daemon's pieces come from, for [`init`].
pub struct InitParts<'a> {
    /// Runtime dir holding `inference.json`.
    pub dir: &'a Path,
    /// This node's id.
    pub node_id: String,
    /// The mesh runtime this daemon owns (None in service mode or without a mesh).
    pub mesh: Option<Arc<MeshRuntime>>,
    /// True when the machine mesh service owns the mesh (ADR-103).
    pub service_mode: bool,
    /// Where binds, refusals and allowlist changes are recorded.
    pub audit: Option<Arc<dyn ProxyAudit>>,
    /// Proxy and forwarding limits.
    pub limits: ProxyLimits,
}

/// Build the placement state from `<runtime>/inference.json` and run one
/// local pass, without installing anything process-wide. `Ok(None)` when
/// the file is absent (the default: off).
pub async fn build(p: InitParts<'_>) -> Result<Option<(Arc<InferState>, FileCfg)>, String> {
    let Some(cfg) = load_config(p.dir)? else { return Ok(None) };
    let upstream = Arc::new(Upstream::new(p.limits.clone()).map_err(|e| e.to_string())?);
    let (hub, mesh_note) = match (&p.mesh, p.service_mode) {
        (Some(rt), false) => {
            let hub = InferHub::new(rt.clone(), upstream.clone(), Arc::new(ServeGate::default()), p.audit.clone());
            (Some(hub), "hub: adverts and requests travel as control messages".to_string())
        }
        (_, true) => (
            None,
            "unavailable in service mode: the mesh service hands this daemon deliveries as an \
             unverified peer, so it holds no admission grant to serve or trust a remote node on"
                .to_string(),
        ),
        (None, false) => (None, "unavailable: this daemon has no mesh runtime".to_string()),
    };
    let dialer = hub.clone().map(|h| h as Arc<dyn clawft_kernel::infer_proxy::MeshDialer>);
    let table = Arc::new(PlacementTable::new(p.node_id.clone(), dialer, p.audit.clone()));
    if let Some(h) = &hub {
        h.attach(table.clone());
        h.install();
        for role in &cfg.mesh.expose {
            table.expose_to_mesh(role, true);
        }
        for (role, peers) in &cfg.mesh.serve_peers {
            peers.iter().for_each(|n| table.allow_mesh_peer(role, n, true));
        }
        for (role, nodes) in &cfg.mesh.remote_nodes {
            nodes.iter().for_each(|n| table.allow_remote_node(role, n, true));
        }
    } else if !cfg.mesh.expose.is_empty() || !cfg.mesh.serve_peers.is_empty() || !cfg.mesh.remote_nodes.is_empty() {
        warn!(reason = %mesh_note, "{CONFIG_FILE}: mesh settings ignored");
    }

    let mut roles = Vec::new();
    for r in &cfg.roles {
        let flavor = flavor_of(&r.flavor).ok_or("flavor")?;
        let proxy = match r.proxy_port {
            None => (ProxyState::Off, None),
            Some(port) => {
                let policy = if r.on_occupied.as_deref() == Some("adopt") {
                    OccupiedPolicy::Adopt
                } else {
                    OccupiedPolicy::Refuse
                };
                let addr: SocketAddr = ([127, 0, 0, 1], port).into();
                match InferProxy::start(&r.role, addr, policy, table.clone(), p.limits.clone(), p.audit.clone()).await {
                    Ok(Started::Running(px)) => (ProxyState::Listening(px.addr()), Some(px)),
                    Ok(Started::Adopted(a)) => (ProxyState::Adopted(a), None),
                    Err(e) => {
                        warn!(role = %r.role, error = %e, "inference proxy not started");
                        (ProxyState::Refused(e.to_string()), None)
                    }
                }
            }
        };
        roles.push(RoleState {
            cfg: r.clone(),
            rt: InferRuntime::new(InferConfig::adopted(flavor)),
            handle: Mutex::new(None),
            proxy: std::sync::Mutex::new(proxy.0),
            _proxy: std::sync::Mutex::new(proxy.1),
        });
    }
    let state = Arc::new(InferState { node_id: p.node_id, table: table.clone(), hub: hub.clone(), roles, mesh_note });
    state.sync_once().await;
    Ok(Some((state, cfg)))
}

/// [`build`], then make it the daemon's: the global state the `infer.*`
/// verbs read, the hook local agents follow, cache invalidation, and the
/// periodic sync and announcement loop.
pub async fn init(p: InitParts<'_>) -> Result<Option<Arc<InferState>>, String> {
    let Some((state, cfg)) = build(p).await? else { return Ok(None) };
    let table = state.table.clone();

    // Local agents: the `local` provider follows its role unless an explicit
    // setting chose the endpoint (checked where the adapter is built).
    let provider_roles: HashMap<String, String> = cfg
        .roles
        .iter()
        .filter_map(|r| r.provider.clone().map(|p| (p, r.role.clone())))
        .collect();
    if !provider_roles.is_empty() {
        let t = table.clone();
        clawft_core::placement_hook::install(
            Arc::new(move |role: &str| t.base_url_for_role(role)),
            Duration::from_secs(5),
            provider_roles,
        );
    }
    // Cached router answers follow every table change.
    let mut rx = table.subscribe();
    tokio::spawn(async move {
        while rx.changed().await.is_ok() {
            clawft_core::placement_hook::invalidate_all();
        }
    });
    let (st, sync, advert) = (state.clone(), cfg.sync_secs.unwrap_or(DEFAULT_SYNC).max(1), cfg.advert_secs.unwrap_or(DEFAULT_ADVERT).max(1));
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(sync));
        let mut since_advert = 0u64;
        loop {
            tick.tick().await;
            st.sync_once().await;
            since_advert += sync;
            if since_advert >= advert {
                since_advert = 0;
                if let Some(h) = &st.hub {
                    h.announce(now_secs()).await;
                }
            }
        }
    });
    info!(roles = cfg.roles.len(), mesh = %state.mesh_note, "inference placement on");
    let _ = STATE.set(state.clone());
    Ok(Some(state))
}
