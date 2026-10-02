//! Delivery between tenants, inbound mesh traffic, outbound stamping and the
//! verdict round trip, over real sockets.

mod common;

use std::str::FromStr;
use std::time::Duration;

use clawft_kernel::ipc::{KernelMessage, MessageTarget};
use clawft_kernel::mesh::MeshStream;
use clawft_kernel::mesh_ipc::{MeshIpcEnvelope, Scope};
use clawft_mesh_local::client::{MeshLocalClient, RegisterParams};
use clawft_mesh_local::proto::{
    Deliver, ErrorKind, Message, PeerInfo, ProjectBinding, VerdictRequest, VerdictSubject,
};
use clawft_mesh_local::WeftAddr;
use common::*;

const ULID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

fn km(topic: &str) -> KernelMessage {
    KernelMessage::text(0, MessageTarget::Topic(topic.into()), "hello")
}

async fn next_deliver(c: &mut MeshLocalClient) -> Option<Deliver> {
    let f = tokio::time::timeout(Duration::from_millis(1500), c.next_event()).await.ok()??;
    match f.msg {
        Message::Deliver(d) => Some(d),
        _ => None,
    }
}

async fn no_deliver(c: &mut MeshLocalClient) -> bool {
    tokio::time::timeout(Duration::from_millis(300), c.next_event()).await.is_err()
}

async fn peer_stream(h: &Harness) -> Box<dyn MeshStream> {
    let t = clawft_kernel::mesh_serve::transport_for("tcp", None);
    t.connect(&h.svc().mesh_addr.unwrap().to_string()).await.expect("connect to the mesh listener")
}

#[tokio::test]
async fn tenants_on_one_machine_deliver_to_each_other_with_the_senders_cert() {
    let h = Harness::start().await;
    let a = h.connect(None, 1, RegisterParams::default()).await.unwrap();
    let mut b = h.connect(Some(9001), 2, RegisterParams::default()).await.unwrap();
    let dest = WeftAddr::from_str(&format!("weft://local/{}/_/chat", user_id(2))).unwrap();
    let reply = a.send(&dest, serde_json::to_value(km("ignored")).unwrap()).await.unwrap();
    assert!(matches!(reply, Message::Ack {}));
    let d = next_deliver(&mut b).await.expect("B receives it");
    assert_eq!(d.source_node, h.svc().node_id);
    assert_eq!(d.scope.user_id, user_id(2));
    assert_eq!(d.source_cert.expect("sender's cert").user_id, user_id(1));
    let got: KernelMessage = serde_json::from_value(d.message).unwrap();
    assert!(matches!(got.target, MessageTarget::Topic(t) if t == "chat"));
}

