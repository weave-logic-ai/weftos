//! The HTTP licence transport: link rules (pinned or explicit lab opt-in),
//! timeouts and response caps, against a canned server on loopback.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::client::MAX_RESPONSE_BODY;
use super::tests_common::*;
use super::*;
use crate::workload_runtime::LinkSecurity;
use crate::workload_runtime::seed_tls::SeedTls;

/// What the canned server answers with.
#[derive(Clone)]
enum Answer {
    /// Status line, extra headers and body; `Content-Length` is added.
    Fixed(&'static str, Vec<u8>),
    /// A body with no `Content-Length` (closed at the end).
    Unframed(Vec<u8>),
    /// Never answer.
    Hang,
}

/// A loopback server that records each request head and answers `answer`.
async fn serve(answer: Answer) -> (String, Arc<Mutex<Vec<String>>>) {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let heads = Arc::new(Mutex::new(Vec::new()));
    let seen = heads.clone();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            let (answer, seen) = (answer.clone(), seen.clone());
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut b = [0u8; 4096];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut b).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&b[..n]),
                    }
                }
                seen.lock().unwrap().push(String::from_utf8_lossy(&buf).to_lowercase());
                match answer {
                    Answer::Fixed(status, body) => {
                        let head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                        let _ = s.write_all(head.as_bytes()).await;
                        let _ = s.write_all(&body).await;
                    }
                    Answer::Unframed(body) => {
                        let _ = s.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n").await;
                        let _ = s.write_all(&body).await;
                    }
                    Answer::Hang => tokio::time::sleep(Duration::from_secs(30)).await,
                }
                let _ = s.shutdown().await;
            });
        }
    });
    (format!("http://{addr}"), heads)
}

fn lab(url: &str, limits: TransportLimits) -> HttpLicenceTransport {
    HttpLicenceTransport::new(LicenceLinkConfig {
        url: url.into(),
        tls: SeedTls::WebPki,
        allow_unpinned_lab_link: true,
        limits,
    })
    .expect("lab link")
}

fn signed_get(path: &str) -> LicenceRequest {
    sign_request(&sk(21), "node-steward", "seed-test", "GET", path, Vec::new(), 1_790_000_000_000, &"a".repeat(32))
}

#[test]
fn an_unpinned_link_needs_the_explicit_lab_opt_in() {
    let cfg = |url: &str, tls, opt| LicenceLinkConfig { url: url.into(), tls, allow_unpinned_lab_link: opt, limits: TransportLimits::default() };
    let e = HttpLicenceTransport::new(cfg("http://100.64.0.10:8700", SeedTls::WebPki, false)).err().unwrap();
    assert!(e.to_string().contains("pinned transport"), "{e}");
    // https with WebPKI only is not a pin either.
    assert!(HttpLicenceTransport::new(cfg("https://100.64.0.10:8700", SeedTls::WebPki, false)).is_err());
    // A pin on plain http protects nothing and is refused.
    assert!(HttpLicenceTransport::new(cfg("http://100.64.0.10:8700", SeedTls::PinnedSpki([1; 32]), false)).is_err());
    let pinned = HttpLicenceTransport::new(cfg("https://100.64.0.10:8700", SeedTls::PinnedSpki([1; 32]), false)).unwrap();
    assert_eq!(pinned.link_security(), LinkSecurity::Pinned);
    let opted = HttpLicenceTransport::new(cfg("http://100.64.0.10:8700", SeedTls::WebPki, true)).unwrap();
    assert_eq!(opted.link_security(), LinkSecurity::LabOptIn);
    // Not a base URL.
    assert!(HttpLicenceTransport::new(cfg("http://100.64.0.10:8700/x?y", SeedTls::WebPki, true)).is_err());
}

