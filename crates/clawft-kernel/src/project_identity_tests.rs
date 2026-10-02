//! Tests for [`super`]. Golden values match
//! `clawft-types/src/project/cert_tests.rs`.

use super::*;
use crate::chain::ChainManager;
use chrono::Duration;
use serde_json::json;

const PID: &str = "01JB8Z3Q0V6X9KQ4M2N7T5R1WD";
const OTHER_PID: &str = "01JB8Z3Q0V6X9KQ4M2N7T5R1WE";
const CERT_SIG: &str = "081ef31a626b34369d2da9df686d53f2ed1f96e9fc78ab5c554e5921b96bdf7e77d612e6196a0c60ac69c952e0a4d91e2ed1b8d70224274fb53479d8160cb701";
const POP_NONCE: &str = "00112233445566778899aabbccddeeff";
const POP_SIG: &str = "71077e668d8fff3f84e60927fe75ef129c31b3faae3b40216001047f6eb8122e0df9885b654756429be0a310b16073d03d23cf8d0e51dbdcc13ddf25c67ac405";

fn user_key() -> SigningKey {
    SigningKey::from_bytes(&[1u8; 32])
}
fn proj_key() -> SigningKey {
    SigningKey::from_bytes(&[2u8; 32])
}
fn t(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
}
fn now() -> DateTime<Utc> {
    t("2026-10-02T00:00:00Z")
}
fn user_pk() -> [u8; 32] {
    user_key().verifying_key().to_bytes()
}
fn req_for(pid: &str, key: &SigningKey, serial: u64) -> CertRequest {
    CertRequest {
        project_id: pid.into(),
        project_pubkey: key.verifying_key().to_bytes(),
        serial,
        issued_at: t("2026-10-01T09:30:00Z"),
        expires_at: None,
    }
}
fn cert() -> ProjectCert {
    sign_cert(&user_key(), &req_for(PID, &proj_key(), 1)).unwrap()
}

#[test]
fn golden_cert_signature_is_reproduced() {
    let c = cert();
    assert_eq!(c.sig, CERT_SIG);
    verify_cert_at(&c, &user_pk(), now()).unwrap();
}

#[test]
fn issuing_twice_is_byte_identical() {
    let (a, b) = (cert(), cert());
    assert_eq!(a.canonical_bytes(), b.canonical_bytes());
    assert_eq!(a, b);
}

#[test]
fn tampering_with_every_field_fails() {
    let other = SigningKey::from_bytes(&[3u8; 32]);
    let other_pk = hex(&other.verifying_key().to_bytes());
    let cases: Vec<(&str, Box<dyn Fn(&mut ProjectCert)>)> = vec![
        ("v", Box::new(|c| c.v = 2)),
        ("type", Box::new(|c| c.kind = "x".into())),
        ("project_id", Box::new(|c| c.project_id = OTHER_PID.into())),
        ("project_pubkey", Box::new(move |c| c.project_pubkey = other_pk.clone())),
        ("project_key_id", Box::new(|c| c.project_key_id = "0".repeat(32))),
        ("user_key_id", Box::new(|c| c.user_key_id = "0".repeat(32))),
        ("user_pubkey", Box::new(|c| c.user_pubkey = "0".repeat(64))),
        ("serial", Box::new(|c| c.serial = 2)),
        ("issued_at", Box::new(|c| c.issued_at = "2026-10-01T09:30:01Z".into())),
        ("expires_at", Box::new(|c| c.expires_at = Some("2030-01-01T00:00:00Z".into()))),
        ("sig", Box::new(|c| c.sig = "0".repeat(128))),
    ];
    for (name, mutate) in cases {
        let mut c = cert();
        mutate(&mut c);
        assert!(verify_cert_at(&c, &user_pk(), now()).is_err(), "tampered {name} verified");
    }
}

#[test]
fn wrong_user_key_and_key_id_mismatch_fail() {
    let stranger = SigningKey::from_bytes(&[9u8; 32]).verifying_key().to_bytes();
    assert_eq!(verify_cert_at(&cert(), &stranger, now()), Err(CertError::UntrustedUser));
    let mut c = cert();
    c.project_key_id = key_id(&[7u8; 32]);
    assert_eq!(
        verify_cert_at(&c, &user_pk(), now()),
        Err(CertError::KeyIdMismatch("project_key_id"))
    );
}