#[tokio::test]
async fn a_send_to_an_unregistered_user_or_unknown_peer_is_an_error() {
    let h = Harness::start().await;
    let a = h.connect(None, 1, RegisterParams::default()).await.unwrap();
    let ghost = WeftAddr::from_str(&format!("weft://local/{}/_/t", "e".repeat(32))).unwrap();
    match a.send(&ghost, serde_json::to_value(km("t")).unwrap()).await {
        Err(clawft_mesh_local::client::ClientError::Server(e)) => assert_eq!(e.kind, ErrorKind::UnknownScope),
        other => panic!("{other:?}"),
    }
    let far = WeftAddr::from_str(&format!("weft://{}/_/_/t", "d".repeat(32))).unwrap();
    match a.send(&far, serde_json::to_value(km("t")).unwrap()).await {
        Err(clawft_mesh_local::client::ClientError::Server(e)) => assert_eq!(e.kind, ErrorKind::PeerUnreachable),
        other => panic!("{other:?}"),
    }
    match a.request(Message::Send { dest: "not an address".into(), message: serde_json::json!({}), request_id: None }).await {
        Err(clawft_mesh_local::client::ClientError::Server(e)) => assert_eq!(e.kind, ErrorKind::BadRequest),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn an_unadmitted_mesh_peer_reaches_only_the_default_tenant() {
    let h = Harness::start().await;
    // A registers first, so A is the cluster owner and the default tenant.
    let mut a = h.connect(None, 1, RegisterParams::default()).await.unwrap();
    let mut b = h
        .connect(
            Some(9001),
            2,
            RegisterParams { topic_prefixes: vec!["b/".into()], ..Default::default() },
        )
        .await
        .unwrap();
    let node = h.svc().node_id.clone();
    let mut peer = peer_stream(&h).await;

    // No scope: the default tenant, even for a topic inside B's prefix.
    let env = MeshIpcEnvelope::new("c".repeat(32), node.clone(), km("b/secret"));
    peer.send(&env.to_bytes().unwrap()).await.unwrap();
    let d = next_deliver(&mut a).await.expect("A is the default tenant");
    assert_eq!(d.source_node, "c".repeat(32));
    assert!(d.source_cert.is_none());
    assert!(no_deliver(&mut b).await);

    // A scope naming B is only a claim from an unadmitted peer: dropped, not rerouted.
    let mut env = MeshIpcEnvelope::new("c".repeat(32), node.clone(), km("t"));
    env.dest_scope = Some(Scope { user_id: user_id(2), project_id: None });
    peer.send(&env.to_bytes().unwrap()).await.unwrap();
    assert!(no_deliver(&mut b).await, "B must not receive it");
    assert!(no_deliver(&mut a).await, "and it is not rerouted to A");
    let status = h.admin_ok(Message::Status {}).await;
    assert_eq!(status["router"]["denied_scope"], 1);

    // Naming the default tenant is fine.
    let mut env = MeshIpcEnvelope::new("c".repeat(32), node, km("t"));
    env.dest_scope = Some(Scope { user_id: user_id(1), project_id: None });
    peer.send(&env.to_bytes().unwrap()).await.unwrap();
    assert!(next_deliver(&mut a).await.is_some());
}

#[tokio::test]
async fn outbound_envelopes_carry_the_machine_node_and_a_stamped_src_scope() {
    let h = Harness::start().await;
    let mut a = h.connect(None, 1, RegisterParams::default()).await.unwrap();
    let node = h.svc().node_id.clone();
    let peer_id = "c".repeat(32);
    let mut peer = peer_stream(&h).await;
    // The peer introduces itself with a first envelope, which also registers its route.
    let hi = MeshIpcEnvelope::new(peer_id.clone(), node.clone(), km("hi"));
    peer.send(&hi.to_bytes().unwrap()).await.unwrap();
    assert!(next_deliver(&mut a).await.is_some());

    let dest = WeftAddr::from_str(&format!("weft://{peer_id}/{}/_/topic.x", user_id(2))).unwrap();
    // The daemon puts whatever it likes in the message about who it is.
    let mut m = km("whatever");
    m.from = 4242;
    assert!(matches!(a.send(&dest, serde_json::to_value(m).unwrap()).await.unwrap(), Message::Ack {}));
    let bytes = tokio::time::timeout(Duration::from_secs(2), peer.recv()).await.unwrap().unwrap();
    let env = MeshIpcEnvelope::from_bytes(&bytes).unwrap();
    assert_eq!(env.source_node, node, "source_node is the machine node id");
    assert_eq!(env.dest_node, peer_id);
    assert_eq!(env.src_scope.unwrap().user_id, user_id(1), "src_scope is stamped from the registration");
    assert_eq!(env.dest_scope.unwrap().user_id, user_id(2));
    assert!(matches!(env.message.target, MessageTarget::Topic(t) if t == "topic.x"));
}

#[tokio::test]
async fn prefix_and_project_claims_conflict_across_users() {
    let h = Harness::start().await;
    let project = |seed: u8| ProjectBinding { project_id: ULID.into(), project_pubkey: [seed; 32], cert_sig: [0; 64] };
    let a = h
        .connect(
            None,
            1,
            RegisterParams { projects: vec![project(1)], topic_prefixes: vec!["shared/".into()], ..Default::default() },
        )
        .await
        .unwrap();
    assert_eq!(a.register_ack().accepted.addresses, vec![ULID.to_string()]);
    let b = h
        .connect(
            Some(9001),
            2,
            RegisterParams { projects: vec![project(2)], topic_prefixes: vec!["shared/x".into(), "mine/".into()], ..Default::default() },
        )
        .await
        .unwrap();
    let ack = b.register_ack();
    assert!(ack.accepted.addresses.is_empty(), "a project another user owns is refused");
    assert_eq!(ack.accepted.topic_prefixes, vec!["mine/".to_string()], "an overlapping prefix is refused");
    assert_eq!(ack.rejected.len(), 2);
}

#[tokio::test]
async fn the_verdict_round_trip_reaches_the_cluster_owner_and_only_the_owner_can_answer() {
    let h = Harness::start().await;
    let mut owner = h.connect(None, 1, RegisterParams::default()).await.unwrap();
    let mut other = h.connect(Some(9001), 2, RegisterParams::default()).await.unwrap();
    let broker = h.svc().state.verdicts.clone();
    let req = VerdictRequest {
        subject: VerdictSubject::PeerAdmit,
        peer: PeerInfo {
            node_id: "n1".into(),
            pubkey: clawft_mesh_local::hexser::encode(&[3; 32]),
            platform: "linux".into(),
            capabilities: vec![],
            genesis_hash: String::new(),
            chain_seq: 0,
        },
        topic: None,
    };
    let ask = tokio::spawn({
        let req = req.clone();
        async move { broker.ask(req).await }
    });
    // Only the cluster owner (the first bound uid) is asked.
    let f = tokio::time::timeout(Duration::from_secs(2), owner.next_event()).await.unwrap().unwrap();
    let id = f.id.expect("service-originated id");
    assert!(id & clawft_mesh_local::proto::SERVICE_ID_FLAG != 0);
    assert!(matches!(f.msg, Message::VerdictRequest(_)));
    assert!(no_deliver(&mut other).await, "the other daemon is never asked");
    // The other daemon cannot answer for the owner.
    other
        .reply(id, Message::VerdictReply { allow: false, ttl_s: 60, reason: "forged".into(), rule_hash: String::new() })
        .await
        .unwrap();
    owner
        .reply(id, Message::VerdictReply { allow: true, ttl_s: 60, reason: String::new(), rule_hash: "r1".into() })
        .await
        .unwrap();
    let d = ask.await.unwrap();
    assert_eq!(d, clawft_mesh_service::verdicts::Decision::Allow { rule_hash: "r1".into(), stale: false });
}

#[tokio::test]
async fn verdicts_fail_closed_when_the_owner_is_gone() {
    let h = Harness::with(|c, _| c.verdict_timeout_s = 1).await;
    let broker = h.svc().state.verdicts.clone();
    let d = broker
        .ask(VerdictRequest {
            subject: VerdictSubject::Publish,
            peer: PeerInfo {
                node_id: "n1".into(),
                pubkey: clawft_mesh_local::hexser::encode(&[3; 32]),
                platform: String::new(),
                capabilities: vec![],
                genesis_hash: String::new(),
                chain_seq: 0,
            },
            topic: Some("t".into()),
        })
        .await;
    assert!(matches!(d, clawft_mesh_service::verdicts::Decision::Unavailable(_)));
}

