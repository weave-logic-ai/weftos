//! Daemon side of the placement control plane (ADR-099 sections 3 and 7,
//! card mesh-placement-12): `workload.place`, `workload.explain`,
//! `workload.status`, `workload.stop`, `workload.logs` and
//! `workload.unload {instance_id}`.
//!
//! The daemon is the controller. It signs with its node key
//! (`<runtime>/node.key`), chains every decision on the kernel chain, and
//! reads operator policy from the runtime directory (see
//! `workload_place_policy`: permits, trust, peers bound to node keys, the
//! optional container adapter and Seeds). Known targets and placements are
//! persisted in `workload-placements.json`, so instances stay manageable
//! across a daemon restart.
//!
//! This node is a candidate too, through its own `workload-host` (native
//! adapter, the node's re-probed signed facts), in process.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use clawft_kernel::artifact_store::ArtifactStore;
use clawft_kernel::boot::Kernel;
use clawft_kernel::chain::ChainManager;
use clawft_kernel::mesh_artifact::{ArtifactExchange, ExchangeConfig};
use clawft_kernel::workload_ctl::msg::method;
use clawft_kernel::workload_ctl::{
    CtlConfig, FactsSource, MeshConnector, PlaceOrder, PlacementControlPlane, StorePinOrder,
    WorkloadHostService,
};
use clawft_kernel::workload_governance::WorkloadGate;
use clawft_kernel::workload_runtime::RunMode;
use clawft_platform::NativePlatform;
use clawft_rpc::Response;
use clawft_types::placement::TrustTier;
use ed25519_dalek::SigningKey;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{OnceCell, RwLock};

use crate::workload_host_serve::{HostParts, load_host_config, local_host, serve};
pub use crate::workload_place_policy::{
    PEERS_FILE, PERMITS_FILE, STATE_FILE, TRUST_FILE, load_anchors, load_peers, load_permits,
};
use crate::workload_place_policy::{load_container, load_seeds};

/// Methods served here (the rest of `workload.*` is in `workload_rpc`).
pub const METHODS: &[&str] = &[
    "workload.place",
    "workload.explain",
    "workload.status",
    "workload.stop",
    "workload.logs",
];

struct Boot {
    key: SigningKey,
    runtime_dir: PathBuf,
}

static BOOT: OnceLock<Boot> = OnceLock::new();
static PLANE: OnceCell<Arc<PlacementControlPlane>> = OnceCell::const_new();
/// This node's `workload-host` (in-process target and, when configured,
/// served to other nodes).
static HOST: OnceLock<Arc<WorkloadHostService>> = OnceLock::new();
/// Where it is served, once serving started.
static SERVED: OnceLock<SocketAddr> = OnceLock::new();
const LOCAL_ADDR: &str = "mem://local";

/// Record the daemon key and runtime dir (call once at daemon boot).
pub fn init(key: SigningKey, runtime_dir: PathBuf) {
    let _ = BOOT.set(Boot { key, runtime_dir });
}

/// True for the methods this module serves (`unload` only with an instance id).
pub fn handles(m: &str, params: &Value) -> bool {
    METHODS.contains(&m) || (m == "workload.unload" && params.get("instance_id").is_some())
}

/// Make `plane`'s remote targets match `workload-peers.json` (read on
/// every call, so removing a peer or lowering its tier takes effect
/// without a restart; see `apply_operator_peers`).
pub async fn sync_peers(plane: &PlacementControlPlane, dir: &Path) -> Result<(), String> {
    let peers = load_peers(dir)?;
    for (addr, e) in plane.apply_operator_peers(&peers, &[LOCAL_ADDR]).await {
        tracing::warn!(peer = %addr, error = %e, "workload peer not reachable");
    }
    Ok(())
}

/// The gate for placement decisions. On a project kernel (ADR-103 D8) it is
/// built from the effective rules (parent policy plus overlay) as of this
/// build, so an overlay deny on `workload.*` applies; a later push shows up
/// when the control plane is rebuilt.
fn gate(
    dir: &Path,
    chain: &Arc<ChainManager>,
    effective: Option<(Vec<clawft_kernel::governance::GovernanceRule>, f64, bool)>,
) -> Result<Arc<WorkloadGate>, String> {
    let base = match effective {
        Some((rules, threshold, human)) => WorkloadGate::with_rules(threshold.min(0.95), human, rules),
        None => WorkloadGate::new(0.95, false),
    };
    let mut g = base.with_chain(chain.clone());
    for p in load_permits(dir)? {
        g = g.with_permit(p)?;
    }
    Ok(Arc::new(g))
}

