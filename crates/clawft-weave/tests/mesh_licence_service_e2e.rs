//! ADR-106 phase 3: the licence floods and catch-up sync in service mode,
//! through real machine mesh services and the real daemon link.
//!
//! Each node is a real `clawft-mesh-service` on tempdirs with its cluster
//! owner's daemon linked through the production glue (`mesh_local_glue`), the
//! cog mesh router in front of its inbox with the node's licence links
//! (`ServiceLicenceLinks` over `cog_swarm::ForwarderLink`), and a
//! `LicenceExchange` over those links. Services are joined in process the way
//! the kernel's mesh listener joins them after admission: a verified route in
//! each runtime and the envelopes handed over with the admitted peer's
//! context. From there it is the production path: the runtime's control
//! sink, the owner-only reserved-topic routing, the origin stamp, the unix
//! socket, the client, the sink and the router.
#![cfg(all(unix, feature = "mesh", feature = "ecc", feature = "exochain"))]

mod mesh_e2e;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use clawft_kernel::ipc::MessageTarget;
use clawft_kernel::licence::*;
use clawft_kernel::mesh_admit::PeerClass;
use clawft_kernel::mesh_cog::{CogMeshDelivery, CogMeshSlot};
use clawft_kernel::mesh_delivery::{LocalDelivery, PeerCtx};
use clawft_kernel::mesh_ipc::MeshIpcEnvelope;
use clawft_kernel::mesh_runtime::{RouteTally, COG_BINDING_TOPIC};
use clawft_kernel::workload_pkg::codec::hex_encode;
use clawft_kernel::workload_pkg::{KeyOrigin, TrustAnchors};
use clawft_types::config::MeshServicePolicy;
use clawft_weave::cog_swarm::ForwarderLink;
use ed25519_dalek::SigningKey;
use mesh_e2e::*;

fn sk(n: u8) -> SigningKey {
    SigningKey::from_bytes(&[n; 32])
}

fn pk(k: &SigningKey) -> String {
    hex_encode(&k.verifying_key().to_bytes())
}

fn now() -> u64 {
    clawft_mesh_local::client::now_unix()
}

fn mesh() -> MeshId {
    MeshId::derive(&[9; 32], &[7; 32])
}

fn anchors() -> Arc<TrustAnchors> {
    let mut a = TrustAnchors::default();
    a.push_signer("operator-1", &pk(&sk(1)), KeyOrigin::Operator).unwrap();
    Arc::new(a)
}

fn binding(steward: &str) -> SignedBinding {
    let rec = BindingRecord {
        v: 2,
        device_id: "seed-e2e".into(),
        device_pubkey: pk(&sk(20)),
        mesh_id: mesh().to_hex(),
        grant_pubkey: pk(&sk(2)),
        steward_node_id: steward.into(),
        steward_pubkey: pk(&sk(21)),
        state: BindState::Bound,
        seq: 1,
        bound_at: now(),
    };
    sign_binding(&rec, &sk(1)).unwrap()
}

fn grant() -> SignedGrant {
    let b = b"cog-binary-for-e2e".to_vec();
    let g = CheckoutGrant {
        v: 1,
        grant_id: String::new(),
        mesh_id: mesh().to_hex(),
        seed_device_id: "seed-e2e".into(),
        grant_key_id: String::new(),
        source: "cognitum".into(),
        registry: "registry.example".into(),
        cog_id: "fall-detect".into(),
        version: "1.2.0".into(),
        artifacts: vec![GrantArtifact {
            arch: "aarch64".into(),
            size: b.len() as u64,
            sha256: sha256_hex(&b),
            blake3: hex_encode(blake3::hash(&b).as_bytes()),
        }],
        manifest_sha256: sha256_hex(b"manifest"),
        licence: LicenceRef { ref_sha256: sha256_hex(b"licence"), expires: now() + 86_400 * 30 },
        seq: 1,
        issued_at: now(),
        expires_at: now() + 72 * 3600,
    };
    sign_grant(&g, &sk(2)).unwrap()
}

fn approval() -> SignedApproval {
    let a = Approval {
        v: 1,
        mesh_id: mesh().to_hex(),
        cog_id: "fall-detect".into(),
        version: "1.2.0".into(),
        sha256: vec![sha256_hex(b"cog-binary-for-e2e")],
        approved_at: now(),
    };
    sign_approval(&a, &sk(1)).unwrap()
}

/// One machine: its service, its owner's daemon and that daemon's licence state.
struct Node {
    svc: Svc,
    owner: Daemon,
    links: Arc<ServiceLicenceLinks>,
    store: Arc<CheckoutGrantStore>,
    approvals: Arc<ApprovalStore>,
    ex: Arc<LicenceExchange>,
    _dir: tempfile::TempDir,
}

