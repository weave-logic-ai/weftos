//! FU mesh-kernel: O(1) route-loss detection, bidirectional/reconnecting
//! seed dials, Noise listener resilience, and forward-compatible envelopes.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;

use super::*;
use crate::error::KernelResult;
use crate::ipc::{KernelMessage, MessageTarget};
use crate::mesh::MeshTransport;
use crate::mesh_delivery::{LocalDelivery, PeerCtx};
use crate::mesh_ipc::{MeshIpcEnvelope, Scope};
use crate::mesh_noise::{NoiseChannel, NoiseConfig, NoisePattern};

#[derive(Default)]
struct Rec(Mutex<Vec<String>>);

#[async_trait]
impl LocalDelivery for Rec {
    async fn deliver(&self, _: &PeerCtx, _: Option<&Scope>, m: KernelMessage) -> KernelResult<()> {
        if let MessageTarget::Topic(t) = m.target {
            self.0.lock().unwrap().push(t);
        }
        Ok(())
    }
}

/// Poll `f` until it holds (up to 10 s); no fixed sleeps in the tests.
async fn until(mut f: impl FnMut() -> bool) -> bool {
    for _ in 0..2000 {
        if f() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    false
}

async fn seen(rec: &Rec, topic: &str) -> bool {
    until(|| rec.0.lock().unwrap().iter().any(|t| t == topic)).await
}

/// A listener that counts accepted connections and keeps them open and
/// silent (`hold`) or closes them at once.
async fn counting_listener(hold: bool) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
    let mut l = crate::mesh_tcp::TcpTransport.listen("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let n = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let n2 = Arc::clone(&n);
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((stream, _)) = l.accept().await {
            n2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if hold {
                held.push(stream);
            }
        }
    });
    (addr, n)
}

fn count(n: &std::sync::atomic::AtomicUsize) -> usize {
    n.load(std::sync::atomic::Ordering::SeqCst)
}

fn runtime(id: &str) -> (Arc<MeshRuntime>, Arc<Rec>) {
    let rec = Arc::new(Rec::default());
    let mut rt = MeshRuntime::new(id.into());
    rt.set_local_delivery(rec.clone());
    (Arc::new(rt), rec)
}

fn frame(src: &str, topic: &str) -> Vec<u8> {
    let msg = KernelMessage::text(0, MessageTarget::Topic(topic.into()), "x");
    MeshIpcEnvelope::new(src.into(), "dst".into(), msg).to_bytes().unwrap()
}

async fn listen(
    rt: &Arc<MeshRuntime>,
    noise: Option<Arc<NoiseConfig>>,
) -> (String, tokio::task::JoinHandle<()>) {
    let l = crate::mesh_tcp::TcpTransport.listen("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let task = tokio::spawn(serve_listener(
        Arc::clone(rt), l, noise, "tcp", "x", Arc::new(crate::mesh_admit::AllowAll),
    ));
    (addr, task)
}

fn noise_cfg() -> Arc<NoiseConfig> {
    let kp = snow::Builder::new("Noise_XX_25519_ChaChaPoly_SHA256".parse().unwrap())
        .generate_keypair()
        .unwrap();
    Arc::new(NoiseConfig {
        pattern: NoisePattern::XX,
        local_private_key: kp.private.try_into().unwrap(),
        remote_static_key: None,
    })
}

// ── route tally ──────────────────────────────────────────────────

#[test]
fn tally_follows_registration_replacement_and_removal() {
    let rt = MeshRuntime::new("local".into());
    let t = RouteTally::default();
    let (a, _ra) = tokio::sync::mpsc::channel(4);
    let (b, _rb) = tokio::sync::mpsc::channel(4);
    rt.add_peer_tallied("n".into(), a.clone(), &t);
    assert_eq!(t.live(), 1);
    // Same channel again: no double count.
    rt.add_peer_tallied("n".into(), a.clone(), &t);
    assert_eq!(t.live(), 1);
    // Replaced by another connection: this connection's count drops.
    rt.add_peer("n".into(), b);
    assert_eq!(t.live(), 0);
    // Removal path.
    rt.add_peer_tallied("m".into(), a, &t);
    assert_eq!(t.live(), 1);
    rt.disconnect_peer("m");
    assert_eq!(t.live(), 0);
}

