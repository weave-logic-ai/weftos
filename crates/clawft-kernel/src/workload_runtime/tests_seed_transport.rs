//! Seed HTTP transport: certificate pinning and the response-size cap,
//! against real local servers (a rustls server with a self-signed
//! certificate, and a raw TCP server streaming an endless chunked body).

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clawft_types::secret::SecretString;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::seed_http::{HttpSeedTransport, MAX_RESPONSE_BYTES, Method, SeedTransport};
use super::seed_tls::SeedTls;
use super::types::RuntimeError;

const TOKEN: &str = "seed-token-under-test";

fn pin_of(cert: &CertificateDer<'_>) -> [u8; 32] {
    match SeedTls::pinned(&SeedTls::fingerprint(cert.as_ref())).unwrap() {
        SeedTls::PinnedSha256(p) => p,
        _ => unreachable!(),
    }
}

/// A TLS server with a fresh self-signed certificate. Every request that
/// completes a handshake is recorded (head only) and answered `200 []`.
fn tls_seed() -> (u16, CertificateDer<'static>, Arc<Mutex<Vec<String>>>) {
    let kp = rcgen::KeyPair::generate().unwrap();
    let (port, cert, seen) = tls_seed_with(&kp, 1);
    (port, cert, seen)
}

/// A TLS server whose self-signed certificate (serial `serial`) is issued
/// for `kp`: the same key with another serial is a renewed certificate.
fn tls_seed_with(
    kp: &rcgen::KeyPair,
    serial: u64,
) -> (u16, CertificateDer<'static>, Arc<Mutex<Vec<String>>>) {
    let mut params = rcgen::CertificateParams::new(vec!["seed.local".into()]).unwrap();
    params.serial_number = Some(rcgen::SerialNumber::from(serial));
    let cert = params.self_signed(kp).unwrap().der().clone();
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(kp.serialize_der()));
    let cfg = Arc::new(
        rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert.clone()], key)
        .unwrap(),
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for tcp in listener.incoming().flatten() {
            let _ = tcp.set_read_timeout(Some(Duration::from_secs(5)));
            let conn = rustls::ServerConnection::new(cfg.clone()).unwrap();
            let mut tls = rustls::StreamOwned::new(conn, tcp);
            let mut head = Vec::new();
            let mut buf = [0u8; 1024];
            while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                match tls.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => head.extend_from_slice(&buf[..n]),
                }
            }
            if head.is_empty() {
                continue; // handshake refused by the client
            }
            log.lock()
                .unwrap()
                .push(String::from_utf8_lossy(&head).into_owned());
            let _ = tls.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                  Content-Length: 2\r\nConnection: close\r\n\r\n[]",
            );
            tls.conn.send_close_notify();
            let _ = tls.flush();
        }
    });
    (port, cert, seen)
}

async fn get(t: &HttpSeedTransport) -> Result<(u16, serde_json::Value), RuntimeError> {
    t.request(
        Method::Get,
        "/api/v1/apps",
        None,
        &SecretString::new(TOKEN.to_string()),
        Duration::from_secs(5),
    )
    .await
}

#[test]
fn pins_parse_in_the_usual_fingerprint_forms() {
    let hex = "ab".repeat(32);
    let colon = vec!["AB"; 32].join(":");
    for text in [format!("sha256:{hex}"), format!("sha256:{colon}")] {
        assert_eq!(
            SeedTls::pinned(&text).unwrap(),
            SeedTls::PinnedSha256([0xab; 32])
        );
    }
    let bare = hex.clone();
    let short = format!("sha256:{}", "ab".repeat(31));
    let long = format!("sha256:{}", "ab".repeat(33));
    let nonhex = format!("sha256:{}", "zz".repeat(32));
    for bad in ["", "sha256:", bare.as_str(), &short, &long, &nonhex] {
        assert!(SeedTls::pinned(bad).is_err(), "{bad:?} accepted");
    }
    assert_eq!(SeedTls::fingerprint(b"x").len(), "sha256:".len() + 64);
}

#[tokio::test]
async fn the_pinned_certificate_is_trusted_and_carries_the_token() {
    let (port, cert, seen) = tls_seed();
    let t = HttpSeedTransport::new(
        &format!("https://127.0.0.1:{port}"),
        SeedTls::PinnedSha256(pin_of(&cert)),
    )
    .unwrap();
    let (status, body) = get(&t).await.expect("pinned handshake");
    assert_eq!((status, body), (200, serde_json::json!([])));
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert!(
        seen[0]
            .to_ascii_lowercase()
            .contains(&format!("authorization: bearer {TOKEN}"))
    );
}

