use serde_json::json;

use super::*;

const P1: &str = "01JB8Z3Q0V6X9KQ4M2N7T5R1WD";
const P2: &str = "01JB8Z3Q0V6X9KQ4M2N7T5R1WE";
const KID: &str = "6a3803d5f059902a1c6dafbc9ba47292";
const SIBLING: &str = "0123456789abcdef0123456789abcdef";
const T: u64 = 1_000_000;

fn keys() -> (SigningKey, ForwardVerifier) {
    let k = SigningKey::from_bytes(&[7u8; 32]);
    let v = ForwardVerifier::new(&k.verifying_key().to_bytes()).unwrap();
    (k, v)
}

fn b<'a>(method: &'a str, params: &'a Value) -> ForwardBinding<'a> {
    ForwardBinding { method, params, target_key_id: KID }
}

#[test]
fn a_fresh_signed_header_verifies_once() {
    let (k, v) = keys();
    let p = json!({"a": 1});
    let h = sign_forward(&k, P1, T, &b("kernel.ps", &p));
    let t0 = Instant::now();
    let vp = v.verify(&h, P1, &b("kernel.ps", &p), T + 100, t0).unwrap();
    assert_eq!(vp.as_str(), P1);
    assert_eq!(v.verify(&h, P1, &b("kernel.ps", &p), T + 200, t0), Err(ForwardError::Replayed));
}

#[test]
fn forged_header_is_refused_and_does_not_poison_replay_table() {
    let (k, v) = keys();
    let p = json!({});
    let t0 = Instant::now();
    let other = SigningKey::from_bytes(&[9u8; 32]);
    let forged = sign_forward(&other, P1, T, &b("m", &p));
    assert_eq!(v.verify(&forged, P1, &b("m", &p), T, t0), Err(ForwardError::BadSignature));
    let mut h = sign_forward(&k, P1, T, &b("m", &p));
    h.issued_at_ms += 1;
    assert_eq!(v.verify(&h, P1, &b("m", &p), T, t0), Err(ForwardError::BadSignature));
    let mut h = sign_forward(&k, P1, T, &b("m", &p));
    h.sig = "zz".into();
    assert_eq!(v.verify(&h, P1, &b("m", &p), T, t0), Err(ForwardError::Malformed));
    assert!(v.verify(&sign_forward(&k, P1, T, &b("m", &p)), P1, &b("m", &p), T, t0).is_ok());
}

#[test]
fn header_for_another_project_is_refused() {
    let (k, v) = keys();
    let p = json!({});
    let h = sign_forward(&k, P2, T, &b("m", &p));
    assert_eq!(v.verify(&h, P1, &b("m", &p), T, Instant::now()), Err(ForwardError::WrongProject));
    let mut relabelled = h.clone();
    relabelled.project_id = P1.into();
    assert_eq!(
        v.verify(&relabelled, P1, &b("m", &p), T, Instant::now()),
        Err(ForwardError::BadSignature)
    );
}

#[test]
fn a_header_reattached_to_another_method_params_or_child_is_refused() {
    let (k, v) = keys();
    let p = json!({"id": "x"});
    let h = sign_forward(&k, P1, T, &b("agent.list", &p));
    let t0 = Instant::now();
    assert_eq!(v.verify(&h, P1, &b("kernel.shutdown", &p), T, t0), Err(ForwardError::BadSignature));
    let other = json!({"id": "y"});
    assert_eq!(v.verify(&h, P1, &b("agent.list", &other), T, t0), Err(ForwardError::BadSignature));
    let sibling = ForwardBinding { method: "agent.list", params: &p, target_key_id: SIBLING };
    assert_eq!(v.verify(&h, P1, &sibling, T, t0), Err(ForwardError::BadSignature));
    // Key order in params does not matter (canonical form).
    let reordered: Value = serde_json::from_str(r#"{"id":"x"}"#).unwrap();
    assert!(v.verify(&h, P1, &b("agent.list", &reordered), T, t0).is_ok());
}

#[test]
fn window_is_five_seconds_either_way() {
    let (k, v) = keys();
    let p = json!({});
    let t0 = Instant::now();
    let h = sign_forward(&k, P1, T, &b("m", &p));
    assert_eq!(
        v.verify(&h, P1, &b("m", &p), T + FORWARD_WINDOW_MS + 1, t0),
        Err(ForwardError::OutsideWindow)
    );
    let h = sign_forward(&k, P1, T + FORWARD_WINDOW_MS + 1, &b("m", &p));
    assert_eq!(v.verify(&h, P1, &b("m", &p), T, t0), Err(ForwardError::OutsideWindow));
    let h = sign_forward(&k, P1, T, &b("m", &p));
    assert!(v.verify(&h, P1, &b("m", &p), T + FORWARD_WINDOW_MS, t0).is_ok());
}

#[test]
fn replay_survives_a_wall_clock_step_and_is_pruned_on_the_monotonic_clock() {
    let (k, v) = keys();
    let p = json!({});
    let t0 = Instant::now();
    let h = sign_forward(&k, P1, T, &b("m", &p));
    v.verify(&h, P1, &b("m", &p), T, t0).unwrap();
    // Wall clock stepped back to the same instant: still a replay.
    assert_eq!(v.verify(&h, P1, &b("m", &p), T, t0 + Duration::from_secs(9)), Err(ForwardError::Replayed));
    // Past twice the window the entry is dropped (the signature is long
    // outside its own window by then, so it is refused as expired instead).
    let late = t0 + Duration::from_millis(2 * FORWARD_WINDOW_MS + 1);
    assert_eq!(v.verify(&h, P1, &b("m", &p), T + 60_000, late), Err(ForwardError::OutsideWindow));
    for i in 0..50 {
        let h = sign_forward(&k, P1, T + i, &b("n", &p));
        v.verify(&h, P1, &b("n", &p), T + i, t0).unwrap();
    }
    let h = sign_forward(&k, P1, T + 60_000, &b("n", &p));
    v.verify(&h, P1, &b("n", &p), T + 60_000, late + Duration::from_secs(1)).unwrap();
    assert_eq!(v.seen.lock().unwrap().len(), 1);
}

#[test]
fn without_installed_trust_forward_is_unavailable() {
    let (k, _) = keys();
    let p = json!({});
    let h = sign_forward(&k, P1, T, &b("m", &p));
    // No instance attestation / trust in this test binary unless another
    // test installed one; either way it must not verify.
    assert!(verify_installed(&h, P1, "m", &p).is_err());
}

#[test]
fn stamp_forward_binds_the_request_and_pins_the_project() {
    let (k, v) = keys();
    let mut req = clawft_rpc::Request::with_params("agent.list", json!({"x": 1}));
    stamp_forward(&mut req, &k, P1, KID, T);
    assert_eq!(req.project.as_deref(), Some(P1));
    let h = req.forward.clone().unwrap();
    assert!(v.verify(&h, P1, &b("agent.list", &req.params), T, Instant::now()).is_ok());
}

#[test]
fn domain_tag_is_distinct_from_the_other_project_tags() {
    for other in [
        "weftos-project-cert-v1\n",
        "weftos-project-anchor-v1\n",
        "weftos-mesh-local-pop-v1\n",
        "weftos-mesh-local-pop-v2\n",
        "weftos-project-forward-v1\n",
    ] {
        assert_ne!(FORWARD_DOMAIN, other);
    }
}
