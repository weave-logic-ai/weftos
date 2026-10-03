//! Binding beyond loopback: only with a governance permit and a bearer
//! token. Tests bind 127.0.0.1 (the exposure API accepts any address; the
//! address check is tested as a pure function), so nothing here listens on
//! a LAN-reachable address.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::http::ct_eq;
use super::listener::{ExposureAuth, ExposurePermit, InferProxy, OccupiedPolicy, Started, check_bind};
use super::support::*;
use super::table::PlacementTable;
use super::types::*;
use crate::gate::GateDecision;

const TOKEN: &str = "t0ken-0123456789-abcdefghijklmnopqrstuvwxyz";

fn permit() -> ExposurePermit {
    ExposurePermit::from_decision(&GateDecision::Permit { token: None }).unwrap()
}

async fn exposed(t: &Arc<PlacementTable>, audit: Option<Arc<dyn ProxyAudit>>) -> InferProxy {
    match InferProxy::start_exposed(
        "hermes",
        "127.0.0.1:0".parse().unwrap(),
        OccupiedPolicy::Refuse,
        t.clone(),
        small_limits(),
        audit,
        permit(),
        ExposureAuth::new(TOKEN).unwrap(),
    )
    .await
    .unwrap()
    {
        Started::Running(p) => p,
        _ => panic!(),
    }
}

fn req(_addr: SocketAddr, host: &str, auth: Option<&str>, path: &str) -> Vec<u8> {
    let a = auth.map(|a| format!("Authorization: {a}\r\n")).unwrap_or_default();
    format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n{a}\r\n").into_bytes()
}

#[tokio::test]
async fn an_exposed_listener_needs_the_token_and_never_forwards_it() {
    let up = fake(Reply::ok("served")).await;
    let t = Arc::new(PlacementTable::new("node-a", None, None));
    t.register_local("hermes", &up.base(), None, "openai-v1", "LlamaCpp").unwrap();
    let audit = Arc::new(Audit::default());
    let p = exposed(&t, Some(audit.clone())).await;
    let a = p.addr();
    // No token, wrong token, wrong scheme, a prefix of the token: all 401.
    for auth in [None, Some("Bearer nope"), Some(&*format!("Basic {TOKEN}")), Some(&*format!("Bearer {}", &TOKEN[..20])), Some(&*format!("Bearer {TOKEN}x"))] {
        let r = raw(a, &req(a, "containers.internal", auth, "/v1/models")).await;
        assert_eq!(status(&r), 401, "{auth:?}: {r}");
        assert!(r.contains("WWW-Authenticate: Bearer"));
    }
    assert_eq!(up.count(), 0, "an unauthenticated request reached the model server");
    // The right token works from any Host (a container's name for this machine).
    let r = raw(a, &req(a, "containers.internal", Some(&format!("Bearer {TOKEN}")), "/v1/models")).await;
    assert_eq!(status(&r), 200, "{r}");
    assert_eq!(body(&r), "served");
    assert_eq!(up.last().header("authorization"), None, "the listener's credential was forwarded");
    // The token is nowhere in the audit trail.
    assert!(!format!("{:?}", audit.0.lock().unwrap()).contains(TOKEN));
    let binds: Vec<_> = audit.0.lock().unwrap().iter().filter(|(k, _)| k == "infer.proxy.bind").cloned().collect();
    assert_eq!(binds.len(), 1);
    assert_eq!(binds[0].1["detail"]["exposed"], true);
}

#[tokio::test]
async fn an_unauthenticated_request_is_answered_before_its_body_is_read() {
    let up = fake(Reply::ok("x")).await;
    let t = Arc::new(PlacementTable::new("node-a", None, None));
    t.register_local("hermes", &up.base(), None, "openai-v1", "LlamaCpp").unwrap();
    let p = exposed(&t, None).await;
    let mut s = TcpStream::connect(p.addr()).await.unwrap();
    // Promise a large body and never send it.
    s.write_all(b"POST /v1/chat/completions HTTP/1.1\r\nHost: x\r\nContent-Length: 1000000\r\n\r\n").await.unwrap();
    let mut out = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), s.read_to_end(&mut out)).await.expect("answered without waiting for the body").unwrap();
    assert!(String::from_utf8_lossy(&out).starts_with("HTTP/1.1 401"));
}

