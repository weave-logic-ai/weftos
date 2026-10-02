//! Tests that need the crate-private raw writer (forging validly signed but
//! semantically invalid records, which is exactly what the public door forbids).

use serde_json::json;

use crate::bindings::{BindError, BindHow, BindMeta, Bindings, Check, ConflictReason};
use crate::journal::{Journal, JournalError};
use clawft_mesh_local::{node_id_from_pubkey, hexser, Principal};
use ed25519_dalek::SigningKey;

fn key() -> SigningKey {
    SigningKey::from_bytes(&[7u8; 32])
}

fn bind_body(uid: u32, k: u8) -> serde_json::Value {
    json!({"principal": {"kind":"uid","id":uid}, "user_pubkey": hexser::encode(&[k; 32]),
           "user_id": node_id_from_pubkey(&[k; 32]), "how": "tofu"})
}

#[test]
fn invalid_signed_record_degrades_instead_of_bricking() {
    let dir = tmpdir();
    let mut j = Journal::open(dir.path(), key()).unwrap();
    j.append_raw(1, "user.bind", bind_body(501, 1)).unwrap();
    j.append_raw(2, "user.bind", bind_body(501, 1)).unwrap(); // duplicate: AlreadyBound
    j.append_raw(3, "user.bind", bind_body(502, 2)).unwrap();
    assert!(matches!(Bindings::fold(&j), Err(BindError::Replay { seq: 1, .. })));
    let mut b = Bindings::fold_lenient(&j);
    assert!(b.degraded().is_some());
    let r = b.bind(&mut j, &Principal::Uid(503), &[3; 32], BindHow::Tofu, BindMeta::default());
    assert!(matches!(r, Err(BindError::Conflict(ConflictReason::Degraded))));
    // check() fails closed, even for the binding that exists.
    assert_eq!(b.check(&Principal::Uid(501), &[1; 32]), Check::Conflict(ConflictReason::Degraded));
    assert!(b.is_serial_revoked("anything", 1));
    assert_eq!(b.key_of(&Principal::Uid(501)), None, "no partial state served");
    assert!(b.principal_of(&[1; 32]).is_none());
    assert!(b.serials(&Principal::Uid(501)).is_empty());
    assert!(matches!(b.revoked_serials(), Err(BindError::Degraded(_))));
    let r = b.revoke(&mut j, &Principal::Uid(501), "x", &Principal::Uid(0));
    assert!(matches!(r, Err(BindError::Degraded(_))));
    let _ = JournalError::ReadOnly;
}

#[test]
fn replay_requires_exactly_the_next_serial() {
    let dir = tmpdir();
    let mut j = Journal::open(dir.path(), key()).unwrap();
    j.append_raw(1, "user.bind", bind_body(501, 1)).unwrap();
    let uid = node_id_from_pubkey(&[1; 32]);
    j.append_raw(2, "user.cert.issue", json!({"user_id": uid, "serial": 5, "issued_at": 1, "not_after": 2}))
        .unwrap();
    assert!(matches!(
        Bindings::fold(&j),
        Err(BindError::Replay { seq: 1, source }) if matches!(*source, BindError::SerialOutOfSequence { got: 5, expected: 1 })
    ));
}

#[test]
fn serial_exhaustion_is_an_error_not_a_wrap() {
    let dir = tmpdir();
    let mut j = Journal::open(dir.path(), key()).unwrap();
    let mut b = Bindings::default();
    let p = Principal::Uid(501);
    b.bind(&mut j, &p, &[1; 32], BindHow::Tofu, BindMeta::default()).unwrap();
    b.apply_lost(u64::MAX, &[]);
    assert!(matches!(b.issue_cert(&mut j, &p, 1, 2), Err(BindError::SerialExhausted)));
}

/// A temp dir the state-dir safety check accepts (0700).
fn tmpdir() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    d
}

#[test]
fn degraded_fold_still_applies_quarantine_facts() {
    let dir = tmpdir();
    let mut j = Journal::open(dir.path(), key()).unwrap();
    j.append_raw(1, "user.bind", bind_body(501, 1)).unwrap();
    j.append_raw(2, "user.bind", bind_body(501, 1)).unwrap(); // invalid: degrades here
    let uid = node_id_from_pubkey(&[4; 32]);
    j.append_raw(
        3,
        "journal.quarantine",
        json!({"lost_from_seq": 2, "lost_count": 1, "serial_high_water": 9, "raw_serial_high_water": 9,
               "revoked_user_ids": [uid], "quarantine": []}),
    )
    .unwrap();
    let b = Bindings::fold_lenient(&j);
    assert!(b.degraded().is_some());
    assert_eq!(b.last_serial(), 9);
    assert!(b.is_serial_revoked(&uid, 9));
}

#[test]
fn replay_rejects_an_accept_without_a_pending_quarantine() {
    let dir = tmpdir();
    let mut j = Journal::open(dir.path(), key()).unwrap();
    j.append_raw(1, "journal.accept_truncate", json!({"quarantine_seq": 0, "serial_floor": 0, "quarantine": [], "by": {"kind":"uid","id":0}}))
        .unwrap();
    assert!(matches!(Bindings::fold(&j), Err(BindError::Replay { seq: 0, .. })));
}
