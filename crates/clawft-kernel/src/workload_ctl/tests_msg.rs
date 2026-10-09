//! Signed control messages: tamper, key binding, addressee, window,
//! controller authorisation and replay.

use ed25519_dalek::SigningKey;
use serde_json::json;

use super::*;

fn key(n: u8) -> SigningKey {
    SigningKey::from_bytes(&[n; 32])
}

fn id(k: &SigningKey) -> String {
    node_id_from_pubkey(&k.verifying_key().to_bytes())
}

fn ctl() -> Vec<[u8; 32]> {
    vec![key(1).verifying_key().to_bytes()]
}

const NOW: u64 = 1_800_000_000_000;

fn req(m: &str, target: &str) -> CtlRequest {
    CtlRequest::new(
        &key(1),
        m,
        target,
        NOW,
        60_000,
        Some("d".into()),
        json!({"x": 1}),
    )
}

#[test]
fn valid_request_verifies_once_then_replay_is_refused() {
    let host = id(&key(2));
    let r = req(method::STATUS, &host);
    let s = r.sign(&key(1)).unwrap();
    let guard = NonceGuard::new();
    assert_eq!(verify_request(&s, &host, NOW, &ctl(), &guard).unwrap(), r);
    let again = verify_request(&s, &host, NOW + 1, &ctl(), &guard).unwrap_err();
    assert_eq!(again.code, RefusalCode::Replay);
}

#[test]
fn tampered_payload_is_refused() {
    let host = id(&key(2));
    let mut s = req(method::STOP, &host).sign(&key(1)).unwrap();
    s.payload = s.payload.replace("workload.stop", "workload.unload");
    let e = verify_request(&s, &host, NOW, &ctl(), &NonceGuard::new()).unwrap_err();
    assert_eq!(e.code, RefusalCode::Signature);
}

#[test]
fn requester_must_be_the_signing_key() {
    let host = id(&key(2));
    let mut r = req(method::STATUS, &host);
    r.requester = id(&key(3));
    let s = r.sign(&key(1)).unwrap();
    let e = verify_request(&s, &host, NOW, &ctl(), &NonceGuard::new()).unwrap_err();
    assert_eq!(e.code, RefusalCode::Signature);
}

#[test]
fn wrong_addressee_and_any_target_only_for_describe() {
    let host = id(&key(2));
    let other = req(method::STATUS, &id(&key(3))).sign(&key(1)).unwrap();
    let g = NonceGuard::new();
    assert_eq!(
        verify_request(&other, &host, NOW, &ctl(), &g)
            .unwrap_err()
            .code,
        RefusalCode::NotForMe
    );
    let any_status = req(method::STATUS, ANY_TARGET).sign(&key(1)).unwrap();
    assert_eq!(
        verify_request(&any_status, &host, NOW, &ctl(), &g)
            .unwrap_err()
            .code,
        RefusalCode::NotForMe
    );
    let any_describe = req(method::DESCRIBE, ANY_TARGET).sign(&key(1)).unwrap();
    assert!(verify_request(&any_describe, &host, NOW, &ctl(), &g).is_ok());
}

#[test]
fn expired_future_and_overlong_requests_are_refused() {
    let host = id(&key(2));
    let g = NonceGuard::new();
    let s = req(method::STATUS, &host).sign(&key(1)).unwrap();
    assert_eq!(
        verify_request(&s, &host, NOW + 60_000, &ctl(), &g)
            .unwrap_err()
            .code,
        RefusalCode::Expired
    );
    let fut = CtlRequest::new(
        &key(1),
        method::STATUS,
        &host,
        NOW + MAX_SKEW_MS + 1_000,
        10_000,
        None,
        json!({}),
    );
    let s = fut.sign(&key(1)).unwrap();
    assert_eq!(
        verify_request(&s, &host, NOW, &ctl(), &g).unwrap_err().code,
        RefusalCode::Expired
    );
    let long = CtlRequest::new(
        &key(1),
        method::STATUS,
        &host,
        NOW,
        MAX_TTL_MS + 1,
        None,
        json!({}),
    );
    let s = long.sign(&key(1)).unwrap();
    assert_eq!(
        verify_request(&s, &host, NOW, &ctl(), &g).unwrap_err().code,
        RefusalCode::Expired
    );
}

#[test]
fn unknown_controller_is_unauthorised_and_does_not_burn_the_nonce() {
    let host = id(&key(2));
    let r = CtlRequest::new(&key(9), method::STATUS, &host, NOW, 60_000, None, json!({}));
    let s = r.sign(&key(9)).unwrap();
    let g = NonceGuard::new();
    assert_eq!(
        verify_request(&s, &host, NOW, &ctl(), &g).unwrap_err().code,
        RefusalCode::Unauthorized
    );
    // Once the key is authorised the same (unused) nonce is accepted.
    let allowed = vec![key(9).verifying_key().to_bytes()];
    assert!(verify_request(&s, &host, NOW, &allowed, &g).is_ok());
}

#[test]
fn response_is_bound_to_request_and_responder() {
    let host_key = key(2);
    let host = id(&host_key);
    let r = req(method::STATUS, &host);
    let resp = CtlResponse {
        version: CTL_VERSION,
        method: r.method.clone(),
        responder: host.clone(),
        request_nonce: r.nonce.clone(),
        outcome: CtlOutcome::Ok { result: json!({}) },
        trailing: None,
    };
    let s = resp.sign(&host_key);
    let pk = host_key.verifying_key().to_bytes();
    assert!(verify_response(&s, &r, Some(&pk)).is_ok());
    // Another request (other nonce) cannot take this response.
    let r2 = req(method::STATUS, &host);
    assert_eq!(
        verify_response(&s, &r2, Some(&pk)).unwrap_err().code,
        RefusalCode::Replay
    );
    // Signed by a different key than expected.
    let forged = resp.sign(&key(5));
    assert_eq!(
        verify_response(&forged, &r, Some(&pk)).unwrap_err().code,
        RefusalCode::Signature
    );
}

#[test]
fn nonce_guard_fails_closed_when_full_of_live_nonces() {
    let g = NonceGuard::new();
    for i in 0..MAX_NONCES {
        assert!(g.admit(&format!("{i:032x}"), NOW + 1_000, NOW));
    }
    assert!(!g.admit("ff", NOW + 1_000, NOW));
    // Expired entries are pruned to make room.
    assert!(g.admit("fe", NOW + 5_000, NOW + 2_000));
}
