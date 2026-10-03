//! The service-stamped `deliver` origin (ADR-103 amendment, ADR-106 5.3).

use clawft_mesh_local::proto::Frame;

const PROTO_ORIGIN: u32 = 2;
use clawft_mesh_local::{node_id_from_pubkey, Principal};
use clawft_types::config::MeshAdmissionMode;
use tokio::sync::mpsc;

use super::*;

struct Tenant {
    reg: Arc<Registration>,
    rx: mpsc::Receiver<Frame>,
    id: String,
}

fn rig(uids: &[u32]) -> (Arc<TenantRouter>, Vec<Tenant>) {
    let registry = Arc::new(Registry::new());
    let policy = PolicyCell::new(Some(uids[0]), MeshAdmissionMode::Observe);
    let router = TenantRouter::new(Arc::clone(&registry), policy, "node-local".into());
    let tenants = uids
        .iter()
        .map(|&uid| {
            let id = node_id_from_pubkey(&[uid as u8; 32]);
            let (reg, rx, _) = Registration::new(
                u64::from(uid),
                Principal::Uid(uid),
                id.clone(),
                [uid as u8; 32],
                uid,
                String::new(),
                vec![],
                0,
            );
            reg.set_accept_from(vec!["*".into()]);
            registry.register(&reg, &[], &[]).unwrap();
            Tenant { reg, rx, id }
        })
        .collect();
    (router, tenants)
}

fn ctx(verified: bool, class: PeerClass) -> PeerCtx {
    PeerCtx { peer_id: "remote".into(), node_verified: verified, class, remote_static: None, src_scope: None }
}

fn msg() -> KernelMessage {
    KernelMessage::text(0, MessageTarget::Topic("t".into()), "hi")
}

fn origin(t: &mut Tenant) -> Option<DeliverOrigin> {
    match t.rx.try_recv() {
        Ok(Frame { msg: Message::Deliver(d), .. }) => d.origin,
        other => panic!("expected a deliver, got {other:?}"),
    }
}

#[tokio::test]
async fn an_admitted_node_is_stamped_with_its_verified_id_and_class() {
    let (r, mut t) = rig(&[501]);
    t[0].reg.set_proto(PROTO_ORIGIN);
    r.deliver(&ctx(true, PeerClass::Node), None, msg()).await.unwrap();
    assert_eq!(
        origin(&mut t[0]),
        Some(DeliverOrigin::AdmittedPeer { node_id: "remote".into(), class: OriginClass::Node })
    );
    r.deliver(&ctx(true, PeerClass::Leaf), None, msg()).await.unwrap();
    assert_eq!(
        origin(&mut t[0]),
        Some(DeliverOrigin::AdmittedPeer { node_id: "remote".into(), class: OriginClass::Leaf })
    );
}

#[tokio::test]
async fn unverified_and_legacy_peers_are_stamped_unadmitted() {
    let (r, mut t) = rig(&[501]);
    t[0].reg.set_proto(PROTO_ORIGIN);
    // Legacy, and a peer claiming class node without admission verifying it.
    r.deliver(&ctx(false, PeerClass::Legacy), None, msg()).await.unwrap();
    assert_eq!(origin(&mut t[0]), Some(DeliverOrigin::Unadmitted));
    r.deliver(&ctx(false, PeerClass::Node), None, msg()).await.unwrap();
    assert_eq!(origin(&mut t[0]), Some(DeliverOrigin::Unadmitted));
}

#[tokio::test]
async fn a_local_tenant_is_stamped_local_never_admitted() {
    let (r, mut t) = rig(&[501, 502]);
    t[1].reg.set_proto(PROTO_ORIGIN);
    let dest = <clawft_mesh_local::WeftAddr as std::str::FromStr>::from_str(&format!("weft://local/{}/_/t", t[1].id)).unwrap();
    r.route_outbound(&t[0].reg, &dest, msg()).await.unwrap();
    assert_eq!(origin(&mut t[1]), Some(DeliverOrigin::LocalTenant));
}

#[tokio::test]
async fn no_stamp_is_written_for_a_connection_on_the_old_protocol() {
    let (r, mut t) = rig(&[501]);
    assert!(t[0].reg.proto() < PROTO_ORIGIN, "registrations default to the oldest protocol");
    r.deliver(&ctx(true, PeerClass::Node), None, msg()).await.unwrap();
    assert_eq!(origin(&mut t[0]), None, "an old daemon reads a missing field as unadmitted");
}

fn remote(topic: &str) -> clawft_mesh_local::WeftAddr {
    <clawft_mesh_local::WeftAddr as std::str::FromStr>::from_str(&format!("weft://{}/_/_/{topic}", "d".repeat(32)))
        .unwrap()
}

#[tokio::test]
async fn only_the_owner_registration_may_send_reserved_topics() {
    let (r, t) = rig(&[501, 502]);
    for topic in ["mesh.cog.checkout", "mesh.artifact.tunnel", "mesh.licence.x"] {
        let e = r.route_outbound(&t[1].reg, &remote(topic), msg()).await.unwrap_err();
        assert!(matches!(e, SendError::Forbidden(_)), "{topic}: {e:?}");
    }
    // The owner is not refused by this rule (it fails later: no mesh runtime here).
    let e = r.route_outbound(&t[0].reg, &remote("mesh.cog.checkout"), msg()).await.unwrap_err();
    assert!(matches!(e, SendError::Failed(_)), "{e:?}");
    // An unrelated topic is open to everyone.
    let e = r.route_outbound(&t[1].reg, &remote("chat"), msg()).await.unwrap_err();
    assert!(matches!(e, SendError::Failed(_)), "{e:?}");
    // Local delivery between tenants obeys the same rule.
    let local = <clawft_mesh_local::WeftAddr as std::str::FromStr>::from_str(&format!("weft://local/{}/_/mesh.cog.checkout", t[0].id)).unwrap();
    assert!(matches!(r.route_outbound(&t[1].reg, &local, msg()).await, Err(SendError::Forbidden(_))));
    assert_eq!(r.counters.reserved_refused.load(Ordering::Relaxed), 4);
}