#[test]
fn tally_ignores_every_route_it_does_not_own() {
    // The per-connection check reads only its own counter, so unrelated
    // peers (added, replaced or removed) never change it.
    let rt = MeshRuntime::new("local".into());
    let t = RouteTally::default();
    let mut keep = Vec::new();
    for i in 0..5_000 {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        rt.add_peer(format!("p{i}"), tx);
        keep.push(rx);
    }
    assert_eq!(t.live(), 0);
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    rt.add_peer_tallied("mine".into(), tx, &t);
    assert_eq!(t.live(), 1);
    for i in 0..5_000 {
        rt.disconnect_peer(&format!("p{i}"));
    }
    assert_eq!(t.live(), 1);
}

#[tokio::test]
async fn connection_closes_when_its_route_is_revoked() {
    let (rt, _) = runtime("srv");
    let (addr, _task) = listen(&rt, None).await;
    let mut c = crate::mesh_tcp::TcpTransport.connect(&addr).await.unwrap();
    c.send(&frame("n1", "t.a")).await.unwrap();
    assert!(until(|| !rt.peer_ids().is_empty()).await);
    assert_eq!(rt.peer_ids(), vec!["n1".to_string()]);
    rt.disconnect_peer("n1");
    assert!(matches!(tokio::time::timeout(Duration::from_secs(2), c.recv()).await, Ok(Err(_))),
        "revoked connection must close within the route-check interval");
}

#[tokio::test]
async fn old_connection_closes_when_a_new_one_replaces_its_route() {
    let (rt, _) = runtime("srv");
    let (addr, _task) = listen(&rt, None).await;
    let mut old = crate::mesh_tcp::TcpTransport.connect(&addr).await.unwrap();
    old.send(&frame("n1", "t.a")).await.unwrap();
    assert!(until(|| !rt.peer_ids().is_empty()).await);
    let mut new = crate::mesh_tcp::TcpTransport.connect(&addr).await.unwrap();
    new.send(&frame("n1", "t.b")).await.unwrap();
    assert!(matches!(tokio::time::timeout(Duration::from_secs(2), old.recv()).await, Ok(Err(_))),
        "replaced connection must close");
    // The replacement stays up and is the live route.
    assert_eq!(rt.peer_ids(), vec!["n1".to_string()]);
    let msg = KernelMessage::text(0, MessageTarget::Topic("t.back".into()), "x");
    rt.send_to_peer("n1", MeshIpcEnvelope::new("srv".into(), "n1".into(), msg)).await.unwrap();
    assert!(matches!(tokio::time::timeout(Duration::from_secs(5), new.recv()).await, Ok(Ok(_))));
}

// ── Noise listener ───────────────────────────────────────────────

#[tokio::test]
async fn noise_listener_serves_and_survives_a_failed_handshake() {
    let (rt, rec) = runtime("srv");
    let (addr, task) = listen(&rt, Some(noise_cfg())).await;

    // Garbage instead of a Noise handshake, then hang up.
    let mut bad = crate::mesh_tcp::TcpTransport.connect(&addr).await.unwrap();
    let _ = bad.send(&[0xde, 0xad, 0xbe, 0xef, 1, 2, 3]).await;
    drop(bad);

    // The listener is still alive and a real initiator gets through.
    let stream = crate::mesh_tcp::TcpTransport.connect(&addr).await.unwrap();
    let mut ch = NoiseChannel::initiate(stream, &noise_cfg()).await.unwrap();
    ch.send_encrypted(&frame("n1", "t.noise")).await.unwrap();
    assert!(seen(&rec, "t.noise").await);
    assert!(!task.is_finished());
}

#[tokio::test]
async fn silent_noise_handshake_is_dropped_and_listener_lives() {
    let (rt, _) = runtime("srv");
    let (addr, task) = listen(&rt, Some(noise_cfg())).await;
    // Connect and say nothing; the cap on handshake time frees the slot
    // (HANDSHAKE_TIMEOUT is 10 s; assert only that nothing is wedged).
    let _idle = crate::mesh_tcp::TcpTransport.connect(&addr).await.unwrap();
    let stream = crate::mesh_tcp::TcpTransport.connect(&addr).await.unwrap();
    assert!(NoiseChannel::initiate(stream, &noise_cfg()).await.is_ok(),
        "one stalled handshake must not block others");
    assert!(!task.is_finished());
}

// ── seed dials ───────────────────────────────────────────────────

fn quick() -> SeedTiming {
    SeedTiming {
        base: Duration::from_millis(20),
        max: Duration::from_millis(100),
        stable: Duration::from_secs(60),
        idle: Duration::from_secs(60),
    }
}

