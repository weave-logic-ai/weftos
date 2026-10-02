//! Review round tests: durable read-only, lost-range floor, filesystem trust,
//! bounded reads, poisoning, approval rules.
use std::fs;
use std::os::unix::fs::PermissionsExt;

use clawft_mesh_local::{node_id_from_pubkey, Principal};
use clawft_mesh_service::{
    AdminAck, BindError, BindHow, BindMeta, Bindings, Check, ConflictReason, Journal, JournalError,
};
use ed25519_dalek::SigningKey;
use serde_json::json;

fn key() -> SigningKey {
    SigningKey::from_bytes(&[7u8; 32])
}
fn k(n: u8) -> [u8; 32] {
    [n; 32]
}
fn u(n: u32) -> Principal {
    Principal::Uid(n)
}
fn ack() -> AdminAck {
    AdminAck::admin_verified(u(0))
}
fn approver() -> BindMeta {
    BindMeta { by: Some(u(0)), ..Default::default() }
}

/// bind(501,k1), cert 1, cert 2, cert 3, revoke(501); then corrupt the signature
/// of the line holding cert 2 so cert 3 and the revoke are quarantined "valid
/// but unreachable" records.
fn journal_with_lost_revoke(dir: &std::path::Path) {
    let mut j = Journal::open(dir, key()).unwrap();
    let mut b = Bindings::default();
    b.bind(&mut j, &u(501), &k(1), BindHow::Tofu, BindMeta::default()).unwrap();
    for _ in 0..3 {
        b.issue_cert(&mut j, &u(501), 10, 20).unwrap();
    }
    b.revoke(&mut j, &u(501), "compromised", &u(0)).unwrap();
    drop(j);
    let path = dir.join("journal.jsonl");
    let text = fs::read_to_string(&path).unwrap();
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    let l = &mut lines[2]; // cert 2
    let at = l.rfind("\"sig\":\"").unwrap() + 7;
    let c = l.as_bytes()[at];
    let repl = if c == b'0' { "1" } else { "0" };
    l.replace_range(at..at + 1, repl);
    fs::write(&path, lines.join("\n") + "\n").unwrap();
}

#[test]
fn reopen_without_accept_stays_read_only() {
    let dir = tmpdir();
    journal_with_lost_revoke(dir.path());
    let j = Journal::open(dir.path(), key()).unwrap();
    assert!(j.read_only() && j.quarantined().is_some());
    let lost = j.lost().unwrap().clone();
    assert_eq!((lost.lost_from_seq, lost.lost_count, lost.serial_high_water), (2, 3, 3));
    drop(j);
    assert!(dir.path().join("journal.truncated").exists());

    let mut j = Journal::open(dir.path(), key()).unwrap();
    assert!(j.read_only(), "restart must not lift read-only");
    assert!(j.quarantined().is_none(), "nothing newly quarantined");
    let mut b = Bindings::fold(&j).unwrap();
    let r = b.bind(&mut j, &u(502), &k(9), BindHow::Tofu, BindMeta::default());
    assert!(matches!(r, Err(BindError::Journal(JournalError::ReadOnly))));
}

#[test]
fn lost_revoke_and_serials_stay_effective_through_accept() {
    let dir = tmpdir();
    journal_with_lost_revoke(dir.path());
    let mut j = Journal::open(dir.path(), key()).unwrap();
    let mut b = Bindings::fold(&j).unwrap();
    // The revoke lost with the tail is honoured: no resurrection.
    assert_eq!(b.key_of(&u(501)), None);
    assert_eq!(b.check(&u(501), &k(1)), Check::Conflict(ConflictReason::KeyRevoked));
    assert!(b.issue_cert(&mut j, &u(501), 1, 2).is_err());

    let q = pq(&j);

    b.accept_truncate(&mut j, ack(), q, None).unwrap();
    assert!(!j.read_only());
    assert!(!dir.path().join("journal.truncated").exists());
    // Needs an approved bind with an approver after a revoke.
    assert!(matches!(
        b.bind(&mut j, &u(501), &k(2), BindHow::Tofu, BindMeta::default()),
        Err(BindError::ApprovalRequired)
    ));
    assert!(matches!(
        b.bind(&mut j, &u(501), &k(2), BindHow::Approved, BindMeta::default()),
        Err(BindError::ApprovalWithoutApprover)
    ));
    b.bind(&mut j, &u(501), &k(2), BindHow::Approved, approver()).unwrap();
    // Serial never at or below the lost high-water mark (3): 2 would be a re-issue.
    assert_eq!(b.issue_cert(&mut j, &u(501), 10, 20).unwrap(), 4);

    drop(j);
    let j = Journal::open(dir.path(), key()).unwrap();
    assert!(!j.read_only());
    assert_eq!(Bindings::fold(&j).unwrap(), b, "state survives restart after accept");
}

