use super::*;
use crate::mesh_admit::PeerClass;
use crate::mesh_heartbeat::HeartbeatState;
use crate::mesh_runtime::RouteTally;

fn ctx(peer: &str, verified: bool) -> PeerCtx {
    PeerCtx {
        node_verified: verified,
        class: if verified { PeerClass::Node } else { PeerClass::Legacy },
        ..PeerCtx::unauthenticated(peer)
    }
}

/// A runtime with one verified peer `p1` and one unverified peer `u1`, plus
/// the receivers of their outbound channels.
fn rig() -> (Arc<MeshRuntime>, tokio::sync::mpsc::Receiver<Vec<u8>>, tokio::sync::mpsc::Receiver<Vec<u8>>) {
    let rt = Arc::new(MeshRuntime::with_discovery("local".into(), [0u8; 32]));
    let tally = RouteTally::default();
    let (tx, rx) = tokio::sync::mpsc::channel(16);
    let (tx2, rx2) = tokio::sync::mpsc::channel(16);
    rt.register_authenticated_as("p1".into(), tx, true, PeerClass::Node, &tally);
    rt.register_authenticated_as("u1".into(), tx2, false, PeerClass::Legacy, &tally);
    (rt, rx, rx2)
}

/// The nonce of the ping in an outbound frame, if it is one.
fn ping_nonce(frame: &[u8]) -> Option<u64> {
    let v: Value = serde_json::from_slice(frame).ok()?;
    let p = &v["message"]["payload"]["Json"];
    (p["t"] == "ping").then(|| p["n"].as_u64()).flatten()
}

#[tokio::test]
async fn a_tick_pings_only_verified_peers() {
    let (rt, mut rx, mut rx2) = rig();
    let lv = Liveness::new(&rt, LivenessConfig::default());
    lv.tick().await;
    let frame = rx.try_recv().expect("verified peer is pinged");
    assert!(ping_nonce(&frame).is_some(), "{}", String::from_utf8_lossy(&frame));
    assert!(rx2.try_recv().is_err(), "unverified peer is never pinged");
}

#[tokio::test]
async fn a_matching_pong_from_the_verified_peer_counts_and_feeds_the_heartbeat() {
    let (rt, mut rx, _rx2) = rig();
    let lv = Liveness::new(&rt, LivenessConfig::default());
    lv.tick().await;
    let n = ping_nonce(&rx.try_recv().unwrap()).unwrap();
    assert!(lv.peer("p1").is_none());
    assert!(lv.on_peer_control(&ctx("p1", true), 0, &json!({"t": "pong", "n": n})).is_empty());
    let l = lv.peer("p1").expect("counted");
    assert_eq!(l.missed, 0);
    assert!(l.rtt_ms >= 0.0);
    let d = rt.peer_details().into_iter().find(|d| d.node_id == "p1").unwrap();
    assert_eq!(d.heartbeat, Some(HeartbeatState::Alive));
    // A replay of the same nonce does not count again (it is no longer outstanding).
    let before = lv.peer("p1").unwrap().last_seen;
    lv.on_peer_control(&ctx("p1", true), 0, &json!({"t": "pong", "n": n}));
    assert_eq!(lv.peer("p1").unwrap().last_seen, before);
}

#[tokio::test]
async fn pongs_that_were_not_asked_for_or_come_unverified_are_ignored() {
    let (rt, mut rx, _rx2) = rig();
    let lv = Liveness::new(&rt, LivenessConfig::default());
    lv.tick().await;
    let n = ping_nonce(&rx.try_recv().unwrap()).unwrap();
    // Unverified sender claiming p1's id.
    lv.on_peer_control(&ctx("p1", false), 0, &json!({"t": "pong", "n": n}));
    // Wrong nonce.
    lv.on_peer_control(&ctx("p1", true), 0, &json!({"t": "pong", "n": n + 1000}));
    // A verified *other* peer answering p1's nonce.
    lv.on_peer_control(&ctx("u1", true), 0, &json!({"t": "pong", "n": n}));
    assert!(lv.peer("p1").is_none());
    assert!(lv.peer("u1").is_none());
}