#[tokio::test]
async fn any_other_certificate_is_refused_before_the_token_is_sent() {
    let (port, cert, seen) = tls_seed();
    let mut wrong = pin_of(&cert);
    wrong[0] ^= 1;
    let base = format!("https://127.0.0.1:{port}");
    let pinned_wrong = HttpSeedTransport::new(&base, SeedTls::PinnedSha256(wrong)).unwrap();
    let err = get(&pinned_wrong).await.expect_err("wrong pin accepted");
    assert!(!err.to_string().contains(TOKEN));
    // Without a pin, the Seed's self-signed certificate fails WebPKI.
    let webpki = HttpSeedTransport::new(&base, SeedTls::WebPki).unwrap();
    get(&webpki)
        .await
        .expect_err("self-signed accepted without a pin");
    assert!(
        seen.lock().unwrap().is_empty(),
        "a request (and its bearer token) reached an untrusted server"
    );
}

#[test]
fn a_pin_on_a_plain_http_base_is_refused() {
    let r = HttpSeedTransport::new("http://127.0.0.1:1", SeedTls::PinnedSha256([0; 32]));
    assert!(matches!(r, Err(RuntimeError::InvalidConfig(_))));
}

/// A server that answers every request with `head` and then streams
/// `total` bytes of body in `chunk`-sized pieces; returns its port and the
/// number of body bytes it managed to write.
async fn streaming_seed(
    head: &'static str,
    total: usize,
    chunked: bool,
) -> (u16, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let sent = Arc::new(AtomicUsize::new(0));
    let count = sent.clone();
    tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 4096];
        let _ = s.read(&mut buf).await;
        if s.write_all(head.as_bytes()).await.is_err() {
            return;
        }
        let piece = vec![b' '; 64 * 1024];
        while count.load(Ordering::SeqCst) < total {
            let r = if chunked {
                let mut c = format!("{:x}\r\n", piece.len()).into_bytes();
                c.extend_from_slice(&piece);
                c.extend_from_slice(b"\r\n");
                s.write_all(&c).await
            } else {
                s.write_all(&piece).await
            };
            if r.is_err() {
                return; // the client hung up
            }
            count.fetch_add(piece.len(), Ordering::SeqCst);
        }
    });
    (port, sent)
}

#[tokio::test]
async fn a_body_without_a_length_is_cut_off_at_the_cap() {
    let total = 256 * 1024 * 1024;
    let (port, sent) = streaming_seed(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n",
        total,
        true,
    )
    .await;
    let t = HttpSeedTransport::new(&format!("http://127.0.0.1:{port}"), SeedTls::WebPki).unwrap();
    let err = get(&t).await.expect_err("oversized body accepted");
    assert!(err.to_string().contains("too large"), "{err}");
    // The client stopped reading at the cap: the server could not push the
    // whole body (it only gets as far as the cap plus socket buffers).
    tokio::time::sleep(Duration::from_millis(500)).await;
    let n = sent.load(Ordering::SeqCst);
    assert!(n < 8 * MAX_RESPONSE_BYTES, "server wrote {n} bytes");
}

#[tokio::test]
async fn a_declared_length_over_the_cap_is_refused_before_reading() {
    let (port, sent) = streaming_seed(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 1073741824\r\n\r\n",
        1024 * 1024 * 1024,
        false,
    )
    .await;
    let t = HttpSeedTransport::new(&format!("http://127.0.0.1:{port}"), SeedTls::WebPki).unwrap();
    let err = get(&t).await.expect_err("oversized body accepted");
    assert!(err.to_string().contains("too large"), "{err}");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(sent.load(Ordering::SeqCst) < 8 * MAX_RESPONSE_BYTES);
}

fn spki_pin(cert: &CertificateDer<'_>) -> SeedTls {
    SeedTls::pinned_spki(&SeedTls::spki_fingerprint(cert.as_ref()).unwrap()).unwrap()
}

#[test]
fn spki_pins_parse_and_reject_junk() {
    let hex = "cd".repeat(32);
    assert_eq!(
        SeedTls::pinned_spki(&format!("spki-sha256:{hex}")).unwrap(),
        SeedTls::PinnedSpki([0xcd; 32])
    );
    for bad in [
        "",
        "sha256:".to_string().as_str(),
        &format!("sha256:{hex}"),
        "spki-sha256:zz",
    ] {
        assert!(SeedTls::pinned_spki(bad).is_err(), "{bad:?} accepted");
    }
    assert!(SeedTls::spki_fingerprint(b"not a certificate").is_none());
}

