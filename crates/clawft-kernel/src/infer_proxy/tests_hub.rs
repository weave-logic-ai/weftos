//! The hub over two real [`MeshRuntime`]s joined by in-process channels.
//! Each side's inbound frames are fed to the runtime with the
//! [`PeerCtx`] the test chooses, standing in for the connection's
//! admission result.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;

use super::hub::InferHub;
use super::listener::{InferProxy, OccupiedPolicy, Started};
use super::mesh_forward::ServeGate;
use super::support::*;
use super::table::PlacementTable;
use super::types::*;
use super::upstream::Upstream;
use crate::mesh_admit::PeerClass;
use crate::mesh_delivery::PeerCtx;
use crate::mesh_runtime::MeshRuntime;

struct Side {
    rt: Arc<MeshRuntime>,
    hub: Arc<InferHub>,
    table: Arc<PlacementTable>,
}

fn ctx(peer: &str, verified: bool, class: PeerClass) -> PeerCtx {
    PeerCtx {
        peer_id: peer.into(),
        node_verified: verified,
        class,
        remote_static: None,
        src_scope: None,
    }
}

fn side(node: &str, audit: Arc<Audit>) -> Side {
    let rt = Arc::new(MeshRuntime::new(node.into()));
    let up = Arc::new(Upstream::new(small_limits()).unwrap());
    let hub = InferHub::new(rt.clone(), up, Arc::new(ServeGate::new(4, 16)), Some(audit.clone()));
    let table = Arc::new(PlacementTable::new(node, Some(hub.clone() as Arc<dyn MeshDialer>), Some(audit)));
    hub.attach(table.clone());
    hub.install();
    Side { rt, hub, table }
}

/// Join `a` and `b`; frames from each arrive at the other with `ctx_of_a`
/// (how B sees A) and `ctx_of_b` (how A sees B).
fn join(a: &Side, b: &Side, ctx_of_a: PeerCtx, ctx_of_b: PeerCtx) {
    let (a2b_tx, mut a2b_rx) = mpsc::channel::<Vec<u8>>(256);
    let (b2a_tx, mut b2a_rx) = mpsc::channel::<Vec<u8>>(256);
    a.rt.add_peer(ctx_of_b.peer_id.clone(), a2b_tx.clone());
    b.rt.add_peer(ctx_of_a.peer_id.clone(), b2a_tx.clone());
    let (rb, ra) = (b.rt.clone(), a.rt.clone());
    let (b2a, a2b) = (b2a_tx, a2b_tx);
    tokio::spawn(async move {
        while let Some(bytes) = a2b_rx.recv().await {
            let _ = rb.handle_incoming_peer(&bytes, b2a.clone(), Some(&ctx_of_a)).await;
        }
    });
    tokio::spawn(async move {
        while let Some(bytes) = b2a_rx.recv().await {
            let _ = ra.handle_incoming_peer(&bytes, a2b.clone(), Some(&ctx_of_b)).await;
        }
    });
}

async fn proxy(t: &Arc<PlacementTable>) -> InferProxy {
    match InferProxy::start("hermes", "127.0.0.1:0".parse().unwrap(), OccupiedPolicy::Refuse, t.clone(), small_limits(), None)
        .await
        .unwrap()
    {
        Started::Running(p) => p,
        _ => panic!(),
    }
}

async fn meet(w: &World) {
    w.a.hub.announce(0).await;
    w.b.hub.announce(0).await;
    settle().await;
}

async fn settle() {
    tokio::time::sleep(Duration::from_millis(150)).await;
}

struct World {
    a: Side,
    b: Side,
    up: Fake,
    audit_a: Arc<Audit>,
    audit_b: Arc<Audit>,
}

