use std::fs;
use std::path::Path;

use clawft_mesh_service::journal::{Journal, JournalError, JournalOptions};
use ed25519_dalek::SigningKey;
use serde_json::json;
use sha2::{Digest, Sha256};

fn key() -> SigningKey {
    SigningKey::from_bytes(&[7u8; 32])
}

fn fill(j: &mut Journal, n: u64) {
    for i in 0..n {
        j.append_at(1_790_000_000 + i, "policy.set", json!({"key": format!("k{i}"), "old": null, "new": i, "by": "x"}))
            .unwrap();
    }
}

fn copy_dir(from: &Path, to: &Path) {
    for e in fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        if e.file_name() != "mesh.lock" {
            fs::copy(e.path(), to.join(e.file_name())).unwrap();
        }
    }
}

#[test]
fn append_replay_golden() {
    let dir = tmpdir();
    let mut j = Journal::open(dir.path(), key()).unwrap();
    assert!(j.head().is_none());
    j.append_at(1_790_000_000, "machine.init", json!({"node_id": "n", "machine_pubkey": "p", "key_origin": "generated", "build_sha": "abc"})).unwrap();
    j.append_at(1_790_000_001, "service.start", json!({"build_sha": "abc", "pid": 1})).unwrap();
    let head = j.append_at(1_790_000_002, "policy.set", json!({"key": "a", "old": 1, "new": 2, "by": "u"})).unwrap();
    assert_eq!(head.seq, 2);
    drop(j);

    let got = fs::read_to_string(dir.path().join("journal.jsonl")).unwrap();
    let golden_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/journal.jsonl");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        fs::write(&golden_path, &got).unwrap();
    }
    assert_eq!(got, fs::read_to_string(&golden_path).unwrap(), "journal bytes drifted from golden");
    assert!(got.lines().next().unwrap().contains(&"0".repeat(64)), "seq 0 prev is 64 zeros");

    let j = Journal::open(dir.path(), key()).unwrap();
    assert!(!j.read_only() && j.quarantined().is_none());
    assert_eq!(j.len(), 3);
    assert_eq!(j.head().unwrap(), head);
    let last = got.lines().last().unwrap();
    assert_eq!(head.hash, clawft_mesh_local::hexser::encode(&Sha256::digest(last.as_bytes())));
    let kinds: Vec<_> = j.iter().map(|r| r.kind.as_str()).collect();
    assert_eq!(kinds, ["machine.init", "service.start", "policy.set"]);
}

#[test]
fn tamper_every_byte_position_is_detected() {
    let src = tmpdir();
    let mut j = Journal::open(src.path(), key()).unwrap();
    fill(&mut j, 4);
    drop(j);
    let orig = fs::read(src.path().join("journal.jsonl")).unwrap();
    for pos in 0..orig.len() {
        let dst = tmpdir();
        copy_dir(src.path(), dst.path());
        let mut bad = orig.clone();
        bad[pos] ^= 0x01;
        fs::write(dst.path().join("journal.jsonl"), &bad).unwrap();
        match Journal::open(dst.path(), key()) {
            Err(JournalError::Unverifiable { .. }) => {}
            Ok(j) => {
                assert!(j.quarantined().is_some() && j.read_only(), "byte {pos} not detected");
                assert!(real(&j) < 4, "byte {pos}: all records survived");
            }
            Err(e) => panic!("byte {pos}: unexpected {e}"),
        }
    }
}

#[test]
fn wrong_key_is_an_error_and_modifies_nothing() {
    let dir = tmpdir();
    let mut j = Journal::open(dir.path(), key()).unwrap();
    fill(&mut j, 2);
    drop(j);
    let before = fs::read(dir.path().join("journal.jsonl")).unwrap();
    let other = SigningKey::from_bytes(&[8u8; 32]);
    assert!(matches!(Journal::open(dir.path(), other), Err(JournalError::Unverifiable { .. })));
    assert_eq!(before, fs::read(dir.path().join("journal.jsonl")).unwrap());
}

