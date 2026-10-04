//! ADR-106 phase 1c through the real machine mesh service and the real
//! daemon delivery path: the service stamps each `deliver` with an origin, the
//! daemon's link builds the peer context from it, and the cog mesh router
//! serves artifact sessions and relays checkouts only for a verified node.
//!
//! One machine node hosts every registration here, so a remote admitted peer
//! is injected the way the kernel's mesh listener does it: a verified
//! `PeerCtx` handed to the service's router. Everything after that (the
//! stamp, the unix socket, the client, the sink, the router in front of the
//! daemon) is the production code. The steward's licence is a stub that
//! speaks the signed-request contract.
#![cfg(all(unix, feature = "mesh", feature = "ecc", feature = "exochain"))]

mod mesh_e2e;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use clawft_kernel::error::KernelResult;
use clawft_kernel::gate::{GateBackend, GateDecision};
use clawft_kernel::ipc::{KernelMessage, MessagePayload, MessageTarget};
use clawft_kernel::licence::*;
use clawft_kernel::mesh_admit::PeerClass;
use clawft_kernel::mesh_artifact::{ArtifactExchange, ExchangeConfig};
use clawft_kernel::mesh_artifact_tunnel::PeerSender;
use clawft_kernel::mesh_artifact_types::ArtifactKey;
use clawft_kernel::mesh_artifact_wire::ArtifactMsg;
use clawft_kernel::mesh_cog::{CogMesh, CogMeshDelivery, CogMeshSlot};
use clawft_kernel::mesh_delivery::{LocalDelivery, PeerCtx};
use clawft_kernel::mesh_ipc::Scope;
use clawft_kernel::workload_pkg::codec::hex_encode;
use clawft_kernel::workload_pkg::{KeyOrigin, TrustAnchors};
use clawft_mesh_local::client::{ClientError, MeshLocalClient};
use clawft_mesh_local::peer::InjectedPeer;
use clawft_types::config::MeshServicePolicy;
use ed25519_dalek::SigningKey;
use mesh_e2e::*;
use serde_json::{Value, json};

const PEER: &str = "peer-node-p";
const ARCH: &str = "aarch64";

fn sk(n: u8) -> SigningKey {
    SigningKey::from_bytes(&[n; 32])
}

fn pk(k: &SigningKey) -> String {
    hex_encode(&k.verifying_key().to_bytes())
}

fn bytes() -> Vec<u8> {
    b"cog-binary-for-e2e".to_vec()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64
}

fn now() -> u64 {
    clawft_mesh_local::client::now_unix()
}

// ── stub weft-licence ────────────────────────────────────────────

struct Link(Arc<Licence>);

struct Licence {
    mesh: MeshId,
    node: String,
    checkouts: AtomicU32,
    transfers: AtomicU32,
}

