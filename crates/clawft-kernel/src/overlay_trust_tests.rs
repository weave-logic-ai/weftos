//! Trust root, rollback pin, revocation and history checks of a project
//! kernel's governance (ADR-103 D8, package E review round).

use chrono::{Duration, Utc};
use clawft_types::config::overlay::Limits;
use clawft_types::project::cert::{CertRequest, ProjectCert};
use ed25519_dalek::SigningKey;

use crate::chain::ChainManager;
use crate::governance_overlay::OverlayError;
use crate::governance_overlay_tests::{base_parent, parent_with, user_key};
use crate::overlay_runtime::prepare;
use crate::overlay_trust::{REVOKED_FILE, USER_PIN_FILE, VERSION_PIN_FILE, write_user_pin};
use crate::overlay_runtime_tests::{fixture, start, write_parent};
use crate::parent_policy::{
    VERSION_SLACK_SECS, export_rules, export_to, write_atomic_0600,
};

fn pin_path(f: &crate::overlay_runtime_tests::Fixture) -> std::path::PathBuf {
    f.paths.state_dir().unwrap().join(VERSION_PIN_FILE)
}

fn prepare_err(f: &crate::overlay_runtime_tests::Fixture) -> OverlayError {
    prepare(&f.paths).err().expect("must be refused")
}

#[test]
fn boot_writes_the_rollback_pin_and_an_unreadable_pin_refuses_boot() {
    let f = fixture(&parent_with(base_parent().rules, base_parent().limits, 9), None);
    assert!(!pin_path(&f).exists());
    let _r = start(&f);
    assert_eq!(std::fs::read_to_string(pin_path(&f)).unwrap().trim(), "9");

    // Garbage in the pin file is an error, not "no pin".
    std::fs::write(pin_path(&f), "nine").unwrap();
    assert!(matches!(prepare_err(&f), OverlayError::PinCorrupt { .. }));
    std::fs::write(pin_path(&f), b"\xff\xfe").unwrap();
    assert!(matches!(prepare_err(&f), OverlayError::PinCorrupt { .. }));
}

#[test]
fn a_missing_pin_after_an_applied_overlay_refuses_boot() {
    let f = fixture(&base_parent(), None);
    let r = start(&f);
    std::fs::remove_file(pin_path(&f)).unwrap();
    // Same chain (it records `governance.overlay.applied`), pin gone.
    let again = prepare(&f.paths).unwrap();
    assert_eq!(again.commit(&r.cm).unwrap_err(), OverlayError::PinMissing);
    // A fresh chain with no history is a first boot and writes the pin.
    again.commit(&ChainManager::new(0, 1000)).unwrap();
    assert!(pin_path(&f).exists());
}

#[test]
fn a_deleted_overlay_cannot_clear_a_non_empty_one_at_boot() {
    let f = fixture(
        &base_parent(),
        Some("[[deny]]\nid = \"d1\"\nactions = [\"a.b\"]\n"),
    );
    let r = start(&f);
    std::fs::remove_file(f.paths.overlay().unwrap()).unwrap();
    let again = prepare(&f.paths).unwrap();
    assert_eq!(again.commit(&r.cm).unwrap_err(), OverlayError::OverlayMissing);

    // An explicit empty file is the deliberate clear.
    std::fs::write(f.paths.overlay().unwrap(), "").unwrap();
    prepare(&f.paths).unwrap().commit(&r.cm).unwrap();

    // And a project that never had an overlay may keep having none.
    let g = fixture(&base_parent(), None);
    let r2 = start(&g);
    prepare(&g.paths).unwrap().commit(&r2.cm).unwrap();
}

#[test]
fn the_user_pin_outside_the_project_overrides_the_certificate() {
    let f = fixture(&base_parent(), None);
    let user_pk = user_key().verifying_key().to_bytes();

    // Matching pin: accepted.
    write_user_pin(f.paths.root(), &user_pk).unwrap();
    assert!(f.paths.root().join(USER_PIN_FILE).exists());
    prepare(&f.paths).unwrap();

    // A different pin: the certificate is refused even though it is
    // self-consistent.
    write_user_pin(f.paths.root(), &SigningKey::from_bytes(&[9u8; 32]).verifying_key().to_bytes())
        .unwrap();
    let e = prepare_err(&f);
    assert!(matches!(&e, OverlayError::Cert(m) if m.contains("user.pub")), "{e}");

    // An unreadable pin is refused, not ignored.
    std::fs::write(f.paths.root().join(USER_PIN_FILE), "not hex").unwrap();
    assert!(matches!(prepare_err(&f), OverlayError::Cert(_)));

    // Absent pin: development fallback to the certificate.
    std::fs::remove_file(f.paths.root().join(USER_PIN_FILE)).unwrap();
    prepare(&f.paths).unwrap();
}

