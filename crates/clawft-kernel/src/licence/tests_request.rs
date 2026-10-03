//! Steward to `weft-licence` request signing.

use super::request::*;
use super::tests_common::*;
use super::*;

const NODE: &str = "node-steward";

fn steward() -> ed25519_dalek::SigningKey {
    sk(21)
}

fn pk() -> [u8; 32] {
    steward().verifying_key().to_bytes()
}

fn signed(path: &str, body: &[u8], ts: u64) -> LicenceRequest {
    sign_request(&steward(), NODE, "POST", path, body.to_vec(), ts, [7; 16])
}

#[test]
fn the_signed_string_starts_with_the_request_domain_and_binds_every_field() {
    let b = signing_bytes("POST", "/licence/v1/checkout", NODE, T0, "ab", b"{}");
    let s = String::from_utf8(b).unwrap();
    let lines: Vec<&str> = s.split('\n').collect();
    assert_eq!(lines[0], REQUEST_DOMAIN);
    assert_eq!(REQUEST_DOMAIN, "weft-licence-v1/request");
    assert_eq!(&lines[1..6], &["POST", "/licence/v1/checkout", NODE, &T0.to_string(), "ab"]);
    assert_eq!(lines[6], sha256_hex(b"{}"));
}

#[test]
fn a_good_request_verifies_once_and_a_replay_is_refused() {
    let r = signed("/licence/v1/checkout", b"{\"a\":1}", T0);
    let mut g = ReplayGuard::default();
    assert_eq!(verify_request(&r, &pk(), T0 + 5, &mut g), Ok(()));
    assert_eq!(verify_request(&r, &pk(), T0 + 5, &mut g), Err(RequestRefused::Replay));
}

#[test]
fn changing_any_signed_field_breaks_the_signature() {
    let base = signed("/licence/v1/checkout", b"body", T0);
    let mut cases = Vec::new();
    let mut r = base.clone();
    r.method = "GET".into();
    cases.push(("method", r));
    let mut r = base.clone();
    r.path = "/licence/v1/grants".into();
    cases.push(("path", r));
    let mut r = base.clone();
    r.body = b"other".to_vec();
    cases.push(("body", r));
    let mut r = base.clone();
    r.auth.as_mut().unwrap().node = "another".into();
    cases.push(("node", r));
    let mut r = base.clone();
    r.auth.as_mut().unwrap().timestamp += 1;
    cases.push(("timestamp", r));
    let mut r = base.clone();
    r.auth.as_mut().unwrap().nonce = "00".repeat(16);
    cases.push(("nonce", r));
    for (what, r) in cases {
        let mut g = ReplayGuard::default();
        assert_eq!(verify_request(&r, &pk(), T0, &mut g), Err(RequestRefused::BadSignature), "{what}");
    }
}

#[test]
fn unsigned_wrong_key_stale_and_oversized_requests_are_refused_before_the_signature() {
    let mut g = ReplayGuard::default();
    let mut r = signed("/p", b"", T0);
    r.auth = None;
    assert_eq!(verify_request(&r, &pk(), T0, &mut g), Err(RequestRefused::Unsigned));

    let r = sign_request(&sk(99), NODE, "POST", "/p", vec![], T0, [1; 16]);
    assert_eq!(verify_request(&r, &pk(), T0, &mut g), Err(RequestRefused::WrongKey));

    let r = signed("/p", b"", T0);
    let late = T0 + REQUEST_WINDOW_SECS + 1;
    assert_eq!(verify_request(&r, &pk(), late, &mut g), Err(RequestRefused::Stale));
    assert_eq!(verify_request(&r, &pk(), T0 - REQUEST_WINDOW_SECS - 1, &mut g), Err(RequestRefused::Stale));

    let big = vec![0u8; MAX_REQUEST_BODY + 1];
    let r = signed("/p", &big, T0);
    assert_eq!(verify_request(&r, &pk(), T0, &mut g), Err(RequestRefused::TooLarge));
}

#[test]
fn a_refused_request_does_not_use_up_a_nonce() {
    // Only a request whose signature verified is remembered, so a forger
    // cannot burn the steward's nonces.
    let good = signed("/p", b"x", T0);
    let mut forged = good.clone();
    forged.body = b"y".to_vec();
    let mut g = ReplayGuard::default();
    assert_eq!(verify_request(&forged, &pk(), T0, &mut g), Err(RequestRefused::BadSignature));
    assert_eq!(verify_request(&good, &pk(), T0, &mut g), Ok(()));
}

#[test]
fn a_grant_or_binding_signature_is_not_a_request_signature() {
    // Domain separation: the grant key's grant envelope cannot be replayed as
    // request headers even over identical bytes.
    let env = grant(1, T0, 1000, &["aarch64"]);
    let mut r = signed("/p", b"", T0);
    let a = r.auth.as_mut().unwrap();
    a.public_key = env.public_key.clone();
    a.signature = env.signature.clone();
    let mut g = ReplayGuard::default();
    assert!(verify_request(&r, &hex_decode_exact::<32>(&env.public_key).unwrap(), T0, &mut g).is_err());
}