async fn node() -> Node {
    let svc = Svc::with_config(Default::default(), |c| c.cluster_owner_uid = Some(OTHER_UID)).await;
    let dir = tempfile::tempdir().unwrap();
    let local = LocalMeshId::new(mesh());
    let store = Arc::new(CheckoutGrantStore::open(dir.path(), anchors(), local.clone(), system_clock()).unwrap());
    let approvals = Arc::new(ApprovalStore::open(dir.path(), anchors(), local).unwrap());
    // The daemon's own pieces: the late-bound link, the licence links, the wrap.
    let fl = Arc::new(ForwarderLink::default());
    let links = ServiceLicenceLinks::new(fl.clone(), fl.clone());
    let l2 = links.clone();
    let wrap: Box<dyn FnOnce(Arc<Inbox>) -> Arc<dyn LocalDelivery>> = Box::new(move |inbox| {
        Arc::new(CogMeshDelivery::new(inbox, Arc::new(CogMeshSlot::default())).with_licence(l2))
    });
    svc.next_uid.store(OTHER_UID, Ordering::SeqCst);
    let cfg = svc.mesh_cfg(MeshServicePolicy::Required);
    let owner = link_via(&cfg, other_endpoint(&svc, key(2), vec!["*".into()]), fast(), Some(wrap)).await;
    svc.next_uid.store(REAL, Ordering::SeqCst);
    fl.bind(owner.handle.as_ref().unwrap().forwarder.clone());
    let posture = AdmissionPosture { enforce: true, verdict_source_bound: true, open_membership: false };
    let ex = LicenceExchange::start(LicenceExchangeParts {
        store: store.clone(),
        approvals: approvals.clone(),
        anchors: anchors(),
        runtime: links.clone(),
        posture: Arc::new(move || posture),
        admission: Arc::new(CtxAdmission),
        sink: Arc::new(NoopSink),
        config: LicenceExchangeConfig::default(),
    });
    Node { svc, owner, links, store, approvals, ex, _dir: dir }
}

fn admitted(peer: &str, class: PeerClass) -> PeerCtx {
    PeerCtx { peer_id: peer.into(), node_verified: true, class, remote_static: None, src_scope: None }
}

/// Join two services as the kernel listener does after admitting each other:
/// a verified `node` route both ways, frames handed over with that context.
fn wire(a: &Node, b: &Node) {
    let (ra, rb) = (a.svc.running().runtime().clone(), b.svc.running().runtime().clone());
    let (ida, idb) = (a.svc.node_id(), b.svc.node_id());
    let (tx_ab, mut rx_ab) = tokio::sync::mpsc::channel::<Vec<u8>>(1024);
    let (tx_ba, mut rx_ba) = tokio::sync::mpsc::channel::<Vec<u8>>(1024);
    let tally = RouteTally::default();
    assert!(ra.register_authenticated_as(idb.clone(), tx_ab.clone(), true, PeerClass::Node, &tally));
    assert!(rb.register_authenticated_as(ida.clone(), tx_ba.clone(), true, PeerClass::Node, &tally));
    let (rb2, ctx_a, back_a) = (rb.clone(), admitted(&ida, PeerClass::Node), tx_ba.clone());
    tokio::spawn(async move {
        while let Some(bytes) = rx_ab.recv().await {
            let _ = rb2.handle_incoming_peer(&bytes, back_a.clone(), Some(&ctx_a)).await;
        }
    });
    let (ra2, ctx_b, back_b) = (ra.clone(), admitted(&idb, PeerClass::Node), tx_ab.clone());
    tokio::spawn(async move {
        while let Some(bytes) = rx_ba.recv().await {
            let _ = ra2.handle_incoming_peer(&bytes, back_b.clone(), Some(&ctx_b)).await;
        }
    });
}

/// An admitted leaf on `n`'s service; counts what is sent to it by topic.
fn leaf(n: &Node) -> Arc<AtomicUsize> {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
    let tally = RouteTally::default();
    assert!(n.svc.running().runtime().register_authenticated_as(leaf_id(), tx, true, PeerClass::Leaf, &tally));
    let got = Arc::new(AtomicUsize::new(0));
    let g = got.clone();
    tokio::spawn(async move {
        while let Some(bytes) = rx.recv().await {
            if let Ok(env) = MeshIpcEnvelope::from_bytes(&bytes)
                && matches!(&env.message.target, MessageTarget::Topic(t) if t.starts_with("mesh.cog."))
            {
                g.fetch_add(1, Ordering::SeqCst);
            }
        }
    });
    got
}

fn leaf_id() -> String {
    clawft_kernel::node_id_from_pubkey(&[0x71; 32])
}

fn is_cog(m: &clawft_kernel::ipc::KernelMessage) -> bool {
    matches!(&m.target, MessageTarget::Topic(t) if t.starts_with("mesh.cog."))
}

fn bound(n: &Node) -> bool {
    n.store.active_binding().is_some()
}

fn has_grant(n: &Node) -> bool {
    n.store.held_grant("fall-detect", "1.2.0").is_some()
}

