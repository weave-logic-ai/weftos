//! The licence exchange over service links (ADR-106 phase 3, service mode):
//! inbound records arrive as stamped deliveries, outbound ones leave as
//! sends, and the peer view comes from the service. An in-process bus stands
//! in for the services: a send from X to Y is delivered to Y's links with the
//! context the service would stamp for X.

use std::sync::{Arc, Mutex};
use std::sync::atomic::Ordering;

use async_trait::async_trait;
use dashmap::DashMap;

use super::tests_common::*;
use super::tests_exchange::wait_for;
use super::*;
use crate::error::KernelResult;
use crate::ipc::{KernelMessage, MessagePayload, MessageTarget};
use crate::mesh_admit::PeerClass;
use crate::mesh_artifact_tunnel::PeerSender;
use crate::mesh_delivery::{LocalDelivery, PeerCtx};
use crate::mesh_ipc::Scope;
use crate::mesh_runtime::{COG_BINDING_TOPIC, COG_SYNC_TOPIC};

#[derive(Default)]
struct Bus {
    links: DashMap<String, Arc<ServiceLicenceLinks>>,
    /// How each receiver's service classes each sender (default: licensed node).
    class: DashMap<(String, String), (bool, PeerClass)>,
    sent: Mutex<Vec<(String, String, String)>>,
}

impl Bus {
    fn sent_to(&self, to: &str) -> usize {
        self.sent.lock().unwrap().iter().filter(|(_, t, _)| t == to).count()
    }

    fn sent_on(&self, from: &str, to: &str, topic: &str) -> usize {
        self.sent.lock().unwrap().iter().filter(|(f, t, p)| f == from && t == to && p == topic).count()
    }
}

struct BusSender {
    me: String,
    bus: Arc<Bus>,
}

#[async_trait]
impl PeerSender for BusSender {
    async fn send_to_node(&self, to: &str, msg: KernelMessage) -> KernelResult<()> {
        let topic = match &msg.target {
            MessageTarget::Topic(t) => t.clone(),
            _ => String::new(),
        };
        self.bus.sent.lock().unwrap().push((self.me.clone(), to.to_owned(), topic));
        let (verified, class) =
            self.bus.class.get(&(to.to_owned(), self.me.clone())).map_or((true, PeerClass::Node), |c| *c);
        if let Some(l) = self.bus.links.get(to).map(|l| l.clone()) {
            let ctx = PeerCtx { peer_id: self.me.clone(), node_verified: verified, class, remote_static: None, src_scope: None };
            tokio::spawn(async move {
                l.deliver(&ctx, msg).await;
            });
        }
        Ok(())
    }
}

#[derive(Default)]
struct Dir(Mutex<PeerSnapshot>);

#[async_trait]
impl PeerDirectory for Dir {
    async fn peers(&self) -> Result<PeerSnapshot, String> {
        Ok(self.0.lock().unwrap().clone())
    }
}

struct SNode {
    fx: Fx,
    links: Arc<ServiceLicenceLinks>,
    dir: Arc<Dir>,
    ex: Option<Arc<LicenceExchange>>,
}

fn snode(bus: &Arc<Bus>, id: &str, connected: &[&str], licensed: &[&str], with_exchange: bool) -> SNode {
    let fx = Fx::new();
    let dir = Arc::new(Dir(Mutex::new(PeerSnapshot {
        connected: connected.iter().map(|s| s.to_string()).collect(),
        licensed: licensed.iter().map(|s| s.to_string()).collect(),
        reserved_holder: None,
    })));
    let links = ServiceLicenceLinks::new(Arc::new(BusSender { me: id.into(), bus: bus.clone() }), dir.clone());
    bus.links.insert(id.into(), links.clone());
    let ex = with_exchange.then(|| {
        LicenceExchange::start(LicenceExchangeParts {
            store: fx.store.clone(),
            approvals: fx.approvals.clone(),
            anchors: anchors(),
            runtime: links.clone(),
            posture: Arc::new(posture),
            admission: Arc::new(CtxAdmission),
            sink: fx.sink.clone(),
            config: LicenceExchangeConfig::default(),
        })
    });
    SNode { fx, links, dir, ex }
}