async fn build(
    kernel: &Arc<RwLock<Kernel<NativePlatform>>>,
) -> Result<Arc<PlacementControlPlane>, String> {
    let boot = BOOT
        .get()
        .ok_or("placement not initialised (no daemon key)")?;
    let dir = &boot.runtime_dir;
    let k = kernel.read().await;
    let chain = k
        .chain_manager()
        .cloned()
        .ok_or("placement needs the kernel chain (decisions are chained)")?;
    let membership = k.cluster_membership().clone();
    let effective = k.governance_overlay().map(|o| o.effective_rules());
    drop(k);
    let pk = boot.key.verifying_key().to_bytes();
    let id = clawft_kernel::node_id_from_pubkey(&pk);
    let anchors = load_anchors(dir)?;
    let gate = gate(dir, &chain, effective)?;
    let store = ArtifactStore::new_file(dir.join("workload-artifacts"));
    let mut ex = ArtifactExchange::new(&id, Arc::new(store), ExchangeConfig::default())
        .map_err(|e| e.to_string())?;
    ex.set_chain_manager(chain.clone());
    let ex = Arc::new(ex);
    let serving = load_host_config(dir)?;
    let container = load_container(dir, &dir.join("workload-containers"))?;
    // `describe` always answers with the facts the daemon re-probes.
    let (fm, fid) = (membership.clone(), id.clone());
    let facts: FactsSource = Arc::new(move || {
        let now = chrono::Utc::now().timestamp().max(0) as u64;
        fm.facts().get(&fid, now).map(|c| c.signed)
    });
    let local = Arc::new(local_host(HostParts {
        key: &boot.key,
        dir,
        chain: &chain,
        gate: gate.clone(),
        exchange: ex.clone(),
        anchors: anchors.clone(),
        facts,
        serving: serving.as_ref(),
        container,
    })?);
    let _ = HOST.set(local.clone());
    let conn = Arc::new(MeshConnector::new(true));
    let local_addr = conn.register_local("local", local);
    let seeds = load_seeds(dir, gate.clone(), &chain)?;
    let plane =
        PlacementControlPlane::new(boot.key.clone(), gate, chain, ex, anchors, conn)
            .with_membership(membership)
            .with_state_file(dir.join(STATE_FILE))
            .map_err(|e| format!("placement state: {e}"))?;
    for s in seeds {
        plane
            .add_seed(&s.node_id, s.host, s.tier)
            .map_err(|e| format!("seed {}: {e}", s.node_id))?;
    }
    if let Err(e) = plane.add_target(&local_addr, TrustTier::Pinned).await {
        tracing::warn!(error = %e, "this node is not a placement candidate yet (no signed facts?)");
    }
    // Targets restored from the state file are re-described with the key
    // each was learned with.
    plane.refresh().await;
    Ok(Arc::new(plane))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlaceParams {
    package_dir: PathBuf,
    #[serde(default)]
    peers: Vec<String>,
    #[serde(default)]
    pin: Option<String>,
    #[serde(default)]
    prefer: Vec<String>,
    #[serde(default)]
    avoid: Vec<String>,
    #[serde(default)]
    allow_emulated: bool,
    /// `once`, `listener`, or absent for `interval`.
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    interval: Option<u32>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    csi_port: Option<u16>,
    #[serde(default)]
    start: Option<bool>,
}

/// `workload.place {store_pin: ...}`: an operator-pinned store cog on a
/// Seed's operator-assigned node id (ADR-100 section 5).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StorePinParams {
    node_id: String,
    #[serde(default = "cognitum")]
    registry: String,
    id: String,
    version: String,
    #[serde(default)]
    sha256: Option<String>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    interval: Option<u32>,
    #[serde(default)]
    csi_port: Option<u16>,
    #[serde(default)]
    start: Option<bool>,
}

fn cognitum() -> String {
    "cognitum".into()
}

fn run_mode(mode: Option<&str>, interval: Option<u32>) -> Result<RunMode, String> {
    Ok(match mode {
        Some("once") => RunMode::Once,
        Some("listener") => RunMode::Listener,
        None | Some("interval") => RunMode::Interval {
            secs: interval.unwrap_or(10),
        },
        Some(other) => return Err(format!("unknown mode {other:?} (once|interval|listener)")),
    })
}

fn store_pin_order(v: Value) -> Result<StorePinOrder, String> {
    let p: StorePinParams =
        serde_json::from_value(v).map_err(|e| format!("invalid store_pin: {e}"))?;
    Ok(StorePinOrder {
        node_id: p.node_id,
        registry: p.registry,
        id: p.id,
        version: p.version,
        sha256: p.sha256,
        config: CtlConfig {
            mode: run_mode(p.mode.as_deref(), p.interval)?,
            args: Vec::new(),
            csi_port: p.csi_port.unwrap_or(5006),
        },
        start: p.start.unwrap_or(true),
    })
}

fn order(p: PlaceParams, dry_run: bool) -> Result<(PlaceOrder, Vec<String>), String> {
    let mode = run_mode(p.mode.as_deref(), p.interval)?;
    if !p.package_dir.is_absolute() {
        return Err(format!(
            "package_dir {} must be absolute (the daemon does not share the caller's working directory)",
            p.package_dir.display()
        ));
    }
    if !p.package_dir.is_dir() {
        return Err(format!(
            "package_dir {} is not a directory",
            p.package_dir.display()
        ));
    }
    Ok((
        PlaceOrder {
            package_dir: p.package_dir,
            config: CtlConfig {
                mode,
                args: p.args,
                csi_port: p.csi_port.unwrap_or(5006),
            },
            pin: p.pin,
            prefer: p.prefer,
            avoid: p.avoid,
            allow_emulated: p.allow_emulated,
            start: p.start.unwrap_or(true),
            dry_run,
        },
        p.peers,
    ))
}