#[async_trait]
impl LicenceTransport for Link {
    async fn call(&self, req: LicenceRequest) -> Result<LicenceResponse, LicenceClientError> {
        let this = &self.0;
        let mut replay = ReplayGuard::default();
        let ok = verify_request(&req, &sk(21).verifying_key().to_bytes(), &this.node, "seed-e2e", now_ms(), &mut replay).is_ok();
        let reply = |status, body: Value| Ok(LicenceResponse { status, body: serde_json::to_vec(&body).unwrap() });
        if !ok {
            return reply(401, json!({"error": "bad_request_signature"}));
        }
        if req.path == CHECKOUT_PATH {
            this.checkouts.fetch_add(1, Ordering::SeqCst);
            let b = bytes();
            let g = CheckoutGrant {
                v: 1,
                grant_id: String::new(),
                mesh_id: this.mesh.to_hex(),
                seed_device_id: "seed-e2e".into(),
                grant_key_id: String::new(),
                source: "cognitum".into(),
                registry: "registry.example".into(),
                cog_id: "fall-detect".into(),
                version: "1.2.0".into(),
                artifacts: vec![GrantArtifact {
                    arch: ARCH.into(),
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
            return reply(200, json!({"grant": sign_grant(&g, &sk(2)).unwrap()}));
        }
        this.transfers.fetch_add(1, Ordering::SeqCst);
        Ok(LicenceResponse { status: 200, body: bytes() })
    }
}

struct PermitAll;

impl GateBackend for PermitAll {
    fn check(&self, _: &str, _: &str, _: &Value) -> GateDecision {
        GateDecision::Permit { token: None }
    }
}

#[derive(Default)]
struct Sent(Mutex<Vec<(String, KernelMessage)>>);

#[async_trait]
impl PeerSender for Sent {
    async fn send_to_node(&self, node: &str, msg: KernelMessage) -> KernelResult<()> {
        self.0.lock().unwrap().push((node.to_owned(), msg));
        Ok(())
    }
}

impl Sent {
    fn to(&self, node: &str, topic: &str) -> Vec<Value> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|(n, m)| n == node && matches!(&m.target, MessageTarget::Topic(t) if t == topic))
            .filter_map(|(_, m)| match &m.payload {
                MessagePayload::Json(v) => Some(v.clone()),
                _ => None,
            })
            .collect()
    }

    fn total(&self) -> usize {
        self.0.lock().unwrap().len()
    }
}

// ── the rig ──────────────────────────────────────────────────────

struct Rig {
    svc: Svc,
    /// The steward's daemon: the cog mesh sits in front of its inbox.
    x: Daemon,
    /// A plain daemon on the same machine, used as a local tenant.
    y: Daemon,
    cog: Arc<CogMesh>,
    ex: Arc<ArtifactExchange>,
    store: Arc<CheckoutGrantStore>,
    sent: Arc<Sent>,
    licence: Arc<Licence>,
    _dir: tempfile::TempDir,
}

async fn rig(x_proto: Option<(u32, u32)>) -> Rig {
    // X is the cluster owner, so it is the default tenant an unadmitted peer reaches.
    let svc = Svc::with_config(Default::default(), |c| c.cluster_owner_uid = Some(OTHER_UID)).await;
    let dir = tempfile::tempdir().unwrap();
    let mesh = MeshId::derive(&[9; 32], &[7; 32]);
    let mut anchors = TrustAnchors::default();
    anchors.push_signer("operator-1", &pk(&sk(1)), KeyOrigin::Operator).unwrap();
    let store = Arc::new(
        CheckoutGrantStore::open(dir.path(), Arc::new(anchors), LocalMeshId::new(mesh), system_clock()).unwrap(),
    );
    let binding = BindingRecord {
        v: 2,
        device_id: "seed-e2e".into(),
        device_pubkey: pk(&sk(20)),
        mesh_id: mesh.to_hex(),
        grant_pubkey: pk(&sk(2)),
        steward_node_id: svc.node_id(),
        steward_pubkey: pk(&sk(21)),
        state: BindState::Bound,
        seq: 1,
        bound_at: now(),
    };
    let posture = AdmissionPosture { enforce: true, verdict_source_bound: true, open_membership: false };
    store.accept_binding(&sign_binding(&binding, &sk(1)).unwrap(), posture, &NoExtraChecks).unwrap();
    let ex = Arc::new(
        ArtifactExchange::new(
            "steward-ex",
            Arc::new(clawft_kernel::artifact_store::ArtifactStore::new_memory()),
            ExchangeConfig {
                redistribution: Arc::new(MeshCheckoutPolicy::new(store.clone())),
                ..ExchangeConfig::default()
            },
        )
        .unwrap(),
    );
    let licence = Arc::new(Licence { mesh, node: svc.node_id(), checkouts: AtomicU32::new(0), transfers: AtomicU32::new(0) });
    let client = SignedLicenceClient::new(sk(21), svc.node_id(), "seed-e2e", Link(licence.clone()), system_clock_ms());
    let relay = Arc::new(CheckoutRelay::new(
        store.clone(),
        ex.clone(),
        client,
        Arc::new(PermitAll),
        Arc::new(NoFlood),
        None,
    ));
    let sent = Arc::new(Sent::default());
    let cog = CogMesh::new(ex.clone(), store.clone(), sent.clone(), Some(relay));
    let slot = Arc::new(CogMeshSlot::default());
    slot.install(cog.clone());

    // X: another uid, accepting local sends from anyone, cog mesh in front.
    svc.next_uid.store(OTHER_UID, Ordering::SeqCst);
    let cfg = svc.mesh_cfg(MeshServicePolicy::Required);
    let mut ep = other_endpoint(&svc, key(2), vec!["*".into()]);
    if let Some(p) = x_proto {
        ep.client.proto = p;
    }
    let wrap: Box<dyn FnOnce(Arc<Inbox>) -> Arc<dyn LocalDelivery>> =
        Box::new(move |inbox| Arc::new(CogMeshDelivery::new(inbox, slot)));
    let x = link_via(&cfg, ep, fast(), Some(wrap)).await;
    svc.next_uid.store(REAL, Ordering::SeqCst);
    let home = tempfile::tempdir().unwrap();
    let y = daemon_as_me(&svc, home.path()).await;
    std::mem::forget(home);
    Rig { svc, x, y, cog, ex, store, sent, licence, _dir: dir }
}

fn json_msg(topic: &str, v: Value) -> KernelMessage {
    KernelMessage::new(0, MessageTarget::Topic(topic.into()), MessagePayload::Json(v))
}

fn checkout_msg(id: &str) -> KernelMessage {
    json_msg(
        "mesh.cog.checkout",
        json!({"request_id": id, "cog_id": "fall-detect", "version": "1.2.0", "arch": ARCH}),
    )
}

fn tunnel_meta_request(sid: &str, hash: [u8; 32]) -> KernelMessage {
    let raw = ArtifactMsg::MetaRequest { key: ArtifactKey::Content(hash) }.to_wire().unwrap();
    json_msg(
        "mesh.artifact.tunnel",
        json!({"v": 1, "sid": sid, "dir": "req", "part": 0, "last": true, "data": hex_encode(&raw)}),
    )
}

fn peer_ctx(verified: bool, class: PeerClass) -> PeerCtx {
    PeerCtx { peer_id: PEER.into(), node_verified: verified, class, remote_static: None, src_scope: None }
}

impl Rig {
    /// What the kernel's mesh listener does with a message from a remote peer.
    async fn deliver_from_remote(&self, ctx: PeerCtx, msg: KernelMessage) {
        let scope = Scope { user_id: self.x.user_id.clone(), project_id: None };
        self.svc.running().state.router.deliver(&ctx, Some(&scope), msg).await.unwrap();
    }