#[test]
fn revoke_then_tofu_needs_approval_even_without_loss() {
    let dir = tmpdir();
    let mut j = Journal::open(dir.path(), key()).unwrap();
    let mut b = Bindings::default();
    b.bind(&mut j, &u(501), &k(1), BindHow::Tofu, BindMeta::default()).unwrap();
    b.revoke(&mut j, &u(501), "x", &u(0)).unwrap();
    assert!(matches!(
        b.bind(&mut j, &u(501), &k(2), BindHow::Tofu, BindMeta::default()),
        Err(BindError::ApprovalRequired)
    ));
    b.bind_pending(&mut j, &u(501), &k(2), BindMeta::default()).unwrap();
    b.bind(&mut j, &u(501), &k(2), BindHow::Approved, approver()).unwrap();
    assert_eq!(b, Bindings::fold(&j).unwrap());
    assert_eq!(node_id_from_pubkey(&k(2)).len(), 32);
}

#[test]
fn user_kinds_cannot_be_appended_raw() {
    let dir = tmpdir();
    let mut j = Journal::open(dir.path(), key()).unwrap();
    for kind in ["user.bind", "user.revoke", "user.cert.issue", "user.whatever", "journal.accept_truncate"] {
        assert!(matches!(j.append(kind, json!({})), Err(JournalError::ReservedKind(_))), "{kind}");
    }
    assert!(j.is_empty());
}

#[test]
fn state_dir_must_be_private_and_real() {
    let base = tmpdir();
    let loose = base.path().join("loose");
    fs::create_dir(&loose).unwrap();
    fs::set_permissions(&loose, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(Journal::open(&loose, key()), Err(JournalError::UnsafePath(_))));

    let real = base.path().join("real");
    fs::create_dir(&real).unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o700)).unwrap();
    let link = base.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert!(matches!(Journal::open(&link, key()), Err(JournalError::UnsafePath(_))));

    // A directory we create is 0700.
    let fresh = base.path().join("fresh");
    drop(Journal::open(&fresh, key()).unwrap());
    assert_eq!(fs::metadata(&fresh).unwrap().permissions().mode() & 0o777, 0o700);
}

#[test]
fn symlinked_journal_is_refused_and_never_truncated_through() {
    let base = tmpdir();
    let dir = base.path().join("state");
    let mut j = Journal::open(&dir, key()).unwrap();
    j.append("policy.set", json!({"a": 1})).unwrap();
    j.append("policy.set", json!({"a": 2})).unwrap();
    drop(j);
    let victim = base.path().join("victim.txt");
    fs::copy(dir.join("journal.jsonl"), &victim).unwrap();
    let before = fs::read(&victim).unwrap();
    fs::remove_file(dir.join("journal.jsonl")).unwrap();
    std::os::unix::fs::symlink(&victim, dir.join("journal.jsonl")).unwrap();
    assert!(Journal::open(&dir, key()).is_err());
    assert_eq!(fs::read(&victim).unwrap(), before, "target untouched");
}

#[test]
fn fifo_journal_is_refused_without_hanging() {
    let base = tmpdir();
    let dir = base.path().join("state");
    drop(Journal::open(&dir, key()).unwrap());
    let fifo = std::ffi::CString::new(dir.join("journal.jsonl").to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    assert!(matches!(Journal::open(&dir, key()), Err(JournalError::UnsafePath(_))));
}

#[test]
fn oversized_records_are_refused_on_write_and_quarantined_on_read() {
    let dir = tmpdir();
    let mut j = Journal::open(dir.path(), key()).unwrap();
    j.append("policy.set", json!({"a": 1})).unwrap();
    let big = "a".repeat(2 * 1024 * 1024);
    assert!(matches!(j.append("policy.set", json!({"x": big})), Err(JournalError::RecordTooLarge(_))));
    assert_eq!(j.len(), 1);
    drop(j);
    let path = dir.path().join("journal.jsonl");
    let mut data = fs::read(&path).unwrap();
    data.extend(std::iter::repeat_n(b'a', 3 * 1024 * 1024));
    data.push(b'\n');
    fs::write(&path, data).unwrap();
    let j = Journal::open(dir.path(), key()).unwrap();
    assert!(j.read_only());
    assert_eq!(j.iter().filter(|r| r.kind != "journal.quarantine").count(), 1);
}

#[test]
fn failed_write_with_failed_rollback_poisons_the_journal() {
    let dir = tmpdir();
    let mut j = Journal::open(dir.path(), key()).unwrap();
    j.append("policy.set", json!({"a": 1})).unwrap();
    j.inject_write_failure();
    assert!(matches!(j.append("policy.set", json!({"a": 2})), Err(JournalError::Io(_))));
    assert!(j.poisoned());
    assert!(matches!(j.append("policy.set", json!({"a": 3})), Err(JournalError::Poisoned)));
    drop(j);
    assert_eq!(Journal::open(dir.path(), key()).unwrap().len(), 1, "nothing half-written");
}

/// A temp dir the state-dir safety check accepts (0700).
fn tmpdir() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    d
}

fn pq(j: &Journal) -> u64 {
    j.latest_pending_quarantine().unwrap()
}
