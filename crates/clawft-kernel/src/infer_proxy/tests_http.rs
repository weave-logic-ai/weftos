//! The HTTP edge and the wire format: what the proxy accepts at all.

use tokio::io::{AsyncWriteExt, duplex};

use super::http::*;
use super::types::*;
use super::wire::{self, Resp};

fn limits() -> ProxyLimits {
    ProxyLimits {
        max_request_body: 1024,
        max_head_bytes: 2048,
        head_timeout: std::time::Duration::from_millis(500),
        body_timeout: std::time::Duration::from_millis(500),
        ..ProxyLimits::default()
    }
}

async fn read(raw: &[u8]) -> Result<ProxyRequest, ProxyError> {
    let (mut c, mut s) = duplex(64 * 1024);
    c.write_all(raw).await.unwrap();
    read_request(&mut s, "hermes", &limits()).await
}

const H: &str = "Host: 127.0.0.1:8090\r\n";

#[test]
fn path_allowlist() {
    for ok in [
        "/v1/chat/completions",
        "/v1/models",
        "/v1/embeddings?x=1",
        "/health",
        "/api/tags",
        "/api/chat",
        "/api/generate",
    ] {
        assert!(validate_path(ok).is_ok(), "{ok}");
    }
    for bad in [
        "/api/pull",
        "/api/delete",
        "/api/create",
        "/api/push",
        "/admin",
        "/",
        "",
        "//evil.example/v1/x",
        "http://evil.example/v1/x",
        "/v1/../admin",
        "/v1/%2e%2e/admin",
        "/v1/%2E%2E/admin",
        "/v1/a%2fb",
        "/v1/a\\b",
        "/v1/a b",
        "/v1/a\r\nHost: x",
        "/v1/x#frag",
    ] {
        assert!(validate_path(bad).is_err(), "{bad:?}");
    }
    assert!(validate_path(&format!("/v1/{}", "a".repeat(3000))).is_err());
}

#[test]
fn host_and_origin_guards() {
    for ok in ["127.0.0.1:8090", "localhost", "localhost:1", "[::1]:8090", "127.0.0.1"] {
        assert!(loopback_host(ok), "{ok}");
    }
    for bad in [
        "evil.example",
        "10.0.0.2:8090",
        "localhost.evil.example",
        "0.0.0.0:80",
        "127.0.0.1:80.evil.example",
        "127.0.0.1:",
        "::1",
        "[::1",
        "",
    ] {
        assert!(!loopback_host(bad), "{bad}");
    }
    assert!(origin_ok(None));
    assert!(origin_ok(Some("http://localhost:3000")));
    assert!(!origin_ok(Some("https://evil.example")));
    assert!(!origin_ok(Some("http://evil.example")));
    assert!(!origin_ok(Some("null")));
}

