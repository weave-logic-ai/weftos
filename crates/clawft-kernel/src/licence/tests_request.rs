//! Steward to `weft-licence` request signing.

use super::request::*;
use super::tests_common::*;
use super::*;

const NODE: &str = "node-steward";
const AUD: &str = "seed-test";
/// A time past the clock floor, in ms.
pub(super) const NOW_MS: u64 = 1_790_000_000_000;
const NONCE: &str = "abcdefghijklmnop0123456789ABCDEF";

fn steward() -> ed25519_dalek::SigningKey {
    sk(21)
}

fn pk() -> [u8; 32] {
    steward().verifying_key().to_bytes()
}

fn signed(path: &str, body: &[u8], ts: u64) -> LicenceRequest {
    sign_request(&steward(), NODE, AUD, "POST", path, body.to_vec(), ts, NONCE)
}

fn verify(r: &LicenceRequest, now: u64, g: &mut ReplayGuard) -> Result<(), RequestRefused> {
    verify_request(r, &pk(), NODE, AUD, now, g)
}

#[test]
fn golden_vector_matches_weft_licence() {
    // The exact string `weft-licence` (crates/weft-licence/src/request.rs,
    // `signing_string`, 0eeffd88e) builds for these inputs: method, target,
    // node, seed_device_id (audience), timestamp in milliseconds, nonce,
    // sha256 of the body.
    let s = signing_string("POST", "/licence/v1/checkout", "node-steward", "seed-1", 1_790_000_000_000, "abcdefghijklmnop", b"{\"a\":1}");
    assert_eq!(
        s,
        "weft-licence-v1/request\nPOST\n/licence/v1/checkout\nnode-steward\nseed-1\n1790000000000\n\
         abcdefghijklmnop\n015abd7f5cc57a2dd94b7590f04ad8084273905ee33ec5cebeae62276a97f862"
    );
    assert_eq!(REQUEST_DOMAIN, "weft-licence-v1/request");
    assert_eq!(REQUEST_WINDOW_MS, 120_000);
    assert_eq!(CLOCK_FLOOR_SECS, 1_780_000_000);
}

#[test]
fn nonces_are_16_to_64_alphanumerics() {
    assert!(valid_nonce(&"a".repeat(16)) && valid_nonce(&"Z9".repeat(32)));
    assert!(!valid_nonce(&"a".repeat(15)) && !valid_nonce(&"a".repeat(65)));
    assert!(!valid_nonce("abcdefghijklmnop-"));
    assert!(valid_nonce(&super::request_nonce()), "generated nonces are accepted");
}

#[test]
fn a_good_request_verifies_once_and_a_replay_is_refused() {
    let r = signed("/licence/v1/checkout", b"{\"a\":1}", NOW_MS);
    let mut g = ReplayGuard::default();
    assert_eq!(verify(&r, NOW_MS + 5_000, &mut g), Ok(()));
    assert_eq!(verify(&r, NOW_MS + 5_000, &mut g), Err(RequestRefused::Replay));
}

#[test]
fn changing_any_signed_field_breaks_the_signature() {
    let base = signed("/licence/v1/checkout", b"body", NOW_MS);
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
    r.auth.as_mut().unwrap().timestamp_ms += 1;
    cases.push(("timestamp", r));
    let mut r = base.clone();
    r.auth.as_mut().unwrap().nonce = "z".repeat(32);
    cases.push(("nonce", r));
    for (what, r) in cases {
        let mut g = ReplayGuard::default();
        assert_eq!(verify(&r, NOW_MS, &mut g), Err(RequestRefused::BadSignature), "{what}");
    }
}