#[tokio::test]
async fn the_default_listener_still_refuses_a_foreign_host_and_a_wide_bind() {
    let up = fake(Reply::ok("x")).await;
    let t = Arc::new(PlacementTable::new("node-a", None, None));
    t.register_local("hermes", &up.base(), None, "openai-v1", "LlamaCpp").unwrap();
    let plain = match InferProxy::start("hermes", "127.0.0.1:0".parse().unwrap(), OccupiedPolicy::Refuse, t.clone(), small_limits(), None).await.unwrap() {
        Started::Running(p) => p,
        _ => panic!(),
    };
    let a = plain.addr();
    let r = raw(a, &req(a, "containers.internal", Some(&format!("Bearer {TOKEN}")), "/v1/models")).await;
    assert_eq!(status(&r), 403, "{r}");
    for addr in ["0.0.0.0:0", "192.0.2.1:0", "[::]:0"] {
        let r = InferProxy::start("hermes", addr.parse().unwrap(), OccupiedPolicy::Refuse, t.clone(), small_limits(), None).await;
        assert!(matches!(r, Err(ProxyError::NotLoopback(_))), "{addr}");
    }
}

#[test]
fn the_bind_rule_is_loopback_or_exposed() {
    let wide: SocketAddr = "0.0.0.0:8090".parse().unwrap();
    let lo: SocketAddr = "127.0.0.1:8090".parse().unwrap();
    assert!(check_bind(lo, false).is_ok());
    assert!(check_bind(wide, false).is_err());
    assert!(check_bind(wide, true).is_ok());
}

#[test]
fn only_a_permit_makes_an_exposure_permit_and_tokens_are_checked() {
    assert!(ExposurePermit::from_decision(&GateDecision::Permit { token: None }).is_some());
    assert!(ExposurePermit::from_decision(&GateDecision::Deny { reason: "no".into(), receipt: None }).is_none());
    assert!(ExposurePermit::from_decision(&GateDecision::Defer { reason: "ask".into() }).is_none());
    assert!(ExposureAuth::new("short").is_err());
    assert!(ExposureAuth::new(&"a b ".repeat(20)).is_err(), "whitespace inside");
    assert!(ExposureAuth::new(&"é".repeat(40)).is_err(), "non-ASCII");
    assert!(ExposureAuth::new(&format!("  {TOKEN}\n")).is_ok(), "trimmed");
    assert!(!format!("{:?}", ExposureAuth::new(TOKEN).unwrap()).contains("t0ken"));
    assert!(ct_eq(b"abc", b"abc") && !ct_eq(b"abc", b"abd") && !ct_eq(b"abc", b"ab") && !ct_eq(b"", b"a") && ct_eq(b"", b""));
}

async fn plain(t: &Arc<PlacementTable>) -> InferProxy {
    match InferProxy::start("hermes", "127.0.0.1:0".parse().unwrap(), OccupiedPolicy::Refuse, t.clone(), small_limits(), None).await.unwrap() {
        Started::Running(p) => p,
        _ => panic!(),
    }
}