fn instance_id(params: &Value, m: &str) -> Result<String, String> {
    params
        .get("instance_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("{m} requires a string 'instance_id'"))
}

/// Serve one placement method on `plane`.
pub async fn route(plane: &PlacementControlPlane, m: &str, params: Value) -> Response {
    let res: Result<Value, String> = async {
        match m {
            "workload.place" if params.get("store_pin").is_some() => {
                let o = store_pin_order(params["store_pin"].clone())?;
                let rec = plane.place_store_pin(&o).await.map_err(|e| e.to_string())?;
                Ok(json!({ "placed": rec, "route": clawft_kernel::workload_ctl::SEED_ROUTE }))
            }
            "workload.place" | "workload.explain" => {
                let p: PlaceParams = serde_json::from_value(params).map_err(|e| format!("invalid {m} params: {e}"))?;
                let (o, peers) = order(p, m == "workload.explain")?;
                // A caller cannot assign trust: a peer named in a request
                // is `discovered` unless the operator already knows it
                // (workload-peers.json assigns the tier).
                let known: Vec<String> = plane.targets().into_iter().map(|t| t.addr).collect();
                for addr in peers.into_iter().filter(|a| !known.contains(a)) {
                    plane.add_target(&addr, TrustTier::Discovered).await.map_err(|e| format!("peer {addr}: {e}"))?;
                }
                plane.refresh().await;
                let r = plane.place(&o).await.map_err(|e| e.to_string())?;
                serde_json::to_value(r).map_err(|e| e.to_string())
            }
            "workload.status" if params.get("instance_id").is_none() => {
                plane.settle_unsettled().await;
                let mut rows = Vec::new();
                for rec in plane.placements() {
                    let st = plane.instance(method::STATUS, &rec.instance_id).await;
                    rows.push(json!({ "placement": rec, "status": st.map_err(|e| e.to_string()) }));
                }
                let host = HOST.get().map(|h| h.advertisement());
                Ok(json!({ "controller": plane.node_id(), "targets": plane.targets(), "instances": rows,
                           "unsettled": plane.unsettled(),
                           "workload_host": host, "served_on": SERVED.get().map(|a| a.to_string()) }))
            }
            "workload.status" | "workload.stop" | "workload.logs" | "workload.unload" => {
                let id = instance_id(&params, m)?;
                plane.instance(m, &id).await.map_err(|e| e.to_string())
            }
            other => Err(format!("{other} is not a placement method")),
        }
    }
    .await;
    match res {
        Ok(v) => Response::success(v),
        Err(e) => Response::error(e),
    }
}

/// Daemon entry: lazily builds the control plane, then serves `m`.
pub async fn dispatch(
    m: &str,
    params: Value,
    kernel: Arc<RwLock<Kernel<NativePlatform>>>,
) -> Response {
    let plane = match PLANE.get_or_try_init(|| build(&kernel)).await {
        Ok(p) => p.clone(),
        Err(e) => return Response::error(format!("placement unavailable: {e}")),
    };
    if HOST.get().is_some() && !plane.targets().iter().any(|t| t.addr == LOCAL_ADDR) {
        // Facts were not probed yet when the plane was built.
        let _ = plane.add_target(LOCAL_ADDR, TrustTier::Pinned).await;
    }
    if let Some(dir) = BOOT.get().map(|b| b.runtime_dir.clone())
        && let Err(e) = sync_peers(&plane, &dir).await
    {
        // Fail closed: a broken peers file places nowhere remote.
        let _ = plane.apply_operator_peers(&[], &[LOCAL_ADDR]).await;
        return Response::error(e);
    }
    route(&plane, m, params).await
}

/// Daemon boot: when `workload-host.json` is present, build the control
/// plane (and with it this node's `workload-host`) and serve the host to
/// the configured controllers. Returns where it is served.
pub async fn start_serving(
    kernel: Arc<RwLock<Kernel<NativePlatform>>>,
) -> Result<Option<SocketAddr>, String> {
    let dir = BOOT
        .get()
        .map(|b| b.runtime_dir.clone())
        .ok_or("placement not initialised (no daemon key)")?;
    let Some(cfg) = load_host_config(&dir)? else {
        return Ok(None);
    };
    PLANE.get_or_try_init(|| build(&kernel)).await?;
    let host = HOST.get().cloned().ok_or("workload-host was not built")?;
    let adv = host.advertisement();
    let bound = serve(&cfg, host).await?;
    let _ = SERVED.set(bound);
    tracing::info!(addr = %bound, advertise = %cfg.advertised(), node = %adv.node_id,
        methods = adv.methods.len(), "workload-host served");
    Ok(Some(bound))
}

#[cfg(test)]
#[path = "workload_place_rpc_tests.rs"]
pub(crate) mod tests;

#[cfg(test)]
#[path = "workload_place_rpc_daemon_tests.rs"]
mod daemon_tests;