#[tokio::test]
async fn accepts_a_valid_post_and_keeps_headers_we_forward() {
    let body = r#"{"a":1}"#;
    let raw = format!(
        "POST /v1/chat/completions HTTP/1.1\r\n{H}Content-Type: application/json\r\nAuthorization: Bearer k\r\nAccept: text/event-stream\r\nX-Other: dropped\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let r = read(raw.as_bytes()).await.unwrap();
    assert_eq!(r.method, Method::Post);
    assert_eq!(r.path, "/v1/chat/completions");
    assert_eq!(r.role, "hermes");
    assert_eq!(r.content_type.as_deref(), Some("application/json"));
    assert_eq!(r.authorization.as_deref(), Some("Bearer k"));
    assert_eq!(r.accept.as_deref(), Some("text/event-stream"));
    assert_eq!(r.body, body.as_bytes());
}

#[tokio::test]
async fn refuses_absolute_form_targets_and_foreign_hosts() {
    // The open-relay shapes.
    let e = read(format!("GET http://evil.example/v1/models HTTP/1.1\r\n{H}\r\n").as_bytes())
        .await
        .unwrap_err();
    assert!(matches!(e, ProxyError::Forbidden(_)), "{e:?}");
    let e = read(b"GET /v1/models HTTP/1.1\r\nHost: evil.example\r\n\r\n")
        .await
        .unwrap_err();
    assert!(matches!(e, ProxyError::Forbidden(_)), "{e:?}");
    let e = read(b"GET /v1/models HTTP/1.1\r\n\r\n").await.unwrap_err();
    assert!(matches!(e, ProxyError::Forbidden(_)), "missing Host: {e:?}");
    let e = read(format!("GET /v1/models HTTP/1.1\r\n{H}Origin: https://evil.example\r\n\r\n").as_bytes())
        .await
        .unwrap_err();
    assert!(matches!(e, ProxyError::Forbidden(_)), "{e:?}");
    let e = read(format!("CONNECT evil.example:443 HTTP/1.1\r\n{H}\r\n").as_bytes())
        .await
        .unwrap_err();
    assert_eq!(e, ProxyError::MethodNotAllowed);
}

#[tokio::test]
async fn refuses_other_methods() {
    for m in ["PUT", "DELETE", "PATCH", "OPTIONS", "HEAD", "TRACE"] {
        let e = read(format!("{m} /v1/models HTTP/1.1\r\n{H}\r\n").as_bytes())
            .await
            .unwrap_err();
        assert_eq!(e, ProxyError::MethodNotAllowed, "{m}");
    }
}

#[tokio::test]
async fn framing_attacks_are_refused() {
    let chunked = format!("POST /v1/chat/completions HTTP/1.1\r\n{H}Transfer-Encoding: chunked\r\n\r\n0\r\n\r\n");
    assert!(matches!(read(chunked.as_bytes()).await.unwrap_err(), ProxyError::Unsupported(_)));
    let dup = format!("POST /v1/chat/completions HTTP/1.1\r\n{H}Content-Length: 1\r\nContent-Length: 2\r\n\r\nab");
    assert!(matches!(read(dup.as_bytes()).await.unwrap_err(), ProxyError::BadRequest(_)));
    let neg = format!("POST /v1/chat/completions HTTP/1.1\r\n{H}Content-Length: -1\r\n\r\n");
    assert!(matches!(read(neg.as_bytes()).await.unwrap_err(), ProxyError::BadRequest(_)));
    let fold = format!("GET /v1/models HTTP/1.1\r\n{H}X-A: b\r\n c: d\r\n\r\n");
    assert!(matches!(read(fold.as_bytes()).await.unwrap_err(), ProxyError::BadRequest(_)));
    let ctl = format!("GET /v1/models HTTP/1.1\r\n{H}X-A: b\x01c\r\n\r\n");
    assert!(matches!(read(ctl.as_bytes()).await.unwrap_err(), ProxyError::BadRequest(_)));
    assert!(matches!(read(b"GET /v1/models HTTP/2.0\r\n\r\n").await.unwrap_err(), ProxyError::BadRequest(_)));
}

#[tokio::test]
async fn sizes_are_bounded() {
    let big = format!("POST /v1/chat/completions HTTP/1.1\r\n{H}Content-Length: 1025\r\n\r\n");
    assert!(matches!(read(big.as_bytes()).await.unwrap_err(), ProxyError::TooLarge(_)));
    let huge_head = format!("GET /v1/models HTTP/1.1\r\n{H}X-A: {}\r\n\r\n", "a".repeat(4000));
    assert!(matches!(read(huge_head.as_bytes()).await.unwrap_err(), ProxyError::TooLarge(_)));
    let many: String = (0..100).map(|i| format!("X-{i}: v\r\n")).collect();
    let many = format!("GET /v1/models HTTP/1.1\r\n{H}{many}\r\n");
    assert!(matches!(read(many.as_bytes()).await.unwrap_err(), ProxyError::TooLarge(_)));
}

#[tokio::test]
async fn slow_and_truncated_requests_time_out_or_fail() {
    // Head never completes.
    let (mut c, mut s) = duplex(1024);
    c.write_all(b"GET /v1/models HTTP/1.1\r\n").await.unwrap();
    let e = read_request(&mut s, "r", &limits()).await.unwrap_err();
    assert!(matches!(e, ProxyError::Timeout(_)), "{e:?}");
    // Body shorter than declared, then close.
    let (mut c, mut s) = duplex(1024);
    c.write_all(format!("POST /v1/chat/completions HTTP/1.1\r\n{H}Content-Length: 10\r\n\r\nabc").as_bytes())
        .await
        .unwrap();
    drop(c);
    let e = read_request(&mut s, "r", &limits()).await.unwrap_err();
    assert!(matches!(e, ProxyError::BadRequest(_)), "{e:?}");
}

#[tokio::test]
async fn a_bare_lf_head_is_refused_at_once() {
    let e = read(b"GET /v1/models HTTP/1.1\nHost: 127.0.0.1\n\n").await.unwrap_err();
    assert!(matches!(e, ProxyError::BadRequest(ref m) if m.contains("bare LF")), "{e:?}");
    let e = read(b"GET /v1/models HTTP/1.1\r\nHost: 127.0.0.1\nX: y\r\n\r\n").await.unwrap_err();
    assert!(matches!(e, ProxyError::BadRequest(_)), "{e:?}");
}

#[tokio::test]
async fn bytes_after_the_declared_body_are_discarded() {
    let raw = format!(
        "POST /v1/chat/completions HTTP/1.1\r\n{H}Content-Length: 2\r\n\r\n{{}}GET /v1/models HTTP/1.1\r\n{H}\r\n"
    );
    let r = read(raw.as_bytes()).await.unwrap();
    assert_eq!(r.body, b"{}");
    // And a GET with a pipelined request behind it carries no body at all.
    let raw = format!("GET /v1/models HTTP/1.1\r\n{H}\r\nGET /health HTTP/1.1\r\n{H}\r\n");
    let r = read(raw.as_bytes()).await.unwrap();
    assert_eq!(r.path, "/v1/models");
    assert!(r.body.is_empty());
}

#[test]
fn header_values_from_servers_cannot_split_the_response() {
    assert!(safe_header_value("application/json").is_some());
    assert!(safe_header_value("text/plain\r\nSet-Cookie: x=1").is_none());
    assert!(safe_header_value("a\u{7f}").is_none());
    assert!(safe_header_value("").is_none());
    assert!(safe_header_value(&"a".repeat(201)).is_none());
}

fn req(path: &str) -> ProxyRequest {
    ProxyRequest {
        role: "hermes".into(),
        method: Method::Post,
        path: path.into(),
        content_type: Some("application/json".into()),
        accept: None,
        authorization: Some("Bearer secret".into()),
        body: b"{}".to_vec(),
    }
}

#[test]
fn wire_request_roundtrip_drops_authorization() {
    let enc = wire::encode_request(&req("/v1/chat/completions")).unwrap();
    assert!(!String::from_utf8_lossy(&enc).contains("secret"));
    let d = wire::decode_request(&enc, &ProxyLimits::default()).unwrap();
    assert_eq!(d.role, "hermes");
    assert_eq!(d.path, "/v1/chat/completions");
    assert_eq!(d.authorization, None);
    assert_eq!(d.body, b"{}");
}

#[test]
fn wire_request_is_revalidated() {
    let l = ProxyLimits::default();
    let enc = |r: ProxyRequest| wire::encode_request(&r).unwrap();
    let mut r = req("/v1/x");
    r.role = "a/b".into();
    assert!(wire::decode_request(&enc(r), &l).is_err(), "role with slash");
    let mut r = req("/v1/x");
    r.role = "x".repeat(100);
    assert!(wire::decode_request(&enc(r), &l).is_err(), "long role");
    assert!(wire::decode_request(&enc(req("//evil/v1/x")), &l).is_err());
    assert!(wire::decode_request(&enc(req("/api/pull")), &l).is_err());
    let mut r = req("/v1/x");
    r.content_type = Some("a\r\nb".into());
    assert!(wire::decode_request(&enc(r), &l).is_err());
    let mut r = req("/v1/x");
    r.body = vec![0; 2048];
    let small = ProxyLimits { max_request_body: 1024, ..l.clone() };
    assert!(matches!(wire::decode_request(&enc(r), &small), Err(ProxyError::TooLarge(_))));
    assert!(wire::decode_request(b"\x09junk", &l).is_err(), "version");
    assert!(wire::decode_request(b"", &l).is_err());
    let mut bad = enc(req("/v1/x"));
    bad[1..5].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(wire::decode_request(&bad, &l).is_err(), "head length");
}

#[test]
fn wire_response_frames_roundtrip_and_reject_garbage() {
    for r in [
        Resp::Head { status: 200, content_type: Some("text/plain".into()) },
        Resp::Chunk(vec![1, 2, 3]),
        Resp::End,
        Resp::Error("nope".into()),
    ] {
        assert_eq!(wire::decode_resp(&wire::encode_resp(&r)).unwrap(), r);
    }
    assert!(wire::decode_resp(&[]).is_err());
    assert!(wire::decode_resp(&[9]).is_err());
    let mut h = wire::encode_resp(&Resp::Head { status: 200, content_type: None });
    h.splice(1.., br#"{"status":42}"#.iter().copied());
    assert!(wire::decode_resp(&h).is_err(), "status out of range");
    // A peer-supplied content type with CRLF is dropped, not echoed.
    let mut h = vec![0u8];
    h.extend(br#"{"status":200,"content_type":"a\r\nX: y"}"#);
    assert_eq!(
        wire::decode_resp(&h).unwrap(),
        Resp::Head { status: 200, content_type: None }
    );
}
