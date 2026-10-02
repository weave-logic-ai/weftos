//! Quarantine semantics: clamped floor, signed facts, crash windows, marker
//! deletion, lost-tail revocations, over-long lines.
use std::fs;
use std::path::Path;

use clawft_mesh_local::{node_id_from_pubkey, Principal};
use clawft_mesh_service::{
    AdminAck, BindError, BindHow, BindMeta, Bindings, Check, ConflictReason, Journal, JournalOptions,
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

fn tmpdir() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    d
}

/// Flip one signature hex digit of the first line (in any journal file)
/// containing `needle`; the line stays parseable JSON but fails verification.
fn corrupt_line_containing(dir: &Path, needle: &str) {
    for e in fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        if !(name.starts_with("journal.") && name.ends_with(".jsonl")) {
            continue;
        }
        let text = fs::read_to_string(&p).unwrap();
        if !text.contains(needle) {
            continue;
        }
        let mut lines: Vec<String> = text.lines().map(String::from).collect();
        let l = lines.iter_mut().find(|l| l.contains(needle)).unwrap();
        let at = l.rfind("\"sig\":\"").unwrap() + 7;
        let repl = if l.as_bytes()[at] == b'0' { "1" } else { "0" };
        l.replace_range(at..at + 1, repl);
        fs::write(&p, lines.join("\n") + "\n").unwrap();
        return;
    }
    panic!("no line contains {needle}");
}

/// bind(501,k1); certs 1..=3; revoke(501). One record per segment when `tiny`.
fn build(dir: &Path, tiny: bool) {
    let opts = JournalOptions { max_segment_bytes: if tiny { 100 } else { 8 << 20 } };
    let mut j = Journal::open_with(dir, key(), opts).unwrap();
    let mut b = Bindings::default();
    b.bind(&mut j, &u(501), &k(1), BindHow::Tofu, BindMeta::default()).unwrap();
    for _ in 0..3 {
        b.issue_cert(&mut j, &u(501), 10, 20).unwrap();
    }
    b.revoke(&mut j, &u(501), "compromised", &u(0)).unwrap();
}

fn quarantine_body(j: &Journal) -> serde_json::Value {
    j.iter().find(|r| r.kind == "journal.quarantine").expect("quarantine record").body.clone()
}

fn forged_setup(dir: &Path) {
    {
        let mut j = Journal::open(dir, key()).unwrap();
        let mut b = Bindings::default();
        b.bind(&mut j, &u(501), &k(1), BindHow::Tofu, BindMeta::default()).unwrap();
        b.issue_cert(&mut j, &u(501), 10, 20).unwrap();
    }
    let path = dir.join("journal.jsonl");
    let mut data = fs::read(&path).unwrap();
    data.extend(format!("{{\"kind\":\"user.cert.issue\",\"body\":{{\"serial\":{}}}}}\n", u64::MAX).bytes());
    fs::write(&path, data).unwrap();
}

#[test]
fn forged_high_water_is_clamped() {
    let dir = tmpdir();
    forged_setup(dir.path());
    let mut j = Journal::open(dir.path(), key()).unwrap();
    let body = quarantine_body(&j);
    assert_eq!(body["raw_serial_high_water"].as_u64(), Some(u64::MAX), "raw kept for information");
    assert_eq!(body["serial_high_water"].as_u64(), Some(2), "clamped to prefix floor 1 + 1 lost line");
    let mut b = Bindings::fold(&j).unwrap();
    assert_eq!(b.last_serial(), 2);
    let q = pq(&j);
    b.accept_truncate(&mut j, ack(), q, None).unwrap();
    assert_eq!(b.issue_cert(&mut j, &u(501), 10, 20).unwrap(), 3, "issuance is not bricked");
}

