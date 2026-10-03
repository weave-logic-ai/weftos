//! This daemon's cog ingest bridge and store owner (mesh-placement-10;
//! ADR-100 "Decision 5 resolved"; `docs/cogs/ingest-bridge.md`).
//!
//! A placed cog's vectors enter through the bridge on this node and are
//! forwarded to the node that owns the placing project's store. All knobs
//! are optional; `<runtime>/cog-ingest.json` overrides the defaults:
//!
//! ```json
//! {
//!   "bridge": { "bind": "127.0.0.1:80", "requests_per_sec": 20,
//!               "vectors_per_sec": 2048, "container_bind": "192.168.64.1" },
//!   "routes": [
//!     { "project": "<project id>", "owner": "local",
//!       "controllers": ["<node id>"] },
//!     { "project": "<project id>",
//!       "owner": { "node": "<node id>", "key": "<64 hex>", "addr": "host:9472", "noise": true } },
//!     { "controller": "<node id>", "owner": "local" }
//!   ],
//!   "store_owner": { "listen": "0.0.0.0:9472", "noise": true,
//!                    "forwarders": [ { "key": "<64 hex>", "projects": ["<id>"] },
//!                                    { "key": "<64 hex>", "projects": "*" } ],
//!                    "projects": ["<id>"], "fallback": false }
//! }
//! ```
//!
//! Defaults: the bridge listens on loopback `127.0.0.1:80` (the address
//! released cogs post to); no container listeners; one route sending
//! project-less placements made by this node's own key to this node's
//! store; no store-owner service.
//!
//! Who may place for a project: this node's own key, the node ids in the
//! project route's `controllers`, and the node of the project's bound key
//! (from the identity records, on the user daemon). A project with no route,
//! or a controller with no route, is refused at place time, never placed.
//!
//! If the bridge cannot bind (the port is taken, or unprivileged on Linux)
//! cogs are still placed, with no token and no URL, and every place result,
//! status and advertisement says `ingest: disabled`. The user daemon and
//! each project daemon would all default to `127.0.0.1:80`, and only the
//! first to bind wins: give the others a distinct `bridge.bind` port in
//! their own runtime dir's `cog-ingest.json` (cogs that honour
//! `COGNITUM_INGEST_URL` follow it; a released cog that posts to the fixed
//! port 80 can be served by one daemon per host).

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::Arc;

use clawft_kernel::cog_ingest::{
    BridgeConfig, BridgeHandle, BridgeScope, Forwarder, IngestBridge, IngestHooks, KeyPolicy,
    LocalForwarder, MeshForwarder, OwnerConnector, RateBudget, StaticRouter, StoreOwnerService,
    ProjectDirectory, TokenRegistry, VectorDirectory, owner, valid_project_id,
};
use clawft_kernel::workload_ctl::listen_tcp;
use clawft_kernel::workload_pkg::codec::hex_decode_exact;
use ed25519_dalek::SigningKey;
use serde::Deserialize;

/// Config file under the runtime dir.
pub const INGEST_FILE: &str = "cog-ingest.json";
const MAX_FILE: u64 = 64 * 1024;
const MAX_ROUTES: usize = 256;

/// Whole file.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IngestConfig {
    /// Bridge listener and budgets.
    #[serde(default)]
    pub bridge: BridgeSection,
    /// Where each placement's vectors go. Empty: the default route.
    #[serde(default)]
    pub routes: Vec<Route>,
    /// Serve the `cog-store` service to other nodes' bridges.
    #[serde(default)]
    pub store_owner: Option<OwnerSection>,
}

/// Bridge settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeSection {
    /// Shared listener (loopback only).
    #[serde(default = "default_bind")]
    pub bind: String,
    /// Requests per second per instance.
    #[serde(default = "default_requests")]
    pub requests_per_sec: u32,
    /// Vectors per second per instance.
    #[serde(default = "default_vectors")]
    pub vectors_per_sec: u32,
    /// Address to bind token-scoped listeners on for container relays (the
    /// engine or VM gateway). Absent: containers get no scoped listener.
    #[serde(default)]
    pub container_bind: Option<String>,
}

fn default_bind() -> String {
    "127.0.0.1:80".into()
}
fn default_requests() -> u32 {
    20
}
fn default_vectors() -> u32 {
    2048
}

impl Default for BridgeSection {
    fn default() -> Self {
        Self {
            bind: default_bind(),
            requests_per_sec: default_requests(),
            vectors_per_sec: default_vectors(),
            container_bind: None,
        }
    }
}