#[test]
fn truncated_tail_is_quarantined_and_binds_refused() {
    use clawft_mesh_local::{node_id_from_pubkey, Principal};
    use clawft_mesh_service::{BindError, BindHow, BindMeta, Bindings};

    let dir = tmpdir();
    let mut j = Journal::open(dir.path(), key()).unwrap();
    fill(&mut j, 3);
    drop(j);
    let path = dir.path().join("journal.jsonl");
    let data = fs::read(&path).unwrap();
    fs::write(&path, &data[..data.len() - 10]).unwrap();

    let mut j = Journal::open(dir.path(), key()).unwrap();
    assert_eq!(real(&j), 2);
    assert!(j.read_only());
    let q = j.quarantined().unwrap().to_path_buf();
    assert!(q.file_name().unwrap().to_string_lossy().starts_with("journal.corrupt."));
    assert!(fs::metadata(&q).unwrap().len() > 0);

    let pk = [9u8; 32];
    assert_ne!(node_id_from_pubkey(&pk), "");
    let mut b = Bindings::fold(&j).unwrap();
    let r = b.bind(&mut j, &Principal::Uid(501), &pk, BindHow::Tofu, BindMeta::default());
    assert!(matches!(r, Err(BindError::Journal(JournalError::ReadOnly))));
    let r = b.bind_pending(&mut j, &Principal::Uid(501), &pk, BindMeta::default());
    assert!(matches!(r, Err(BindError::Journal(JournalError::ReadOnly))));

    let q = pq(&j);

    b.accept_truncate(&mut j, clawft_mesh_service::AdminAck::admin_verified(Principal::Uid(0)), q, None).unwrap();
    b.bind(&mut j, &Principal::Uid(501), &pk, BindHow::Tofu, BindMeta::default()).unwrap();
    drop(j);
    let j = Journal::open(dir.path(), key()).unwrap();
    assert!(!j.read_only());
    assert_eq!(j.len(), 5, "prefix + quarantine record + acceptance + bind");
}

#[test]
fn deleting_a_middle_record_breaks_the_chain() {
    let dir = tmpdir();
    let mut j = Journal::open(dir.path(), key()).unwrap();
    fill(&mut j, 4);
    drop(j);
    let path = dir.path().join("journal.jsonl");
    let text = fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let edited = format!("{}\n{}\n{}\n", lines[0], lines[2], lines[3]);
    fs::write(&path, edited).unwrap();
    let j = Journal::open(dir.path(), key()).unwrap();
    assert!(j.read_only());
    assert_eq!(real(&j), 1);
}

#[test]
fn segment_rollover_keeps_the_chain() {
    let dir = tmpdir();
    let opts = JournalOptions { max_segment_bytes: 400 };
    let mut j = Journal::open_with(dir.path(), key(), opts).unwrap();
    fill(&mut j, 12);
    let head = j.head().unwrap();
    drop(j);
    let segs = fs::read_dir(dir.path())
        .unwrap()
        .filter(|e| {
            let n = e.as_ref().unwrap().file_name().to_string_lossy().into_owned();
            n.starts_with("journal.") && n.ends_with(".jsonl") && n != "journal.jsonl"
        })
        .count();
    assert!(segs >= 3, "expected several rolled segments, got {segs}");

    let mut j = Journal::open_with(dir.path(), key(), opts).unwrap();
    assert!(!j.read_only());
    assert_eq!(j.len(), 12);
    assert_eq!(j.head().unwrap(), head);
    fill(&mut j, 1);
    drop(j);
    let j = Journal::open_with(dir.path(), key(), opts).unwrap();
    assert_eq!(j.len(), 13);

    // Removing a whole middle segment is detected at the seam.
    drop(j);
    fs::remove_file(dir.path().join("journal.001.jsonl")).unwrap();
    let j = Journal::open_with(dir.path(), key(), opts).unwrap();
    assert!(j.read_only() && real(&j) < 13);
}

#[test]
fn second_opener_fails_naming_the_holder() {
    let dir = tmpdir();
    let j = Journal::open(dir.path(), key()).unwrap();
    match Journal::open(dir.path(), key()) {
        Err(JournalError::Locked { holder_pid }) => assert_eq!(holder_pid, Some(std::process::id())),
        other => panic!("expected Locked, got {:?}", other.map(|_| ())),
    }
    drop(j);
    Journal::open(dir.path(), key()).unwrap();
}

#[test]
fn concurrent_appenders_are_serialised_by_the_lock() {
    let dir = tmpdir();
    let path = dir.path().to_path_buf();
    let threads: Vec<_> = (0..8)
        .map(|t| {
            let path = path.clone();
            std::thread::spawn(move || {
                for i in 0..5 {
                    loop {
                        match Journal::open(&path, key()) {
                            Ok(mut j) => {
                                j.append("policy.set", json!({"t": t, "i": i})).unwrap();
                                break;
                            }
                            Err(JournalError::Locked { .. }) => std::thread::sleep(std::time::Duration::from_millis(1)),
                            Err(e) => panic!("{e}"),
                        }
                    }
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    let j = Journal::open(dir.path(), key()).unwrap();
    assert!(!j.read_only() && j.quarantined().is_none());
    assert_eq!(j.len(), 40);
    for (i, r) in j.iter().enumerate() {
        assert_eq!(r.seq, i as u64);
    }
}

/// A temp dir the state-dir safety check accepts (0700).
fn tmpdir() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    d
}

/// Records excluding the quarantine bookkeeping record.
fn real(j: &Journal) -> usize {
    j.iter().filter(|r| r.kind != "journal.quarantine").count()
}

fn pq(j: &Journal) -> u64 {
    j.pending_quarantines()[0]
}
