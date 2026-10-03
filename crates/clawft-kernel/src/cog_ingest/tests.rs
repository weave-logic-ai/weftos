//! Hermetic tests: loopback sockets on ephemeral ports, in-process mesh
//! streams, in-memory stores. Nothing touches `~/.weftos` or `~/.clawft`.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use ed25519_dalek::SigningKey;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};

use super::*;
use crate::workload_runtime::HostContract;

pub(super) const PROJECT: &str = "01J9ZXW0PRJCTAAAAAAAAAAAAA";

pub(super) fn key(n: u8) -> SigningKey {
    SigningKey::from_bytes(&[n; 32])
}

pub(super) fn pubkey(k: &SigningKey) -> [u8; 32] {
    k.verifying_key().to_bytes()
}

pub(super) fn node_id(k: &SigningKey) -> String {
    crate::node_registry::node_id_from_pubkey(&pubkey(k))
}

pub(super) fn mem() -> Arc<MemoryIngestStore> {
    Arc::new(MemoryIngestStore::new(10_000))
}

/// Raw HTTP exchange; returns (status, body).
async fn http(addr: SocketAddr, raw: &[u8]) -> (u16, String) {
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(raw).await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut out)).await;
    let text = String::from_utf8_lossy(&out).to_string();
    let status = text
        .split(' ')
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    (status, body)
}

