//! A stub cog placed through the real placement path (controller, signed
//! `workload.ctl`, fetch-before-load, native adapter, governance) posts to
//! the ingest bridge and its vectors appear in the placing project's store;
//! stop and unload revoke its token. Also who may place for a project, what
//! is refused at place time, and the degraded (bridge down) behaviour.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use clawft_types::placement::TrustTier;
use ed25519_dalek::SigningKey;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::host_service::CtlConfig;
use super::msg::method;
use super::plane::PlacementControlPlane;
use super::plane_place::{PlaceOrder, PlaceReport};
use super::test_support::*;
use super::transport::MeshConnector;
use crate::cog_ingest::{
    BridgeConfig, BridgeScope, IngestBridge, IngestHooks, IngestStore, LocalForwarder,
    MemoryIngestStore, ProjectDirectory, RateBudget, StaticDirectory, StaticRouter,
    StoreDirectory, TokenRegistry,
};
use crate::node_registry::node_id_from_pubkey;
use crate::workload_runtime::RunMode;

const PROJECT: &str = "01J9ZXW0PRJCTAAAAAAAAAAAAA";
const OTHER: &str = "01J9ZXW0PRJCTBBBBBBBBBBBBB";
const UNROUTED: &str = "01J9ZXW0PRJCTCCCCCCCCCCCCC";

/// Posts one vector with the token from the environment, then idles. It
/// also prints a lying stdout line (a sentinel: stdout is evidence only; the
/// real guard is that no code path parses it).
const COG: &str = r#"#!/bin/sh
echo "$COGNITUM_COG_TOKEN" > "$COGNITUM_COG_DATA_DIR/token"
echo '{"ingested":999,"vectors":[[999,[9,9,9,9,9,9,9,9]]]}'
curl -s -m 5 -X POST -H "Authorization: Bearer $COGNITUM_COG_TOKEN" -H 'Content-Type: application/json' \
  -d '{"vectors":[[1,[0.1,0.2,0.3,0.4,0.5,0.6,0.7,0.8]]],"dedup":true}' \
  "$COGNITUM_INGEST_URL" > "$COGNITUM_COG_DATA_DIR/resp.json"
exec sleep 30
"#;

fn need_curl() {
    assert!(
        std::process::Command::new("curl").arg("--version").output().is_ok(),
        "curl is required for the ingest end-to-end tests"
    );
}

async fn post(addr: std::net::SocketAddr, token: &str, body: &str) -> u16 {
    let raw = format!(
        "POST /api/v1/store/ingest HTTP/1.1\r\nAuthorization: Bearer {token}\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(raw.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out).await;
    String::from_utf8_lossy(&out)
        .split(' ')
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0)
}

async fn until(what: &str, mut f: impl FnMut() -> bool) {
    for _ in 0..100 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out waiting for {what}");
}

fn node(key: &SigningKey) -> String {
    node_id_from_pubkey(&key.verifying_key().to_bytes())
}

/// A host node with the bridge wired in, a controller, and the stores.
struct Rig {
    host: HostNode,
    plane: PlacementControlPlane,
    registry: Arc<TokenRegistry>,
    proj_store: Arc<MemoryIngestStore>,
    ctl_store: Arc<MemoryIngestStore>,
    addr: std::net::SocketAddr,
    pkg: std::path::PathBuf,
    _tmp: tempfile::TempDir,
    _bridge: crate::cog_ingest::BridgeHandle,
}

struct RigOpts {
    /// Host node key seed (its node id is the "own node").
    host_seed: u8,
    /// Controller key.
    ctl: SigningKey,
    /// Project routes to register (to the project store).
    routed: Vec<&'static str>,
    /// Controller route for project-less placements (by this controller).
    ctl_route: bool,
    /// Controllers listed per project.
    controllers: HashMap<String, HashSet<String>>,
    directory: Option<Arc<dyn ProjectDirectory>>,
    /// Build disabled hooks instead of a running bridge.
    disabled: bool,
}

