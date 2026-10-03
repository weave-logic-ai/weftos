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
/// The effective-rules hash (hex) the control plane's gate was built from, or
/// `None` on a kernel without a governance overlay. Compared on every call:
/// the gate is fixed at build, so a later governance push must not leave an
/// older, possibly looser, gate deciding (ADR-103 D8: applies after a restart).
static BUILT_RULES: OnceLock<Option<String>> = OnceLock::new();
/// This node's `workload-host` (in-process target and, when configured,
/// served to other nodes).
static HOST: OnceLock<Arc<WorkloadHostService>> = OnceLock::new();
/// Keeps the cog ingest bridge listening for the daemon's life.
static INGEST: OnceLock<crate::cog_ingest_serve::IngestRuntime> = OnceLock::new();
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

/// Refuse when the governance in force is no longer the governance the gate
/// was built from. Fail closed: the placement gate does not follow a push, so
/// after one it declines to decide until the kernel restarts.
fn governance_changed(built: Option<&Option<String>>, now: Option<&str>) -> Result<(), String> {
    match built {
        Some(b) if b.as_deref() != now => Err(
            "governance changed since placement started; restart this kernel to apply it (ADR-103 D8)"
                .into(),
        ),
        _ => Ok(()),
    }
}

/// The gate for placement decisions. On a project kernel (ADR-103 D8) it is
/// built from the effective rules (parent policy plus overlay) as of this
/// build, so an overlay deny on `workload.*` applies. It does not follow a
/// later push: [`dispatch`] refuses (`governance_changed`) until restart.
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

/// Give the exchange this node's revocation list (a revoked package, signer
/// or artifact hash stops seeding at once) and, with a mesh, start taking
/// signed revocation notices from peers. The handle the runtime keeps is the
/// only owner needed: it lives as long as the runtime does.
fn wire_revocations(
    ex: &Arc<ArtifactExchange>,
    list: Arc<clawft_kernel::revocation::RevocationList>,
    anchors: &clawft_kernel::workload_pkg::TrustAnchors,
    mesh: Option<Arc<clawft_kernel::mesh_runtime::MeshRuntime>>,
) {
    match mesh {
        Some(rt) => {
            clawft_kernel::mesh_swarm_revoke::RevocationExchange::start(
                ex.clone(),
                list,
                anchors.clone(),
                rt,
            );
        }
        None => {
            ex.set_revocations(list);
        }
    }
    let swept = ex.apply_revocations();
    if !swept.is_empty() {
        tracing::warn!(n = swept.len(), "revoked artifacts evicted at startup");
    }
}

/// The admission state a binding is judged against (ADR-106): enforce mode
/// with open membership off. The daemon always has the governance gate that
/// answers verdicts (boot refuses `enforce` without one), so a verdict source
/// is bound whenever this runs.
fn licence_posture(
    mesh: Option<&clawft_types::config::MeshConfig>,
) -> clawft_kernel::licence::PostureFn {
    use clawft_types::config::MeshAdmissionMode;
    let p = clawft_kernel::licence::AdmissionPosture {
        enforce: mesh.is_some_and(|m| m.admission == MeshAdmissionMode::Enforce),
        verdict_source_bound: true,
        open_membership: mesh.is_none_or(|m| m.admission_open_membership),
    };
    Arc::new(move || p)
}

/// How often the daemon ticks the licence store.
const LICENCE_TICK: std::time::Duration = std::time::Duration::from_secs(60);

/// Tick the grant store every `every`: it records the clock and persists the
/// floor's high-water mark and any restrictive record that failed to save.
/// Nothing else writes on the read side, so the licence path needs this loop.
/// The task ends with the store.
fn spawn_licence_tick(store: Arc<clawft_kernel::licence::CheckoutGrantStore>, every: std::time::Duration) {
    let weak = Arc::downgrade(&store);
    drop(store);
    tokio::spawn(async move {
        let mut t = tokio::time::interval(every);
        loop {
            t.tick().await;
            let Some(s) = weak.upgrade() else { break };
            // The tick writes (fsync): keep it off the async workers, and
            // let a panic cost one tick, not the loop.
            if let Err(e) = tokio::task::spawn_blocking(move || s.tick()).await {
                tracing::error!(error = %e, "licence store tick failed; will retry");
            }
        }
    });
}

