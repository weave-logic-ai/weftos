//! The loopback proxy end to end: real listener on a random loopback
//! port, fake upstreams on other random ports. Port 8090, 8081 and 11434
//! are never used.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use super::listener::{InferProxy, OccupiedPolicy, Started};
use super::support::*;
use super::table::PlacementTable;
use super::types::*;
use crate::workload_runtime::infer::fakes as f;
use crate::workload_runtime::infer::{InferFlavor, InferRuntime};

const LO: &str = "127.0.0.1:0";

fn table() -> Arc<PlacementTable> {
    Arc::new(PlacementTable::new("node-a", None, None))
}

async fn start(t: &Arc<PlacementTable>, limits: ProxyLimits) -> InferProxy {
    match InferProxy::start(
        "hermes",
        LO.parse().unwrap(),
        OccupiedPolicy::Refuse,
        t.clone(),
        limits,
        None,
    )
    .await
    .unwrap()
    {
        Started::Running(p) => p,
        Started::Adopted(_) => panic!("adopted"),
    }
}

fn local(t: &PlacementTable, fk: &Fake) {
    t.register_local("hermes", &fk.base(), Some("m".into()), "openai-v1", "LlamaCpp")
        .unwrap();
}

#[tokio::test]
async fn forwards_a_post_to_the_local_instance() {
    let up = fake(Reply::ok(r#"{"id":"x"}"#)).await;
    let t = table();
    local(&t, &up);
    let p = start(&t, small_limits()).await;
    let body = r#"{"model":"m","messages":[]}"#;
    let mut req = post(p.addr(), "/v1/chat/completions", body);
    // Add an Authorization header by rebuilding the head.
    let s = String::from_utf8(req.clone()).unwrap().replace(
        "Content-Type:",
        "Authorization: Bearer abc\r\nContent-Type:",
    );
    req = s.into_bytes();
    let resp = raw(p.addr(), &req).await;
    assert_eq!(status(&resp), 200, "{resp}");
    assert_eq!(body_of(&resp), r#"{"id":"x"}"#);
    let seen = up.last();
    assert_eq!(seen.request_line, "POST /v1/chat/completions HTTP/1.1");
    assert_eq!(seen.body, body.as_bytes());
    assert_eq!(seen.header("content-type"), Some("application/json"));
    assert_eq!(seen.header("authorization"), Some("Bearer abc"));
    assert_eq!(p.stats().local.load(Ordering::Relaxed), 1);
}

fn body_of(r: &str) -> &str {
    body(r)
}

#[tokio::test]
async fn relays_a_streamed_response_in_order() {
    let mut r = Reply::ok("");
    r.content_type = "text/event-stream";
    r.chunks = (0..5).map(|i| format!("data: {i}\n\n").into_bytes()).collect();
    r.gap = Duration::from_millis(20);
    let up = fake(r).await;
    let t = table();
    local(&t, &up);
    let p = start(&t, small_limits()).await;
    let resp = raw(p.addr(), &post(p.addr(), "/v1/chat/completions", "{}")).await;
    assert_eq!(status(&resp), 200);
    assert!(resp.contains("text/event-stream"));
    assert_eq!(body_of(&resp), "data: 0\n\ndata: 1\n\ndata: 2\n\ndata: 3\n\ndata: 4\n\n");
}

#[tokio::test]
async fn an_unserved_role_is_a_503_not_a_hang() {
    let t = table();
    let p = start(&t, small_limits()).await;
    let resp = raw(p.addr(), &get(p.addr(), "/v1/models")).await;
    assert_eq!(status(&resp), 503, "{resp}");
    assert!(body_of(&resp).contains("no instance"));
}

#[tokio::test]
async fn a_client_cannot_make_the_proxy_reach_another_host() {
    let victim = fake(Reply::ok("secret")).await;
    let up = fake(Reply::ok("ok")).await;
    let t = table();
    local(&t, &up);
    let p = start(&t, small_limits()).await;
    let v = victim.addr;
    let attempts: Vec<Vec<u8>> = vec![
        format!("GET http://{v}/v1/models HTTP/1.1\r\nHost: {v}\r\n\r\n").into_bytes(),
        format!("GET //{v}/v1/models HTTP/1.1\r\nHost: {}\r\n\r\n", p.addr()).into_bytes(),
        format!("GET /v1/models HTTP/1.1\r\nHost: {v}.evil.example\r\n\r\n").into_bytes(),
        format!("GET @{v}/v1/models HTTP/1.1\r\nHost: {}\r\n\r\n", p.addr()).into_bytes(),
        format!("CONNECT {v} HTTP/1.1\r\nHost: {v}\r\n\r\n").into_bytes(),
        format!("GET /v1/models HTTP/1.1\r\nHost: {}\r\nX-Forwarded-Host: {v}\r\nOrigin: http://{v}.evil.example\r\n\r\n", p.addr()).into_bytes(),
    ];
    for a in attempts {
        let resp = raw(p.addr(), &a).await;
        assert!(
            matches!(status(&resp), 400 | 403 | 405),
            "{} -> {resp}",
            String::from_utf8_lossy(&a)
        );
    }
    assert_eq!(victim.count(), 0, "the proxy contacted a host a client named");
    assert_eq!(up.count(), 0);
}

#[tokio::test]
async fn refuses_management_paths_and_oversized_bodies() {
    let up = fake(Reply::ok("ok")).await;
    let t = table();
    local(&t, &up);
    let p = start(&t, small_limits()).await;
    let r = raw(p.addr(), &post(p.addr(), "/api/pull", "{}")).await;
    assert_eq!(status(&r), 403);
    let huge = ProxyLimits { max_request_body: 64, ..small_limits() };
    let p2 = start(&t, huge).await;
    let r = raw(p2.addr(), &post(p2.addr(), "/v1/chat/completions", &"x".repeat(200))).await;
    assert_eq!(status(&r), 413);
    assert_eq!(up.count(), 0);
}

#[tokio::test]
async fn response_size_is_bounded() {
    let mut r = Reply::ok("");
    r.chunks = (0..50).map(|_| vec![b'z'; 1024]).collect();
    let up = fake(r).await;
    let t = table();
    local(&t, &up);
    let limits = ProxyLimits { max_response_body: 4096, ..small_limits() };
    let p = start(&t, limits).await;
    let resp = raw(p.addr(), &get(p.addr(), "/v1/models")).await;
    assert_eq!(status(&resp), 200);
    assert!(body_of(&resp).len() <= 4096, "{}", body_of(&resp).len());
}

#[tokio::test]
async fn a_stalled_upstream_is_cut_by_the_timeout() {
    let mut r = Reply::ok("part");
    r.hold = Duration::from_secs(30);
    let up = fake(r).await;
    let t = table();
    local(&t, &up);
    let p = start(&t, small_limits()).await;
    let t0 = std::time::Instant::now();
    let resp = raw(p.addr(), &get(p.addr(), "/v1/models")).await;
    assert!(t0.elapsed() < Duration::from_secs(8), "hung for {:?}", t0.elapsed());
    assert_eq!(status(&resp), 200);
    assert_eq!(body_of(&resp), "part");
}

#[tokio::test]
async fn a_dead_upstream_is_a_502() {
    let t = table();
    let dead = f::free_port();
    t.register_local("hermes", &format!("http://127.0.0.1:{dead}"), None, "openai-v1", "LlamaCpp")
        .unwrap();
    let p = start(&t, small_limits()).await;
    let resp = raw(p.addr(), &get(p.addr(), "/v1/models")).await;
    assert_eq!(status(&resp), 502, "{resp}");
}

#[tokio::test]
async fn the_address_stays_while_the_instance_moves() {
    let (a, b) = (fake(Reply::ok("from-a")).await, fake(Reply::ok("from-b")).await);
    let t = table();
    local(&t, &a);
    let p = start(&t, small_limits()).await;
    let addr = p.addr();
    let g0 = t.generation();
    assert_eq!(body_of(&raw(addr, &get(addr, "/v1/models")).await), "from-a");
    // Moves: same proxy address, new instance.
    local(&t, &b);
    assert!(t.generation() > g0, "a move bumps the generation");
    assert_eq!(body_of(&raw(addr, &get(addr, "/v1/models")).await), "from-b");
    // Placement dies: clean 503, and it recovers when it comes back.
    t.deregister_local("hermes");
    assert_eq!(status(&raw(addr, &get(addr, "/v1/models")).await), 503);
    local(&t, &a);
    assert_eq!(body_of(&raw(addr, &get(addr, "/v1/models")).await), "from-a");
    assert_eq!(t.base_url_for_role("hermes"), Some(format!("{}/v1", a.base())));
}

fn held_loopback() -> (std::net::TcpListener, SocketAddr) {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let a = l.local_addr().unwrap();
    (l, a)
}

#[tokio::test]
async fn an_occupied_port_is_never_bound_over() {
    let (holder, addr) = held_loopback();
    holder.set_nonblocking(true).unwrap();
    let audit = Arc::new(Audit::default());
    let t = table();
    let r = InferProxy::start("hermes", addr, OccupiedPolicy::Refuse, t.clone(), small_limits(), Some(audit.clone())).await;
    assert!(matches!(r, Err(ProxyError::Occupied(p)) if p == addr.port()));
    let r = InferProxy::start("hermes", addr, OccupiedPolicy::Adopt, t.clone(), small_limits(), Some(audit.clone())).await;
    assert!(matches!(r, Ok(Started::Adopted(a)) if a == addr));
    assert!(audit.kinds().iter().all(|k| k == "infer.proxy.occupied"), "{:?}", audit.kinds());
    // The holder is still the one that owns the port: a connection reaches it.
    let _c = std::net::TcpStream::connect(addr).unwrap();
    assert!(holder.accept().is_ok(), "the existing server must still get connections");
    assert_eq!(t.base_url_for_role("hermes"), None, "an adopted port is not a proxy port");
}

#[tokio::test]
async fn a_wildcard_listener_also_blocks_the_bind() {
    let wild = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
    let port = wild.local_addr().unwrap().port();
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let r = InferProxy::start("hermes", addr, OccupiedPolicy::Refuse, table(), small_limits(), None).await;
    assert!(matches!(r, Err(ProxyError::Occupied(_))), "bound over a wildcard listener");
}

#[tokio::test]
async fn a_port_freed_later_can_be_bound() {
    let (holder, addr) = held_loopback();
    drop(holder);
    let r = InferProxy::start("hermes", addr, OccupiedPolicy::Refuse, table(), small_limits(), None).await;
    let Ok(Started::Running(p)) = r else { panic!("not bound") };
    assert_eq!(p.addr(), addr);
}

#[tokio::test]
async fn binds_loopback_only() {
    for a in ["0.0.0.0:0", "192.0.2.7:0", "[::]:0"] {
        let r = InferProxy::start("hermes", a.parse().unwrap(), OccupiedPolicy::Refuse, table(), small_limits(), None).await;
        assert!(matches!(r, Err(ProxyError::NotLoopback(_))), "{a}");
    }
}

#[tokio::test]
async fn local_registration_demands_a_loopback_endpoint() {
    let t = table();
    for base in [
        "http://10.0.0.5:8090",
        "http://example.com:8090",
        "https://127.0.0.1:8090",
        "http://127.0.0.1",
        "http://127.0.0.1:0",
        "http://localhost:8090",
        "http://127.0.0.1:8090/v1",
    ] {
        assert!(t.register_local("hermes", base, None, "openai-v1", "x").is_err(), "{base}");
    }
    assert!(t.register_local("a/b", "http://127.0.0.1:9", None, "openai-v1", "x").is_err());
    assert!(t.register_local("hermes", "http://127.0.0.1:9", None, "openai-v1", "x").is_ok());
}

#[tokio::test]
async fn sync_local_follows_the_adapter() {
    // An adopted fake llama.cpp server; the adapter's health and endpoint
    // decide whether the role is served.
    let server = f::server_on(f::free_port()).await;
    f::mount_llama(&server, 200, "m.gguf").await;
    let rt: InferRuntime = f::adopted(InferFlavor::LlamaCpp);
    let h = f::load(&rt, f::spec("hermes", InferFlavor::LlamaCpp, f::port_of(&server))).await;
    let t = table();
    let o = t.sync_local("hermes", &rt, &h).await;
    assert!(o.registered, "{o:?}");
    assert_eq!(
        t.resolve("hermes"),
        Some(Target::Local { base: format!("http://127.0.0.1:{}", f::port_of(&server)) })
    );
    // The server goes away: the role stops resolving.
    drop(server);
    let mut o = t.sync_local("hermes", &rt, &h).await;
    for _ in 0..40 {
        if !o.registered {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        o = t.sync_local("hermes", &rt, &h).await;
    }
    assert!(!o.registered, "{o:?}");
    assert_eq!(t.resolve("hermes"), None);
}