#[test]
fn a_cert_and_policy_swapped_together_are_caught_by_the_pin() {
    // An attacker with the project dir writes their own key, cert and policy.
    let f = fixture(&base_parent(), None);
    write_user_pin(f.paths.root(), &user_key().verifying_key().to_bytes()).unwrap();
    let evil = SigningKey::from_bytes(&[9u8; 32]);
    let cert = ProjectCert::sign(
        &evil,
        &CertRequest {
            project_id: f.paths.child_id().unwrap().to_owned(),
            project_pubkey: SigningKey::from_bytes(&[3u8; 32]).verifying_key().to_bytes(),
            serial: 1,
            issued_at: Utc::now() - Duration::minutes(1),
            expires_at: None,
        },
    );
    std::fs::write(f.paths.project_cert().unwrap(), serde_json::to_vec(&cert).unwrap()).unwrap();
    let relaxed = export_rules(vec![], 0.99, false, &Limits::default(), &evil, 1, Utc::now()).unwrap();
    write_parent(&f.paths, &relaxed);
    assert!(matches!(prepare_err(&f), OverlayError::Cert(_)));
}

#[test]
fn a_revoked_project_stops_boot_reload_and_update() {
    let f = fixture(&base_parent(), None);
    let r = start(&f);
    std::fs::write(f.paths.root().join(REVOKED_FILE), "").unwrap();
    assert!(matches!(r.rt.reload().unwrap_err(), OverlayError::Revoked(_)));
    let newer = parent_with(base_parent().rules, base_parent().limits, 3);
    assert!(matches!(r.rt.apply_parent_update(newer).unwrap_err(), OverlayError::Revoked(_)));
    assert!(matches!(prepare_err(&f), OverlayError::Revoked(_)));
}

#[test]
fn an_expired_certificate_stops_reload_and_update() {
    let f = fixture(&base_parent(), None);
    let r = start(&f);
    let cert = ProjectCert::sign(
        &user_key(),
        &CertRequest {
            project_id: f.paths.child_id().unwrap().to_owned(),
            project_pubkey: SigningKey::from_bytes(&[3u8; 32]).verifying_key().to_bytes(),
            serial: 2,
            issued_at: Utc::now() - Duration::minutes(10),
            expires_at: Some(Utc::now() - Duration::minutes(1)),
        },
    );
    std::fs::write(f.paths.project_cert().unwrap(), serde_json::to_vec(&cert).unwrap()).unwrap();
    let e = r.rt.reload().unwrap_err();
    assert_eq!(e.key(), "project.cert.json");
    let newer = parent_with(base_parent().rules, base_parent().limits, 3);
    assert_eq!(r.rt.apply_parent_update(newer).unwrap_err().key(), "project.cert.json");
}

#[test]
fn export_only_trusts_a_previous_policy_this_key_signed_and_caps_the_version() {
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("parent-policy.json");
    let engine = crate::governance::GovernanceEngine::new(0.8, false);
    let now = Utc::now().timestamp() as u64;

    // Signed by someone else with a huge version: ignored.
    let other = SigningKey::from_bytes(&[9u8; 32]);
    let theirs = export_rules(vec![], 0.8, false, &Limits::default(), &other, u64::MAX / 2, Utc::now()).unwrap();
    write_atomic_0600(&path, &serde_json::to_vec(&theirs).unwrap()).unwrap();
    let a = export_to(&path, &engine, &Limits::default(), &user_key()).unwrap();
    assert!(a.version <= now + 5, "{}", a.version);

    // Ours, but far in the future: capped, not ratcheted.
    let ours = export_rules(vec![], 0.8, false, &Limits::default(), &user_key(), u64::MAX / 2, Utc::now()).unwrap();
    write_atomic_0600(&path, &serde_json::to_vec(&ours).unwrap()).unwrap();
    let b = export_to(&path, &engine, &Limits::default(), &user_key()).unwrap();
    assert!(b.version <= now + VERSION_SLACK_SECS + 5, "{}", b.version);

    // Garbage: the version starts from the clock.
    std::fs::write(&path, "{").unwrap();
    let c = export_to(&path, &engine, &Limits::default(), &user_key()).unwrap();
    assert!(c.version <= now + 5);
}
