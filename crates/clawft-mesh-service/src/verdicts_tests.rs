use std::sync::atomic::{AtomicUsize, Ordering};

use clawft_mesh_local::proto::Frame;
use clawft_mesh_local::Principal;
use clawft_types::config::MeshAdmissionMode;
use tokio::sync::mpsc;

use super::*;

struct Rig {
    broker: Arc<VerdictBroker>,
    registry: Arc<Registry>,
}

fn rig(timeout_ms: u64, grace_ms: u64) -> Rig {
    let registry = Arc::new(Registry::new());
    let policy = PolicyCell::new(Some(501), MeshAdmissionMode::Observe);
    let broker = VerdictBroker::new(
        Arc::clone(&registry),
        policy,
        Duration::from_millis(timeout_ms),
        Duration::from_millis(grace_ms),
    );
    Rig { broker, registry }
}

fn owner(rig: &Rig, conn: u64) -> (Arc<Registration>, mpsc::Receiver<Frame>) {
    let (reg, rx, _) = Registration::new(
        conn, Principal::Uid(501), "owner".into(), [1; 32], 1, String::new(), vec![], 0,
    );
    rig.registry.register(&reg, &[], &[]).unwrap();
    (reg, rx)
}

fn req(node: &str, key: u8) -> VerdictRequest {
    VerdictRequest {
        subject: VerdictSubject::PeerAdmit,
        peer: PeerInfo {
            node_id: node.into(),
            pubkey: hexser::encode(&[key; 32]),
            platform: "linux".into(),
            capabilities: vec!["node".into()],
            genesis_hash: String::new(),
            chain_seq: 0,
        },
        topic: None,
    }
}

/// Answer every verdict.request on `rx` with `allow`, counting them.
fn answer(
    rig: &Rig,
    conn: u64,
    mut rx: mpsc::Receiver<Frame>,
    allow: bool,
    ttl_s: u64,
) -> Arc<AtomicUsize> {
    let asked = Arc::new(AtomicUsize::new(0));
    let (a, broker) = (Arc::clone(&asked), Arc::clone(&rig.broker));
    tokio::spawn(async move {
        while let Some(f) = rx.recv().await {
            if let (Some(id), Message::VerdictRequest(_)) = (f.id, &f.msg) {
                a.fetch_add(1, Ordering::SeqCst);
                broker.on_reply(
                    conn,
                    id,
                    Reply { allow, ttl_s, reason: "rule says so".into(), rule_hash: "abc".into() },
                );
            }
        }
    });
    asked
}

#[tokio::test]
async fn allow_is_cached_for_its_ttl() {
    let r = rig(500, 600_000);
    let (_reg, rx) = owner(&r, 7);
    let asked = answer(&r, 7, rx, true, 60);
    let d = r.broker.ask(req("n1", 1)).await;
    assert_eq!(d, Decision::Allow { rule_hash: "abc".into(), stale: false });
    assert_eq!(r.broker.ask(req("n1", 1)).await, d);
    assert_eq!(asked.load(Ordering::SeqCst), 1, "second answer came from the cache");
    assert_eq!(r.broker.rule_hash_for("n1").as_deref(), Some("abc"));
}

#[tokio::test]
async fn cache_key_includes_the_peers_key() {
    let r = rig(500, 600_000);
    let (_reg, rx) = owner(&r, 7);
    let asked = answer(&r, 7, rx, true, 60);
    r.broker.ask(req("n1", 1)).await;
    r.broker.ask(req("n1", 2)).await;
    assert_eq!(asked.load(Ordering::SeqCst), 2, "same node id, different key: asked again");
}