async fn world(ctx_of_a: PeerCtx, ctx_of_b: PeerCtx) -> World {
    let (audit_a, audit_b) = (Arc::new(Audit::default()), Arc::new(Audit::default()));
    let (a, b) = (side("node-a", audit_a.clone()), side("node-b", audit_b.clone()));
    let up = fake(Reply::ok(r#"{"from":"b"}"#)).await;
    b.table.register_local("hermes", &up.base(), Some("m".into()), "openai-v1", "LlamaCpp").unwrap();
    b.table.expose_to_mesh("hermes", true);
    b.table.allow_mesh_peer("hermes", "node-a", true);
    a.table.allow_remote_node("hermes", "node-b", true);
    join(&a, &b, ctx_of_a, ctx_of_b);
    World { a, b, up, audit_a, audit_b }
}

fn node(id: &str) -> PeerCtx {
    ctx(id, true, PeerClass::Node)
}

#[tokio::test]
async fn an_advert_and_a_request_cross_two_runtimes() {
    let w = world(node("node-a"), node("node-b")).await;
    // Each side learns the other's standing, then B announces.
    meet(&w).await;
    w.b.hub.announce(10).await;
    settle().await;
    assert!(matches!(w.a.table.resolve("hermes"), Some(Target::Remote { ref node_id }) if node_id == "node-b"));
    // The request goes A's proxy -> hub -> mesh -> B's hub -> B's fake.
    let p = proxy(&w.a.table).await;
    let resp = raw(p.addr(), &post(p.addr(), "/v1/chat/completions", r#"{"messages":[],"adapters":"x"}"#)).await;
    assert_eq!(status(&resp), 200, "{resp}");
    assert_eq!(body(&resp), r#"{"from":"b"}"#);
    let sent: serde_json::Value = serde_json::from_slice(&w.up.last().body).unwrap();
    assert_eq!(sent["model"], "m");
    assert!(sent.get("adapters").is_none());
    assert_eq!(w.a.hub.qualifying_peers(), ["node-b"]);
}

#[tokio::test]
async fn a_large_streamed_response_survives_the_control_channel() {
    let (audit_a, audit_b) = (Arc::new(Audit::default()), Arc::new(Audit::default()));
    let (a, b) = (side("node-a", audit_a), side("node-b", audit_b));
    let mut r = Reply::ok("");
    let big: Vec<u8> = (0..150_000u32).map(|i| b'a' + (i % 26) as u8).collect();
    r.chunks = vec![big.clone()];
    let up = fake(r).await;
    b.table.register_local("hermes", &up.base(), None, "openai-v1", "LlamaCpp").unwrap();
    b.table.expose_to_mesh("hermes", true);
    b.table.allow_mesh_peer("hermes", "node-a", true);
    a.table.allow_remote_node("hermes", "node-b", true);
    join(&a, &b, node("node-a"), node("node-b"));
    a.hub.announce(0).await;
    b.hub.announce(0).await;
    settle().await;
    b.hub.announce(10).await;
    settle().await;
    let p = proxy(&a.table).await;
    let resp = raw(p.addr(), &get(p.addr(), "/v1/models")).await;
    assert_eq!(body(&resp).as_bytes(), big.as_slice());
}

#[tokio::test]
async fn adverts_are_only_sent_to_listed_qualifying_peers() {
    let w = world(node("node-a"), node("node-b")).await;
    meet(&w).await;
    w.b.table.allow_mesh_peer("hermes", "node-a", false);
    w.b.hub.announce(11).await;
    settle().await;
    assert_eq!(w.a.table.resolve("hermes"), None, "B does not announce to a peer it does not serve");
}

#[tokio::test]
async fn the_serving_hub_refuses_peers_that_do_not_qualify() {
    for (name, c) in [
        ("unverified", ctx("node-a", false, PeerClass::Node)),
        ("leaf", ctx("node-a", true, PeerClass::Leaf)),
        ("legacy", ctx("node-a", true, PeerClass::Legacy)),
    ] {
        let w = world(c, node("node-b")).await;
        // A (consumer) believes B qualifies and has an advert for it.
        meet(&w).await;
        let ad = w.b.table.advertisement("hermes", 10).unwrap();
        w.a.hub.attach(w.a.table.clone());
        w.a.table.ingest_advertisement("node-b", &ad);
        let p = proxy(&w.a.table).await;
        let resp = raw(p.addr(), &get(p.addr(), "/v1/models")).await;
        assert_eq!(status(&resp), 502, "{name}: {resp}");
        assert_eq!(w.up.count(), 0, "{name}: the model server was reached");
        let _ = (&w.audit_a, &w.audit_b);
    }
}

#[tokio::test]
async fn an_advert_for_another_node_is_not_taken_from_a_peers_connection() {
    let w = world(node("node-a"), node("node-b")).await;
    w.b.table.expose_to_mesh("hermes", true);
    let mut ad = w.b.table.advertisement("hermes", 10).unwrap();
    ad.node_id = "node-x".into();
    w.a.table.allow_remote_node("hermes", "node-x", true);
    // B sends an advert whose payload names node-x; the sender is B.
    let m = crate::ipc::KernelMessage::new(
        0,
        crate::ipc::MessageTarget::Topic(crate::mesh_runtime::INFER_TOPIC.into()),
        crate::ipc::MessagePayload::Json(serde_json::json!({"t": "advert", "ad": ad})),
    );
    w.b.rt.route_to_remote("node-a", m).await.unwrap();
    settle().await;
    assert_eq!(w.a.table.resolve("hermes"), None);
    assert!(w.audit_a.kinds().contains(&"infer.advert.refused".to_string()));
}

#[tokio::test]
async fn a_response_is_accepted_only_from_the_peer_the_request_went_to() {
    use crate::mesh_runtime::PeerControlSink;
    let w = world(node("node-a"), node("node-b")).await;
    meet(&w).await;
    let d = w.a.hub.clone();
    let stream = d.dial_for_test("node-b").await;
    // A different peer answering id 1 reaches nothing.
    let forged = serde_json::json!({"t": "resp", "id": 1, "data": "0200"});
    d.on_peer_control(&node("node-evil"), 0, &forged);
    assert_eq!(stream.pending_len(), 1, "exchange untouched by a stranger");
    d.on_peer_control(&node("node-b"), 0, &forged);
    drop(stream);
}