/// One routing rule: `project` (that project's batches) or `controller`
/// (project-less batches placed by that node), never both.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Route {
    /// Project id.
    #[serde(default)]
    pub project: Option<String>,
    /// Placing controller node id.
    #[serde(default)]
    pub controller: Option<String>,
    /// Who owns the store.
    pub owner: Owner,
    /// Node ids besides this node allowed to place cogs for the project
    /// (project routes only; default: this node alone).
    #[serde(default)]
    pub controllers: Vec<String>,
}

/// A store owner: this node, or another.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Owner {
    /// `"local"`.
    Local(LocalMarker),
    /// A remote node's `cog-store`.
    Remote(RemoteOwner),
}

/// The literal string `local`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LocalMarker {
    /// This node.
    Local,
}

/// A remote owner, pinned by key.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteOwner {
    /// Its node id.
    pub node: String,
    /// Its Ed25519 key (64 hex); its answers must verify against it.
    pub key: String,
    /// `host:port` of its `cog-store` listener.
    pub addr: String,
    /// Noise XX over TCP (default on).
    #[serde(default = "yes")]
    pub noise: bool,
}

fn yes() -> bool {
    true
}

/// Serve the `cog-store` service.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerSection {
    /// Bind address; never the mesh port or the workload-host port.
    pub listen: String,
    /// Noise XX (default on).
    #[serde(default = "yes")]
    pub noise: bool,
    /// Bridge node keys allowed to forward here.
    pub forwarders: Vec<ForwarderKey>,
    /// Projects whose stores this node owns.
    #[serde(default)]
    pub projects: Vec<String>,
    /// Also own the project-less (controller) store.
    #[serde(default)]
    pub fallback: bool,
}

/// One forwarder key and the projects it may forward for (all when absent).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForwarderKey {
    /// Ed25519 key, 64 hex.
    pub key: String,
    /// The projects it may forward for, or `"*"` for any (and for
    /// project-less batches). Required: there is no default.
    pub projects: ForwarderProjects,
}

/// A forwarder's scope.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ForwarderProjects {
    /// `"*"`.
    Any(AnyMarker),
    /// Listed project ids.
    List(Vec<String>),
}

/// The literal string `*`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub enum AnyMarker {
    /// Any project.
    #[serde(rename = "*")]
    Any,
}

impl IngestConfig {
    /// Boundary validation.
    pub fn validate(&self) -> Result<(), String> {
        let bad = |m: String| Err(format!("{INGEST_FILE}: {m}"));
        let b = &self.bridge;
        let addr: SocketAddr = b
            .bind
            .parse()
            .map_err(|e| format!("{INGEST_FILE}: bridge.bind {:?}: {e}", b.bind))?;
        if !addr.ip().is_loopback() {
            return bad(format!("bridge.bind {addr} must be loopback"));
        }
        if b.requests_per_sec == 0 || b.vectors_per_sec == 0 {
            return bad("bridge budgets must be above zero".into());
        }
        if let Some(c) = &b.container_bind {
            let ip = c
                .parse::<IpAddr>()
                .map_err(|e| format!("{INGEST_FILE}: bridge.container_bind {c:?}: {e}"))?;
            if ip.is_unspecified() || ip.is_multicast() {
                return bad(format!(
                    "bridge.container_bind {ip} must be one gateway address, not unspecified or multicast"
                ));
            }
        }
        if self.routes.len() > MAX_ROUTES {
            return bad(format!("at most {MAX_ROUTES} routes"));
        }
        for r in &self.routes {
            if r.project.is_none() && !r.controllers.is_empty() {
                return bad("`controllers` belongs on a `project` route".into());
            }
            if r.controllers.iter().any(|c| c.is_empty() || c.len() > 128) {
                return bad("controllers must be node ids".into());
            }
            match (&r.project, &r.controller) {
                (Some(p), None) if valid_project_id(p) => {}
                (Some(p), None) => return bad(format!("route project {p:?} is not a project id")),
                (None, Some(c)) if !c.is_empty() && c.len() <= 128 => {}
                _ => return bad("a route names exactly one of `project` or `controller`".into()),
            }
            if let Owner::Remote(o) = &r.owner {
                if hex_decode_exact::<32>(&o.key).is_none() {
                    return bad(format!("owner key for {} is not 64 hex", o.node));
                }
                if o.node.is_empty() || o.addr.is_empty() || o.addr.len() > 253 {
                    return bad("owner needs node and addr".into());
                }
            }
        }
        if let Some(s) = &self.store_owner {
            s.listen
                .parse::<SocketAddr>()
                .map_err(|e| format!("{INGEST_FILE}: store_owner.listen {:?}: {e}", s.listen))?;
            if s.forwarders.is_empty() {
                return bad("store_owner.forwarders must list at least one key".into());
            }
            for f in &s.forwarders {
                if hex_decode_exact::<32>(&f.key).is_none() {
                    return bad(format!("forwarder key {:?} is not 64 hex", f.key));
                }
                if let ForwarderProjects::List(ps) = &f.projects
                    && ps.iter().any(|p| !valid_project_id(p))
                {
                    return bad("forwarder projects must be project ids".into());
                }
            }
            if s.projects.iter().any(|p| !valid_project_id(p)) {
                return bad("store_owner.projects must be project ids".into());
            }
        }
        Ok(())
    }
}