#[test]
fn unsigned_wrong_node_stale_malformed_and_oversized_requests_are_refused() {
    let mut g = ReplayGuard::default();
    let mut r = signed("/p", b"", NOW_MS);
    r.auth = None;
    assert_eq!(verify(&r, NOW_MS, &mut g), Err(RequestRefused::Unsigned));

    let r = sign_request(&steward(), "another-node", AUD, "POST", "/p", vec![], NOW_MS, NONCE);
    assert_eq!(verify(&r, NOW_MS, &mut g), Err(RequestRefused::WrongNode));

    let r = sign_request(&sk(99), NODE, AUD, "POST", "/p", vec![], NOW_MS, NONCE);
    assert_eq!(verify(&r, NOW_MS, &mut g), Err(RequestRefused::BadSignature), "another key");

    let r = signed("/p", b"", NOW_MS);
    assert_eq!(verify(&r, NOW_MS + REQUEST_WINDOW_MS + 1, &mut g), Err(RequestRefused::Stale));
    assert_eq!(verify(&r, NOW_MS - REQUEST_WINDOW_MS - 1, &mut g), Err(RequestRefused::Stale));
    assert_eq!(verify(&r, NOW_MS + REQUEST_WINDOW_MS, &mut g), Ok(()), "the window edge is inside");

    let r = sign_request(&steward(), NODE, AUD, "POST", "/p", vec![], NOW_MS, "short");
    assert_eq!(verify(&r, NOW_MS, &mut g), Err(RequestRefused::Malformed));

    let big = vec![0u8; MAX_REQUEST_BODY + 1];
    let r = signed("/p", &big, NOW_MS);
    assert_eq!(verify(&r, NOW_MS, &mut g), Err(RequestRefused::TooLarge));
}

#[test]
fn the_replay_memory_evicts_by_timestamp_and_refuses_when_full_never_forgetting_everything() {
    let mut g = ReplayGuard::default();
    let t0 = NOW_MS;
    for i in 0..4096 {
        assert!(g.first_use(&format!("n{i:020}"), t0, t0), "{i}");
    }
    // Full of live entries: a new nonce is refused, and the old ones are still remembered.
    assert!(!g.first_use("fresh0000000000000", t0, t0 + 1));
    assert!(!g.first_use(&format!("n{:020}", 7), t0, t0 + 1), "an old nonce is still a replay");
    // Once they age out of the window the memory frees up by itself.
    let later = t0 + REQUEST_WINDOW_MS + 1;
    assert!(g.first_use("fresh0000000000000", later, later));
    assert!(g.first_use(&format!("n{:020}", 7), later, later), "evicted by timestamp");
}

#[test]
fn a_refused_request_does_not_use_up_a_nonce() {
    let good = signed("/p", b"x", NOW_MS);
    let mut forged = good.clone();
    forged.body = b"y".to_vec();
    let mut g = ReplayGuard::default();
    assert_eq!(verify(&forged, NOW_MS, &mut g), Err(RequestRefused::BadSignature));
    assert_eq!(verify(&good, NOW_MS, &mut g), Ok(()));
}

#[test]
fn a_request_signed_for_another_seed_is_refused() {
    let r = sign_request(&steward(), NODE, "another-seed", "POST", "/p", vec![], NOW_MS, NONCE);
    assert_eq!(verify(&r, NOW_MS, &mut ReplayGuard::default()), Err(RequestRefused::BadSignature));
}

#[test]
fn a_grant_signature_is_not_a_request_signature() {
    let env = grant(1, T0, 1000, &["aarch64"]);
    let mut r = signed("/p", b"", NOW_MS);
    r.auth.as_mut().unwrap().signature = env.signature.clone();
    let mut g = ReplayGuard::default();
    assert!(verify(&r, NOW_MS, &mut g).is_err());
}

#[tokio::test]
async fn a_client_whose_clock_is_below_the_floor_signs_nothing() {
    use super::tests_stub::*;
    use std::sync::atomic::AtomicU64;
    let stub = StubLicence::new(Arc::new(AtomicU64::new(T0)), std::time::Duration::ZERO);
    let early: ClockMs = Arc::new(|| (CLOCK_FLOOR_SECS - 1) * 1000);
    let c = SignedLicenceClient::new(sk(21), STEWARD_NODE, "seed-test", StubLink(stub.clone()), early);
    let e = c.checkout(&wire("aarch64")).await.unwrap_err();
    assert_eq!(e, LicenceClientError::Refused { status: 0, code: "clock_not_set".into() });
    assert_eq!(stub.checkouts.load(std::sync::atomic::Ordering::SeqCst), 0);
}
