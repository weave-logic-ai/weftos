//! Loopback only: the real listener on 127.0.0.1, signed requests over TCP.
mod common;
use common::*;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicU64, Ordering};
use weft_licence_wire::{hex_encode, verify_grant};

const BIN: &[u8] = b"\x7fELF over the wire";
static N: AtomicU64 = AtomicU64::new(1_000_000);

fn roundtrip(addr: std::net::SocketAddr, h: &Harness, method: &str, target: &str, body: &[u8], signed: bool) -> (u16, Vec<u8>) {
    let mut s = TcpStream::connect(addr).unwrap();
    let mut head = format!("{method} {target} HTTP/1.1\r\nHost: {addr}\r\nContent-Length: {}\r\n", body.len());
    if signed {
        let nonce = format!("{:032x}", N.fetch_add(1, Ordering::SeqCst));
        for (k, v) in weft_licence::request::sign_request(&steward(), NODE, "seed-test", method, target, body, h.now() * 1000, &nonce) {
            head.push_str(&format!("{k}: {v}\r\n"));
        }
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes()).unwrap();
    s.write_all(body).unwrap();
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).unwrap();
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let status: u16 = String::from_utf8_lossy(&raw[..split]).split_whitespace().nth(1).unwrap().parse().unwrap();
    (status, raw[split + 4..].to_vec())
}

#[test]
fn checkout_and_transfer_over_loopback_with_the_signature_rules() {
    let h = Harness::new(&[("fall-detect", "arm", BIN)]);
    let server = weft_licence::http::serve(h.svc.clone(), &["127.0.0.1:0".parse().unwrap()]).unwrap();
    let addr = server.addrs()[0];
    assert!(addr.ip().is_loopback());

    let (st, body) = roundtrip(addr, &h, "GET", "/licence/v1/identity", b"", false);
    assert_eq!(st, 200);
    assert_eq!(serde_json::from_slice::<serde_json::Value>(&body).unwrap()["service"], "weft-licence");
    // Unsigned: refused.
    assert_eq!(roundtrip(addr, &h, "GET", "/licence/v1/grants?since=0", b"", false).0, 400);
    // Signed checkout, then the bytes, which hash to the signed values.
    let req = br#"{"request_id":"r","cog_id":"fall-detect","version":"latest","arch":"arm"}"#;
    let (st, body) = roundtrip(addr, &h, "POST", "/licence/v1/checkout", req, true);
    assert_eq!(st, 200, "{}", String::from_utf8_lossy(&body));
    let j: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let signed: weft_licence_wire::SignedGrant = serde_json::from_value(j["grant"].clone()).unwrap();
    let g = verify_grant(&signed, &h.grant_key(), &mesh()).unwrap();
    let path = j["artifacts"][0]["path"].as_str().unwrap().to_string();
    let (st, bytes) = roundtrip(addr, &h, "GET", &path, b"", true);
    assert_eq!(st, 200);
    assert_eq!(bytes, BIN);
    assert_eq!(hex_encode(blake3::hash(&bytes).as_bytes()), g.artifacts[0].blake3);
    assert_eq!(weft_licence_wire::sha256_hex(&bytes), g.artifacts[0].sha256);
    // Oversized body and a garbage request line are refused, not crashed on.
    let big = roundtrip(addr, &h, "POST", "/licence/v1/checkout", &vec![b'x'; 70_000], true);
    assert_eq!(big.0, 413, "{}", String::from_utf8_lossy(&big.1));
    server.stop();
}

fn open_conn(addr: std::net::SocketAddr) -> TcpStream {
    let s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
    s
}

#[test]
fn slow_loris_is_cut_off_by_the_total_deadline_and_the_per_source_cap() {
    let h = Harness::new(&[]);
    let opts = weft_licence::http::ServerOpts { read_deadline: std::time::Duration::from_millis(700), per_ip: 4 };
    let server = weft_licence::http::serve_with(h.svc.clone(), &["127.0.0.1:0".parse().unwrap()], opts).unwrap();
    let addr = server.addrs()[0];
    // A client that drips one byte at a time never finishes inside the
    // deadline: it is answered 408 and closed well before 10 drips.
    let mut slow = open_conn(addr);
    let start = std::time::Instant::now();
    for b in b"GET /licence/v1/identity HTTP/1.1\r\nHost: x".iter() {
        if slow.write_all(&[*b]).is_err() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        if start.elapsed() > std::time::Duration::from_secs(4) {
            break;
        }
    }
    let mut out = Vec::new();
    let _ = slow.read_to_end(&mut out);
    assert!(start.elapsed() < std::time::Duration::from_secs(3), "held for {:?}", start.elapsed());
    assert!(String::from_utf8_lossy(&out).contains(" 408 "), "{}", String::from_utf8_lossy(&out));
    // Four idle connections from one address fill the cap; the fifth is refused.
    let idle: Vec<TcpStream> = (0..4).map(|_| open_conn(addr)).collect();
    std::thread::sleep(std::time::Duration::from_millis(150));
    let mut fifth = open_conn(addr);
    let mut out = Vec::new();
    let _ = fifth.read_to_end(&mut out);
    assert!(String::from_utf8_lossy(&out).contains(" 503 "), "{}", String::from_utf8_lossy(&out));
    drop(idle);
    // After they close (or time out), service resumes.
    std::thread::sleep(std::time::Duration::from_millis(900));
    assert_eq!(roundtrip(addr, &h, "GET", "/licence/v1/identity", b"", false).0, 200);
    server.stop();
}