fn ctx(peer: &str, verified: bool, class: PeerClass) -> PeerCtx {
    PeerCtx { peer_id: peer.into(), node_verified: verified, class, remote_static: None, src_scope: None }
}

fn json_msg(topic: &str, v: serde_json::Value) -> KernelMessage {
    KernelMessage::new(0, MessageTarget::Topic(topic.into()), MessagePayload::Json(v))
}

#[tokio::test]
async fn floods_through_service_links_reach_licensed_peers_and_never_a_leaf() {
    let bus = Arc::new(Bus::default());
    let a = snode(&bus, "node-a", &["node-b", "leaf-l"], &["node-b"], true);
    let b = snode(&bus, "node-b", &["node-a"], &["node-a"], true);
    let _l = snode(&bus, "leaf-l", &["node-a"], &["node-a"], true);
    a.links.refresh().await.unwrap();
    b.links.refresh().await.unwrap();
    let ax = a.ex.as_ref().unwrap();
    assert_eq!(ax.issue_binding(binding(1, BindState::Bound)).await, Ok(Receipt::New));
    wait_for("B to hold the binding", || b.fx.store.active_binding().is_some()).await;
    ax.issue_grant(grant(1, T0, 3600, &["x86_64"])).await.unwrap();
    wait_for("B to hold the grant", || b.fx.store.held_grant("fall-detect", "1.2.0").is_some()).await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(bus.sent_to("leaf-l"), 0, "a leaf is connected but not licensed: no flood, no sync");
    assert!(b.links.counters.delivered.load(Ordering::Relaxed) >= 2);
    // B knew both records once applied, so it forwarded nothing back to A.
    assert_eq!(bus.sent_on("node-b", "node-a", COG_BINDING_TOPIC), 0);
}

#[tokio::test]
async fn a_late_joiner_syncs_as_soon_as_the_service_reports_it_licensed() {
    let bus = Arc::new(Bus::default());
    let a = snode(&bus, "node-a", &[], &[], true);
    let ax = a.ex.as_ref().unwrap();
    ax.issue_binding(binding(1, BindState::Bound)).await.unwrap();
    ax.issue_grant(grant(1, T0, 3600, &["x86_64"])).await.unwrap();
    let shas = vec![sha_of("x86_64")];
    ax.issue_approval(approval(&shas)).await.unwrap();
    assert_eq!(bus.sent.lock().unwrap().len(), 0, "nobody was connected to flood to");

    // C comes up later; its service reports A connected and licensed.
    let c = snode(&bus, "node-c", &[], &[], true);
    *c.dir.0.lock().unwrap() = PeerSnapshot { connected: vec!["node-a".into()], licensed: vec!["node-a".into()], ..Default::default() };
    c.links.refresh().await.unwrap();
    wait_for("C to catch up by sync", || {
        c.fx.store.active_binding().is_some()
            && c.fx.store.held_grant("fall-detect", "1.2.0").is_some()
            && c.fx.approvals.len() == 1
    })
    .await;
    assert!(bus.sent_on("node-c", "node-a", COG_SYNC_TOPIC) >= 1, "C asked A");
    assert!(bus.sent_on("node-a", "node-c", COG_SYNC_TOPIC) >= 1, "A answered through its own links");
    // Settled, a second refresh with no change raises no second join.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let before = bus.sent_on("node-c", "node-a", COG_SYNC_TOPIC);
    c.links.refresh().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(bus.sent_on("node-c", "node-a", COG_SYNC_TOPIC), before);
}

