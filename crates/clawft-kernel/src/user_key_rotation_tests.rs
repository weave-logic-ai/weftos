use super::*;
use crate::project_identity::{self as ident, RevocationView, verify_cert_historic};
use chrono::TimeZone;
use clawft_types::project::CertRequest;

fn key(n: u8) -> SigningKey {
    SigningKey::from_bytes(&[n; 32])
}
fn t(secs: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(1_800_000_000 + secs, 0).unwrap()
}
fn pk(k: &SigningKey) -> [u8; 32] {
    k.verifying_key().to_bytes()
}
const PID: &str = "01JABCDEFGHJKMNPQRSTVWXYZ0";

fn cert(user: &SigningKey, issued: DateTime<Utc>, serial: u64) -> clawft_types::project::cert::ProjectCert {
    ident::sign_cert(
        user,
        &CertRequest {
            project_id: PID.into(),
            project_pubkey: pk(&key(50)),
            serial,
            issued_at: issued,
            expires_at: None,
        },
    )
    .unwrap()
}

#[test]
fn a_record_needs_both_signatures() {
    let (a, b) = (key(1), key(2));
    let r = RotationRecord::sign(&a, &b, None, t(0));
    r.verify().unwrap();
    let mut forged = r.clone();
    forged.sig_new = r.sig_old.clone();
    assert!(matches!(forged.verify(), Err(RotationError::BadSignature { which: "new", .. })));
    let mut forged = r.clone();
    forged.sig_old = r.sig_new.clone();
    assert!(matches!(forged.verify(), Err(RotationError::BadSignature { which: "old", .. })));
    let mut moved = r;
    moved.rotated_at = ts(t(5));
    assert!(moved.verify().is_err(), "the rotation point is signed");
}

#[test]
fn history_chains_and_must_end_at_the_current_key() {
    let (a, b, c) = (key(1), key(2), key(3));
    let r1 = RotationRecord::sign(&a, &b, None, t(10));
    let r2 = RotationRecord::sign(&b, &c, Some(&r1), t(20));
    let h = UserKeyHistory::from_records(&pk(&c), &[r1.clone(), r2.clone()]).unwrap();
    assert_eq!(h.rotations(), 2);
    assert!(h.accepts(&pk(&a), t(10)) && !h.accepts(&pk(&a), t(11)));
    assert!(h.accepts(&pk(&b), t(20)) && !h.accepts(&pk(&b), t(21)));
    assert!(h.accepts(&pk(&c), t(10_000)));
    // Wrong current key, skipped record, bad prev.
    assert!(matches!(UserKeyHistory::from_records(&pk(&b), &[r1.clone(), r2.clone()]), Err(RotationError::NotCurrent { .. })));
    assert!(UserKeyHistory::from_records(&pk(&c), &[r2.clone()]).is_err());
    let orphan = RotationRecord::sign(&b, &c, None, t(20));
    assert!(UserKeyHistory::from_records(&pk(&c), &[r1.clone(), orphan]).is_err());
    // A handover forged with the stolen old key alone names a key nobody holds.
    let thief = key(9);
    let forged = RotationRecord::sign(&a, &thief, None, t(10));
    assert!(UserKeyHistory::from_records(&pk(&b), &[forged]).is_err());
}

#[test]
fn a_certificate_across_a_rotation_verifies_only_up_to_the_rotation_point() {
    let (old, new) = (key(1), key(2));
    let rec = RotationRecord::sign(&old, &new, None, t(100));
    let h = UserKeyHistory::from_records(&pk(&new), &[rec.clone()]).unwrap();
    let before = cert(&old, t(50), 1);
    let at_point = cert(&old, t(100), 2);
    let after = cert(&old, t(101), 3);
    let by_new = cert(&new, t(150), 4);
    verify_cert_historic(&before, &h).unwrap();
    verify_cert_historic(&at_point, &h).unwrap();
    verify_cert_historic(&by_new, &h).unwrap();
    assert!(verify_cert_historic(&after, &h).is_err(), "old key after its rotation point is refused");
    // Without the history the old certificate is untrusted.
    assert!(verify_cert_historic(&before, &UserKeyHistory::single(&pk(&new))).is_err());

    // The chain holds the old-key certificate before the rotation event.
    let chain = crate::chain::ChainManager::new(0, 1000);
    chain.append(ident::SOURCE, ident::KIND_REGISTER, Some(serde_json::json!({ "cert": before })));
    chain.append(ident::SOURCE, ident::KIND_ROTATED, Some(serde_json::json!({ "record": rec })));
    let v = RevocationView::build_with(
        &h,
        &chain.tail_from(0),
        &[],
        &[before.clone(), after.clone(), by_new.clone()],
    );
    assert_eq!(v.rejected(), 1);
    assert_eq!(v.last_serial(PID), 4);
    assert_eq!(v.current_cert(PID).map(|c| c.serial), Some(4));
    // Without chain evidence the old-key certificate is dropped too, however it is dated.
    let v = RevocationView::build_with(&h, &[], &[], &[before.clone()]);
    assert_eq!(v.rejected(), 1);
    assert_eq!(v.last_serial(PID), 0);
}

#[test]
fn the_log_is_append_only_and_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let log = RotationLog::new(dir.path());
    let (a, b, c) = (key(1), key(2), key(3));
    assert!(log.read().unwrap().is_empty());
    assert_eq!(log.history(&pk(&a)).unwrap().rotations(), 0);
    let r1 = RotationRecord::sign(&a, &b, None, t(1));
    log.append(&r1).unwrap();
    // A record that does not extend the log is refused and changes nothing.
    assert!(log.append(&RotationRecord::sign(&a, &c, None, t(2))).is_err());
    let r2 = RotationRecord::sign(&b, &c, Some(&r1), t(2));
    log.append(&r2).unwrap();
    assert_eq!(log.history(&pk(&c)).unwrap().rotations(), 2);
    std::fs::write(log.path(), "garbage\n").unwrap();
    assert!(matches!(log.history(&pk(&c)), Err(RotationError::Log { .. })));
}