async fn push(from: &Arc<MeshRuntime>, to_addr: &str, src: &str, topic: &str) -> bool {
    let msg = KernelMessage::text(0, MessageTarget::Topic(topic.into()), "x");
    let env = MeshIpcEnvelope::new(src.into(), "srv".into(), msg);
    from.send_to_peer(to_addr, env).await.is_ok()
}

#[tokio::test]
async fn seed_connection_reads_inbound_frames_from_the_seed() {
    let (srv, _) = runtime("srv");
    let (addr, _task) = listen(&srv, None).await;
    let (dialer, drec) = runtime("dialer");
    let handles = connect_seeds_with(&dialer, std::slice::from_ref(&addr), "tcp", None, None, quick());

    // Dialer introduces itself so the seed can route back.
    let mut sent = false;
    for _ in 0..2000 {
        if push(&dialer, &addr, "dialer", "t.up").await {
            sent = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(sent);
    assert!(until(|| srv.peer_ids().contains(&"dialer".to_string())).await);
    // Seed pushes to the dialer over the same connection.
    let msg = KernelMessage::text(0, MessageTarget::Topic("t.down".into()), "x");
    srv.send_to_peer("dialer", MeshIpcEnvelope::new("srv".into(), "dialer".into(), msg))
        .await
        .unwrap();
    assert!(seen(&drec, "t.down").await, "dialled connection must read the seed's frames");
    for h in handles {
        h.abort();
    }
}

#[tokio::test]
async fn seed_connection_redials_after_the_seed_drops_it() {
    let (srv, srec) = runtime("srv");
    let (addr, _task) = listen(&srv, None).await;
    let (dialer, _) = runtime("dialer");
    let handles = connect_seeds_with(&dialer, std::slice::from_ref(&addr), "tcp", None, None, quick());

    let mut sent = false;
    for _ in 0..2000 {
        if push(&dialer, &addr, "dialer", "t.one").await {
            sent = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(sent);
    assert!(seen(&srec, "t.one").await);
    // Seed cuts the connection (route check closes it).
    srv.disconnect_peer("dialer");
    // The dialer redials and traffic flows again.
    let mut n = 0;
    let mut ok = false;
    while n < 2000 && !ok {
        n += 1;
        if push(&dialer, &addr, "dialer", "t.two").await {
            ok = seen_for(&srec, "t.two").await;
        } else {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
    assert!(ok, "dialer never reconnected");
    for h in handles {
        h.abort();
    }
}

/// Bounded check that a send which went into a dying connection did not
/// arrive (the dialer resends on the next loop turn).
async fn seen_for(rec: &Rec, topic: &str) -> bool {
    for _ in 0..20 {
        if rec.0.lock().unwrap().iter().any(|t| t == topic) {
            return true;
        }
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    false
}

#[tokio::test]
async fn seed_dial_keeps_redialling_after_failures_until_aborted() {
    // Every connection is closed at once, so each dial "fails"; the loop
    // must come back repeatedly.
    let (addr, n) = counting_listener(false).await;
    let (dialer, _) = runtime("dialer");
    let handles = connect_seeds_with(&dialer, std::slice::from_ref(&addr), "tcp", None, None, quick());
    assert!(until(|| count(&n) >= 3).await, "dial loop gave up");
    for h in &handles {
        h.abort();
    }
}

#[tokio::test]
async fn silent_seed_is_dropped_and_redialled() {
    // The seed accepts and never speaks: a half-open connection.
    let (addr, n) = counting_listener(true).await;
    let (dialer, _) = runtime("dialer");
    let timing = SeedTiming { idle: Duration::from_millis(100), ..quick() };
    let handles = connect_seeds_with(&dialer, std::slice::from_ref(&addr), "tcp", None, None, timing);
    assert!(until(|| count(&n) >= 2).await, "silent seed was never redialled");
    for h in &handles {
        h.abort();
    }
}

#[tokio::test]
async fn duplicate_seed_addresses_are_dialled_once() {
    let (addr, n) = counting_listener(true).await;
    let (dialer, _) = runtime("dialer");
    let seeds = vec![addr.clone(), addr.clone(), addr];
    let handles = connect_seeds_with(&dialer, &seeds, "tcp", None, None, quick());
    assert_eq!(handles.len(), 1);
    assert!(until(|| count(&n) >= 1).await);
    handles[0].abort();
}

#[tokio::test]
async fn aborting_a_dial_removes_its_route_and_emits_left() {
    let (srv, _) = runtime("srv");
    let (addr, _task) = listen(&srv, None).await;
    let (dialer, _) = runtime("dialer");
    let mut events = dialer.subscribe_peer_events();
    let handles = connect_seeds_with(&dialer, std::slice::from_ref(&addr), "tcp", None, None, quick());
    assert!(until(|| dialer.peer_ids() == vec![addr.clone()]).await);
    for h in &handles {
        h.abort();
    }
    assert!(until(|| dialer.peer_ids().is_empty()).await, "dead route left after abort");
    let mut left = false;
    while let Ok(ev) = events.try_recv() {
        left |= matches!(ev, crate::mesh_discovery::MeshPeerEvent::Left { .. });
    }
    assert!(left, "no Left event after abort");
}

#[tokio::test]
async fn dialled_connection_binds_to_the_first_id_and_cannot_claim_a_routed_one() {
    use crate::mesh_noise::{EncryptedChannel, PassthroughChannel};
    let mut l = crate::mesh_tcp::TcpTransport.listen("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let (dialer, drec) = runtime("dialer");
    // An id already routed through another connection.
    let (victim_tx, _victim_rx) = tokio::sync::mpsc::channel(4);
    dialer.add_peer("victim".into(), victim_tx.clone());
    let handles = connect_seeds_with(&dialer, std::slice::from_ref(&addr), "tcp", None, None, quick());
    let (stream, _) = l.accept().await.unwrap();
    let mut seed = PassthroughChannel::new(stream);
    for (src, topic) in [("victim", "t.1"), ("a", "t.2"), ("b", "t.3"), ("a", "t.4")] {
        seed.send_encrypted(&frame(src, topic)).await.unwrap();
    }
    assert!(seen(&drec, "t.4").await);
    let got = drec.0.lock().unwrap().clone();
    assert_eq!(got, vec!["t.2".to_string(), "t.4".to_string()], "binding must drop 'victim' and 'b'");
    // The victim's route is untouched.
    assert!(!dialer.route_is_foreign("victim", &victim_tx));
    for h in &handles {
        h.abort();
    }
}

#[test]
fn backoff_grows_is_capped_and_jittered() {
    let (base, max) = (Duration::from_secs(1), Duration::from_secs(60));
    for f in 0..40u32 {
        let d = seed_backoff(f, base, max);
        let nominal = base.saturating_mul(1u32 << f.min(16)).min(max);
        assert!(d >= nominal / 2 && d <= nominal, "failure {f}: {d:?} vs {nominal:?}");
    }
    let samples: std::collections::HashSet<_> =
        (0..20).map(|_| seed_backoff(5, base, max).as_nanos()).collect();
    assert!(samples.len() > 1, "no jitter");
}

// ── forward compatibility ────────────────────────────────────────

#[tokio::test]
async fn envelope_from_a_newer_peer_with_an_unknown_field_is_delivered() {
    let (rt, rec) = runtime("srv");
    let (addr, _task) = listen(&rt, None).await;
    let mut v: serde_json::Value = serde_json::from_slice(&frame("n1", "t.future")).unwrap();
    v["from_the_future"] = serde_json::json!({"x": 1});
    let mut c = crate::mesh_tcp::TcpTransport.connect(&addr).await.unwrap();
    c.send(&serde_json::to_vec(&v).unwrap()).await.unwrap();
    assert!(seen(&rec, "t.future").await, "unknown fields must be ignored, not rejected");
}

#[test]
fn an_older_relay_reserialising_strips_scope_fields() {
    // Documents ADR-103 A10: a peer whose envelope type predates the scope
    // fields drops them when it deserialises and re-serialises.
    #[derive(serde::Serialize, serde::Deserialize)]
    struct OldEnvelope {
        source_node: String,
        dest_node: String,
        message: KernelMessage,
        hop_count: u8,
        envelope_id: String,
    }
    let msg = KernelMessage::text(0, MessageTarget::Topic("t".into()), "x");
    let mut env = MeshIpcEnvelope::new("a".into(), "b".into(), msg);
    env.dest_scope = Some(Scope { user_id: "u".repeat(32), project_id: None });
    let old: OldEnvelope = serde_json::from_slice(&env.to_bytes().unwrap()).unwrap();
    let relayed = serde_json::to_vec(&old).unwrap();
    let back = MeshIpcEnvelope::from_bytes(&relayed).unwrap();
    assert!(back.dest_scope.is_none());
}