#[test]
fn expiry_is_honoured_and_a_backwards_lifetime_refused() {
    let mut r = req_for(PID, &proj_key(), 1);
    r.expires_at = Some(r.issued_at + Duration::hours(1));
    let c = sign_cert(&user_key(), &r).unwrap();
    verify_cert_at(&c, &user_pk(), t("2026-10-01T09:40:00Z")).unwrap();
    assert_eq!(verify_cert_at(&c, &user_pk(), now()), Err(CertError::Expired));
    r.expires_at = Some(r.issued_at);
    assert!(matches!(sign_cert(&user_key(), &r), Err(IdentityError::BadLifetime)));
    r.expires_at = None;
    r.project_id = "not-a-ulid".into();
    assert!(sign_cert(&user_key(), &r).is_err());
}

#[test]
fn pop_matches_golden_and_binds_nonce_id_and_key() {
    let pk = proj_key().verifying_key().to_bytes();
    let sig = pop_sign(&proj_key(), POP_NONCE, PID).unwrap();
    assert_eq!(hex(&sig), POP_SIG);
    pop_verify(&pk, POP_NONCE, PID, &sig).unwrap();
    assert!(pop_verify(&pk, "ff".repeat(16).as_str(), PID, &sig).is_err());
    assert!(pop_verify(&pk, POP_NONCE, OTHER_PID, &sig).is_err());
    let stranger = SigningKey::from_bytes(&[3u8; 32]).verifying_key().to_bytes();
    assert!(pop_verify(&stranger, POP_NONCE, PID, &sig).is_err());
    assert_eq!(pop_sign(&proj_key(), "short", PID), Err(CertError::BadNonce));
}

#[test]
fn signatures_are_domain_separated() {
    // The project key signing the bare PoP payload without its tag, or a
    // cert's signed bytes, does not pass as a PoP, and vice versa.
    let pk = proj_key().verifying_key().to_bytes();
    let untagged = proj_key().sign(format!("{POP_NONCE}\n{PID}").as_bytes()).to_bytes();
    assert!(pop_verify(&pk, POP_NONCE, PID, &untagged).is_err());
    let as_cert_bytes = proj_key().sign(&cert().canonical_bytes()).to_bytes();
    assert!(pop_verify(&pk, POP_NONCE, PID, &as_cert_bytes).is_err());
    let pop = pop_sign(&proj_key(), POP_NONCE, PID).unwrap();
    let mut forged = cert();
    forged.sig = hex(&pop);
    assert!(verify_cert_at(&forged, &proj_key().verifying_key().to_bytes(), now()).is_err());
}

#[test]
fn key_file_roundtrip_and_refusals() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a/project.key");
    let k = load_or_create_project_key(&p).unwrap();
    assert_eq!(std::fs::read(&p).unwrap(), k.to_bytes());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
    }
    assert_eq!(load_or_create_project_key(&p).unwrap().to_bytes(), k.to_bytes());
    // Same format as ChainManager::load_or_create_key.
    assert_eq!(ChainManager::load_or_create_key(&p).unwrap().to_bytes(), k.to_bytes());

    // Wrong size.
    let short = dir.path().join("short.key");
    write_private_atomic(&short, &[1, 2, 3], true).unwrap();
    assert!(matches!(load_or_create_project_key(&short), Err(IdentityError::KeyFile { .. })));
    // A directory.
    assert!(load_or_create_project_key(dir.path()).is_err());
}

#[cfg(unix)]
#[test]
fn loose_permissions_and_symlinks_are_refused_not_fixed() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("project.key");
    load_or_create_project_key(&p).unwrap();
    for mode in [0o644, 0o640, 0o604] {
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
        assert!(
            matches!(load_or_create_project_key(&p), Err(IdentityError::KeyFile { .. })),
            "mode {mode:o} accepted"
        );
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, mode, "was rewritten");
    }
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
    let link = dir.path().join("link.key");
    std::os::unix::fs::symlink(&p, &link).unwrap();
    assert!(load_or_create_project_key(&link).is_err());
}

#[test]
fn exclusive_write_refuses_to_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("f");
    write_private_atomic(&p, b"one", true).unwrap();
    assert!(write_private_atomic(&p, b"two", true).is_err());
    assert_eq!(std::fs::read(&p).unwrap(), b"one");
    write_private_atomic(&p, b"three", false).unwrap();
    assert_eq!(std::fs::read(&p).unwrap(), b"three");
    // No temp files left behind.
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