#[tokio::test]
async fn a_verified_ping_is_answered_and_an_unverified_one_is_not() {
    let (rt, _rx, _rx2) = rig();
    let lv = Liveness::new(&rt, LivenessConfig::default());
    let reply = lv.on_peer_control(&ctx("p1", true), 0, &json!({"t": "ping", "n": 7}));
    assert_eq!(reply.len(), 1);
    assert_eq!((reply[0]["t"].as_str(), reply[0]["n"].as_u64()), (Some("pong"), Some(7)));
    #[cfg(unix)]
    assert!(LoadSample::from_json(&reply[0]["load"]).is_some(), "the pong carries this host's load: {}", reply[0]);
    assert!(lv.on_peer_control(&ctx("x", false), 0, &json!({"t": "ping", "n": 7})).is_empty());
    assert!(lv.on_peer_control(&ctx("p1", true), 0, &json!({"t": "ping"})).is_empty(), "no nonce, no answer");
}

#[tokio::test]
async fn unanswered_pings_expire_count_as_misses_and_stay_bounded() {
    let (rt, mut rx, _rx2) = rig();
    let cfg = LivenessConfig { interval: Duration::from_millis(10), timeout: Duration::from_millis(0) };
    let lv = Liveness::new(&rt, cfg);
    // First answer one ping so p1 has a record, then let later ones time out.
    lv.tick().await;
    let n = ping_nonce(&rx.try_recv().unwrap()).unwrap();
    lv.on_peer_control(&ctx("p1", true), 0, &json!({"t": "pong", "n": n}));
    for _ in 0..3 {
        lv.tick().await;
        std::thread::sleep(Duration::from_millis(2));
    }
    lv.tick().await;
    assert!(lv.peer("p1").unwrap().missed >= 3);
    assert!(lv.state.lock().unwrap().outstanding.len() <= MAX_OUTSTANDING_PER_PEER);
}

#[tokio::test]
async fn peer_details_carry_last_seen_and_rtt_once_liveness_runs() {
    let (rt, mut rx, _rx2) = rig();
    rt.start_liveness(LivenessConfig { interval: Duration::from_secs(3600), timeout: Duration::from_secs(30) });
    let lv = rt.liveness().unwrap().clone();
    // The spawned loop's first tick fires immediately; take that ping.
    let frame = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await.unwrap().unwrap();
    let n = ping_nonce(&frame).unwrap();
    lv.on_peer_control(&ctx("p1", true), 0, &json!({"t": "pong", "n": n, "load": {"load1": 1.5, "load5": 0.25}}));
    let d = rt.peer_details().into_iter().find(|d| d.node_id == "p1").unwrap();
    assert!(d.last_seen.is_some() && d.rtt_ms.is_some());
    assert_eq!((d.load.map(|l| l.load5), d.missed_pongs), (Some(0.25), Some(0)));
    let u = rt.peer_details().into_iter().find(|d| d.node_id == "u1").unwrap();
    assert!(u.last_seen.is_none() && u.rtt_ms.is_none());
}

#[tokio::test]
async fn a_pong_carries_the_peers_load_and_a_bad_load_is_dropped_but_still_counts() {
    let (rt, mut rx, _rx2) = rig();
    let lv = Liveness::new(&rt, LivenessConfig::default());
    lv.tick().await;
    let n = ping_nonce(&rx.try_recv().unwrap()).unwrap();
    let load = json!({"load1": 0.5, "load5": 0.25, "cores": 4, "mem_avail": 10, "mem_total": 20});
    lv.on_peer_control(&ctx("p1", true), 0, &json!({"t": "pong", "n": n, "load": load}));
    let l = lv.peer("p1").unwrap().load.expect("load kept");
    assert_eq!((l.load1, l.cores), (0.5, Some(4)));

    lv.tick().await;
    let n2 = ping_nonce(&rx.try_recv().unwrap()).unwrap();
    lv.on_peer_control(&ctx("p1", true), 0, &json!({"t": "pong", "n": n2, "load": {"load1": -3}}));
    let p = lv.peer("p1").unwrap();
    assert!(p.load.is_none(), "an out-of-range load is not shown");
    assert_eq!(p.missed, 0, "the pong itself still counted");
}
