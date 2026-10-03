//! P3-K1: identity binding, subscribe authorisation, and peer events in
//! `MeshRuntime`, plus the `LocalDelivery` trust contract.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::*;
use crate::capability::CapabilityChecker;
use crate::error::KernelResult;
use crate::ipc::{KernelMessage, MessagePayload, MessageTarget};
use crate::mesh_admit::PeerClass;
use crate::mesh_delivery::{LocalDelivery, PeerCtx};
use crate::mesh_ipc::Scope;
use crate::process::{ProcessEntry, ProcessState, ProcessTable, ResourceUsage};
use crate::topic::TopicRouter;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct Sink {
    seen: Mutex<Vec<(PeerCtx, Option<Scope>)>>,
    allow_subscribe: bool,
}

#[async_trait]
impl LocalDelivery for Sink {
    async fn deliver(&self, from: &PeerCtx, scope: Option<&Scope>, _m: KernelMessage) -> KernelResult<()> {
        self.seen.lock().unwrap().push((from.clone(), scope.cloned()));
        Ok(())
    }
    async fn authorize_subscribe(&self, _: &PeerCtx, _: &str, _: Option<&Scope>) -> bool {
        self.allow_subscribe
    }
}

fn runtime(allow_subscribe: bool) -> (MeshRuntime, Arc<Sink>) {
    let sink = Arc::new(Sink { allow_subscribe, ..Default::default() });
    let mut rt = MeshRuntime::new("local".into());
    rt.set_local_delivery(sink.clone());
    (rt, sink)
}

fn env(src: &str, topic: &str, payload: MessagePayload) -> Vec<u8> {
    let msg = KernelMessage::new(0, MessageTarget::Topic(topic.into()), payload);
    MeshIpcEnvelope::new(src.into(), "local".into(), msg).to_bytes().unwrap()
}

fn text(src: &str, topic: &str) -> Vec<u8> {
    env(src, topic, MessagePayload::Text("x".into()))
}

fn verified(id: &str) -> PeerCtx {
    PeerCtx { node_verified: true, class: PeerClass::Node, ..PeerCtx::unauthenticated(id) }
}

fn chan() -> (tokio::sync::mpsc::Sender<Vec<u8>>, tokio::sync::mpsc::Receiver<Vec<u8>>) {
    tokio::sync::mpsc::channel(8)
}