#[tokio::test]
async fn deliveries_from_anything_but_a_licensed_node_are_dropped() {
    let bus = Arc::new(Bus::default());
    let b = snode(&bus, "node-b", &[], &[], true);
    let v = serde_json::to_value(binding(1, BindState::Bound)).unwrap();
    for c in [ctx("x", false, PeerClass::Legacy), ctx("x", true, PeerClass::Leaf), ctx("x", false, PeerClass::Node)] {
        assert!(b.links.deliver(&c, json_msg(COG_BINDING_TOPIC, v.clone())).await, "consumed, not passed on");
    }
    assert_eq!(b.links.counters.refused_unverified.load(Ordering::Relaxed), 3);
    assert!(b.fx.store.active_binding().is_none());
    // A sync request from a leaf is not answered either.
    let req = serde_json::to_value(SyncMsg::Request { grant_after: None, approval_after: None }).unwrap();
    b.links.deliver(&ctx("leaf", true, PeerClass::Leaf), json_msg(COG_SYNC_TOPIC, req)).await;
    assert_eq!(bus.sent_to("leaf"), 0);
    // Not a licence topic: not ours.
    assert!(!b.links.deliver(&ctx("x", true, PeerClass::Node), json_msg("plain", v)).await);
}

#[derive(Default)]
struct Inner(Mutex<Vec<String>>);

#[async_trait]
impl LocalDelivery for Inner {
    async fn deliver(&self, _: &PeerCtx, _: Option<&Scope>, msg: KernelMessage) -> KernelResult<()> {
        if let MessageTarget::Topic(t) = msg.target {
            self.0.lock().unwrap().push(t);
        }
        Ok(())
    }
}