async fn rig(o: RigOpts) -> Rig {
    let (proj_store, ctl_store) = (
        Arc::new(MemoryIngestStore::new(1000)),
        Arc::new(MemoryIngestStore::new(1000)),
    );
    let dir: Arc<dyn StoreDirectory> = Arc::new(
        StaticDirectory::new()
            .with_project(PROJECT, proj_store.clone())
            .with_project(OTHER, proj_store.clone())
            .with_fallback(ctl_store.clone()),
    );
    let fwd = Arc::new(LocalForwarder::new("host", dir));
    let mut router = StaticRouter::new();
    for p in &o.routed {
        router = router.with_project(p, fwd.clone());
    }
    if o.ctl_route {
        router = router.with_controller(&node(&o.ctl), fwd.clone());
    }
    let registry = Arc::new(TokenRegistry::new());
    let bridge = IngestBridge::new(
        registry.clone(),
        Arc::new(router),
        RateBudget::default(),
        BridgeConfig::default(),
    );
    let listener = bridge
        .bind("127.0.0.1:0".parse().unwrap(), BridgeScope::Any)
        .await
        .unwrap();
    let addr = listener.addr();
    let own = node(&SigningKey::from_bytes(&[o.host_seed; 32]));
    let mut hooks = if o.disabled {
        IngestHooks::disabled(own, "test: port taken")
    } else {
        IngestHooks::new(own, registry.clone(), bridge, addr, None)
    }
    .with_project_controllers(o.controllers);
    if let Some(d) = o.directory {
        hooks = hooks.with_project_directory(d);
    }
    let host = host_node_ingest(o.host_seed, board_caps("pi5"), &o.ctl, hooks);
    let tmp = tempfile::tempdir().unwrap();
    let pkg = package(tmp.path(), "ingest-cog", COG, &[arch()]);
    let conn = Arc::new(MeshConnector::new(false));
    let host_addr = conn.register_local("host", host.svc.clone());
    let (plane, _chain) = controller(&o.ctl, conn);
    plane
        .add_target(&host_addr, TrustTier::Paired)
        .await
        .unwrap();
    Rig {
        host,
        plane,
        registry,
        proj_store,
        ctl_store,
        addr,
        pkg,
        _tmp: tmp,
        _bridge: listener,
    }
}

fn opts(ctl: SigningKey) -> RigOpts {
    RigOpts {
        host_seed: 11,
        ctl,
        routed: vec![PROJECT],
        ctl_route: true,
        controllers: HashMap::new(),
        directory: None,
        disabled: false,
    }
}

impl Rig {
    async fn place(&self, project: Option<&str>, csi_port: u16) -> PlaceReport {
        let order = PlaceOrder {
            package_dir: self.pkg.clone(),
            config: CtlConfig {
                mode: RunMode::Listener,
                args: vec![],
                csi_port,
            },
            pin: None,
            prefer: vec![],
            avoid: vec![],
            allow_emulated: false,
            start: true,
            dry_run: false,
            project_id: project.map(String::from),
        };
        self.plane.place(&order).await.unwrap()
    }
}

fn listed(project: &str, node: String) -> HashMap<String, HashSet<String>> {
    HashMap::from([(project.to_string(), HashSet::from([node]))])
}

struct FakeDirectory(HashMap<String, String>);
impl ProjectDirectory for FakeDirectory {
    fn bound_node(&self, p: &str) -> Option<String> {
        self.0.get(p).cloned()
    }
}