#[tokio::test]
async fn verified_peer_cannot_register_or_deliver_under_another_id() {
    let (rt, sink) = runtime(true);
    let (tx, _rx) = chan();
    let r = rt.handle_incoming_peer(&text("victim", "t"), tx, Some(&verified("mallory"))).await;
    assert!(r.is_err());
    assert!(rt.peer_ids().is_empty(), "no route may be created from a spoofed claim");
    assert!(sink.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn verified_peer_registers_under_admitted_id_and_delivery_sees_identity() {
    let (rt, sink) = runtime(true);
    let (tx, _rx) = chan();
    rt.handle_incoming_peer(&text("n1", "t"), tx, Some(&verified("n1"))).await.unwrap();
    assert_eq!(rt.peer_ids(), vec!["n1".to_string()]);
    let seen = sink.seen.lock().unwrap();
    assert!(seen[0].0.node_verified && seen[0].0.peer_id == "n1");
}

#[tokio::test]
async fn unverified_connection_cannot_take_over_a_verified_route() {
    let (rt, _sink) = runtime(true);
    let mut events = rt.subscribe_peer_events();
    let (good_tx, mut good_rx) = chan();
    rt.handle_incoming_peer(&text("n1", "t"), good_tx, Some(&verified("n1"))).await.unwrap();
    assert!(matches!(events.try_recv(), Ok(MeshPeerEvent::Joined { verified: true, .. })));

    let (bad_tx, mut bad_rx) = chan();
    let r = rt.handle_incoming_from(&text("n1", "t"), bad_tx).await;
    assert!(r.is_err());
    assert!(events.try_recv().is_err(), "a refused claim is not a join, recovery or leave");

    let msg = KernelMessage::text(0, MessageTarget::Topic("back".into()), "hi");
    rt.send_to_peer("n1", MeshIpcEnvelope::new("local".into(), "n1".into(), msg)).await.unwrap();
    assert!(good_rx.try_recv().is_ok(), "route still reaches the admitted connection");
    assert!(bad_rx.try_recv().is_err());
}

#[tokio::test]
async fn unauthenticated_peers_keep_the_old_behaviour() {
    let (rt, sink) = runtime(true);
    let (tx, _rx) = chan();
    rt.handle_incoming_from(&text("esp32", "t"), tx).await.unwrap();
    assert_eq!(rt.peer_ids(), vec!["esp32".to_string()]);
    let seen = sink.seen.lock().unwrap();
    assert!(!seen[0].0.node_verified);
    assert_eq!(seen[0].0.peer_id, "esp32");
}

#[tokio::test]
async fn subscribe_registers_under_admitted_id_and_honours_the_veto() {
    let sub = |src: &str| {
        env(src, "mesh.subscribe", MessagePayload::Json(serde_json::json!({"topic": "push.x"})))
    };
    let (rt, _) = runtime(true);
    let (tx, _rx) = chan();
    rt.handle_incoming_peer(&sub("n1"), tx, Some(&verified("n1"))).await.unwrap();
    assert_eq!(rt.peers_for_topic("push.x"), vec!["n1".to_string()]);

    let (rt, _) = runtime(false);
    let (tx, _rx) = chan();
    rt.handle_incoming_peer(&sub("n1"), tx, Some(&verified("n1"))).await.unwrap();
    assert!(rt.peers_for_topic("push.x").is_empty(), "vetoed subscribe must not register");

    // A spoofed subscribe cannot subscribe someone else.
    let (rt, _) = runtime(true);
    let (tx, _rx) = chan();
    assert!(rt.handle_incoming_peer(&sub("victim"), tx, Some(&verified("mallory"))).await.is_err());
    assert!(rt.peers_for_topic("push.x").is_empty());
}

#[tokio::test]
async fn recovered_is_emitted_only_on_a_real_transition() {
    let (rt, _) = runtime(true);
    let mut events = rt.subscribe_peer_events();
    let (tx, _rx) = chan();
    for _ in 0..3 {
        rt.handle_incoming_from(&text("n1", "t"), tx.clone()).await.unwrap();
    }
    assert!(matches!(events.try_recv(), Ok(MeshPeerEvent::Joined { .. })));
    assert!(events.try_recv().is_err(), "same channel must not emit Recovered");

    let (tx2, _rx2) = chan();
    rt.handle_incoming_from(&text("n1", "t"), tx2).await.unwrap();
    assert!(matches!(events.try_recv(), Ok(MeshPeerEvent::Recovered { .. })));
    assert!(events.try_recv().is_err());
}

#[tokio::test]
async fn a2a_router_delivery_ignores_scope() {
    let table = Arc::new(ProcessTable::new(8));
    let pid = table
        .insert(ProcessEntry {
            pid: 0,
            agent_id: "a".into(),
            state: ProcessState::Running,
            capabilities: Default::default(),
            resource_usage: ResourceUsage::default(),
            cancel_token: CancellationToken::new(),
            parent_pid: None,
        })
        .unwrap();
    let checker = Arc::new(CapabilityChecker::new(table.clone()));
    let router = A2ARouter::new(table.clone(), checker, Arc::new(TopicRouter::new(table)));
    let mut inbox = router.create_inbox(pid);
    let scope = Scope { user_id: "a".repeat(32), project_id: None };
    let from = PeerCtx::unauthenticated("p");
    for s in [None, Some(&scope)] {
        let m = KernelMessage::text(pid, MessageTarget::Process(pid), "x");
        router.deliver(&from, s, m).await.unwrap();
        assert!(inbox.try_recv().is_ok(), "scope {s:?} must not change routing");
    }
}

#[test]
fn concurrent_registration_never_lets_unverified_overwrite_verified() {
    for _ in 0..2000 {
        let rt = Arc::new(MeshRuntime::new("local".into()));
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let (vtx, _vrx) = chan();
        let (utx, _urx) = chan();
        let (r1, b1) = (rt.clone(), barrier.clone());
        let v = std::thread::spawn(move || {
            b1.wait();
            r1.register_peer("n".into(), vtx, true, None);
        });
        let (r2, b2) = (rt.clone(), barrier);
        let u = std::thread::spawn(move || {
            b2.wait();
            r2.register_peer("n".into(), utx, false, None);
        });
        v.join().unwrap();
        u.join().unwrap();
        assert!(rt.peers.get("n").unwrap().verified, "verified route was overwritten");
    }
}

#[test]
fn disconnect_channel_only_removes_routes_still_on_that_channel() {
    let rt = MeshRuntime::new("local".into());
    let (old_tx, _o) = chan();
    let (new_tx, _n) = chan();
    rt.register_peer("n".into(), old_tx.clone(), true, None);
    rt.register_peer("n".into(), new_tx.clone(), true, None);
    assert_eq!(rt.disconnect_channel(&old_tx), 0, "route already moved to a newer connection");
    assert_eq!(rt.peer_ids(), vec!["n".to_string()]);
    assert_eq!(rt.disconnect_channel(&new_tx), 1);
    assert!(rt.peer_ids().is_empty());
}

#[tokio::test]
async fn unadmitted_leaf_keeps_its_subscription_across_a_reconnect() {
    let (rt, _) = runtime(true);
    let sub = env("leaf", "mesh.subscribe", MessagePayload::Json(serde_json::json!({"topic": "push.leaf"})));
    let (tx1, _rx1) = chan();
    rt.handle_incoming_from(&sub, tx1.clone()).await.unwrap();
    assert_eq!(rt.peers_for_topic("push.leaf"), vec!["leaf".to_string()]);

    assert_eq!(rt.disconnect_channel(&tx1), 1);
    assert!(rt.peer_ids().is_empty());

    // Reconnect on a fresh channel without re-subscribing.
    let (tx2, mut rx2) = chan();
    rt.handle_incoming_from(&text("leaf", "t"), tx2).await.unwrap();
    assert_eq!(rt.peers_for_topic("push.leaf"), vec!["leaf".to_string()]);
    let msg = KernelMessage::text(0, MessageTarget::Topic("push.leaf".into()), "p");
    rt.send_to_peer("leaf", MeshIpcEnvelope::new("local".into(), "leaf".into(), msg)).await.unwrap();
    assert!(rx2.try_recv().is_ok());
}

#[tokio::test]
async fn admitted_route_loses_its_subscriptions_on_close() {
    let (rt, _) = runtime(true);
    let sub = env("n1", "mesh.subscribe", MessagePayload::Json(serde_json::json!({"topic": "push.n1"})));
    let (tx, _rx) = chan();
    rt.handle_incoming_peer(&sub, tx.clone(), Some(&verified("n1"))).await.unwrap();
    assert_eq!(rt.disconnect_channel(&tx), 1);
    assert!(rt.peers_for_topic("push.n1").is_empty());
}

#[tokio::test]
async fn join_and_recover_events_say_whether_the_id_was_verified() {
    let (rt, _) = runtime(true);
    let mut events = rt.subscribe_peer_events();
    let (a, _ra) = chan();
    rt.handle_incoming_peer(&text("v", "t"), a, Some(&verified("v"))).await.unwrap();
    let (b, _rb) = chan();
    rt.handle_incoming_from(&text("legacy", "t"), b).await.unwrap();
    assert!(matches!(events.try_recv(), Ok(MeshPeerEvent::Joined { node_id, verified: true, .. }) if node_id == "v"));
    assert!(matches!(events.try_recv(), Ok(MeshPeerEvent::Joined { node_id, verified: false, .. }) if node_id == "legacy"));
    // A reconnect on a fresh channel keeps the flag of the new connection.
    let (b2, _rb2) = chan();
    rt.handle_incoming_from(&text("legacy", "t"), b2).await.unwrap();
    assert!(matches!(events.try_recv(), Ok(MeshPeerEvent::Recovered { verified: false, .. })));
    let (a2, _ra2) = chan();
    rt.handle_incoming_peer(&text("v", "t"), a2, Some(&verified("v"))).await.unwrap();
    assert!(matches!(events.try_recv(), Ok(MeshPeerEvent::Recovered { verified: true, .. })));
}
