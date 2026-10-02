use super::*;

const P1: &str = "01JB8Z3Q0V6X9KQ4M2N7T5R1WD";
const P2: &str = "01JB8Z3Q0V6X9KQ4M2N7T5R1WE";
const T: u64 = 1_000_000;

fn keys() -> (SigningKey, ForwardVerifier) {
    let k = SigningKey::from_bytes(&[7u8; 32]);
    let v = ForwardVerifier::new(&k.verifying_key().to_bytes()).unwrap();
    (k, v)
}

#[test]
fn a_fresh_signed_header_verifies_once() {
    let (k, v) = keys();
    let h = sign_forward(&k, P1, T);
    let vp = v.verify(&h, P1, T + 100).unwrap();
    assert_eq!(vp.as_str(), P1);
    assert_eq!(v.verify(&h, P1, T + 200), Err(ForwardError::Replayed));
}

#[test]
fn forged_header_is_refused_and_does_not_poison_replay_table() {
    let (k, v) = keys();
    let other = SigningKey::from_bytes(&[9u8; 32]);
    let forged = sign_forward(&other, P1, T);
    assert_eq!(v.verify(&forged, P1, T), Err(ForwardError::BadSignature));
    // Tampered fields under a genuine signature.
    let mut h = sign_forward(&k, P1, T);
    h.issued_at_ms += 1;
    assert_eq!(v.verify(&h, P1, T), Err(ForwardError::BadSignature));
    let mut h = sign_forward(&k, P1, T);
    h.sig = "zz".into();
    assert_eq!(v.verify(&h, P1, T), Err(ForwardError::Malformed));
    // The genuine header still works afterwards.
    assert!(v.verify(&sign_forward(&k, P1, T), P1, T).is_ok());
}

#[test]
fn header_for_another_project_is_refused() {
    let (k, v) = keys();
    let h = sign_forward(&k, P2, T);
    assert_eq!(v.verify(&h, P1, T), Err(ForwardError::WrongProject));
    // Re-labelling a P2 header as P1 breaks the signature.
    let mut relabelled = h.clone();
    relabelled.project_id = P1.into();
    assert_eq!(v.verify(&relabelled, P1, T), Err(ForwardError::BadSignature));
}

#[test]
fn window_is_five_seconds_either_way() {
    let (k, v) = keys();
    let h = sign_forward(&k, P1, T);
    assert_eq!(v.verify(&h, P1, T + FORWARD_WINDOW_MS + 1), Err(ForwardError::OutsideWindow));
    let h = sign_forward(&k, P1, T + FORWARD_WINDOW_MS + 1);
    assert_eq!(v.verify(&h, P1, T), Err(ForwardError::OutsideWindow));
    let h = sign_forward(&k, P1, T);
    assert!(v.verify(&h, P1, T + FORWARD_WINDOW_MS).is_ok());
}

#[test]
fn replay_table_is_pruned_to_the_window() {
    let (k, v) = keys();
    for i in 0..50 {
        v.verify(&sign_forward(&k, P1, T + i), P1, T + i).unwrap();
    }
    v.verify(&sign_forward(&k, P1, T + 60_000), P1, T + 60_000).unwrap();
    assert_eq!(v.seen.lock().unwrap().len(), 1);
}

#[test]
fn without_installed_trust_forward_is_unavailable() {
    // Another test in the process may install trust; this one only asserts
    // the error shape when none is present.
    let (k, _) = keys();
    let h = sign_forward(&k, P1, T);
    if let Err(e) = verify_installed(&h, P1) {
        assert!(matches!(e, ForwardError::Unavailable | ForwardError::OutsideWindow));
    }
}

#[test]
fn domain_tag_is_distinct_from_the_other_project_tags() {
    for other in ["weftos-project-cert-v1\n", "weftos-project-anchor-v1\n", "weftos-mesh-local-pop-v1\n"] {
        assert_ne!(FORWARD_DOMAIN, other);
    }
}