#[tokio::test]
async fn placed_cog_vectors_reach_the_project_store_and_stop_and_unload_revoke_its_token() {
    need_curl();
    let ctl = SigningKey::from_bytes(&[10; 32]);
    let mut o = opts(ctl.clone());
    o.controllers = listed(PROJECT, node(&ctl));
    let r = rig(o).await;
    let report = r.place(Some(PROJECT), 15007).await;
    let rec = report.placed.clone().expect("placed");
    assert_eq!(rec.project_id.as_deref(), Some(PROJECT), "{}", report.explain);
    assert!(r.registry.contains(&rec.instance_id), "token issued at place time");

    // The cog posts with its injected token and URL; the vector lands in the
    // project's store.
    until("vectors in the project store", || r.proj_store.len() == 1).await;
    let hit = &r.proj_store.query(&[0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8], 1)[0];
    assert_eq!((hit.id, hit.instance_id.as_str()), (1, rec.instance_id.as_str()));
    // Sentinel (cannot fail for a code path that never reads the store of
    // another route; kept to document intent).
    assert_eq!(r.ctl_store.len(), 0);

    let token = std::fs::read_to_string(
        r.host
            ._tmp
            .path()
            .join("instances")
            .join(&rec.instance_id)
            .join("data/token"),
    )
    .unwrap()
    .trim()
    .to_string();
    let more = r#"{"vectors":[[2,[1,1,1,1,1,1,1,1]]]}"#;
    assert_eq!(post(r.addr, &token, more).await, 200);
    assert_eq!(r.proj_store.len(), 2);

    // Sentinel: the cog's stdout claims 999 vectors; nothing parses stdout.
    r.plane.instance(method::STOP, &rec.instance_id).await.unwrap();
    let logs = r.plane.instance(method::LOGS, &rec.instance_id).await.unwrap();
    assert!(logs["stdout"].as_str().unwrap().contains("999"));
    assert_eq!(r.proj_store.len(), 2);

    // Stop revokes the token; start issues it again; unload revokes for good.
    assert!(!r.registry.contains(&rec.instance_id));
    assert_eq!(post(r.addr, &token, more).await, 401, "stopped: revoked");
    r.plane.instance(method::START, &rec.instance_id).await.unwrap();
    assert!(r.registry.contains(&rec.instance_id));
    assert_eq!(post(r.addr, &token, more).await, 200, "restarted: accepted again");
    r.plane.instance(method::STOP, &rec.instance_id).await.unwrap();
    r.plane.instance(method::UNLOAD, &rec.instance_id).await.unwrap();
    assert!(r.registry.is_empty());
    assert_eq!(post(r.addr, &token, more).await, 401, "unloaded: revoked");
    assert_eq!(r.proj_store.len(), 2);
}

#[tokio::test]
async fn a_project_less_placement_delivers_to_the_controllers_store() {
    need_curl();
    let r = rig(opts(SigningKey::from_bytes(&[12; 32]))).await;
    let report = r.place(None, 15008).await;
    let id = report.placed.expect("placed").instance_id;
    until("vectors in the controller's store", || r.ctl_store.len() == 1).await;
    assert_eq!(r.proj_store.len(), 0);
    r.plane.instance(method::UNLOAD, &id).await.unwrap();
    assert!(r.registry.is_empty());
}

#[tokio::test]
async fn a_controller_may_not_claim_a_project_it_is_not_authorised_for() {
    let ctl = SigningKey::from_bytes(&[13; 32]);
    let mut o = opts(ctl.clone());
    o.routed = vec![PROJECT, OTHER];
    o.controllers = listed(PROJECT, node(&ctl)); // listed for PROJECT only
    let r = rig(o).await;

    let report = r.place(Some(OTHER), 15010).await;
    assert!(report.placed.is_none(), "{}", report.explain);
    assert_eq!(report.attempts[0].code.as_deref(), Some("unauthorized"), "{}", report.explain);
    assert!(r.registry.is_empty(), "no token for a refused placement");

    // The project it is listed for is placed.
    let ok = r.place(Some(PROJECT), 15011).await;
    assert!(ok.placed.is_some(), "{}", ok.explain);
    r.plane
        .instance(method::UNLOAD, &ok.placed.unwrap().instance_id)
        .await
        .unwrap();
}

#[tokio::test]
async fn the_host_nodes_own_key_and_the_projects_bound_key_may_place() {
    // Own key: the controller is the host node itself, nothing listed.
    let own = SigningKey::from_bytes(&[16; 32]);
    let mut o = opts(own.clone());
    o.host_seed = 16;
    let r = rig(o).await;
    let a = r.place(Some(PROJECT), 15012).await;
    assert!(a.placed.is_some(), "own node: {}", a.explain);
    r.plane.instance(method::UNLOAD, &a.placed.unwrap().instance_id).await.unwrap();

    // The project's bound key (identity records): a different node, listed
    // nowhere, but it is the node of the key bound to the project.
    let proj_key = SigningKey::from_bytes(&[17; 32]);
    let mut o = opts(proj_key.clone());
    o.directory = Some(Arc::new(FakeDirectory(HashMap::from([(
        PROJECT.to_string(),
        node(&proj_key),
    )]))));
    let r = rig(o).await;
    let b = r.place(Some(PROJECT), 15013).await;
    assert!(b.placed.is_some(), "bound project key: {}", b.explain);
    r.plane.instance(method::UNLOAD, &b.placed.unwrap().instance_id).await.unwrap();

    // Another node's key is not the bound key of that project.
    let stranger = SigningKey::from_bytes(&[18; 32]);
    let mut o = opts(stranger);
    o.directory = Some(Arc::new(FakeDirectory(HashMap::from([(
        PROJECT.to_string(),
        node(&proj_key),
    )]))));
    let r = rig(o).await;
    let c = r.place(Some(PROJECT), 15014).await;
    assert!(c.placed.is_none(), "{}", c.explain);
    assert_eq!(c.attempts[0].code.as_deref(), Some("unauthorized"));
}

