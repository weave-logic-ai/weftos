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

async fn seen(rec: &Rec, topic: &str) -> bool {
    for _ in 0..150 {
        if rec.0.lock().unwrap().iter().any(|t| t == topic) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
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
fn route_loss_check_does_not_scale_with_peer_count() {
    let rt = MeshRuntime::new("local".into());
    let mut keep = Vec::new();
    for i in 0..20_000 {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        rt.add_peer(format!("p{i}"), tx);
        keep.push(rx);
    }
    let t = RouteTally::default();
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    rt.add_peer_tallied("mine".into(), tx, &t);
    // 100k checks; a full scan of 20k peers each would be ~2e9 visits.
    let start = std::time::Instant::now();
    let mut live = 0;
    for _ in 0..100_000 {
        live += usize::from(t.live() > 0);
    }
    assert_eq!(live, 100_000);
    assert!(start.elapsed() < Duration::from_millis(500), "check scales with peers");
}

#[tokio::test]
async fn connection_closes_when_its_route_is_revoked() {
    let (rt, _) = runtime("srv");
    let (addr, _task) = listen(&rt, None).await;
    let mut c = crate::mesh_tcp::TcpTransport.connect(&addr).await.unwrap();
    c.send(&frame("n1", "t.a")).await.unwrap();
    for _ in 0..100 {
        if !rt.peer_ids().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
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
    for _ in 0..100 {
        if !rt.peer_ids().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let mut new = crate::mesh_tcp::TcpTransport.connect(&addr).await.unwrap();
    new.send(&frame("n1", "t.b")).await.unwrap();
    assert!(matches!(tokio::time::timeout(Duration::from_secs(2), old.recv()).await, Ok(Err(_))),
        "replaced connection must close");
    // The replacement stays up.
    assert!(tokio::time::timeout(Duration::from_millis(600), new.recv()).await.is_err());
    assert_eq!(rt.peer_ids(), vec!["n1".to_string()]);
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
    for _ in 0..100 {
        if push(&dialer, &addr, "dialer", "t.up").await {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    for _ in 0..100 {
        if srv.peer_ids().contains(&"dialer".to_string()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
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

    for _ in 0..100 {
        if push(&dialer, &addr, "dialer", "t.one").await {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(seen(&srec, "t.one").await);
    // Seed cuts the connection (route check closes it).
    srv.disconnect_peer("dialer");
    // The dialer redials and traffic flows again.
    let mut ok = false;
    for _ in 0..200 {
        if push(&dialer, &addr, "dialer", "t.two").await && seen_quick(&srec, "t.two").await {
            ok = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    assert!(ok, "dialer never reconnected");
    for h in handles {
        h.abort();
    }
}

async fn seen_quick(rec: &Rec, topic: &str) -> bool {
    tokio::time::sleep(Duration::from_millis(40)).await;
    rec.0.lock().unwrap().iter().any(|t| t == topic)
}

#[tokio::test]
async fn seed_dial_to_a_dead_address_keeps_retrying_until_aborted() {
    // Reserve then free a port so nothing listens on it.
    let l = crate::mesh_tcp::TcpTransport.listen("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    drop(l);
    let (dialer, _) = runtime("dialer");
    let handles = connect_seeds_with(&dialer, std::slice::from_ref(&addr), "tcp", None, None, quick());
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!handles[0].is_finished(), "dial loop must survive failures");
    handles[0].abort();
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