/// The config, or the defaults when the file is absent.
pub fn load_config(dir: &Path) -> Result<IngestConfig, String> {
    let path = dir.join(INGEST_FILE);
    match std::fs::metadata(&path) {
        Err(_) => Ok(IngestConfig::default()),
        Ok(m) if m.len() > MAX_FILE => Err(format!("{INGEST_FILE} is too large")),
        Ok(_) => {
            let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
            let cfg: IngestConfig =
                serde_json::from_str(&text).map_err(|e| format!("{INGEST_FILE}: {e}"))?;
            cfg.validate()?;
            Ok(cfg)
        }
    }
}

/// The identity records of the user daemon: a project's bound key is the
/// node that may place for it. Unavailable (`None`) on a daemon with no
/// project supervisor.
pub struct IdentityDirectory;

impl IdentityDirectory {
    /// True when the identity records can be read here.
    pub fn available() -> bool {
        crate::project_supervisor::global().is_some()
    }
}

impl ProjectDirectory for IdentityDirectory {
    fn bound_node(&self, project_id: &str) -> Option<String> {
        let view = crate::project_supervisor::global()?.identity_view()?;
        let cert = view.current_cert(project_id)?;
        let pk = hex_decode_exact::<32>(&cert.project_pubkey)?;
        Some(clawft_kernel::node_id_from_pubkey(&pk))
    }
}

/// Refuse a project the identity records do not know or have revoked.
/// `dir` is `None` where there are no records (a project daemon): the
/// host's own policy decides there.
pub fn check_project_registered(dir: Option<&dyn ProjectDirectory>, project_id: &str) -> Result<(), String> {
    if !valid_project_id(project_id) {
        return Err(format!("project {project_id:?} is not a valid project id"));
    }
    match dir {
        Some(d) if d.bound_node(project_id).is_none() => Err(format!(
            "project {project_id} is not registered, or its key is revoked"
        )),
        _ => Ok(()),
    }
}

/// [`check_project_registered`] against this daemon's identity records.
pub fn check_project(project_id: &str) -> Result<(), String> {
    let dir = IdentityDirectory;
    check_project_registered(
        IdentityDirectory::available().then_some(&dir as &dyn ProjectDirectory),
        project_id,
    )
}

/// What keeps the bridge and the owner service alive. Dropping it stops
/// both.
pub struct IngestRuntime {
    /// Wiring handed to the workload host (disabled if the bridge could not
    /// bind).
    pub hooks: IngestHooks,
    /// Address the shared bridge is bound to, if it is.
    pub bridge_addr: Option<SocketAddr>,
    /// Why the bridge is not running.
    pub bridge_error: Option<String>,
    /// Address the store-owner service is bound to, if serving.
    pub owner_addr: Option<SocketAddr>,
    _listener: Option<BridgeHandle>,
    owner_task: Option<tokio::task::JoinHandle<()>>,
}

impl Drop for IngestRuntime {
    fn drop(&mut self) {
        if let Some(t) = &self.owner_task {
            t.abort();
        }
    }
}