#[tokio::test]
async fn a_placement_with_no_store_route_is_refused_not_placed_to_lose_its_vectors() {
    let ctl = SigningKey::from_bytes(&[19; 32]);
    let mut o = opts(ctl.clone());
    o.controllers = listed(UNROUTED, node(&ctl));
    o.ctl_route = false; // project-less placements by this controller have no route
    let r = rig(o).await;

    let p = r.place(Some(UNROUTED), 15015).await;
    assert!(p.placed.is_none(), "{}", p.explain);
    assert_eq!(p.attempts[0].code.as_deref(), Some("admission"), "{}", p.explain);
    assert!(p.attempts[0].reason.as_deref().unwrap_or("").contains("not routed"));

    let q = r.place(None, 15016).await;
    assert!(q.placed.is_none(), "{}", q.explain);
    assert_eq!(q.attempts[0].code.as_deref(), Some("admission"));
    assert!(r.registry.is_empty());
}

#[tokio::test]
async fn with_the_bridge_down_a_cog_is_placed_without_a_token_and_says_so() {
    let ctl = SigningKey::from_bytes(&[20; 32]);
    let mut o = opts(ctl);
    o.disabled = true;
    let r = rig(o).await;
    let report = r.place(None, 15017).await;
    let rec = report.placed.clone().expect("placed degraded");
    // The place output itself says so (what `weaver workload place` prints).
    assert_eq!(report.attempts[0].ingest.as_deref(), Some("disabled"));
    assert!(
        report.explain.contains("placed with ingest disabled"),
        "{}",
        report.explain
    );
    assert!(r.registry.is_empty(), "no token is issued while ingest is disabled");
    let st = r.plane.instance(method::STATUS, &rec.instance_id).await.unwrap();
    assert_eq!(st["ingest"], "disabled");
    assert_eq!(r.host.svc.advertisement().metadata["ingest"], "disabled");
    // The cog got no token and no URL in its environment.
    let token_file = r
        .host
        ._tmp
        .path()
        .join("instances")
        .join(&rec.instance_id)
        .join("data/token");
    until("the stub to start", || token_file.exists()).await;
    assert_eq!(std::fs::read_to_string(token_file).unwrap().trim(), "");
    assert_eq!(r.proj_store.len() + r.ctl_store.len(), 0);
    r.plane.instance(method::UNLOAD, &rec.instance_id).await.unwrap();
}

#[tokio::test]
async fn a_malformed_project_id_is_refused_by_the_host() {
    let r = rig(opts(SigningKey::from_bytes(&[14; 32]))).await;
    let report = r.place(Some("not-a-project"), 15009).await;
    assert!(report.placed.is_none(), "{}", report.explain);
    assert!(r.registry.is_empty());
}

#[tokio::test]
async fn the_project_check_runs_even_when_the_bridge_is_down() {
    let ctl = SigningKey::from_bytes(&[21; 32]);
    let mut o = opts(ctl.clone());
    o.disabled = true;
    o.controllers = listed(PROJECT, node(&ctl));
    let r = rig(o).await;
    // A project this controller is not authorised for: refused, not placed
    // degraded with an unverified project id.
    let bad = r.place(Some(OTHER), 15018).await;
    assert!(bad.placed.is_none(), "{}", bad.explain);
    assert_eq!(bad.attempts[0].code.as_deref(), Some("unauthorized"));
    // An authorised project is placed, degraded.
    let ok = r.place(Some(PROJECT), 15019).await;
    assert_eq!(ok.attempts[0].ingest.as_deref(), Some("disabled"), "{}", ok.explain);
    r.plane.instance(method::UNLOAD, &ok.placed.unwrap().instance_id).await.unwrap();
}