#[tokio::test]
async fn zero_ttl_is_not_reused() {
    let r = rig(500, 600_000);
    let (_reg, rx) = owner(&r, 7);
    let asked = answer(&r, 7, rx, true, 0);
    r.broker.ask(req("n1", 1)).await;
    r.broker.ask(req("n1", 1)).await;
    assert_eq!(asked.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn deny_is_reported_with_the_reason() {
    let r = rig(500, 600_000);
    let (_reg, rx) = owner(&r, 7);
    answer(&r, 7, rx, false, 60);
    assert_eq!(r.broker.ask(req("n1", 1)).await, Decision::Deny("rule says so".into()));
}

#[tokio::test]
async fn no_owner_registered_fails_closed() {
    let r = rig(100, 600_000);
    match r.broker.ask(req("n1", 1)).await {
        Decision::Unavailable(why) => assert!(why.contains("no cluster-owner")),
        other => panic!("expected Unavailable, got {other:?}"),
    }
}

#[tokio::test]
async fn silent_owner_times_out_closed() {
    let r = rig(80, 600_000);
    let (_reg, _rx) = owner(&r, 7); // never answers
    let started = Instant::now();
    assert!(matches!(r.broker.ask(req("n1", 1)).await, Decision::Unavailable(_)));
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn already_granted_peer_keeps_its_permit_during_grace_only() {
    let r = rig(80, 400);
    let (reg, rx) = owner(&r, 7);
    let _asked = answer(&r, 7, rx, true, 0); // ttl 0: every ask goes to the owner
    assert!(matches!(r.broker.ask(req("n1", 1)).await, Decision::Allow { stale: false, .. }));
    // The owner goes away.
    r.registry.unregister("owner", 7);
    drop(reg);
    let d = r.broker.ask(req("n1", 1)).await;
    assert_eq!(d, Decision::Allow { rule_hash: "abc".into(), stale: true });
    // A peer never granted gets the static deny.
    assert!(matches!(r.broker.ask(req("n2", 2)).await, Decision::Unavailable(_)));
    // After the grace, even the granted peer is refused.
    tokio::time::sleep(Duration::from_millis(450)).await;
    assert!(matches!(r.broker.ask(req("n1", 1)).await, Decision::Unavailable(_)));
}

#[tokio::test]
async fn a_reply_from_another_connection_is_ignored() {
    let r = rig(150, 600_000);
    let (_reg, mut rx) = owner(&r, 7);
    let broker = Arc::clone(&r.broker);
    let asker = tokio::spawn(async move { broker.ask(req("n1", 1)).await });
    let frame = rx.recv().await.unwrap();
    let id = frame.id.unwrap();
    let reply = || Reply { allow: true, ttl_s: 60, reason: String::new(), rule_hash: "x".into() };
    assert!(!r.broker.on_reply(99, id, reply()), "wrong connection must not answer");
    assert!(matches!(asker.await.unwrap(), Decision::Unavailable(_)), "so the ask times out closed");
}

#[tokio::test]
async fn deny_ttl_is_clamped() {
    let r = rig(500, 600_000);
    let (_reg, rx) = owner(&r, 7);
    let asked = answer(&r, 7, rx, false, 100_000);
    r.broker.ask(req("n1", 1)).await;
    r.broker.ask(req("n1", 1)).await;
    assert_eq!(asked.load(Ordering::SeqCst), 1, "a deny is cached, briefly");
    assert!(MAX_DENY_TTL <= Duration::from_secs(30));
}

#[tokio::test]
async fn forget_conn_drops_waiting_requests() {
    let r = rig(500, 600_000);
    let (_reg, mut rx) = owner(&r, 7);
    let broker = Arc::clone(&r.broker);
    let asker = tokio::spawn(async move { broker.ask(req("n1", 1)).await });
    let _ = rx.recv().await.unwrap();
    r.broker.forget_conn(7);
    assert!(matches!(asker.await.unwrap(), Decision::Unavailable(_)));
}

#[tokio::test]
async fn clear_forgets_cached_verdicts_and_the_stale_grace() {
    let r = rig(500, 600_000);
    let (reg, rx) = owner(&r, 7);
    let asked = answer(&r, 7, rx, true, 60);
    r.broker.ask(req("n1", 1)).await;
    r.broker.clear();
    assert_eq!(r.broker.cache_len(), 0);
    r.broker.ask(req("n1", 1)).await;
    assert_eq!(asked.load(Ordering::SeqCst), 2, "asked again after the owner/mode changed");
    // And a granted peer no longer enjoys stale grace once cleared.
    r.broker.clear();
    r.registry.unregister("owner", 7);
    drop(reg);
    assert!(matches!(r.broker.ask(req("n1", 1)).await, Decision::Unavailable(_)));
}

fn key_for(node: &str) -> CacheKey {
    (0, node.into(), [0; 32], String::new(), String::new(), None)
}

fn entry(allow: bool) -> Entry {
    Entry { at: Instant::now(), ttl: Duration::from_secs(60), allow, reason: String::new(), rule_hash: "r".into() }
}

#[tokio::test]
async fn a_flood_of_denies_cannot_evict_the_allows_stale_grace_depends_on() {
    let r = rig(500, 600_000);
    r.broker.store(key_for("granted"), entry(true));
    for i in 0..(MAX_DENY_CACHE * 3) {
        r.broker.store(key_for(&format!("hostile{i}")), entry(false));
    }
    assert!(r.broker.allows.lock().unwrap().contains_key(&key_for("granted")));
    assert!(r.broker.denies.lock().unwrap().len() <= MAX_DENY_CACHE, "denies are bounded too");
}

#[tokio::test]
async fn concurrent_asks_of_one_key_share_a_single_request() {
    let r = rig(1000, 600_000);
    let (_reg, rx) = owner(&r, 7);
    let asked = answer(&r, 7, rx, true, 60);
    let (a, b, c, d) = tokio::join!(
        r.broker.ask(req("n1", 1)),
        r.broker.ask(req("n1", 1)),
        r.broker.ask(req("n1", 1)),
        r.broker.ask(req("n1", 1)),
    );
    for x in [&a, &b, &c, &d] {
        assert!(matches!(x, Decision::Allow { .. }), "{x:?}");
    }
    assert_eq!(asked.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn one_peer_cannot_ask_the_owner_more_than_its_budget() {
    let r = rig(500, 600_000);
    let (_reg, rx) = owner(&r, 7);
    let asked = answer(&r, 7, rx, false, 0);
    for _ in 0..ASKS_PER_NODE_PER_MINUTE {
        assert!(matches!(r.broker.ask(req("noisy", 1)).await, Decision::Deny(_)));
    }
    assert!(matches!(r.broker.ask(req("noisy", 1)).await, Decision::Unavailable(_)), "over budget fails closed");
    assert_eq!(asked.load(Ordering::SeqCst) as u32, ASKS_PER_NODE_PER_MINUTE);
    assert!(matches!(r.broker.ask(req("other", 2)).await, Decision::Deny(_)), "other peers are unaffected");
}

#[tokio::test]
async fn stale_grace_counts_from_expiry_not_from_the_grant() {
    let r = rig(80, 500);
    let (reg, rx) = owner(&r, 7);
    let _asked = answer(&r, 7, rx, true, 1); // allow, ttl 1 s
    assert!(matches!(r.broker.ask(req("n1", 1)).await, Decision::Allow { stale: false, .. }));
    r.registry.unregister("owner", 7);
    drop(reg);
    tokio::time::sleep(Duration::from_millis(1200)).await; // expired, inside the grace
    assert!(matches!(r.broker.ask(req("n1", 1)).await, Decision::Allow { stale: true, .. }));
    tokio::time::sleep(Duration::from_millis(600)).await; // past expiry + grace
    assert!(matches!(r.broker.ask(req("n1", 1)).await, Decision::Unavailable(_)));
}

#[tokio::test]
async fn refreshes_keep_budget_when_new_peers_exhaust_theirs() {
    let r = rig(500, 600_000);
    r.broker.store(key_for("member"), entry(true));
    for i in 0..(ASKS_PER_MINUTE - REFRESH_RESERVED) {
        assert!(r.broker.allow_ask(&format!("new{i}"), false), "new peer {i}");
    }
    assert!(!r.broker.allow_ask("one-more-new", false), "new peers are capped below the global limit");
    assert!(r.broker.allow_ask("member", true), "a peer already holding an allow can still refresh");
}

#[tokio::test]
async fn an_answer_obtained_before_clear_is_discarded() {
    let r = rig(500, 600_000);
    let (_reg, mut rx) = owner(&r, 7);
    let broker = Arc::clone(&r.broker);
    let asker = tokio::spawn(async move { broker.ask(req("n1", 1)).await });
    let frame = rx.recv().await.unwrap();
    r.broker.clear(); // the authority changed while the owner was thinking
    r.broker.on_reply(7, frame.id.unwrap(), Reply { allow: true, ttl_s: 60, reason: String::new(), rule_hash: "old".into() });
    assert!(matches!(asker.await.unwrap(), Decision::Unavailable(_)));
    assert_eq!(r.broker.cache_len(), 0, "nothing from the old authority is cached");
}