    /// A plain message after which the earlier ones have been through the
    /// link (one registration's deliveries are in order).
    async fn flush(&self, from: PeerCtx) {
        let before = self.x.received().len();
        self.deliver_from_remote(from, KernelMessage::text(0, MessageTarget::Topic("sentinel".into()), "s")).await;
        wait_until("sentinel arrives", || self.x.received().len() > before).await;
    }

    fn seed_and_grant(&self) -> [u8; 32] {
        let b = bytes();
        let hash = *blake3::hash(&b).as_bytes();
        self.ex.seed_bytes(&b).unwrap();
        let g = CheckoutGrant {
            v: 1,
            grant_id: String::new(),
            mesh_id: MeshId::derive(&[9; 32], &[7; 32]).to_hex(),
            seed_device_id: "seed-e2e".into(),
            grant_key_id: String::new(),
            source: "cognitum".into(),
            registry: "registry.example".into(),
            cog_id: "fall-detect".into(),
            version: "1.2.0".into(),
            artifacts: vec![GrantArtifact {
                arch: ARCH.into(),
                size: b.len() as u64,
                sha256: sha256_hex(&b),
                blake3: hex_encode(&hash),
            }],
            manifest_sha256: sha256_hex(b"manifest"),
            licence: LicenceRef { ref_sha256: sha256_hex(b"licence"), expires: now() + 86_400 * 30 },
            seq: 1,
            issued_at: now(),
            expires_at: now() + 72 * 3600,
        };
        let signed = sign_grant(&g, &sk(2)).unwrap();
        install_grant(&self.store, &self.ex, &signed).unwrap();
        hash
    }
}


fn hex_to_bytes(s: &str) -> Vec<u8> {
    (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap()).collect()
}

#[tokio::test]
async fn an_admitted_peer_is_stamped_and_served_through_the_real_service_and_daemon() {
    let r = rig(None).await;
    let hash = r.seed_and_grant();

    // The sink built a verified node context from the service's stamp.
    r.deliver_from_remote(peer_ctx(true, PeerClass::Node), KernelMessage::text(0, MessageTarget::Topic("plain".into()), "hi"))
        .await;
    wait_until("plain message arrives", || !r.x.received().is_empty()).await;
    let got = r.x.received().remove(0);
    assert_eq!((got.peer_id.as_str(), got.verified, got.class), (PEER, true, PeerClass::Node));

    // And the tunnel serves the artifact: the descriptor comes back to the peer.
    r.deliver_from_remote(peer_ctx(true, PeerClass::Node), tunnel_meta_request("s1", hash)).await;
    wait_until("the peer gets a reply", || !r.sent.to(PEER, "mesh.artifact.tunnel").is_empty()).await;
    let first = r.sent.to(PEER, "mesh.artifact.tunnel").remove(0);
    assert_eq!(first["dir"], "rsp");
    let raw = hex_to_bytes(first["data"].as_str().unwrap());
    assert!(matches!(ArtifactMsg::from_wire(&raw).unwrap(), ArtifactMsg::Meta { .. }), "{raw:?}");
    assert_eq!(r.cog.tunnel().counters.served_sessions.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_checkout_from_an_admitted_peer_is_relayed_once_and_answered() {
    let r = rig(None).await;
    r.deliver_from_remote(peer_ctx(true, PeerClass::Node), checkout_msg("c-1")).await;
    wait_until("the reply is sent", || !r.sent.to(PEER, "mesh.cog.checkout.reply").is_empty()).await;
    let reply = r.sent.to(PEER, "mesh.cog.checkout.reply").remove(0);
    assert_eq!(reply["request_id"], "c-1");
    assert!(reply["grant"]["payload"].as_str().unwrap().contains("fall-detect"), "{reply}");
    assert_eq!(r.licence.checkouts.load(Ordering::SeqCst), 1);
    assert_eq!(r.licence.transfers.load(Ordering::SeqCst), 1);
    assert!(r.store.valid_grant_covering(&hex_encode(blake3::hash(&bytes()).as_bytes()), "fall-detect", "1.2.0").is_some());
}

#[tokio::test]
async fn a_local_tenant_is_neither_served_nor_accepted_for_checkout() {
    let r = rig(None).await;
    let hash = r.seed_and_grant();
    // Y, another tenant on the steward's machine, tries the same messages
    // through the service. Reserved topics are the owner daemon's alone, so the
    // service refuses the send before anything is stamped.
    let fwd = r.y.handle.as_ref().unwrap().forwarder.clone();
    let to_x = |topic: &str| {
        <clawft_mesh_local::WeftAddr as std::str::FromStr>::from_str(&format!("weft://local/{}/_/{topic}", r.x.user_id))
            .unwrap()
    };
    assert!(fwd.send_to(to_x("mesh.cog.checkout"), &checkout_msg("t-1")).await.is_err());
    assert!(fwd.send_to(to_x("mesh.artifact.tunnel"), &tunnel_meta_request("t1", hash)).await.is_err());
    fwd.send_to(to_x("plain"), &KernelMessage::text(0, MessageTarget::Topic("plain".into()), "hi")).await.unwrap();
    wait_until("the plain message arrives", || !r.x.received().is_empty()).await;
    let got = r.x.received().remove(0);
    assert!(!got.verified, "a local tenant never becomes a verified peer: {got:?}");
    assert_eq!(got.class, PeerClass::Legacy);

    let st = r.svc.admin(clawft_mesh_local::proto::Message::Status {}).await;
    assert_eq!(st["reserved_topics"]["source"], "cluster_owner_uid");
    assert_eq!(st["reserved_topics"]["holder_uid"], OTHER_UID);
    assert_eq!(r.svc.running().state.router.counters.reserved_refused.load(Ordering::SeqCst), 2);
    assert_eq!(r.cog.counters.refused_unverified.load(Ordering::SeqCst), 0, "they never reached the daemon");
    assert_eq!(r.licence.checkouts.load(Ordering::SeqCst), 0, "no checkout was relayed");
    assert_eq!(r.cog.tunnel().counters.served_sessions.load(Ordering::SeqCst), 0, "nothing was served");
    assert_eq!(r.sent.total(), 0, "nobody was answered");
}

#[tokio::test]
async fn unadmitted_legacy_and_leaf_peers_are_refused_by_the_daemon() {
    let r = rig(None).await;
    let hash = r.seed_and_grant();
    for ctx in [peer_ctx(false, PeerClass::Legacy), peer_ctx(false, PeerClass::Node), peer_ctx(true, PeerClass::Leaf)] {
        r.deliver_from_remote(ctx.clone(), checkout_msg("u-1")).await;
        r.deliver_from_remote(ctx.clone(), tunnel_meta_request("u1", hash)).await;
        r.flush(ctx).await;
    }
    assert_eq!(r.cog.counters.refused_unverified.load(Ordering::SeqCst), 6);
    assert_eq!(r.licence.checkouts.load(Ordering::SeqCst), 0);
    assert_eq!(r.cog.tunnel().counters.served_sessions.load(Ordering::SeqCst), 0);
    assert_eq!(r.sent.total(), 0);
    let kinds: Vec<(bool, PeerClass)> =
        r.x.received().iter().map(|g| (g.verified, g.class)).collect();
    assert_eq!(
        kinds,
        vec![(false, PeerClass::Legacy), (false, PeerClass::Legacy), (false, PeerClass::Leaf)],
        "a leaf is classed but never verified, and the others are unauthenticated"
    );
}

#[tokio::test]
async fn a_daemon_on_the_old_protocol_reads_every_delivery_as_unadmitted() {
    // The client offers only protocol 1, so the service negotiates 1 and writes
    // no origin stamp: even a verified remote node is unadmitted to the daemon.
    let r = rig(Some((1, 1))).await;
    assert_eq!(r.x.state.get().unwrap().proto, Some(1));
    r.deliver_from_remote(peer_ctx(true, PeerClass::Node), checkout_msg("o-1")).await;
    r.flush(peer_ctx(true, PeerClass::Node)).await;
    assert_eq!(r.cog.counters.refused_unverified.load(Ordering::SeqCst), 1);
    assert_eq!(r.licence.checkouts.load(Ordering::SeqCst), 0);
    let g = r.x.received().remove(0);
    assert!(!g.verified, "no stamp, no verification: {g:?}");
}

#[tokio::test]
async fn a_server_that_fails_the_peer_check_never_becomes_a_link_that_could_honour_a_stamp() {
    let svc = Svc::start().await;
    svc.next_uid.store(OTHER_UID, Ordering::SeqCst);
    let mut ep = other_endpoint(&svc, key(2), Vec::new());
    // The process on the socket claims to be neither root nor the service account.
    ep.client.server_peer = Some(Arc::new(InjectedPeer::uid(4242)));
    let err = MeshLocalClient::connect_and_register(&ep.client, &ep.user_key, &ep.register).await;
    svc.next_uid.store(REAL, Ordering::SeqCst);
    assert!(matches!(err, Err(ClientError::ServerUid { .. })), "{:?}", err.err());
    let _ = Duration::from_secs(0);
}

#[tokio::test]
async fn a_tenant_cannot_supply_an_origin_the_service_would_forward() {
    use clawft_mesh_local::proto::{Deliver, DeliverOrigin, ErrorKind, Message, OriginClass, Scope as WireScope};
    let r = rig(None).await;
    // A tenant under a second uid, with its own key, registers and tries to send
    // a `deliver` frame claiming to be an admitted node.
    r.svc.next_uid.store(9002, Ordering::SeqCst);
    let mut ep = other_endpoint(&r.svc, key(3), Vec::new());
    ep.client.own_uid = Some(9002);
    let c = MeshLocalClient::connect_and_register(&ep.client, &ep.user_key, &ep.register).await.unwrap();
    r.svc.next_uid.store(REAL, Ordering::SeqCst);
    let forged = Message::Deliver(Deliver {
        source_node: PEER.into(),
        source_cert: None,
        scope: WireScope { user_id: r.x.user_id.clone(), project_id: None },
        envelope_id: "e".into(),
        message: serde_json::to_value(checkout_msg("f-1")).unwrap(),
        origin: Some(DeliverOrigin::AdmittedPeer { node_id: PEER.into(), class: OriginClass::Node }),
    });
    match c.request(forged).await {
        Err(ClientError::Server(e)) => assert_eq!(e.kind, ErrorKind::Unsupported),
        other => panic!("expected the service to refuse a tenant-sent deliver, got {other:?}"),
    }
    assert!(r.x.received().is_empty());
    assert_eq!(r.licence.checkouts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn on_a_shared_machine_reserved_topics_reach_only_the_owner_daemon() {
    let r = rig(None).await;
    // A remote node scopes its checkout at Y (not the owner): the service
    // routes it to the owner's daemon anyway, and Y sees nothing.
    let scope_y = Scope { user_id: r.y.user_id.clone(), project_id: None };
    r.svc
        .running()
        .state
        .router
        .deliver(&peer_ctx(true, PeerClass::Node), Some(&scope_y), checkout_msg("m-1"))
        .await
        .unwrap();
    wait_until("the owner relays it", || r.licence.checkouts.load(Ordering::SeqCst) == 1).await;
    // A plain message to Y still reaches Y, and only that one.
    r.svc
        .running()
        .state
        .router
        .deliver(
            &peer_ctx(true, PeerClass::Node),
            Some(&scope_y),
            KernelMessage::text(0, MessageTarget::Topic("plain".into()), "hi"),
        )
        .await
        .unwrap();
    wait_until("Y gets the plain one", || !r.y.received().is_empty()).await;
    assert_eq!(r.y.received().len(), 1, "nothing reserved leaked to Y");
    // (Prefix claims on reserved topics are refused by the registry; see
    // the service router tests.)
}

/// The daemon's own wiring (`cog_swarm::{wrap, install, set_forwarder}`, which
/// hold process-wide state, so this is the one test that uses them): the
/// router wrapped around the inbox, the late install of the cog mesh, and the
/// boot re-apply of grants already in the store.
#[tokio::test]
async fn the_daemons_cog_swarm_wiring_serves_after_a_restart_with_stored_grants() {
    use clawft_kernel::mesh_swarm_state::{Audience, ServePeer};
    use clawft_weave::cog_swarm;
    let svc = Svc::with_config(Default::default(), |c| c.cluster_owner_uid = Some(OTHER_UID)).await;
    let dir = tempfile::tempdir().unwrap();
    let mesh = MeshId::derive(&[9; 32], &[7; 32]);
    let mut anchors = TrustAnchors::default();
    anchors.push_signer("operator-1", &pk(&sk(1)), KeyOrigin::Operator).unwrap();
    let store = Arc::new(
        CheckoutGrantStore::open(dir.path(), Arc::new(anchors), LocalMeshId::new(mesh), system_clock()).unwrap(),
    );
    let binding = BindingRecord {
        v: 2,
        device_id: "seed-e2e".into(),
        device_pubkey: pk(&sk(20)),
        mesh_id: mesh.to_hex(),
        grant_pubkey: pk(&sk(2)),
        steward_node_id: svc.node_id(),
        steward_pubkey: pk(&sk(21)),
        state: BindState::Bound,
        seq: 1,
        bound_at: now(),
    };
    let posture = AdmissionPosture { enforce: true, verdict_source_bound: true, open_membership: false };
    store.accept_binding(&sign_binding(&binding, &sk(1)).unwrap(), posture, &NoExtraChecks).unwrap();
    let ex = Arc::new(
        ArtifactExchange::new(
            "restarted-node",
            Arc::new(clawft_kernel::artifact_store::ArtifactStore::new_memory()),
            ExchangeConfig { redistribution: Arc::new(MeshCheckoutPolicy::new(store.clone())), ..ExchangeConfig::default() },
        )
        .unwrap(),
    );
    // A grant that was stored before this "restart", and the bytes it covers.
    let b = bytes();
    let hash = *blake3::hash(&b).as_bytes();
    let d = ex.seed_bytes(&b).unwrap();
    let g = CheckoutGrant {
        v: 1,
        grant_id: String::new(),
        mesh_id: mesh.to_hex(),
        seed_device_id: "seed-e2e".into(),
        grant_key_id: String::new(),
        source: "cognitum".into(),
        registry: "registry.example".into(),
        cog_id: "fall-detect".into(),
        version: "1.2.0".into(),
        artifacts: vec![GrantArtifact {
            arch: ARCH.into(),
            size: b.len() as u64,
            sha256: sha256_hex(&b),
            blake3: hex_encode(&hash),
        }],
        manifest_sha256: sha256_hex(b"manifest"),
        licence: LicenceRef { ref_sha256: sha256_hex(b"licence"), expires: now() + 86_400 * 30 },
        seq: 1,
        issued_at: now(),
        expires_at: now() + 72 * 3600,
    };
    store.accept_grant(&sign_grant(&g, &sk(2)).unwrap()).unwrap();
    let node = ServePeer::verified("peer-node-p");
    assert!(!ex.is_servable_to(&d, &Audience::Serve(&node)), "not shareable before the boot re-apply");

    // The link comes up with the daemon's own wrap; placement installs later.
    svc.next_uid.store(OTHER_UID, Ordering::SeqCst);
    let cfg = svc.mesh_cfg(MeshServicePolicy::Required);
    let ep = other_endpoint(&svc, key(2), Vec::new());
    let wrap: Box<dyn FnOnce(Arc<Inbox>) -> Arc<dyn LocalDelivery>> = Box::new(cog_swarm::wrap_inbox);
    let x = link_via(&cfg, ep, fast(), Some(wrap)).await;
    svc.next_uid.store(REAL, Ordering::SeqCst);
    assert!(cog_swarm::get().is_none(), "nothing installed yet");
    cog_swarm::set_forwarder(x.handle.as_ref().unwrap().forwarder.clone());
    let cog = cog_swarm::install(&ex, &store);
    assert!(ex.is_servable_to(&d, &Audience::Serve(&node)), "the boot re-apply made stored grants shareable");
    assert!(cog_swarm::get().is_some());

    // A verified node's tunnel request now reaches the daemon's serve through
    // the wrapped router; a checkout is answered `no_steward` (no relay yet).
    let scope = Scope { user_id: x.user_id.clone(), project_id: None };
    let router = &svc.running().state.router;
    router.deliver(&peer_ctx(true, PeerClass::Node), Some(&scope), tunnel_meta_request("d1", hash)).await.unwrap();
    router.deliver(&peer_ctx(true, PeerClass::Node), Some(&scope), checkout_msg("d-1")).await.unwrap();
    wait_until("the daemon serves", || cog.tunnel().counters.served_sessions.load(Ordering::SeqCst) == 1).await;
    wait_until("the daemon answers no_steward", || cog.counters.no_steward.load(Ordering::SeqCst) == 1).await;
    // Another feature-less message still reaches the daemon's inbox.
    router
        .deliver(&peer_ctx(true, PeerClass::Node), Some(&scope), KernelMessage::text(0, MessageTarget::Topic("plain".into()), "hi"))
        .await
        .unwrap();
    wait_until("plain reaches the router behind the wrap", || !x.received().is_empty()).await;
}