#[tokio::test]
async fn inbound_reserved_topics_reach_only_the_owner_whatever_the_scope_or_prefixes() {
    let (r, mut t) = rig(&[501, 502, 503]);
    t[0].reg.set_proto(PROTO_ORIGIN);
    t[1].reg.set_proto(PROTO_ORIGIN);
    // Another tenant tries to capture the topics by claiming prefixes.
    for p in ["mesh.", "mesh.artifact.", "mesh.cog.x", "mesh.licence."] {
        assert!(r.registry.add_prefix(&t[1].id, p).is_err(), "{p}");
    }
    assert!(r.registry.add_prefix(&t[1].id, "user/b/").is_ok(), "unrelated prefixes still work");
    let scope = WireScope { user_id: t[1].id.clone(), project_id: None };
    for topic in ["mesh.cog.checkout", "mesh.artifact.tunnel"] {
        let m = KernelMessage::text(0, MessageTarget::Topic(topic.into()), "x");
        r.deliver(&ctx(true, PeerClass::Node), Some(&scope), m.clone()).await.unwrap();
        r.deliver(&ctx(true, PeerClass::Node), None, m.clone()).await.unwrap();
        r.deliver(&ctx(false, PeerClass::Legacy), Some(&scope), m).await.unwrap();
    }
    for _ in 0..6 {
        assert!(matches!(t[0].rx.try_recv(), Ok(Frame { msg: Message::Deliver(_), .. })), "owner gets all six");
    }
    assert!(t[1].rx.try_recv().is_err() && t[2].rx.try_recv().is_err(), "nobody else gets any");
}

fn bare_router() -> (Arc<TenantRouter>, Arc<Registry>) {
    let registry = Arc::new(Registry::new());
    let policy = PolicyCell::new(None, MeshAdmissionMode::Observe);
    (TenantRouter::new(Arc::clone(&registry), policy, "node-local".into()), registry)
}

fn register(registry: &Registry, uid: u32) -> (Arc<Registration>, mpsc::Receiver<Frame>) {
    let (reg, rx, _) = Registration::new(
        u64::from(uid),
        Principal::Uid(uid),
        format!("user-{uid}"),
        [uid as u8; 32],
        uid,
        String::new(),
        vec![],
        0,
    );
    registry.register(&reg, &[], &[]).unwrap();
    (reg, rx)
}

fn reserved_msg() -> KernelMessage {
    KernelMessage::text(0, MessageTarget::Topic("mesh.cog.checkout".into()), "x")
}

#[tokio::test]
async fn with_no_owner_and_no_service_uid_nobody_holds_the_reserved_topics() {
    let (router, registry) = bare_router();
    // The only registration is not trusted for being the only one.
    let (reg, mut rx) = register(&registry, 501);
    router.deliver(&ctx(true, PeerClass::Node), None, reserved_msg()).await.unwrap();
    assert!(rx.try_recv().is_err(), "a lone registration does not become the holder");
    assert_eq!(router.reserved_holder_source(), "none");
    let remote = remote("mesh.cog.checkout");
    assert!(matches!(router.route_outbound(&reg, &remote, reserved_msg()).await, Err(SendError::Forbidden(_))));
    assert_eq!(router.counters.reserved_refused.load(Ordering::Relaxed), 2);
}

#[tokio::test]
async fn a_squatter_registering_first_at_boot_is_not_the_holder() {
    let (router, registry) = bare_router();
    router.set_fallback_uid(501); // the service's own uid
    assert_eq!(router.reserved_holder_source(), "service_uid");
    // Another uid gets in before the service uid's daemon does.
    let (squatter, mut sq_rx) = register(&registry, 777);
    router.deliver(&ctx(true, PeerClass::Node), None, reserved_msg()).await.unwrap();
    assert!(sq_rx.try_recv().is_err(), "the squatter receives nothing reserved");
    assert!(matches!(
        router.route_outbound(&squatter, &remote("mesh.cog.checkout"), reserved_msg()).await,
        Err(SendError::Forbidden(_))
    ));
    // The real holder registers later and gets the topics; a third registration
    // does not disable them (the holder is by uid, not by being alone).
    let (_holder, mut h_rx) = register(&registry, 501);
    let (_third, mut t_rx) = register(&registry, 888);
    router.deliver(&ctx(true, PeerClass::Node), None, reserved_msg()).await.unwrap();
    assert!(h_rx.try_recv().is_ok());
    assert!(sq_rx.try_recv().is_err() && t_rx.try_recv().is_err());
}

#[tokio::test]
async fn a_configured_cluster_owner_wins_over_the_service_uid() {
    let registry = Arc::new(Registry::new());
    let policy = PolicyCell::new(Some(502), MeshAdmissionMode::Observe);
    let router = TenantRouter::new(Arc::clone(&registry), policy, "node-local".into());
    router.set_fallback_uid(501);
    assert_eq!((router.reserved_holder_uid(), router.reserved_holder_source()), (Some(502), "cluster_owner_uid"));
    let (_a, mut a_rx) = register(&registry, 501);
    let (_b, mut b_rx) = register(&registry, 502);
    router.deliver(&ctx(true, PeerClass::Node), None, reserved_msg()).await.unwrap();
    assert!(a_rx.try_recv().is_err() && b_rx.try_recv().is_ok());
}