#[tokio::test]
async fn local_consumers_keep_a_token_free_loopback_listener_beside_the_exposed_one() {
    let up = fake(Reply::ok("served")).await;
    let t = Arc::new(PlacementTable::new("node-a", None, None));
    t.register_local("hermes", &up.base(), None, "openai-v1", "LlamaCpp").unwrap();
    let local = plain(&t).await;
    let wide = exposed(&t, None).await;
    // The role's registered proxy port is the loopback one (what in-process
    // consumers of a remote role use), whichever started last.
    assert_eq!(t.proxy_port("hermes"), Some(local.addr().port()));
    assert_ne!(local.addr(), wide.addr());
    // A local consumer needs no token.
    let a = local.addr();
    let r = raw(a, &req(a, &a.to_string(), None, "/v1/models")).await;
    assert_eq!(status(&r), 200, "{r}");
    // The exposed listener still demands one.
    let b = wide.addr();
    assert_eq!(status(&raw(b, &req(b, "x", None, "/v1/models")).await), 401);
    assert_eq!(status(&raw(b, &req(b, "x", Some(&format!("Bearer {TOKEN}")), "/v1/models")).await), 200);
}

#[tokio::test]
async fn slow_or_many_exposed_clients_cannot_starve_the_loopback_path() {
    let up = fake(Reply::ok("served")).await;
    let t = Arc::new(PlacementTable::new("node-a", None, None));
    t.register_local("hermes", &up.base(), None, "openai-v1", "LlamaCpp").unwrap();
    let limits = ProxyLimits { max_connections: 2, max_connections_per_ip: 2, head_timeout: Duration::from_secs(30), ..small_limits() };
    let local = match InferProxy::start("hermes", "127.0.0.1:0".parse().unwrap(), OccupiedPolicy::Refuse, t.clone(), limits.clone(), None).await.unwrap() {
        Started::Running(p) => p,
        _ => panic!(),
    };
    let wide = match InferProxy::start_exposed(
        "hermes", "127.0.0.1:0".parse().unwrap(), OccupiedPolicy::Refuse, t.clone(), limits, None, permit(), ExposureAuth::new(TOKEN).unwrap(),
    ).await.unwrap() {
        Started::Running(p) => p,
        _ => panic!(),
    };
    // Fill the exposed listener's pool with heads that never finish.
    let mut idle = Vec::new();
    for _ in 0..2 {
        let mut s = TcpStream::connect(wide.addr()).await.unwrap();
        s.write_all(b"GET /v1/models HTTP/1.1\r\nHost: x\r\n").await.unwrap();
        idle.push(s);
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    // A third connection from the same address is shed at once (per-IP cap).
    let mut third = TcpStream::connect(wide.addr()).await.unwrap();
    let mut buf = [0u8; 16];
    let n = tokio::time::timeout(Duration::from_secs(2), third.read(&mut buf)).await.expect("shed, not held").unwrap();
    assert_eq!(n, 0, "no response for a connection over the per-client cap");
    // The loopback listener is untouched by all of it.
    let a = local.addr();
    let t0 = std::time::Instant::now();
    let r = raw(a, &req(a, &a.to_string(), None, "/v1/models")).await;
    assert_eq!(status(&r), 200, "{r}");
    assert!(t0.elapsed() < Duration::from_secs(2));
    // Closing the idle ones frees the slots on the exposed side.
    drop(idle);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let b = wide.addr();
    assert_eq!(status(&raw(b, &req(b, "x", Some(&format!("Bearer {TOKEN}")), "/v1/models")).await), 200);
}

struct FixedGate(bool);

impl crate::gate::GateBackend for FixedGate {
    fn check(&self, _agent: &str, action: &str, ctx: &serde_json::Value) -> GateDecision {
        assert_eq!(action, "workload.start");
        assert_eq!(ctx["workload"]["network"], "lan");
        assert_eq!(ctx["workload"]["package_id"], "inference-expose:hermes");
        if self.0 {
            GateDecision::Permit { token: None }
        } else {
            GateDecision::Deny { reason: "default deny".into(), receipt: None }
        }
    }
}

#[test]
fn a_permit_is_minted_only_by_asking_the_gate() {
    assert!(ExposurePermit::ask(&FixedGate(true), "infer-daemon", "hermes").is_ok());
    let e = ExposurePermit::ask(&FixedGate(false), "infer-daemon", "hermes").unwrap_err();
    assert!(e.contains("governance denied") && e.contains("default deny"), "{e}");
}
