//! Types: mesh id, signing domains, verification profiles.

use sha2::{Digest, Sha256};

use super::tests_common::*;
use super::*;

#[test]
fn mesh_id_is_the_documented_hash_and_depends_on_both_inputs() {
    let mut h = Sha256::new();
    h.update(b"weft-licence-v1/mesh-id\n");
    h.update([9u8; 32]);
    h.update([7u8; 32]);
    assert_eq!(mesh().to_hex(), hex_encode(&h.finalize()));
    assert_ne!(mesh(), other_mesh());
    assert_ne!(mesh(), MeshId::derive(&[1; 32], &[7; 32]));
    assert_eq!(MeshId::from_hex(&mesh().to_hex()), Some(mesh()));
}

#[test]
fn member_profile_accepts_an_operator_signed_binding_for_this_mesh() {
    let rec = verify_binding_member(&binding(3, BindState::Bound), &anchors(), &mesh()).unwrap();
    assert_eq!((rec.seq, rec.state), (3, BindState::Bound));
}

#[test]
fn member_profile_has_no_age_check() {
    let mut rec = binding_rec(1, BindState::Bound, &grant_key(), &mesh());
    rec.bound_at = 1; // decades old: members never look at it
    let s = sign_binding(&rec, &op()).unwrap();
    assert!(verify_binding_member(&s, &anchors(), &mesh()).is_ok());
}

#[test]
fn binding_for_another_mesh_is_refused() {
    let s = sign_binding(&binding_rec(1, BindState::Bound, &grant_key(), &other_mesh()), &op())
        .unwrap();
    assert_eq!(verify_binding_member(&s, &anchors(), &mesh()), Err(LicenceError::WrongMesh));
}

#[test]
fn binding_signed_by_a_non_operator_key_is_refused() {
    let rec = binding_rec(1, BindState::Bound, &grant_key(), &mesh());
    for k in [sk(30), sk(5), grant_key()] {
        // unpinned, pinned-but-WeftOS, and the grant key itself
        let s = sign_binding(&rec, &k).unwrap();
        assert_eq!(
            verify_binding_member(&s, &anchors(), &mesh()),
            Err(LicenceError::UntrustedKey)
        );
    }
}

#[test]
fn tampered_payload_fails_the_signature() {
    let mut s = binding(1, BindState::Bound);
    s.payload = s.payload.replace("\"seq\":1", "\"seq\":2");
    assert_eq!(verify_binding_member(&s, &anchors(), &mesh()), Err(LicenceError::BadSignature));
}

#[test]
fn non_canonical_payload_is_refused_even_when_signed() {
    let rec = binding_rec(1, BindState::Bound, &grant_key(), &mesh());
    let pretty = serde_json::to_string_pretty(&rec).unwrap();
    let sig = op().sign(&signed_bytes(BINDING_DOMAIN, &pretty));
    let s = SignedEnvelope {
        payload: pretty,
        public_key: pk_hex(&op()),
        signature: hex_encode(&sig.to_bytes()),
    };
    assert!(matches!(
        verify_binding_member(&s, &anchors(), &mesh()),
        Err(LicenceError::Malformed(_))
    ));
}

#[test]
fn domain_tags_stop_cross_kind_replay() {
    // An approval signed by the operator is not a binding, and vice versa.
    let a = approval(&[sha_of("aarch64")]);
    assert_eq!(
        verify_envelope(BINDING_DOMAIN, &a, &op().verifying_key().to_bytes()),
        Err(LicenceError::BadSignature)
    );
    let g = grant(1, T0, 3600, &["aarch64"]);
    assert_eq!(
        verify_envelope(APPROVAL_DOMAIN, &g, &grant_key().verifying_key().to_bytes()),
        Err(LicenceError::BadSignature)
    );
}

#[test]
fn grant_must_be_signed_by_the_bound_key_for_this_mesh() {
    let pk = grant_key().verifying_key().to_bytes();
    let g = grant(1, T0, 3600, &["aarch64"]);
    assert!(verify_grant(&g, &pk, &mesh()).is_ok());
    assert_eq!(verify_grant(&g, &pk, &other_mesh()), Err(LicenceError::WrongMesh));
    // Signed by a node key, or by a key the binding does not name.
    for k in [sk(3), sk(4)] {
        let s = sign_grant(&grant_rec(1, T0, 3600, &["aarch64"]), &k).unwrap();
        assert_eq!(verify_grant(&s, &pk, &mesh()), Err(LicenceError::UntrustedKey));
    }
}

#[test]
fn grant_ttl_over_seven_days_is_refused() {
    let pk = grant_key().verifying_key().to_bytes();
    let g = grant(1, T0, MAX_GRANT_TTL_SECS + 1, &["aarch64"]);
    assert_eq!(verify_grant(&g, &pk, &mesh()), Err(LicenceError::TtlTooLong));
}

#[test]
fn grant_id_and_key_id_are_checked() {
    let pk = grant_key().verifying_key().to_bytes();
    let mut rec = grant_rec(1, T0, 3600, &["aarch64"]);
    rec.grant_key_id = key_id(&pk);
    rec.grant_id = "0".repeat(64); // not the hash of the content
    let s = sign_envelope(GRANT_DOMAIN, &rec, &grant_key()).unwrap();
    assert!(matches!(verify_grant(&s, &pk, &mesh()), Err(LicenceError::Malformed(_))));
}

#[test]
fn withdrawal_is_a_renewal_that_expires_at_or_before_issue() {
    assert!(grant_rec(2, T0, 0, &["aarch64"]).is_withdrawal());
    assert!(!grant_rec(2, T0, 1, &["aarch64"]).is_withdrawal());
}

#[test]
fn approval_content_key_ignores_signing_time_and_input_order() {
    let (a, b) = (sha_of("aarch64"), sha_of("x86_64"));
    let key = |shas: &[String], at: u64| {
        let mut rec = approval_rec(shas, &mesh());
        rec.approved_at = at;
        let signed = sign_approval(&rec, &op()).unwrap(); // sorts and de-duplicates
        verify_approval(&signed, &anchors(), &mesh()).unwrap().content_key()
    };
    let base = key(&[a.clone(), b.clone()], T0);
    assert_eq!(base, key(&[b.clone(), a.clone()], T0 + 99));
    assert_eq!(base, key(&[a.clone(), b.clone(), a.clone()], T0));
    assert_ne!(base, key(&[a], T0));
}

#[test]
fn approval_must_be_for_the_local_mesh_and_pinned_operator() {
    let shas = [sha_of("aarch64")];
    assert!(verify_approval(&approval(&shas), &anchors(), &mesh()).is_ok());
    assert_eq!(
        verify_approval(&approval(&shas), &anchors(), &other_mesh()),
        Err(LicenceError::WrongMesh)
    );
    let by_grant_key = sign_approval(&approval_rec(&shas, &mesh()), &grant_key()).unwrap();
    assert_eq!(
        verify_approval(&by_grant_key, &anchors(), &mesh()),
        Err(LicenceError::UntrustedKey)
    );
}
