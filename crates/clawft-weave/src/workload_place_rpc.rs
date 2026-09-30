//! Daemon side of the placement control plane (ADR-099 sections 3 and 7,
//! card mesh-placement-12): `workload.place`, `workload.explain`,
//! `workload.status`, `workload.stop`, `workload.logs` and
//! `workload.unload {instance_id}`.
//!
//! The daemon is the controller. It signs with its node key
//! (`<runtime>/node.key`), chains every decision on the kernel chain, and
//! reads operator policy from the runtime directory:
//!
//! - `workload-permits.json`: `[WorkloadPermitRule, ...]`; missing means no
//!   permits, so every `workload.*` action is denied (default deny);
//! - `workload-trust.json`: a `weftos.workload-trust.v1` trust file with the
//!   operator-pinned package signers; missing means only the compiled-in
//!   WeftOS signer set (so operator-signed packages are refused);
//! - `workload-peers.json`: `[{"addr": "host:port", "tier": "paired"}]`,
//!   the `workload-host`s this daemon may place on, with the trust tier the
//!   operator assigns (a request can name more `peers`; they are only ever
//!   `discovered`);
//! - `workload-host.json`: serve this node's `workload-host` to other
//!   controllers (see `workload_host_serve`).
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
    CtlConfig, FactsSource, MeshConnector, PlaceOrder, PlacementControlPlane, WorkloadHostService,
};
use clawft_kernel::workload_governance::{WorkloadGate, WorkloadPermitRule};
use clawft_kernel::workload_pkg::TrustAnchors;
use clawft_kernel::workload_runtime::RunMode;
use clawft_platform::NativePlatform;
use clawft_rpc::Response;
use clawft_types::placement::TrustTier;
use ed25519_dalek::SigningKey;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{OnceCell, RwLock};

use crate::workload_host_serve::{HostParts, load_host_config, local_host, serve};

/// Methods served here (the rest of `workload.*` is in `workload_rpc`).
pub const METHODS: &[&str] = &[
    "workload.place",
    "workload.explain",
    "workload.status",
    "workload.stop",
    "workload.logs",
];

/// Permit file under the runtime dir.
pub const PERMITS_FILE: &str = "workload-permits.json";
/// Trust file under the runtime dir.
pub const TRUST_FILE: &str = "workload-trust.json";
/// Peers file under the runtime dir.
pub const PEERS_FILE: &str = "workload-peers.json";
const MAX_POLICY_BYTES: u64 = 256 * 1024;

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

fn read_policy(path: &Path) -> Result<Option<String>, String> {
    match std::fs::metadata(path) {
        Err(_) => Ok(None),
        Ok(m) if m.len() > MAX_POLICY_BYTES => Err(format!("{} is too large", path.display())),
        Ok(_) => std::fs::read_to_string(path)
            .map(Some)
            .map_err(|e| e.to_string()),
    }
}

/// Permits from the runtime dir (none if the file is absent).
pub fn load_permits(dir: &Path) -> Result<Vec<WorkloadPermitRule>, String> {
    match read_policy(&dir.join(PERMITS_FILE))? {
        None => Ok(Vec::new()),
        Some(t) => serde_json::from_str(&t).map_err(|e| format!("{PERMITS_FILE}: {e}")),
    }
}

/// Trust anchors from the runtime dir (WeftOS defaults if absent).
pub fn load_anchors(dir: &Path) -> Result<TrustAnchors, String> {
    match read_policy(&dir.join(TRUST_FILE))? {
        None => TrustAnchors::weftos_default(),
        Some(t) => TrustAnchors::from_trust_json(t.as_bytes()),
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PeerEntry {
    addr: String,
    #[serde(default = "paired")]
    tier: TrustTier,
}

fn paired() -> TrustTier {
    TrustTier::Paired
}

fn load_peers(dir: &Path) -> Result<Vec<PeerEntry>, String> {
    match read_policy(&dir.join(PEERS_FILE))? {
        None => Ok(Vec::new()),
        Some(t) => serde_json::from_str(&t).map_err(|e| format!("{PEERS_FILE}: {e}")),
    }
}

fn gate(dir: &Path, chain: &Arc<ChainManager>) -> Result<Arc<WorkloadGate>, String> {
    let mut g = WorkloadGate::new(0.95, false).with_chain(chain.clone());
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
    drop(k);
    let pk = boot.key.verifying_key().to_bytes();
    let id = clawft_kernel::node_id_from_pubkey(&pk);
    let anchors = load_anchors(dir)?;
    let gate = gate(dir, &chain)?;
    let store = ArtifactStore::new_file(dir.join("workload-artifacts"));
    let mut ex = ArtifactExchange::new(&id, Arc::new(store), ExchangeConfig::default())
        .map_err(|e| e.to_string())?;
    ex.set_chain_manager(chain.clone());
    let ex = Arc::new(ex);
    let serving = load_host_config(dir)?;
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
    })?);
    let _ = HOST.set(local.clone());
    let conn = Arc::new(MeshConnector::new(true));
    let local_addr = conn.register_local("local", local);
    let plane = PlacementControlPlane::new(boot.key.clone(), gate, chain, ex, anchors, conn)
        .with_membership(membership);
    if let Err(e) = plane.add_target(&local_addr, TrustTier::Pinned).await {
        tracing::warn!(error = %e, "this node is not a placement candidate yet (no signed facts?)");
    }
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

fn order(p: PlaceParams, dry_run: bool) -> Result<(PlaceOrder, Vec<String>), String> {
    let mode = match p.mode.as_deref() {
        Some("once") => RunMode::Once,
        Some("listener") => RunMode::Listener,
        None | Some("interval") => RunMode::Interval {
            secs: p.interval.unwrap_or(10),
        },
        Some(other) => return Err(format!("unknown mode {other:?} (once|interval|listener)")),
    };
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
                let mut rows = Vec::new();
                for rec in plane.placements() {
                    let st = plane.instance(method::STATUS, &rec.instance_id).await;
                    rows.push(json!({ "placement": rec, "status": st.map_err(|e| e.to_string()) }));
                }
                let host = HOST.get().map(|h| h.advertisement());
                Ok(json!({ "controller": plane.node_id(), "targets": plane.targets(), "instances": rows,
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
    if let Some(dir) = BOOT.get().map(|b| b.runtime_dir.clone()) {
        match load_peers(&dir) {
            Ok(peers) => {
                let known: Vec<String> = plane.targets().into_iter().map(|t| t.addr).collect();
                for p in peers.into_iter().filter(|p| !known.contains(&p.addr)) {
                    if let Err(e) = plane.add_target(&p.addr, p.tier).await {
                        tracing::warn!(peer = %p.addr, error = %e, "workload peer not reachable");
                    }
                }
            }
            Err(e) => return Response::error(e),
        }
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