/// Start the licence exchange (ADR-106 phase 1b): chain the store's events,
/// open the approval store, and with a mesh carry bindings, grants and
/// approvals by flood and catch-up sync. Inert while the local mesh id is
/// unset (no `mesh_nonce` yet), exactly like the policy. Peers count as
/// admitted from the mesh runtime's own verdict until phase 1c brings the
/// service-stamped origin.
fn wire_licence(
    dir: &Path,
    policy: &Arc<clawft_kernel::licence::MeshCheckoutPolicy>,
    anchors: &clawft_kernel::workload_pkg::TrustAnchors,
    chain: &Arc<ChainManager>,
    mesh: Option<Arc<clawft_kernel::mesh_runtime::MeshRuntime>>,
    posture: clawft_kernel::licence::PostureFn,
) {
    use clawft_kernel::licence as l;
    let sink: Arc<dyn l::LicenceEventSink> = Arc::new(l::ChainLicenceSink::new(chain.clone()));
    let store = policy.store().clone();
    store.set_sink(sink.clone());
    spawn_licence_tick(store.clone(), LICENCE_TICK);
    let Some(runtime) = mesh else { return };
    let anchors = Arc::new(anchors.clone());
    let approvals = Arc::new(l::ApprovalStore::open_or_poisoned(
        &dir.join("licence"),
        anchors.clone(),
        store.local_mesh_id().clone(),
    ));
    if let Some(why) = approvals.poisoned() {
        tracing::warn!(%why, "approval store unreadable; approvals stay off");
    }
    l::LicenceExchange::start(l::LicenceExchangeParts {
        store,
        approvals,
        anchors,
        runtime,
        posture,
        admission: Arc::new(l::CtxAdmission),
        sink,
        config: l::LicenceExchangeConfig::default(),
    });
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
    let effective = k.governance_overlay().map(|o| o.effective_rules_and_hash());
    let (effective, built) = match effective {
        Some((rules, hash)) => (Some(rules), Some(hash)),
        None => (None, None),
    };
    let _ = BUILT_RULES.set(built);
    let revocations = k.revocation_list().clone();
    let mesh = k.a2a_router().mesh_runtime().cloned();
    let posture = licence_posture(k.kernel_config().mesh.as_ref());
    drop(k);
    let pk = boot.key.verifying_key().to_bytes();
    let id = clawft_kernel::node_id_from_pubkey(&pk);
    let anchors = load_anchors(dir)?;
    let gate = gate(dir, &chain, effective)?;
    // `open_file` indexes the blobs already on disk (installed workloads'
    // files from earlier runs), so the exchange finds them present and never
    // takes ownership of, or evicts, bytes it did not create.
    let store = ArtifactStore::open_file(dir.join("workload-artifacts"))
        .map_err(|e| format!("workload artifact store: {e}"))?;
    // The licence boundary is explicit: what this node may hand to peers is
    // decided by the signed manifests (`redistributable = true`), except for
    // Cognitum cogs under a valid checkout grant of a bound Seed (ADR-106).
    // The policy is installed unconditionally and reads the binding live, so
    // a binding that arrives later needs no restart; with none (or no mesh id
    // yet) it is exactly `ManifestPolicy`.
    let local_mesh = clawft_kernel::licence::LocalMeshId::unset();
    let policy = clawft_kernel::licence::MeshCheckoutPolicy::open(
        dir,
        anchors.clone(),
        revocations.clone(),
        local_mesh.clone(),
    );
    let cfg = ExchangeConfig {
        redistribution: policy.clone(),
        ..ExchangeConfig::default()
    };
    let mut ex = ArtifactExchange::new(&id, Arc::new(store), cfg)
        .map_err(|e| e.to_string())?;
    ex.set_chain_manager(chain.clone());
    let ex = Arc::new(ex);
    wire_licence(dir, &policy, &anchors, &chain, mesh.clone(), posture);
    wire_revocations(&ex, revocations, &anchors, mesh);
    let serving = load_host_config(dir)?;
    let container = load_container(dir, &dir.join("workload-containers"))?;
    // `describe` always answers with the facts the daemon re-probes.
    let (fm, fid) = (membership.clone(), id.clone());
    let facts: FactsSource = Arc::new(move || {
        let now = chrono::Utc::now().timestamp().max(0) as u64;
        fm.facts().get(&fid, now).map(|c| c.signed)
    });
    let identity: Arc<dyn clawft_kernel::cog_ingest::ProjectDirectory> =
        Arc::new(crate::cog_ingest_serve::IdentityDirectory);
    let ingest = match crate::cog_ingest_serve::load_config(dir) {
        Ok(cfg) => match crate::cog_ingest_serve::start(&cfg, &boot.key, Some(identity)).await {
            Ok(rt) => Some(rt),
            Err(e) => {
                tracing::warn!(error = %e, "cog ingest not started");
                None
            }
        },
        Err(e) => {
            tracing::warn!(error = %e, "cog ingest config invalid; cogs run without ingest");
            None
        }
    };
    let hooks = ingest.as_ref().map(|r| r.hooks.clone());
    if let Some(rt) = ingest {
        tracing::info!(bridge = ?rt.bridge_addr, disabled = ?rt.bridge_error, store_owner = ?rt.owner_addr, "cog ingest");
        let _ = INGEST.set(rt);
    }
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
        ingest: hooks,
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
    /// Project the cog is placed for (a project id). Its ingested vectors
    /// go to that project's store; absent, to the placing controller's.
    /// Refused here unless the project is registered and not revoked, and
    /// again by the target host unless this controller may place for it.
    #[serde(default)]
    project: Option<String>,
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
    if let Some(proj) = &p.project {
        crate::cog_ingest_serve::check_project(proj)?;
    }
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
            project_id: p.project,
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
    let now = kernel.read().await.governance_overlay().map(|o| o.applied().effective_hash);
    if let Err(e) = governance_changed(BUILT_RULES.get(), now.as_deref()) {
        return Response::error(e);
    }
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
mod governance_changed_tests {
    use super::governance_changed;

    #[test]
    fn a_push_after_build_makes_placement_refuse() {
        assert!(governance_changed(None, Some("a")).is_ok(), "not built yet");
        let plain: Option<String> = None;
        assert!(governance_changed(Some(&plain), None).is_ok(), "no overlay, none now");
        let built = Some("aa".to_owned());
        assert!(governance_changed(Some(&built), Some("aa")).is_ok());
        let e = governance_changed(Some(&built), Some("bb")).unwrap_err();
        assert!(e.contains("restart"), "{e}");
        assert!(governance_changed(Some(&built), None).is_err());
    }
}

#[cfg(test)]
#[path = "workload_place_rpc_tests.rs"]
pub(crate) mod tests;

#[cfg(test)]
#[path = "workload_place_rpc_daemon_tests.rs"]
mod daemon_tests;

#[cfg(test)]
mod revocation_wiring_tests {
    use super::*;
    use clawft_kernel::artifact_store::ArtifactStore;
    use clawft_kernel::ipc::{KernelMessage, MessagePayload, MessageTarget};
    use clawft_kernel::mesh_ipc::MeshIpcEnvelope;
    use clawft_kernel::mesh_runtime::{MeshRuntime, REVOKE_TOPIC};
    use clawft_kernel::mesh_swarm_revoke::sign_revocation;
    use clawft_kernel::revocation::{RevocationKind, RevocationList};
    use clawft_kernel::workload_pkg::{KeyOrigin, TrustAnchors};
    use ed25519_dalek::SigningKey;

    #[tokio::test]
    async fn the_daemon_exchange_gets_the_revocation_list_and_takes_mesh_notices() {
        let tmp = tempfile::tempdir().unwrap();
        let list = Arc::new(RevocationList::new(tmp.path().join("revoked.json")));
        let ex = Arc::new(
            ArtifactExchange::new("n", Arc::new(ArtifactStore::new_memory()), ExchangeConfig::default())
                .unwrap(),
        );
        let key = SigningKey::from_bytes(&[4; 32]);
        let pk = key.verifying_key().to_bytes();
        let mut anchors = TrustAnchors::default();
        anchors
            .push_signer("op", &clawft_kernel::workload_pkg::codec::hex_encode(&pk), KeyOrigin::Operator)
            .unwrap();
        let rt = Arc::new(MeshRuntime::new("n".into()));
        wire_revocations(&ex, list.clone(), &anchors, Some(rt.clone()));
        // The exchange already has a list: a second one is refused.
        assert!(!ex.set_revocations(list.clone()));

        // A signed notice arriving over the mesh lands in the node's list.
        let hash = clawft_kernel::workload_pkg::codec::hex_encode(&[9u8; 32]);
        let notice = sign_revocation(RevocationKind::ArtifactHash, &hash, "test", 1, &key).unwrap();
        let msg = KernelMessage::new(
            0,
            MessageTarget::Topic(REVOKE_TOPIC.into()),
            MessagePayload::Json(serde_json::to_value(&notice).unwrap()),
        );
        let bytes = MeshIpcEnvelope::new("peer".into(), "n".into(), msg).to_bytes().unwrap();
        let (tx, _rx) = tokio::sync::mpsc::channel(4);
        rt.handle_incoming_peer(&bytes, tx, None).await.unwrap();
        assert!(list.is_subject_revoked(RevocationKind::ArtifactHash, &hash));
    }

    #[tokio::test]
    async fn the_daemon_licence_exchange_is_inert_until_the_mesh_id_is_set_then_takes_bindings() {
        use clawft_kernel::licence::{
            AdmissionPosture, BindState, BindingRecord, LocalMeshId, MeshCheckoutPolicy, MeshId,
            sign_binding,
        };
        use clawft_kernel::mesh_runtime::COG_BINDING_TOPIC;
        let tmp = tempfile::tempdir().unwrap();
        let list = Arc::new(RevocationList::new(tmp.path().join("revoked.json")));
        let key = SigningKey::from_bytes(&[4; 32]);
        let hex = clawft_kernel::workload_pkg::codec::hex_encode;
        let mut anchors = TrustAnchors::default();
        anchors.push_signer("op", &hex(&key.verifying_key().to_bytes()), KeyOrigin::Operator).unwrap();
        let local = LocalMeshId::unset();
        let policy = MeshCheckoutPolicy::open(tmp.path(), anchors.clone(), list, local.clone());
        let chain = Arc::new(ChainManager::new(0, 1000));
        let rt = Arc::new(MeshRuntime::new("n".into()));
        let enforce = AdmissionPosture { enforce: true, verdict_source_bound: true, open_membership: false };
        wire_licence(tmp.path(), &policy, &anchors, &chain, Some(rt.clone()), Arc::new(move || enforce));

        let mesh = MeshId::derive(&[1; 32], &[2; 32]);
        let rec = BindingRecord {
            v: 2,
            device_id: "seed-x".into(),
            device_pubkey: hex(&[5; 32]),
            mesh_id: mesh.to_hex(),
            grant_pubkey: hex(&[6; 32]),
            steward_node_id: "steward".into(),
            steward_pubkey: hex(&[7; 32]),
            state: BindState::Bound,
            seq: 1,
            bound_at: 1,
        };
        let signed = sign_binding(&rec, &key).unwrap();
        let send = |rt: Arc<MeshRuntime>, v: serde_json::Value| async move {
            let msg = KernelMessage::new(0, MessageTarget::Topic(COG_BINDING_TOPIC.into()), MessagePayload::Json(v));
            let bytes = MeshIpcEnvelope::new("peer".into(), "n".into(), msg).to_bytes().unwrap();
            let (tx, _rx) = tokio::sync::mpsc::channel(4);
            rt.handle_incoming_peer(&bytes, tx, None).await.unwrap();
        };
        send(rt.clone(), serde_json::to_value(&signed).unwrap()).await;
        assert!(policy.store().held_binding().is_none(), "inert: no local mesh id yet");
        local.set(Some(mesh));
        send(rt, serde_json::to_value(&signed).unwrap()).await;
        assert_eq!(policy.store().held_binding().map(|b| b.0), Some(1));
    }

    #[tokio::test]
    async fn the_daemon_tick_loop_persists_the_clock_floor() {
        use clawft_kernel::licence::{
            AdmissionPosture, BindState, BindingRecord, LocalMeshId, MeshCheckoutPolicy, MeshId,
            NoExtraChecks, sign_binding, CheckoutGrant, GrantArtifact, LicenceRef, sign_grant,
        };
        let tmp = tempfile::tempdir().unwrap();
        let key = SigningKey::from_bytes(&[4; 32]);
        let hex = clawft_kernel::workload_pkg::codec::hex_encode;
        let mut anchors = TrustAnchors::default();
        anchors.push_signer("op", &hex(&key.verifying_key().to_bytes()), KeyOrigin::Operator).unwrap();
        let gk = SigningKey::from_bytes(&[6; 32]);
        let mesh = MeshId::derive(&[1; 32], &[2; 32]);
        let list = Arc::new(RevocationList::new(tmp.path().join("revoked.json")));
        let policy = MeshCheckoutPolicy::open(tmp.path(), anchors, list, LocalMeshId::new(mesh));
        let rec = BindingRecord {
            v: 2,
            device_id: "seed-x".into(),
            device_pubkey: hex(&[5; 32]),
            mesh_id: mesh.to_hex(),
            grant_pubkey: hex(&gk.verifying_key().to_bytes()),
            steward_node_id: "steward".into(),
            steward_pubkey: hex(&[7; 32]),
            state: BindState::Bound,
            seq: 1,
            bound_at: 1,
        };
        let posture = AdmissionPosture { enforce: true, verdict_source_bound: true, open_membership: false };
        policy
            .store()
            .accept_binding(&sign_binding(&rec, &key).unwrap(), posture, &NoExtraChecks)
            .unwrap();
        // The floor records the clock only once a grant has been accepted.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let grant = CheckoutGrant {
            v: 1,
            grant_id: String::new(),
            mesh_id: mesh.to_hex(),
            seed_device_id: "seed-x".into(),
            grant_key_id: String::new(),
            source: "cognitum".into(),
            registry: "registry.example".into(),
            cog_id: "probe".into(),
            version: "1.0.0".into(),
            artifacts: vec![GrantArtifact {
                arch: "x86_64".into(),
                size: 1,
                sha256: hex(&[1; 32]),
                blake3: hex(&[2; 32]),
            }],
            manifest_sha256: hex(&[3; 32]),
            licence: LicenceRef { ref_sha256: hex(&[4; 32]), expires: now + 86_400 },
            seq: 1,
            issued_at: now,
            expires_at: now + 3600,
        };
        policy.store().accept_grant(&sign_grant(&grant, &gk).unwrap()).unwrap();
        let file = tmp.path().join("licence").join("checkout_grants.json");
        let hw = |p: &std::path::Path| -> u64 {
            let v: serde_json::Value = serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap();
            v["floors"].as_object().and_then(|m| m.values().next()).map_or(0, |f| f["hw"].as_u64().unwrap_or(0))
        };
        assert_eq!(hw(&file), 0, "the grant save carries no clock high-water mark");
        spawn_licence_tick(policy.store().clone(), std::time::Duration::from_millis(10));
        for _ in 0..200 {
            if hw(&file) >= now {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("the tick loop never persisted the floor");
    }
}