#[test]
fn admin_floor_can_raise_but_is_capped() {
    let dir = tmpdir();
    forged_setup(dir.path());
    let mut j = Journal::open(dir.path(), key()).unwrap();
    let mut b = Bindings::fold(&j).unwrap();
    let q = pq(&j);
    let r = b.accept_truncate(&mut j, ack(), q, Some(u64::MAX));
    assert!(matches!(r, Err(BindError::FloorTooHigh { .. })));
    let r = b.accept_truncate(&mut j, ack(), q, Some(2 + (1 << 32) + 1));
    assert!(matches!(r, Err(BindError::FloorTooHigh { .. })));
    assert!(j.read_only(), "a refused acceptance lifts nothing");
    b.accept_truncate(&mut j, ack(), q, Some(100)).unwrap();
    assert_eq!(b.issue_cert(&mut j, &u(501), 10, 20).unwrap(), 101);
    assert_eq!(Bindings::fold(&j).unwrap(), b);
}

#[test]
fn accepting_one_quarantine_does_not_lift_another() {
    let dir = tmpdir();
    build(dir.path(), false);
    corrupt_line_containing(dir.path(), "\"serial\":2");
    {
        let mut j = Journal::open(dir.path(), key()).unwrap(); // quarantine A
        j.append("policy.set", json!({"n": 1})).unwrap();
        j.append("policy.set", json!({"n": 2})).unwrap();
    }
    corrupt_line_containing(dir.path(), "\"n\":2");
    let mut j = Journal::open(dir.path(), key()).unwrap(); // quarantine B
    let pending = j.pending_quarantines();
    assert_eq!(pending.len(), 2);
    let mut b = Bindings::fold(&j).unwrap();
    b.accept_truncate(&mut j, ack(), pending[0], None).unwrap();
    assert!(j.read_only(), "A accepted, B still pending");
    assert_eq!(j.pending_quarantines(), vec![pending[1]]);
    // Accepting A again, or a seq that is not a quarantine, is refused.
    assert!(matches!(b.accept_truncate(&mut j, ack(), pending[0], None), Err(BindError::NoSuchQuarantine(_))));
    assert!(matches!(b.accept_truncate(&mut j, ack(), 0, None), Err(BindError::NoSuchQuarantine(0))));
    drop(j);
    let mut j = Journal::open(dir.path(), key()).unwrap();
    assert!(j.read_only(), "still read-only after restart");
    let mut b = Bindings::fold(&j).unwrap();
    b.accept_truncate(&mut j, ack(), pending[1], None).unwrap();
    assert!(!j.read_only());
}

#[test]
fn crash_after_renames_before_record_recovers_from_the_files() {
    let dir = tmpdir();
    build(dir.path(), true);
    corrupt_line_containing(dir.path(), "\"serial\":2");
    drop(Journal::open_with(dir.path(), key(), JournalOptions { max_segment_bytes: 100 }).unwrap());

    // Simulate the crash window: the signed record never made it, and the
    // marker is an unrecorded one whose numbers lie.
    let active = dir.path().join("journal.jsonl");
    fs::remove_file(&active).unwrap();
    let marker = dir.path().join("journal.truncated");
    let mut m: serde_json::Value = serde_json::from_slice(&fs::read(&marker).unwrap()).unwrap();
    m["recorded"] = json!(false);
    m["serial_high_water"] = json!(u64::MAX);
    m["raw_serial_high_water"] = json!(u64::MAX);
    m["lost_count"] = json!(1_000_000u64);
    m["revoked_user_ids"] = json!([]);
    // Names that are not quarantine files (a live segment, a path) must be ignored.
    let mut names = m["quarantine"].as_array().unwrap().clone();
    names.push(json!("journal.000.jsonl"));
    names.push(json!("/etc/passwd"));
    names.push(json!("../journal.jsonl"));
    m["quarantine"] = json!(names);
    fs::write(&marker, serde_json::to_vec(&m).unwrap()).unwrap();

    let j = Journal::open_with(dir.path(), key(), JournalOptions { max_segment_bytes: 100 }).unwrap();
    assert!(j.read_only());
    let body = quarantine_body(&j);
    assert_eq!(body["raw_serial_high_water"].as_u64(), Some(3), "recomputed from the moved files");
    assert_eq!(body["lost_count"].as_u64(), Some(3));
    let b = Bindings::fold(&j).unwrap();
    assert_eq!(b.last_serial(), 3);
    assert_eq!(b.key_of(&u(501)), None, "the revoke that was only in a moved segment is honoured");
}

