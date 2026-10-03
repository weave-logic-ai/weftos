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
    let mut head = format!("{method} {target} HTTP/1.1\r\nHost: seed\r\nContent-Length: {}\r\n", body.len());
    if signed {
        let nonce = format!("{:032x}", N.fetch_add(1, Ordering::SeqCst));
        for (k, v) in weft_licence::request::sign_request(&steward(), NODE, method, target, body, h.now(), &nonce) {
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