#[tokio::test]
async fn a_binding_and_a_grant_flood_reach_a_second_service_mode_node_and_no_leaf_or_other_tenant() {
    let (a, b) = (node().await, node().await);
    wire(&a, &b);
    let leaf_got = leaf(&a);
    // A non-owner tenant on B's machine (the current uid; the owner is OTHER_UID).
    let home = tempfile::tempdir().unwrap();
    let tenant = daemon_as_me(&b.svc, home.path()).await;

    // Each daemon reads its service's peer view: B is licensed on A, the leaf is not.
    a.links.refresh().await.unwrap();
    b.links.refresh().await.unwrap();
    assert!(a.links.peer_licensed(&b.svc.node_id()));
    assert!(!a.links.peer_licensed(&leaf_id()), "connected but not a licensed node");
    assert!(a.links.peer_ids().contains(&leaf_id()));

    assert_eq!(a.ex.issue_binding(binding(&a.svc.node_id())).await, Ok(Receipt::New));
    wait_until("B holds the binding", || bound(&b)).await;
    a.ex.issue_grant(grant()).await.unwrap();
    a.ex.issue_approval(approval()).await.unwrap();
    wait_until("B holds the grant and approval", || has_grant(&b) && b.approvals.len() == 1).await;

    // Through B's service: the forwarding sink, then B's owner daemon only.
    let rb = &b.svc.running().state.router.counters;
    assert!(rb.licence_forwarded.load(Ordering::SeqCst) >= 3);
    assert!(b.links.counters.delivered.load(Ordering::SeqCst) >= 3);
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(leaf_got.load(Ordering::SeqCst), 0, "an admitted leaf gets no licence record");
    // Even if the daemon asked, A's service would not send a licence record to the leaf.
    let to_leaf = clawft_kernel::ipc::KernelMessage::new(
        0,
        MessageTarget::Topic(COG_BINDING_TOPIC.into()),
        clawft_kernel::ipc::MessagePayload::Json(serde_json::to_value(binding("x")).unwrap()),
    );
    assert!(a.links.route_to_remote(&leaf_id(), to_leaf).await.is_err(), "refused at the service");
    assert_eq!(a.svc.running().state.router.counters.licence_out_refused.load(Ordering::SeqCst), 1);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(leaf_got.load(Ordering::SeqCst), 0);
    let leaked: Vec<_> = tenant.received().into_iter().filter(|g| is_cog(&g.msg)).collect();
    assert!(leaked.is_empty(), "a non-owner tenant gets nothing: {leaked:?}");
    assert!(
        b.owner.received().iter().all(|g| !is_cog(&g.msg)),
        "licence topics are taken by the licence links, not passed to the router"
    );
}

#[tokio::test]
async fn a_late_joiner_catches_up_by_sync_through_its_service() {
    let (a, b) = (node().await, node().await);
    wire(&a, &b);
    a.links.refresh().await.unwrap();
    b.links.refresh().await.unwrap();
    a.ex.issue_binding(binding(&a.svc.node_id())).await.unwrap();
    a.ex.issue_grant(grant()).await.unwrap();
    a.ex.issue_approval(approval()).await.unwrap();
    wait_until("B is current", || bound(&b) && has_grant(&b) && b.approvals.len() == 1).await;

    // C joins later, connected to B only. Its daemon sees B become licensed
    // in the service's view and syncs with it at once.
    let c = node().await;
    wire(&b, &c);
    assert!(!bound(&c));
    b.links.refresh().await.unwrap();
    c.links.refresh().await.unwrap();
    wait_until("C caught up from B", || bound(&c) && has_grant(&c) && c.approvals.len() == 1).await;
    assert!(c.svc.running().state.router.counters.licence_forwarded.load(Ordering::SeqCst) >= 1);
}

#[tokio::test]
async fn an_unlicensed_peer_reaches_neither_the_daemon_nor_the_exchange() {
    let b = node().await;
    // A frame on a licence topic from an unadmitted connection and from a
    // verified leaf, straight into B's runtime as the listener would hand it.
    let rt = b.svc.running().runtime().clone();
    let msg = clawft_kernel::ipc::KernelMessage::new(
        0,
        MessageTarget::Topic(COG_BINDING_TOPIC.into()),
        clawft_kernel::ipc::MessagePayload::Json(serde_json::to_value(binding("steward")).unwrap()),
    );
    for (id, verified, class) in [("node-u", false, PeerClass::Legacy), ("leaf-l", true, PeerClass::Leaf)] {
        let bytes = MeshIpcEnvelope::new(id.into(), b.svc.node_id(), msg.clone()).to_bytes().unwrap();
        let (tx, _rx) = tokio::sync::mpsc::channel(4);
        let ctx = PeerCtx { peer_id: id.into(), node_verified: verified, class, remote_static: None, src_scope: None };
        rt.handle_incoming_peer(&bytes, tx, Some(&ctx)).await.unwrap();
    }
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert_eq!(b.svc.running().state.router.counters.licence_unlicensed.load(Ordering::SeqCst), 2);
    assert_eq!(b.links.counters.delivered.load(Ordering::SeqCst), 0);
    assert!(!bound(&b));
}