#[test]
fn deleting_the_marker_keeps_floor_and_revocations() {
    let dir = tmpdir();
    build(dir.path(), false);
    corrupt_line_containing(dir.path(), "\"serial\":2");
    drop(Journal::open(dir.path(), key()).unwrap());
    fs::remove_file(dir.path().join("journal.truncated")).unwrap();

    let mut j = Journal::open(dir.path(), key()).unwrap();
    assert!(j.read_only(), "the signed chain keeps it read-only without the marker");
    let mut b = Bindings::fold(&j).unwrap();
    assert!(b.bind(&mut j, &u(502), &k(9), BindHow::Tofu, BindMeta::default()).is_err());
    assert_eq!(b.last_serial(), 3);
    assert_eq!(b.key_of(&u(501)), None);
    assert_eq!(b.check(&u(502), &k(1)), Check::Conflict(ConflictReason::KeyRevoked));
    let uid = node_id_from_pubkey(&k(1));
    assert!(b.is_serial_revoked(&uid, 2) && b.is_serial_revoked(&uid, 3), "lost-tail serials are revoked");

    // Acceptance lifts it for good, with or without a marker.
    let q = pq(&j);
    b.accept_truncate(&mut j, ack(), q, None).unwrap();
    assert!(!j.read_only());
    drop(j);
    let j = Journal::open(dir.path(), key()).unwrap();
    assert!(!j.read_only());
}

#[test]
fn key_whose_bind_and_revoke_were_both_lost_stays_unbindable() {
    let dir = tmpdir();
    {
        let mut j = Journal::open(dir.path(), key()).unwrap();
        j.append("policy.set", json!({"a": 1})).unwrap();
        let mut b = Bindings::default();
        b.bind(&mut j, &u(501), &k(1), BindHow::Tofu, BindMeta::default()).unwrap();
        b.revoke(&mut j, &u(501), "x", &u(0)).unwrap();
    }
    corrupt_line_containing(dir.path(), "\"kind\":\"user.bind\"");
    let mut j = Journal::open(dir.path(), key()).unwrap();
    let mut b = Bindings::fold(&j).unwrap();
    assert_eq!(b.key_of(&u(501)), None);
    assert_eq!(b.check(&u(502), &k(1)), Check::Conflict(ConflictReason::KeyRevoked));
    let q = pq(&j);
    b.accept_truncate(&mut j, ack(), q, None).unwrap();
    let r = b.bind(&mut j, &u(502), &k(1), BindHow::Approved, BindMeta { by: Some(u(0)), ..Default::default() });
    assert!(r.is_err(), "even an approved bind cannot resurrect it");
    r.unwrap_err();
    assert!(b.bind_pending(&mut j, &u(502), &k(1), BindMeta::default()).is_err());
}

#[test]
fn overlong_first_line_still_harvests_later_lines() {
    let dir = tmpdir();
    {
        let mut j = Journal::open(dir.path(), key()).unwrap();
        j.append("policy.set", json!({"a": 1})).unwrap();
    }
    let path = dir.path().join("journal.jsonl");
    let mut data = fs::read(&path).unwrap();
    data.extend(std::iter::repeat_n(b'a', 3 * 1024 * 1024));
    data.push(b'\n');
    data.extend_from_slice(b"{\"kind\":\"user.revoke\",\"body\":{\"user_id\":\"abc\"}}\n");
    data.extend_from_slice(b"{\"kind\":\"user.cert.issue\",\"body\":{\"serial\":2}}\n");
    fs::write(&path, data).unwrap();
    let j = Journal::open(dir.path(), key()).unwrap();
    let body = quarantine_body(&j);
    assert_eq!(body["lost_count"].as_u64(), Some(3));
    assert_eq!(body["raw_serial_high_water"].as_u64(), Some(2));
    assert_eq!(body["revoked_user_ids"], json!(["abc"]));
}

fn pq(j: &Journal) -> u64 {
    j.pending_quarantines()[0]
}