pub(super) async fn post(addr: SocketAddr, token: Option<&str>, body: &str) -> (u16, String) {
    let auth = token
        .map(|t| format!("Authorization: Bearer {t}\r\n"))
        .unwrap_or_default();
    let raw = format!(
        "POST {INGEST_PATH} HTTP/1.1\r\nHost: x\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    http(addr, raw.as_bytes()).await
}

pub(super) fn batch_json(vs: &[(u64, [f32; 8])], dedup: bool) -> String {
    serde_json::json!({
        "vectors": vs.iter().map(|(i, v)| serde_json::json!([i, v])).collect::<Vec<_>>(),
        "dedup": dedup,
    })
    .to_string()
}

pub(super) fn vec8(seed: f32) -> [f32; 8] {
    std::array::from_fn(|i| seed + i as f32 * 0.01)
}

/// Register an instance and return its token.
pub(super) fn register(
    reg: &TokenRegistry,
    instance: &str,
    project: Option<&str>,
    controller: &str,
) -> String {
    let c = HostContract::default_feed();
    reg.register(
        InstanceBinding::new(instance, project.map(String::from), controller),
        &c,
    )
    .unwrap();
    c.token.expose().to_string()
}

pub(super) fn bridge_over(router: StaticRouter, budget: RateBudget) -> (Arc<IngestBridge>, Arc<TokenRegistry>) {
    let reg = Arc::new(TokenRegistry::new());
    (
        IngestBridge::new(reg.clone(), Arc::new(router), budget, BridgeConfig::default()),
        reg,
    )
}

pub(super) fn lo() -> SocketAddr {
    "127.0.0.1:0".parse().unwrap()
}

/// ADR-069 MAGIC_FEATURES packet: magic u32, 12 pad bytes, 8 LE f32.
fn feature_packet(vals: &[f32; 8]) -> Vec<u8> {
    let mut p = Vec::with_capacity(48);
    p.extend_from_slice(&0xC5110003u32.to_le_bytes());
    p.extend_from_slice(&[0u8; 12]);
    for v in vals {
        p.extend_from_slice(&v.to_le_bytes());
    }
    p
}

fn local_router(controller: &str, store: Arc<MemoryIngestStore>) -> StaticRouter {
    let dir: Arc<dyn StoreDirectory> = Arc::new(StaticDirectory::new().with_fallback(store));
    StaticRouter::new().with_controller(controller, Arc::new(LocalForwarder::new("node-a", dir)))
}

// ── Contract validation ─────────────────────────────────────────────────

#[test]
fn parse_accepts_the_contract_shape() {
    let b = parse_batch(br#"{"vectors":[[7,[0,1,2,3,4,5,6,7.5]]],"dedup":true}"#).unwrap();
    assert!(b.dedup);
    assert_eq!(b.vectors[0].id, 7);
    assert_eq!(b.vectors[0].values[7], 7.5);
    assert!(!parse_batch(br#"{"vectors":[[1,[0,0,0,0,0,0,0,0]]]}"#).unwrap().dedup);
}

#[test]
fn parse_rejects_malformed_shapes() {
    let eight = "[0,0,0,0,0,0,0,0]";
    for (name, body) in [
        ("not json", "nope".to_string()),
        ("array body", "[]".to_string()),
        ("empty", r#"{"vectors":[]}"#.to_string()),
        ("missing vectors", r#"{"dedup":true}"#.to_string()),
        ("unknown key", format!(r#"{{"vectors":[[1,{eight}]],"x":1}}"#)),
        ("dedup type", format!(r#"{{"vectors":[[1,{eight}]],"dedup":"yes"}}"#)),
        ("7 dims", r#"{"vectors":[[1,[0,0,0,0,0,0,0]]]}"#.to_string()),
        ("9 dims", r#"{"vectors":[[1,[0,0,0,0,0,0,0,0,0]]]}"#.to_string()),
        ("negative id", format!(r#"{{"vectors":[[-1,{eight}]]}}"#)),
        ("float id", format!(r#"{{"vectors":[[1.5,{eight}]]}}"#)),
        ("string value", r#"{"vectors":[[1,[0,0,0,0,0,0,0,"a"]]]}"#.to_string()),
        ("not a pair", format!(r#"{{"vectors":[[1,{eight},3]]}}"#)),
        ("NaN literal", r#"{"vectors":[[1,[NaN,0,0,0,0,0,0,0]]]}"#.to_string()),
        ("f32 overflow", r#"{"vectors":[[1,[1e39,0,0,0,0,0,0,0]]]}"#.to_string()),
        ("f64 overflow", r#"{"vectors":[[1,[1e999,0,0,0,0,0,0,0]]]}"#.to_string()),
    ] {
        assert!(
            matches!(parse_batch(body.as_bytes()), Err(IngestError::Malformed(_))),
            "{name} must be malformed"
        );
    }
}

#[test]
fn parse_rejects_oversize_batches_and_bodies() {
    let one = "[1,[0,0,0,0,0,0,0,0]]";
    let many = vec![one; types::MAX_VECTORS_PER_BATCH + 1].join(",");
    assert!(matches!(
        parse_batch(format!(r#"{{"vectors":[{many}]}}"#).as_bytes()),
        Err(IngestError::TooLarge(_))
    ));
    let at_cap = vec![one; types::MAX_VECTORS_PER_BATCH].join(",");
    assert!(parse_batch(format!(r#"{{"vectors":[{at_cap}]}}"#).as_bytes()).is_ok());
    let huge = vec![b' '; types::MAX_BODY_BYTES + 1];
    assert!(matches!(parse_batch(&huge), Err(IngestError::TooLarge(_))));
}

// ── Feed -> cog -> bridge -> store ──────────────────────────────────────

#[tokio::test]
async fn replayed_feed_through_a_cog_stub_lands_in_the_store_and_dedups() {
    let store = mem();
    let (bridge, reg) = bridge_over(local_router("ctl", store.clone()), RateBudget::default());
    let token = register(&reg, "inst-1", None, "ctl");
    let h = bridge.bind(lo(), BridgeScope::Any).await.unwrap();

    // The recorded feed: 6 ticks, tick 4 repeats tick 1's values.
    let ticks: Vec<[f32; 8]> = vec![
        vec8(0.1),
        vec8(0.2),
        vec8(0.3),
        vec8(0.4),
        vec8(0.2),
        vec8(0.6),
    ];
    // Cog stub: a UDP listener that turns each feature packet into a vector.
    let cog_sock = UdpSocket::bind(lo()).await.unwrap();
    let cog_addr = cog_sock.local_addr().unwrap();
    let feed = UdpSocket::bind(lo()).await.unwrap();
    for t in &ticks {
        feed.send_to(&feature_packet(t), cog_addr).await.unwrap();
    }
    let mut got = Vec::new();
    for tick in 0..ticks.len() as u64 {
        let mut buf = [0u8; 64];
        let (n, _) = cog_sock.recv_from(&mut buf).await.unwrap();
        assert_eq!(n, 48);
        assert_eq!(u32::from_le_bytes(buf[..4].try_into().unwrap()), 0xC5110003);
        let v: [f32; 8] =
            std::array::from_fn(|i| f32::from_le_bytes(buf[16 + 4 * i..20 + 4 * i].try_into().unwrap()));
        got.push((tick, v));
    }
    let (st, body) = post(h.addr(), Some(&token), &batch_json(&got, true)).await;
    assert_eq!(st, 200, "{body}");
    let r: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!((r["accepted"].as_u64(), r["deduped"].as_u64()), (Some(5), Some(1)));
    assert_eq!(store.len(), 5, "identical values at tick 4 deduped");

    // Posting the same batch again: everything is deduped by id.
    let (st, body) = post(h.addr(), Some(&token), &batch_json(&got, true)).await;
    assert_eq!(st, 200);
    let r: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!((r["accepted"].as_u64(), r["deduped"].as_u64()), (Some(0), Some(6)));
    assert_eq!(store.len(), 5);

    // With dedup off the ids are upserted, not duplicated.
    let (st, _) = post(h.addr(), Some(&token), &batch_json(&got[..2], false)).await;
    assert_eq!(st, 200);
    assert_eq!(store.len(), 5);

    // The vectors are queryable and carry the instance as provenance.
    let hits = store.query(&vec8(0.3), 1);
    assert_eq!(hits[0].id, 2);
    assert_eq!(store.provenance("inst-1", 2).unwrap().instance_id, "inst-1");
    assert_eq!(bridge.stats().vectors.load(std::sync::atomic::Ordering::Relaxed), 7);
}

// ── Rejections at the bridge ────────────────────────────────────────────

#[tokio::test]
async fn bridge_rejects_bad_requests_and_tokens() {
    let store = mem();
    let (bridge, reg) = bridge_over(local_router("ctl", store.clone()), RateBudget::default());
    let t1 = register(&reg, "inst-1", None, "ctl");
    let t2 = register(&reg, "inst-2", None, "ctl");
    let any = bridge.bind(lo(), BridgeScope::Any).await.unwrap();
    let only1 = bridge
        .bind(lo(), BridgeScope::Instance("inst-1".into()))
        .await
        .unwrap();
    let ok = batch_json(&[(1, vec8(1.0))], false);

    assert_eq!(post(any.addr(), None, &ok).await.0, 401, "no token");
    assert_eq!(post(any.addr(), Some("x".repeat(40).as_str()), &ok).await.0, 401, "unknown token");
    // Another instance's token on an instance-scoped listener.
    assert_eq!(post(only1.addr(), Some(&t2), &ok).await.0, 403);
    assert_eq!(post(only1.addr(), Some(&t1), &ok).await.0, 200);
    assert_eq!(store.len(), 1);

    // Revoked at unload.
    assert!(reg.revoke("inst-2"));
    assert_eq!(post(any.addr(), Some(&t2), &ok).await.0, 401);

    // X-API-Key is accepted too.
    let ok2 = batch_json(&[(2, vec8(2.0))], false);
    let raw = format!(
        "POST {INGEST_PATH} HTTP/1.1\r\nX-API-Key: {t1}\r\nContent-Length: {}\r\n\r\n{ok2}",
        ok2.len()
    );
    assert_eq!(http(any.addr(), raw.as_bytes()).await.0, 200);

    // Shape, dimensions, batch size.
    assert_eq!(post(any.addr(), Some(&t1), "garbage").await.0, 400);
    assert_eq!(
        post(any.addr(), Some(&t1), r#"{"vectors":[[1,[0,0,0]]]}"#).await.0,
        400
    );
    let many: Vec<_> = (0..257u64).map(|i| (i, vec8(0.0))).collect();
    assert_eq!(post(any.addr(), Some(&t1), &batch_json(&many, false)).await.0, 413);

    // A declared body over the cap is refused without being read.
    let raw = format!(
        "POST {INGEST_PATH} HTTP/1.1\r\nAuthorization: Bearer {t1}\r\nContent-Length: 10000000\r\n\r\n"
    );
    assert_eq!(http(any.addr(), raw.as_bytes()).await.0, 413);
    // Unauthenticated: 401 before anything about the body is considered.
    let raw = format!("POST {INGEST_PATH} HTTP/1.1\r\nContent-Length: 10000000\r\n\r\n");
    assert_eq!(http(any.addr(), raw.as_bytes()).await.0, 401);

    // Request-shape errors.
    let raw = format!("POST {INGEST_PATH} HTTP/1.1\r\nAuthorization: Bearer {t1}\r\n\r\n");
    assert_eq!(http(any.addr(), raw.as_bytes()).await.0, 400, "no Content-Length");
    let raw = format!(
        "POST {INGEST_PATH} HTTP/1.1\r\nAuthorization: Bearer {t1}\r\nTransfer-Encoding: chunked\r\n\r\n"
    );
    assert_eq!(http(any.addr(), raw.as_bytes()).await.0, 400, "chunked");
    assert_eq!(http(any.addr(), b"GET /api/v1/store/ingest HTTP/1.1\r\n\r\n").await.0, 400);
    assert_eq!(
        http(any.addr(), b"POST /other HTTP/1.1\r\nContent-Length: 0\r\n\r\n").await.0,
        400
    );
    assert_eq!(store.len(), 2, "only the two valid posts landed");
}

#[tokio::test]
async fn bridge_rate_limits_per_instance() {
    let store = mem();
    let budget = RateBudget::new(Duration::from_secs(60), 3, 4);
    let (bridge, reg) = bridge_over(local_router("ctl", store.clone()), budget);
    let t1 = register(&reg, "inst-1", None, "ctl");
    let t2 = register(&reg, "inst-2", None, "ctl");
    let h = bridge.bind(lo(), BridgeScope::Any).await.unwrap();
    let one = batch_json(&[(99, vec8(9.0))], false);
    // Request budget (3): three single-vector posts, then a 2-vector post is over.
    for i in 0..3u64 {
        let b = batch_json(&[(i, vec8(i as f32))], false);
        assert_eq!(post(h.addr(), Some(&t1), &b).await.0, 200);
    }
    let two = batch_json(&[(10, vec8(1.0)), (11, vec8(2.0))], false);
    assert_eq!(post(h.addr(), Some(&t1), &two).await.0, 429, "request budget spent");
    // Another instance is unaffected.
    assert_eq!(post(h.addr(), Some(&t2), &one).await.0, 200);
    assert_eq!(store.len(), 4);
}

#[tokio::test]
async fn bridge_vector_budget_is_separate_from_request_budget() {
    let store = mem();
    let budget = RateBudget::new(Duration::from_secs(60), 100, 3);
    let (bridge, reg) = bridge_over(local_router("ctl", store.clone()), budget);
    let t = register(&reg, "inst-1", None, "ctl");
    let h = bridge.bind(lo(), BridgeScope::Any).await.unwrap();
    let four: Vec<_> = (0..4u64).map(|i| (i, vec8(i as f32))).collect();
    assert_eq!(post(h.addr(), Some(&t), &batch_json(&four, false)).await.0, 429);
    assert_eq!(store.len(), 0);
}

#[tokio::test]
async fn bind_is_loopback_only_unless_an_instance_scope_allows_more() {
    let (bridge, _reg) = bridge_over(StaticRouter::new(), RateBudget::default());
    let public: SocketAddr = "0.0.0.0:0".parse().unwrap();
    assert!(bridge.bind(public, BridgeScope::Any).await.is_err());
    assert!(
        bridge
            .bind(public, BridgeScope::Instance("i".into()))
            .await
            .is_err(),
        "needs the config flag as well"
    );
    let cfg = BridgeConfig {
        allow_non_loopback: true,
        ..Default::default()
    };
    let b2 = IngestBridge::new(
        Arc::new(TokenRegistry::new()),
        Arc::new(StaticRouter::new()),
        RateBudget::default(),
        cfg,
    );
    assert!(b2.bind(public, BridgeScope::Any).await.is_err(), "shared scope stays loopback");
    assert!(b2.bind(public, BridgeScope::Instance("i".into())).await.is_ok());
}

#[tokio::test]
async fn registry_rejects_duplicates_and_weak_tokens() {
    let reg = TokenRegistry::new();
    let c = HostContract::default_feed();
    let b = InstanceBinding::new("i", None, "ctl");
    reg.register(b.clone(), &c).unwrap();
    assert!(reg.register(b, &c).is_err(), "instance twice");
    let mut weak = HostContract::default_feed();
    weak.token = clawft_types::secret::SecretString::new("short");
    assert!(reg.register(InstanceBinding::new("j", None, "ctl"), &weak).is_err());
    assert_eq!(reg.len(), 1);
}

// ── Project routing ─────────────────────────────────────────────────────

#[tokio::test]
async fn project_less_placements_fall_back_to_the_controllers_store() {
    let (ctl_store, proj_store) = (mem(), mem());
    let dir: Arc<dyn StoreDirectory> = Arc::new(
        StaticDirectory::new()
            .with_fallback(ctl_store.clone())
            .with_project(PROJECT, proj_store.clone()),
    );
    let fwd: Arc<dyn Forwarder> = Arc::new(LocalForwarder::new("node-a", dir));
    let router = StaticRouter::new()
        .with_project(PROJECT, fwd.clone())
        .with_controller("ctl", fwd);
    let (bridge, reg) = bridge_over(router, RateBudget::default());
    let tp = register(&reg, "with-project", Some(PROJECT), "ctl");
    let tn = register(&reg, "no-project", None, "ctl");
    let h = bridge.bind(lo(), BridgeScope::Any).await.unwrap();
    assert_eq!(post(h.addr(), Some(&tp), &batch_json(&[(1, vec8(1.0))], false)).await.0, 200);
    assert_eq!(post(h.addr(), Some(&tn), &batch_json(&[(2, vec8(2.0))], false)).await.0, 200);
    assert_eq!((proj_store.len(), ctl_store.len()), (1, 1));
    assert!(proj_store.provenance("with-project", 1).is_some() && ctl_store.provenance("no-project", 2).is_some());
}

#[tokio::test]
async fn a_project_without_a_route_is_refused_not_redirected() {
    let ctl_store = mem();
    // Only the controller route exists; the project has none.
    let (bridge, reg) = bridge_over(local_router("ctl", ctl_store.clone()), RateBudget::default());
    let t = register(&reg, "inst", Some(PROJECT), "ctl");
    let h = bridge.bind(lo(), BridgeScope::Any).await.unwrap();
    let (st, body) = post(h.addr(), Some(&t), &batch_json(&[(1, vec8(1.0))], false)).await;
    assert_eq!(st, 502, "{body}");
    assert_eq!(ctl_store.len(), 0, "project data never lands in the controller's store");
}

// ── UDP relay ───────────────────────────────────────────────────────────

#[tokio::test]
async fn udp_forwarder_relays_feed_packets_and_drops_the_rest() {
    let dest = UdpSocket::bind(lo()).await.unwrap();
    let f = UdpForwarder::spawn(UdpForwardConfig {
        listen: lo(),
        dest: dest.local_addr().unwrap(),
        allowed_source: Some("127.0.0.1".parse().unwrap()),
    })
    .await
    .unwrap();
    let src = UdpSocket::bind(lo()).await.unwrap();
    src.send_to(&[], f.local_addr()).await.unwrap();
    src.send_to(&vec![7u8; 600], f.local_addr()).await.unwrap();
    let pkt = feature_packet(&vec8(0.5));
    src.send_to(&pkt, f.local_addr()).await.unwrap();
    let mut buf = [0u8; 1024];
    let (n, _) = tokio::time::timeout(Duration::from_secs(2), dest.recv_from(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&buf[..n], pkt.as_slice(), "the first datagram to arrive is the valid one");
    assert_eq!((f.forwarded(), f.dropped()), (1, 2));

    // A source other than the allowed sensor is dropped.
    let strict = UdpForwarder::spawn(UdpForwardConfig {
        listen: lo(),
        dest: dest.local_addr().unwrap(),
        allowed_source: Some("10.9.9.9".parse().unwrap()),
    })
    .await
    .unwrap();
    src.send_to(&pkt, strict.local_addr()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!((strict.forwarded(), strict.dropped()), (0, 1));
}

// ── The kernel's vector backend as the store ────────────────────────────

#[cfg(feature = "ecc")]
#[tokio::test]
async fn hnsw_backend_store_takes_batches_with_dedup() {
    let backend: Arc<dyn crate::vector_backend::VectorBackend> = Arc::new(
        crate::vector_hnsw::HnswBackend::new(crate::hnsw_service::HnswServiceConfig::default()),
    );
    let store = Arc::new(VectorBackendStore::new(backend));
    let dir: Arc<dyn StoreDirectory> = Arc::new(StaticDirectory::new().with_fallback(store.clone()));
    let router = StaticRouter::new().with_controller("ctl", Arc::new(LocalForwarder::new("n", dir)));
    let (bridge, reg) = bridge_over(router, RateBudget::default());
    let t = register(&reg, "inst", None, "ctl");
    let h = bridge.bind(lo(), BridgeScope::Any).await.unwrap();
    let vs: Vec<_> = (0..4u64).map(|i| (i, vec8(i as f32))).collect();
    assert_eq!(post(h.addr(), Some(&t), &batch_json(&vs, true)).await.0, 200);
    let (_, body) = post(h.addr(), Some(&t), &batch_json(&vs, true)).await;
    assert!(body.contains(r#""deduped":4"#), "{body}");
    assert_eq!(store.len(), 4);
    assert_eq!(store.query(&vec8(2.0), 1)[0].id, 2);
}