#[test]
fn a_host_header_that_is_not_a_listen_address_is_refused() {
    let h = Harness::new(&[]);
    let server = weft_licence::http::serve(h.svc.clone(), &["127.0.0.1:0".parse().unwrap()]).unwrap();
    let addr = server.addrs()[0];
    let mut s = open_conn(addr);
    s.write_all(b"GET /licence/v1/identity HTTP/1.1\r\nHost: evil.example\r\n\r\n").unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    assert!(String::from_utf8_lossy(&out).contains(" 400 "));
    server.stop();
}

#[test]
fn the_http_layer_throttles_the_transfer() {
    // 96 KiB at 48 KiB/s must take about two seconds on the wire.
    let big = vec![7u8; 96 * 1024];
    let h = Harness::with(&[("big", "arm", &big)], |c| {
        c.limits.rate_bytes_per_sec = 48 * 1024;
        c.limits.requests_per_min = 100;
    });
    let server = weft_licence::http::serve(h.svc.clone(), &["127.0.0.1:0".parse().unwrap()]).unwrap();
    let addr = server.addrs()[0];
    let req = br#"{"request_id":"r","cog_id":"big","version":"latest","arch":"arm"}"#;
    let (st, body) = roundtrip(addr, &h, "POST", "/licence/v1/checkout", req, true);
    assert_eq!(st, 200);
    let j: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let path = j["artifacts"][0]["path"].as_str().unwrap().to_string();
    let start = std::time::Instant::now();
    let (st, bytes) = roundtrip(addr, &h, "GET", &path, b"", true);
    let took = start.elapsed();
    assert_eq!((st, bytes.len()), (200, big.len()));
    assert!(took >= std::time::Duration::from_millis(1500), "took only {took:?}");
    assert!(took < std::time::Duration::from_secs(6), "took {took:?}");
    server.stop();
}

#[test]
fn host_matching_is_semantic_for_ips_and_allows_listed_names() {
    use weft_licence::http::host_allowed;
    let bound: Vec<std::net::SocketAddr> =
        vec!["127.0.0.1:8700".parse().unwrap(), "[::1]:8700".parse().unwrap(), "[fe80::1]:8700".parse().unwrap()];
    let names = vec!["seed.tailnet.example".to_string()];
    // IPv4, and IPv6 in any spelling: uncompressed, upper case.
    assert!(host_allowed("127.0.0.1:8700", &bound, &names));
    assert!(host_allowed("[::1]:8700", &bound, &names));
    assert!(host_allowed("[0:0:0:0:0:0:0:1]:8700", &bound, &names));
    assert!(host_allowed("[0000:0000:0000:0000:0000:0000:0000:0001]:8700", &bound, &names));
    assert!(host_allowed("[FE80:0:0:0:0:0:0:1]:8700", &bound, &names));
    // A link-local address with a zone id still names the same address.
    assert!(host_allowed("[fe80::1%en0]:8700", &bound, &names));
    // IPv4-mapped IPv6 is the IPv4 address.
    assert!(host_allowed("[::ffff:127.0.0.1]:8700", &bound, &names));
    // The port must match, and other addresses and names are refused.
    assert!(!host_allowed("127.0.0.1:8701", &bound, &names));
    assert!(!host_allowed("127.0.0.2:8700", &bound, &names));
    assert!(!host_allowed("::1:8700", &bound, &names), "a bare IPv6 address is not a valid Host");
    assert!(!host_allowed("evil.example:8700", &bound, &names));
    // A listed name is case-insensitive and needs a bound port.
    assert!(host_allowed("Seed.Tailnet.Example:8700", &bound, &names));
    assert!(!host_allowed("seed.tailnet.example:9", &bound, &names));
    assert!(!host_allowed("seed.tailnet.example.evil:8700", &bound, &names));
    assert!(!host_allowed("[::1", &bound, &names));
    assert!(!host_allowed("", &bound, &names));
}

#[test]
fn a_listed_magicdns_name_reaches_the_seed_over_http() {
    let h = Harness::with(&[], |c| c.allowed_hosts = vec!["seed.tailnet.example".into()]);
    let server = weft_licence::http::serve(h.svc.clone(), &["127.0.0.1:0".parse().unwrap()]).unwrap();
    let addr = server.addrs()[0];
    let ask = |host: String| {
        let mut s = open_conn(addr);
        s.write_all(format!("GET /licence/v1/identity HTTP/1.1\r\nHost: {host}\r\n\r\n").as_bytes()).unwrap();
        let mut out = Vec::new();
        let _ = s.read_to_end(&mut out);
        String::from_utf8_lossy(&out).split_whitespace().nth(1).unwrap_or("").to_string()
    };
    assert_eq!(ask(format!("Seed.Tailnet.Example:{}", addr.port())), "200");
    assert_eq!(ask(format!("other.example:{}", addr.port())), "400");
    server.stop();
    assert_eq!(weft_licence::http::ServerOpts::default().per_ip, 2);
}