#[tokio::test]
async fn the_signature_headers_are_sent_and_the_answer_comes_back() {
    let (url, heads) = serve(Answer::Fixed("200 OK", br#"{"grants":[]}"#.to_vec())).await;
    let t = lab(&url, TransportLimits::default());
    let r = t.call(signed_get("/licence/v1/grants?since=0")).await.unwrap();
    assert_eq!((r.status, r.body.as_slice()), (200, &br#"{"grants":[]}"#[..]));
    let head = heads.lock().unwrap()[0].clone();
    for h in ["x-licence-node: node-steward", "x-licence-ts: 1790000000000", "x-licence-nonce:", "x-licence-sig:"] {
        assert!(head.contains(h), "{h} missing from {head}");
    }
    assert!(head.starts_with("get /licence/v1/grants?since=0 "), "{head}");
}

#[tokio::test]
async fn bodies_are_capped_with_or_without_a_content_length() {
    let big = vec![b'x'; MAX_RESPONSE_BODY + 1];
    let (url, _) = serve(Answer::Fixed("200 OK", big.clone())).await;
    let e = lab(&url, TransportLimits::default()).call(signed_get("/licence/v1/grants?since=0")).await.unwrap_err();
    assert!(e.to_string().contains("too large"), "{e}");
    let (url, _) = serve(Answer::Unframed(big)).await;
    let e = lab(&url, TransportLimits::default()).call(signed_get("/licence/v1/grants?since=0")).await.unwrap_err();
    assert!(e.to_string().contains("too large"), "{e}");
    // The artifact path has its own cap.
    let limits = TransportLimits { max_artifact: 1000, ..TransportLimits::default() };
    let path = format!("{ARTIFACT_PATH}{}", "b".repeat(64));
    let (url, _) = serve(Answer::Unframed(vec![7; 1001])).await;
    assert!(lab(&url, limits.clone()).call(signed_get(&path)).await.is_err());
    let (url, _) = serve(Answer::Fixed("200 OK", vec![7; 1000])).await;
    assert_eq!(lab(&url, limits).call(signed_get(&path)).await.unwrap().body.len(), 1000);
}

#[tokio::test]
async fn a_silent_server_times_out_and_a_redirect_is_not_followed() {
    let (url, _) = serve(Answer::Hang).await;
    let limits = TransportLimits { call_timeout: Duration::from_millis(300), ..TransportLimits::default() };
    let started = std::time::Instant::now();
    let e = lab(&url, limits).call(signed_get("/licence/v1/grants?since=0")).await.unwrap_err();
    assert!(matches!(e, LicenceClientError::Transport(_)), "{e}");
    assert!(started.elapsed() < Duration::from_secs(5));
    let (url, heads) = serve(Answer::Fixed("302 Found\r\nLocation: http://127.0.0.1:1/licence/v1/x", Vec::new())).await;
    let r = lab(&url, TransportLimits::default()).call(signed_get("/licence/v1/grants?since=0")).await.unwrap();
    assert_eq!(r.status, 302);
    assert_eq!(heads.lock().unwrap().len(), 1, "the redirect was not followed");
}

#[tokio::test]
async fn an_unsafe_path_or_method_is_refused_before_any_connection() {
    let (url, heads) = serve(Answer::Fixed("200 OK", Vec::new())).await;
    let t = lab(&url, TransportLimits::default());
    assert!(t.call(signed_get("/api/v1/identity")).await.is_err());
    assert!(t.call(signed_get("/licence/v1/../../etc")).await.is_err());
    let mut put = signed_get("/licence/v1/checkout");
    put.method = "PUT".into();
    assert!(t.call(put).await.is_err());
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(heads.lock().unwrap().is_empty());
}

#[tokio::test]
async fn the_steward_client_sends_nothing_unless_the_binding_names_this_node() {
    let (url, heads) = serve(Answer::Fixed("200 OK", br#"{"grants":[]}"#.to_vec())).await;
    let fx = Fx::new();
    let t: Arc<dyn LicenceTransport> = Arc::new(lab(&url, TransportLimits::default()));
    let clock: ClockMs = Arc::new(|| 1_790_000_000_000);
    let c = StewardLicenceClient::new(fx.store.clone(), sk(21), "node-steward", t.clone(), clock.clone());
    let e = c.grants_since(0).await.unwrap_err();
    assert_eq!(e, LicenceClientError::Refused { status: 0, code: "seed_not_bound".into() });
    fx.bind();
    // Another node's key, or another node id: not the steward.
    let other = StewardLicenceClient::new(fx.store.clone(), sk(22), "node-steward", t.clone(), clock.clone());
    assert!(matches!(other.grants_since(0).await, Err(LicenceClientError::Refused { code, .. }) if code == "not_steward"));
    let other = StewardLicenceClient::new(fx.store.clone(), sk(21), "node-x", t, clock);
    assert!(matches!(other.grants_since(0).await, Err(LicenceClientError::Refused { code, .. }) if code == "not_steward"));
    assert!(heads.lock().unwrap().is_empty(), "nothing was sent");
    assert_eq!(c.grants_since(0).await.unwrap(), Vec::new());
    assert_eq!(heads.lock().unwrap().len(), 1);
}
