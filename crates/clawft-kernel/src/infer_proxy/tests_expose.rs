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