#[tokio::test]
async fn a_renewed_leaf_certificate_under_the_same_key_still_connects() {
    let kp = rcgen::KeyPair::generate().unwrap();
    let (_, old_cert, _) = tls_seed_with(&kp, 1);
    let key_pin = spki_pin(&old_cert);
    let cert_pin = SeedTls::PinnedSha256(pin_of(&old_cert));

    // The Seed renews: a new certificate (new serial), same device key.
    let (port, new_cert, seen) = tls_seed_with(&kp, 2);
    assert_ne!(old_cert.as_ref(), new_cert.as_ref());
    let base = format!("https://127.0.0.1:{port}");

    // The old leaf-hash pin breaks on rotation (the card's failure) ...
    let t = HttpSeedTransport::new(&base, cert_pin).unwrap();
    get(&t)
        .await
        .expect_err("leaf-hash pin survived a rotation");
    assert!(seen.lock().unwrap().is_empty());
    // ... the public-key pin does not.
    let t = HttpSeedTransport::new(&base, key_pin).unwrap();
    let (status, _) = get(&t).await.expect("key pin across a rotation");
    assert_eq!(status, 200);
    assert_eq!(seen.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_certificate_under_another_key_is_refused_by_the_key_pin() {
    let (_, old_cert, _) = tls_seed();
    let key_pin = spki_pin(&old_cert);
    // A different device key (a rekey, or an impostor): refused, and the
    // token is never sent. The operator must re-pin over a trusted path.
    let (port, _, seen) = tls_seed();
    let t = HttpSeedTransport::new(&format!("https://127.0.0.1:{port}"), key_pin).unwrap();
    get(&t).await.expect_err("a different key was accepted");
    assert!(seen.lock().unwrap().is_empty());
}

#[test]
fn a_key_pin_on_a_plain_http_base_is_refused() {
    let r = HttpSeedTransport::new("http://127.0.0.1:1", SeedTls::PinnedSpki([0; 32]));
    assert!(matches!(r, Err(RuntimeError::InvalidConfig(_))));
}

/// Plain-http Seed (the USB base `http://169.254.42.1`): a stub that
/// closes each connection after its response while advertising
/// keep-alive, as the Seed does after a few idle seconds. A pooled stale
/// connection makes the second request fail with "error sending request".
#[tokio::test]
async fn plain_http_survives_a_server_that_drops_keep_alive_connections() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let heads = Arc::new(Mutex::new(Vec::<String>::new()));
    let log = heads.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = listener.accept().await else {
                return;
            };
            let mut buf = [0u8; 4096];
            let n = s.read(&mut buf).await.unwrap_or(0);
            log.lock()
                .unwrap()
                .push(String::from_utf8_lossy(&buf[..n]).into_owned());
            let _ = s
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                      Content-Length: 2\r\nConnection: keep-alive\r\n\r\n[]",
                )
                .await;
            let _ = s.shutdown().await; // ...but the connection goes away
        }
    });
    let t = HttpSeedTransport::new(&format!("http://127.0.0.1:{port}"), SeedTls::WebPki).unwrap();
    for i in 0..3 {
        let (status, body) = get(&t).await.unwrap_or_else(|e| panic!("request {i}: {e}"));
        assert_eq!((status, body), (200, serde_json::json!([])));
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let heads = heads.lock().unwrap();
    assert_eq!(heads.len(), 3);
    assert!(
        heads[0]
            .to_ascii_lowercase()
            .contains(&format!("authorization: bearer {TOKEN}"))
    );
}

const PROXY_CHILD: &str = "SEED_TEST_PROXY_CHILD";

/// A proxy in the environment must not capture Seed traffic. The
/// environment is read when a client is built, and mutating it in this
/// process would race the other tests, so the check runs in a child
/// process of this test binary.
#[test]
fn an_environment_proxy_is_not_used_for_a_seed() {
    if std::env::var(PROXY_CHILD).is_ok() {
        return; // the child runs `proxy_child_body` instead
    }
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["proxy_child_body", "--nocapture"])
        .env(PROXY_CHILD, "1")
        .env("HTTP_PROXY", "http://127.0.0.1:1")
        .env("http_proxy", "http://127.0.0.1:1")
        .env("ALL_PROXY", "http://127.0.0.1:1")
        .env_remove("NO_PROXY")
        .env_remove("no_proxy")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "child failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[tokio::test]
async fn proxy_child_body() {
    if std::env::var(PROXY_CHILD).is_err() {
        return; // only meaningful inside the child process above
    }
    let (port, _sent) = streaming_seed(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n[]",
        2,
        false,
    )
    .await;
    let t = HttpSeedTransport::new(&format!("http://127.0.0.1:{port}"), SeedTls::WebPki).unwrap();
    let (status, _) = get(&t).await.expect("request went through the env proxy");
    assert_eq!(status, 200);
}