#[tokio::test]
async fn one_path_only_licence_topics_never_reach_the_router_and_unused_links_handle_nothing() {
    use crate::mesh_cog::{CogMeshDelivery, CogMeshSlot};
    let bus = Arc::new(Bus::default());
    // Links with no exchange over them (the node's exchange runs over the
    // kernel runtime): licence deliveries are consumed and handled by nobody.
    let n = snode(&bus, "node-n", &[], &[], false);
    let inner = Arc::new(Inner::default());
    let d = CogMeshDelivery::new(inner.clone(), Arc::new(CogMeshSlot::default())).with_licence(n.links.clone());
    let v = serde_json::to_value(binding(1, BindState::Bound)).unwrap();
    let node = ctx("node-a", true, PeerClass::Node);
    d.deliver(&node, None, json_msg(COG_BINDING_TOPIC, v.clone())).await.unwrap();
    d.deliver(&node, None, json_msg("plain", v.clone())).await.unwrap();
    assert_eq!(n.links.counters.unhandled.load(Ordering::Relaxed), 1);
    assert_eq!(*inner.0.lock().unwrap(), vec!["plain".to_string()], "only the plain topic reached the router");
    assert!(n.fx.store.active_binding().is_none());
    // Without links at all the topic is still not passed on.
    let bare = CogMeshDelivery::new(inner.clone(), Arc::new(CogMeshSlot::default()));
    bare.deliver(&node, None, json_msg(COG_BINDING_TOPIC, v)).await.unwrap();
    assert_eq!(inner.0.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn the_same_record_on_both_paths_is_applied_once() {
    // A node whose exchange runs on the service links gets one binding as a
    // delivery and again (say, from a stray second path) as a direct accept:
    // the store applies it once and the exchange forwards it once.
    let bus = Arc::new(Bus::default());
    let b = snode(&bus, "node-b", &["node-c"], &["node-c"], true);
    let _c = snode(&bus, "node-c", &[], &[], false);
    b.links.refresh().await.unwrap();
    let env = binding(1, BindState::Bound);
    let v = serde_json::to_value(&env).unwrap();
    b.links.deliver(&ctx("node-a", true, PeerClass::Node), json_msg(COG_BINDING_TOPIC, v.clone())).await;
    b.links.deliver(&ctx("node-a", true, PeerClass::Node), json_msg(COG_BINDING_TOPIC, v)).await;
    assert_eq!(b.ex.as_ref().unwrap().accept_binding(&env, Spend::Exempt), Ok(Receipt::Known));
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(bus.sent_on("node-b", "node-c", COG_BINDING_TOPIC), 1, "forwarded once");
}

/// Sends to `slow` take `delay`; everything else is instant. Counts sends.
struct SlowSender {
    delay: std::time::Duration,
    sent: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait]
impl PeerSender for SlowSender {
    async fn send_to_node(&self, to: &str, _: KernelMessage) -> KernelResult<()> {
        if to == "slow" {
            tokio::time::sleep(self.delay).await;
        }
        self.sent.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn a_slow_peer_does_not_stall_other_deliveries() {
    let fx = Fx::new();
    let sent = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let links = ServiceLicenceLinks::new(
        Arc::new(SlowSender { delay: std::time::Duration::from_secs(5), sent: sent.clone() }),
        Arc::new(Dir::default()),
    );
    let _ex = LicenceExchange::start(LicenceExchangeParts {
        store: fx.store.clone(),
        approvals: fx.approvals.clone(),
        anchors: anchors(),
        runtime: links.clone(),
        posture: Arc::new(posture),
        admission: Arc::new(CtxAdmission),
        sink: fx.sink.clone(),
        config: LicenceExchangeConfig { sync_on_connect: false, ..Default::default() },
    });
    // A sync request from the slow peer: the reply to it takes 5 s to send.
    let req = serde_json::to_value(SyncMsg::Request { grant_after: None, approval_after: None }).unwrap();
    let t = std::time::Instant::now();
    assert!(links.deliver(&ctx("slow", true, PeerClass::Node), json_msg(COG_SYNC_TOPIC, req)).await);
    // The next delivery, from another peer, is handled at once.
    let v = serde_json::to_value(binding(1, BindState::Bound)).unwrap();
    assert!(links.deliver(&ctx("fast", true, PeerClass::Node), json_msg(COG_BINDING_TOPIC, v)).await);
    assert!(t.elapsed() < std::time::Duration::from_secs(1), "the worker waited on a reply: {:?}", t.elapsed());
    assert!(fx.store.active_binding().is_some());
    assert_eq!(sent.load(Ordering::SeqCst), 0, "the slow reply is still in flight on its own task");
}

/// Answers until `down` is set, then fails as a dropped link does.
#[derive(Default)]
struct FlakyDir(std::sync::atomic::AtomicBool);

#[async_trait]
impl PeerDirectory for FlakyDir {
    async fn peers(&self) -> Result<PeerSnapshot, String> {
        if self.0.load(Ordering::SeqCst) {
            return Err("the mesh service link is reconnecting".into());
        }
        Ok(PeerSnapshot { connected: vec!["node-b".into()], licensed: vec!["node-b".into()], reserved_holder: Some(true) })
    }
}

#[tokio::test]
async fn a_failed_refresh_clears_the_licensed_view() {
    let bus = Arc::new(Bus::default());
    let dir = Arc::new(FlakyDir::default());
    let links = ServiceLicenceLinks::new(Arc::new(BusSender { me: "node-d".into(), bus }), dir.clone());
    links.refresh().await.unwrap();
    assert!(links.peer_licensed("node-b"));
    dir.0.store(true, Ordering::SeqCst);
    assert!(links.refresh().await.is_err());
    assert!(!links.peer_licensed("node-b") && links.peer_ids().is_empty(), "nobody is licensed while the link is down");
    assert_eq!(links.counters.refresh_failed.load(Ordering::Relaxed), 1);
    assert_eq!(links.counters.refreshed.load(Ordering::Relaxed), 1);
}

fn quiet_exchange(fx: &Fx, links: &Arc<ServiceLicenceLinks>) -> Arc<LicenceExchange> {
    LicenceExchange::start(LicenceExchangeParts {
        store: fx.store.clone(),
        approvals: fx.approvals.clone(),
        anchors: anchors(),
        runtime: links.clone(),
        posture: Arc::new(posture),
        admission: Arc::new(CtxAdmission),
        sink: fx.sink.clone(),
        config: LicenceExchangeConfig { sync_on_connect: false, ..Default::default() },
    })
}

fn sync_request() -> KernelMessage {
    json_msg(COG_SYNC_TOPIC, serde_json::to_value(SyncMsg::Request { grant_after: None, approval_after: None }).unwrap())
}

#[tokio::test]
async fn a_request_dropped_at_the_reply_cap_is_not_recorded_as_served() {
    let fx = Fx::new();
    let sent = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let sender = Arc::new(SlowSender { delay: std::time::Duration::from_millis(300), sent: sent.clone() });
    let links = ServiceLicenceLinks::with_reply_caps(sender, Arc::new(Dir::default()), 1, 4);
    let _ex = quiet_exchange(&fx, &links);
    // The slow peer's reply holds the only slot; the fast peer's request is
    // dropped before the exchange sees it.
    links.deliver(&ctx("slow", true, PeerClass::Node), sync_request()).await;
    links.deliver(&ctx("fast", true, PeerClass::Node), sync_request()).await;
    assert_eq!(links.counters.replies_dropped.load(Ordering::SeqCst), 1);
    wait_for("the slow reply to go out", || sent.load(Ordering::SeqCst) == 1).await;
    // Asked again within the minute, the fast peer is answered: the drop did
    // not count as having served it.
    links.deliver(&ctx("fast", true, PeerClass::Node), sync_request()).await;
    wait_for("the fast peer is answered", || sent.load(Ordering::SeqCst) == 2).await;
}

#[tokio::test]
async fn one_peer_holds_at_most_its_own_share_of_reply_slots() {
    let fx = Fx::new();
    let sent = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let sender = Arc::new(SlowSender { delay: std::time::Duration::from_secs(5), sent: sent.clone() });
    let links = ServiceLicenceLinks::with_reply_caps(sender, Arc::new(Dir::default()), 64, 1);
    let _ex = quiet_exchange(&fx, &links);
    links.deliver(&ctx("slow", true, PeerClass::Node), sync_request()).await;
    links.deliver(&ctx("slow", true, PeerClass::Node), sync_request()).await;
    assert_eq!(links.counters.replies_dropped.load(Ordering::SeqCst), 1, "the second is over the per-peer cap");
    links.deliver(&ctx("other", true, PeerClass::Node), sync_request()).await;
    assert_eq!(links.counters.replies_dropped.load(Ordering::SeqCst), 1, "another peer still gets a slot");
    assert_eq!(MAX_REPLY_SENDS_PER_PEER, 4);
}

#[tokio::test]
async fn an_unanswered_sync_is_asked_again_a_bounded_number_of_times() {
    let bus = Arc::new(Bus::default());
    let n = snode(&bus, "node-n", &[], &[], true);
    let ex = n.ex.as_ref().unwrap();
    let expire = || {
        ex.pending.insert(
            "node-a".into(),
            super::exchange_sync::Pending {
                sent: std::time::Instant::now().checked_sub(std::time::Duration::from_secs(121)).unwrap(),
                pages: 0,
                grant_after: None,
                approval_after: None,
            },
        );
    };
    for round in 1..=MAX_SYNC_RETRIES as usize {
        expire();
        ex.retry_expired().await;
        assert_eq!(bus.sent_on("node-n", "node-a", COG_SYNC_TOPIC), round, "retry {round}");
    }
    expire();
    ex.retry_expired().await;
    assert_eq!(bus.sent_on("node-n", "node-a", COG_SYNC_TOPIC), MAX_SYNC_RETRIES as usize, "then it waits");
    assert!(ex.pending.is_empty() && ex.retries.is_empty());
}
