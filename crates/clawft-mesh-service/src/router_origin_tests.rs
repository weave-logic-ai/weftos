//! The service-stamped `deliver` origin (ADR-103 amendment, ADR-106 5.3).

use clawft_mesh_local::proto::{Frame, PROTO_ORIGIN};
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