/// Start the bridge and, if configured, the owner service for the node
/// owning `key`. A bridge that cannot bind does not fail the daemon: the
/// runtime comes back with disabled hooks and `bridge_error` (the owner
/// service, if any, keeps running and is reported).
pub async fn start(
    cfg: &IngestConfig,
    key: &SigningKey,
    projects_dir: Option<Arc<dyn ProjectDirectory>>,
) -> Result<IngestRuntime, String> {
    cfg.validate()?;
    let node_id = clawft_kernel::node_id_from_pubkey(&key.verifying_key().to_bytes());
    let routes: Vec<Route> = if cfg.routes.is_empty() {
        vec![Route {
            project: None,
            controller: Some(node_id.clone()),
            owner: Owner::Local(LocalMarker::Local),
            controllers: vec![],
        }]
    } else {
        cfg.routes.clone()
    };
    // What local cogs may write here, and what remote bridges may write
    // here, are separate allow-lists over one set of stores.
    let (mut local_projects, mut local_fallback) = (HashSet::<String>::new(), false);
    for r in &routes {
        if let Owner::Local(_) = r.owner {
            match &r.project {
                Some(p) => {
                    local_projects.insert(p.clone());
                }
                None => local_fallback = true,
            }
        }
    }
    let stores = VectorDirectory::new(std::iter::empty(), false);
    let local_dir = Arc::new(stores.view(local_projects, local_fallback));

    let mut router = StaticRouter::new();
    let mut controllers: HashMap<String, HashSet<String>> = HashMap::new();
    let connector = Arc::new(OwnerConnector::new(true));
    for r in &routes {
        let fwd: Arc<dyn Forwarder> = match &r.owner {
            Owner::Local(_) => Arc::new(LocalForwarder::new(node_id.clone(), local_dir.clone())),
            Owner::Remote(o) => {
                let owner_key = hex_decode_exact::<32>(&o.key).ok_or("owner key")?;
                let c: Arc<dyn clawft_kernel::workload_ctl::CtlConnector> = if o.noise {
                    connector.clone()
                } else {
                    tracing::warn!(owner = %o.node, "cog-store link without Noise: batches travel in clear (signed, not encrypted)");
                    Arc::new(OwnerConnector::new(false))
                };
                Arc::new(MeshForwarder::new(key.clone(), &o.node, owner_key, &o.addr, c))
            }
        };
        router = match (&r.project, &r.controller) {
            (Some(p), _) => {
                controllers
                    .entry(p.clone())
                    .or_default()
                    .extend(r.controllers.iter().cloned());
                router.with_project_route(p, fwd)
            }
            (_, Some(c)) => router.with_controller(c, fwd),
            _ => router,
        };
    }

    let (mut owner_addr, mut owner_task) = (None, None);
    if let Some(s) = &cfg.store_owner {
        let mut policy = KeyPolicy::new();
        for f in &s.forwarders {
            let k = hex_decode_exact::<32>(&f.key).ok_or("forwarder key")?;
            policy = match &f.projects {
                ForwarderProjects::Any(_) => policy.allow_any(k),
                ForwarderProjects::List(ps) => {
                    let ps: Vec<&str> = ps.iter().map(String::as_str).collect();
                    policy.allow_projects(k, &ps)
                }
            };
        }
        if !s.noise {
            tracing::warn!("cog-store listener without Noise: batches travel in clear (signed, not encrypted)");
        }
        // Only what `store_owner` lists: not the local routes' stores.
        let owner_dir = Arc::new(stores.view(s.projects.iter().cloned(), s.fallback));
        let svc = Arc::new(StoreOwnerService::new(key.clone(), Arc::new(policy), owner_dir));
        let listener = listen_tcp(&s.listen)
            .await
            .map_err(|e| format!("cog-store listen {}: {e}", s.listen))?;
        owner_addr = Some(listener.local_addr().map_err(|e| e.to_string())?);
        let noise = s.noise;
        owner_task = Some(tokio::spawn(async move {
            if let Err(e) = owner::serve_listener(listener, svc, noise).await {
                tracing::warn!(error = %e, "cog-store listener stopped");
            }
        }));
    }

    let registry = Arc::new(TokenRegistry::new());
    let container_bind = cfg
        .bridge
        .container_bind
        .as_deref()
        .map(|c| c.parse::<IpAddr>())
        .transpose()
        .map_err(|e| e.to_string())?;
    let bridge = IngestBridge::new(
        registry.clone(),
        Arc::new(router),
        RateBudget::new(
            std::time::Duration::from_secs(1),
            cfg.bridge.requests_per_sec,
            cfg.bridge.vectors_per_sec,
        ),
        BridgeConfig {
            allow_non_loopback: container_bind.is_some_and(|ip| !ip.is_loopback()),
            ..Default::default()
        },
    );
    let bind: SocketAddr = cfg.bridge.bind.parse().map_err(|e| format!("{e}"))?;
    let (listener, bridge_error) = match bridge.bind(bind, BridgeScope::Any).await {
        Ok(l) => (Some(l), None),
        Err(e) => {
            let why = format!("cannot bind {bind}: {e}");
            tracing::warn!(error = %why, owner = ?owner_addr,
                "cog ingest bridge disabled; cogs are placed without a token (ingest: disabled)");
            (None, Some(why))
        }
    };
    let bridge_addr = listener.as_ref().map(BridgeHandle::addr);
    let mut hooks = match bridge_addr {
        Some(addr) => IngestHooks::new(node_id.clone(), registry, bridge, addr, container_bind),
        None => IngestHooks::disabled(node_id, bridge_error.clone().unwrap_or_default()),
    }
    .with_project_controllers(controllers);
    if let Some(d) = projects_dir {
        hooks = hooks.with_project_directory(d);
    }
    Ok(IngestRuntime {
        hooks,
        bridge_addr,
        bridge_error,
        owner_addr,
        _listener: listener,
        owner_task,
    })
}

#[cfg(test)]
#[path = "cog_ingest_serve_tests.rs"]
mod tests;