fn chain_with(events: &[(&str, serde_json::Value)]) -> Vec<ChainEvent> {
    let cm = ChainManager::new(0, 1000);
    for (kind, payload) in events {
        cm.append(SOURCE, kind, Some(payload.clone()));
    }
    cm.tail_from(0)
}

#[test]
fn tofu_rekey_then_old_cert_fails_the_revocation_view() {
    let old = cert();
    let new_key = SigningKey::from_bytes(&[3u8; 32]);
    let new_cert = sign_cert(&user_key(), &req_for(PID, &new_key, 2)).unwrap();
    let register = ("project.register", json!({"cert": old}));

    let view = RevocationView::from_events(&chain_with(&[register.clone()]));
    assert_eq!(view.bound_key_id(PID), Some(old.project_key_id.as_str()));
    view.check_cert(&old).unwrap();
    // Same key again: idempotent, existing cert.
    let pk = proj_key().verifying_key().to_bytes();
    assert_eq!(view.plan_registration(PID, &pk).unwrap(), Registration::Existing(Box::new(old.clone())));
    // A second key for the id: refused.
    let npk = new_key.verifying_key().to_bytes();
    assert!(matches!(view.plan_registration(PID, &npk), Err(IdentityError::KeyConflict { .. })));
    // A different project is unaffected.
    assert_eq!(view.plan_registration(OTHER_PID, &npk).unwrap(), Registration::New { serial: 1 });

    let (old_kid, serial) = view.plan_rekey(PID, &npk).unwrap();
    assert_eq!((old_kid.as_str(), serial), (old.project_key_id.as_str(), 2));
    let rekey = (
        "project.rekey",
        json!({"project_id": PID, "old_key_id": old_kid, "new_cert": new_cert, "reason": "rotate"}),
    );
    let view = RevocationView::from_events(&chain_with(&[register, rekey]));
    assert!(view.is_revoked(PID, &old.project_key_id));
    assert!(matches!(view.check_cert(&old), Err(IdentityError::KeyRevoked { .. })));
    view.check_cert(&new_cert).unwrap();
    assert_eq!(view.last_serial(PID), 2);
    // The revoked key can never be re-certified, nor re-used to rekey.
    assert!(matches!(view.plan_registration(PID, &pk), Err(IdentityError::KeyRevoked { .. })));
    assert!(matches!(view.plan_rekey(PID, &pk), Err(IdentityError::KeyRevoked { .. })));
}

#[test]
fn revoke_unbinds_and_next_registration_gets_the_next_serial() {
    let old = cert();
    let view = RevocationView::from_events(&chain_with(&[
        ("project.register", json!({"cert": old})),
        ("project.revoke", json!({"project_id": PID, "old_key_id": old.project_key_id, "reason": "lost"})),
    ]));
    assert_eq!(view.bound_key_id(PID), None);
    assert!(view.plan_rekey(PID, &[5u8; 32]).is_err());
    let npk = SigningKey::from_bytes(&[3u8; 32]).verifying_key().to_bytes();
    assert_eq!(view.plan_registration(PID, &npk).unwrap(), Registration::New { serial: 2 });
}

#[test]
fn foreign_sources_and_malformed_events_are_ignored() {
    let cm = ChainManager::new(0, 1000);
    cm.append("someone.else", KIND_REGISTER, Some(json!({"cert": cert()})));
    cm.append(SOURCE, KIND_REGISTER, Some(json!({"cert": "garbage"})));
    cm.append(SOURCE, KIND_REVOKE, None);
    let view = RevocationView::from_events(&cm.tail_from(0));
    assert_eq!(view.bound_key_id(PID), None);
}

#[test]
fn chain_events_carry_public_material_only() {
    let c = cert();
    let events = chain_with(&[("project.register", json!({"cert": c, "name": "demo"}))]);
    let dump = serde_json::to_string(&events).unwrap();
    let seed = proj_key().to_bytes();
    assert!(!dump.contains(&hex(&seed)));
    assert!(!dump.contains(&hex(&user_key().to_bytes())));
    assert!(!dump.contains(&format!("{:?}", seed.to_vec())));
    assert!(dump.contains(&c.project_pubkey));
}
